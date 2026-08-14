use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::{Duration, Instant},
};

use relay_runtime::{
    CancellationToken, ExecutionId, Runtime, RuntimeAction, RuntimeEvent, RuntimeOutcome,
    RuntimeOutcomeKind, RuntimeOutputStream, RuntimeRequest,
};
use relay_store::NewExecutionRecord;
use serde_json::json;
use thiserror::Error;
use tokio::sync::{Mutex, Notify, broadcast, mpsc};

use crate::{
    ProjectKey, Relay, RepositoryEligibility, SqliteStore, StoredTask, TaskId,
    TaskPersistenceError, TaskState, TrustedProject, get_stored_task,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionLimits {
    pub max_parallel: usize,
    pub per_project_max_parallel: usize,
    pub max_repair_attempts: u32,
    pub max_duration: Duration,
    pub output_byte_limit: usize,
}

impl ExecutionLimits {
    pub fn new(
        max_parallel: usize,
        per_project_max_parallel: usize,
        max_repair_attempts: u32,
        max_duration: Duration,
        output_byte_limit: usize,
    ) -> Result<Self, ExecutionSchedulerError> {
        if max_parallel == 0 {
            return Err(ExecutionSchedulerError::InvalidLimit {
                name: "max_parallel",
            });
        }
        if per_project_max_parallel == 0 {
            return Err(ExecutionSchedulerError::InvalidLimit {
                name: "per_project_max_parallel",
            });
        }
        if max_duration.is_zero() {
            return Err(ExecutionSchedulerError::InvalidLimit {
                name: "max_duration",
            });
        }
        if output_byte_limit == 0 {
            return Err(ExecutionSchedulerError::InvalidLimit {
                name: "output_byte_limit",
            });
        }

        Ok(Self {
            max_parallel,
            per_project_max_parallel,
            max_repair_attempts,
            max_duration,
            output_byte_limit,
        })
    }
}

impl Default for ExecutionLimits {
    fn default() -> Self {
        Self {
            max_parallel: 2,
            per_project_max_parallel: 1,
            max_repair_attempts: 2,
            max_duration: Duration::from_secs(30),
            output_byte_limit: 64 * 1024,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreExecutionEvent {
    ExecutionQueued {
        task_id: TaskId,
        project_key: ProjectKey,
    },
    ExecutionPreflightStarted {
        task_id: TaskId,
    },
    ExecutionPreflightFailed {
        task_id: TaskId,
        failure: PreflightFailure,
    },
    ExecutionStarted {
        task_id: TaskId,
        execution_id: ExecutionId,
        project_key: ProjectKey,
    },
    RuntimeOutput {
        task_id: TaskId,
        execution_id: ExecutionId,
        stream: RuntimeOutputStream,
        byte_len: usize,
    },
    VerificationStarted {
        task_id: TaskId,
        execution_id: ExecutionId,
    },
    RepairStarted {
        task_id: TaskId,
        execution_id: ExecutionId,
        attempt: u32,
        reason: RuntimeOutcomeKind,
    },
    RepairAttempted {
        task_id: TaskId,
        execution_id: ExecutionId,
        attempt: u32,
    },
    ClarificationNeeded {
        task_id: TaskId,
        execution_id: ExecutionId,
    },
    DecisionNeeded {
        task_id: TaskId,
        execution_id: ExecutionId,
    },
    ExecutionCancelled {
        task_id: TaskId,
        execution_id: Option<ExecutionId>,
    },
    ExecutionFinished {
        task_id: TaskId,
        execution_id: Option<ExecutionId>,
        final_state: TaskState,
        outcome: RuntimeOutcomeKind,
    },
    TaskStateChanged {
        task_id: TaskId,
        from: TaskState,
        to: TaskState,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreflightFailure {
    TaskNotFound,
    TaskNotReady { actual: TaskState },
    ProjectResolution { message: String },
    RepositoryNotEligible { message: String },
    Store { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionReport {
    pub task_id: TaskId,
    pub project_key: ProjectKey,
    pub execution_id: Option<ExecutionId>,
    pub final_state: Option<TaskState>,
    pub outcome: Option<RuntimeOutcomeKind>,
    pub output: BoundedOutputSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedOutputSnapshot {
    pub retained_bytes: usize,
    pub retained_chunks: usize,
    pub dropped_bytes: usize,
    pub dropped_chunks: usize,
}

#[derive(Debug)]
pub struct ExecutionScheduler<R: Runtime> {
    relay: Relay,
    store: SqliteStore,
    runtime: Arc<R>,
    limits: ExecutionLimits,
    inner: Arc<Mutex<SchedulerInner>>,
    notify: Arc<Notify>,
    event_tx: broadcast::Sender<CoreExecutionEvent>,
}

#[derive(Debug, Default)]
struct SchedulerInner {
    queue: VecDeque<QueuedExecution>,
    active: HashMap<TaskId, ActiveExecution>,
    active_by_project: HashMap<ProjectKey, usize>,
}

#[derive(Debug, Clone)]
struct QueuedExecution {
    task_id: TaskId,
    project_key: ProjectKey,
}

#[derive(Debug, Clone)]
struct ActiveExecution {
    project_key: ProjectKey,
    cancellation: CancellationToken,
}

impl<R: Runtime> ExecutionScheduler<R> {
    pub fn new(relay: Relay, store: SqliteStore, runtime: Arc<R>, limits: ExecutionLimits) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        Self {
            relay,
            store,
            runtime,
            limits,
            inner: Arc::new(Mutex::new(SchedulerInner::default())),
            notify: Arc::new(Notify::new()),
            event_tx,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<CoreExecutionEvent> {
        self.event_tx.subscribe()
    }

    pub async fn submit(&self, task_id: TaskId) -> Result<(), ExecutionSchedulerError> {
        let stored = get_stored_task(&self.store, task_id)
            .await?
            .ok_or(ExecutionSchedulerError::TaskNotFound { task_id })?;
        if stored.state != TaskState::Ready {
            return Err(ExecutionSchedulerError::TaskNotReady {
                task_id,
                actual: stored.state,
            });
        }

        let queued = {
            let mut inner = self.inner.lock().await;
            let queued = QueuedExecution {
                task_id,
                project_key: stored.project_key.clone(),
            };
            inner.queue.push_back(queued.clone());
            queued
        };

        self.send(CoreExecutionEvent::ExecutionQueued {
            task_id,
            project_key: queued.project_key,
        });
        self.notify.notify_waiters();
        Ok(())
    }

    pub async fn cancel(&self, task_id: TaskId) -> Result<(), ExecutionSchedulerError> {
        let queued_project = {
            let mut inner = self.inner.lock().await;
            if let Some(position) = inner
                .queue
                .iter()
                .position(|queued| queued.task_id == task_id)
            {
                inner
                    .queue
                    .remove(position)
                    .map(|queued| queued.project_key)
            } else if let Some(active) = inner.active.get(&task_id) {
                active.cancellation.cancel();
                None
            } else {
                None
            }
        };

        if queued_project.is_some() {
            crate::update_stored_task_state(
                &self.store,
                task_id,
                TaskState::Ready,
                TaskState::Cancelled,
            )
            .await?;
            self.send(CoreExecutionEvent::ExecutionCancelled {
                task_id,
                execution_id: None,
            });
            self.send(CoreExecutionEvent::ExecutionFinished {
                task_id,
                execution_id: None,
                final_state: TaskState::Cancelled,
                outcome: RuntimeOutcomeKind::Cancelled,
            });
        }

        self.notify.notify_waiters();
        Ok(())
    }

    pub async fn run_until_idle(&self) -> Result<Vec<ExecutionReport>, ExecutionSchedulerError> {
        let (completion_tx, mut completion_rx) = mpsc::channel(self.limits.max_parallel);
        let mut reports = Vec::new();

        loop {
            self.start_eligible(&completion_tx).await;

            if self.is_idle().await {
                return Ok(reports);
            }

            tokio::select! {
                Some(report) = completion_rx.recv() => {
                    self.release_slot(&report).await;
                    reports.push(report);
                }
                () = self.notify.notified() => {}
            }
        }
    }

    async fn start_eligible(&self, completion_tx: &mpsc::Sender<ExecutionReport>) {
        loop {
            let queued = {
                let mut inner = self.inner.lock().await;
                if inner.active.len() >= self.limits.max_parallel {
                    return;
                }

                let Some(position) = inner.queue.iter().position(|queued| {
                    !inner.active.contains_key(&queued.task_id)
                        && inner
                            .active_by_project
                            .get(&queued.project_key)
                            .copied()
                            .unwrap_or(0)
                            < self.limits.per_project_max_parallel
                }) else {
                    return;
                };

                let queued = inner
                    .queue
                    .remove(position)
                    .expect("position came from queue iteration");
                let cancellation = CancellationToken::new();
                inner.active.insert(
                    queued.task_id,
                    ActiveExecution {
                        project_key: queued.project_key.clone(),
                        cancellation: cancellation.clone(),
                    },
                );
                *inner
                    .active_by_project
                    .entry(queued.project_key.clone())
                    .or_insert(0) += 1;

                (queued, cancellation)
            };

            let relay = self.relay.clone();
            let store = self.store.clone();
            let runtime = self.runtime.clone();
            let limits = self.limits.clone();
            let event_tx = self.event_tx.clone();
            let completion_tx = completion_tx.clone();

            tokio::spawn(async move {
                let report =
                    run_task(relay, store, runtime, limits, event_tx, queued.0, queued.1).await;
                let _ = completion_tx.send(report).await;
            });
        }
    }

    async fn release_slot(&self, report: &ExecutionReport) {
        let mut inner = self.inner.lock().await;
        if let Some(active) = inner.active.remove(&report.task_id) {
            if let Some(count) = inner.active_by_project.get_mut(&active.project_key) {
                *count -= 1;
                if *count == 0 {
                    inner.active_by_project.remove(&active.project_key);
                }
            }
        }
        self.notify.notify_waiters();
    }

    async fn is_idle(&self) -> bool {
        let inner = self.inner.lock().await;
        inner.queue.is_empty() && inner.active.is_empty()
    }

    fn send(&self, event: CoreExecutionEvent) {
        let _ = self.event_tx.send(event);
    }
}

async fn run_task<R: Runtime>(
    relay: Relay,
    store: SqliteStore,
    runtime: Arc<R>,
    limits: ExecutionLimits,
    event_tx: broadcast::Sender<CoreExecutionEvent>,
    queued: QueuedExecution,
    cancellation: CancellationToken,
) -> ExecutionReport {
    let mut output = BoundedOutputBuffer::new(limits.output_byte_limit);
    send(
        &event_tx,
        CoreExecutionEvent::ExecutionPreflightStarted {
            task_id: queued.task_id,
        },
    );

    let preflight = match preflight(&relay, &store, queued.task_id).await {
        Ok(preflight) => preflight,
        Err(failure) => {
            send(
                &event_tx,
                CoreExecutionEvent::ExecutionPreflightFailed {
                    task_id: queued.task_id,
                    failure,
                },
            );
            return ExecutionReport {
                task_id: queued.task_id,
                project_key: queued.project_key,
                execution_id: None,
                final_state: None,
                outcome: None,
                output: output.snapshot(),
            };
        }
    };

    if cancellation.is_cancelled() {
        let _ = crate::update_stored_task_state(
            &store,
            queued.task_id,
            TaskState::Ready,
            TaskState::Cancelled,
        )
        .await;
        send(
            &event_tx,
            CoreExecutionEvent::ExecutionCancelled {
                task_id: queued.task_id,
                execution_id: None,
            },
        );
        return ExecutionReport {
            task_id: queued.task_id,
            project_key: preflight.stored.project_key,
            execution_id: None,
            final_state: Some(TaskState::Cancelled),
            outcome: Some(RuntimeOutcomeKind::Cancelled),
            output: output.snapshot(),
        };
    }

    let execution_id = ExecutionId::new();
    let identity = runtime.identity();
    let repository_snapshot_json = repository_snapshot(&preflight);
    let claim = store
        .claim_ready_task_for_execution(&NewExecutionRecord {
            id: execution_id.to_string(),
            task_id: queued.task_id.to_string(),
            runtime_profile: identity.profile_id.clone(),
            phase: "running".to_owned(),
            repository_snapshot_json: Some(repository_snapshot_json),
        })
        .await;

    if let Err(error) = claim {
        send(
            &event_tx,
            CoreExecutionEvent::ExecutionPreflightFailed {
                task_id: queued.task_id,
                failure: PreflightFailure::Store {
                    message: error.to_string(),
                },
            },
        );
        return ExecutionReport {
            task_id: queued.task_id,
            project_key: preflight.stored.project_key,
            execution_id: None,
            final_state: None,
            outcome: None,
            output: output.snapshot(),
        };
    }

    send(
        &event_tx,
        CoreExecutionEvent::ExecutionStarted {
            task_id: queued.task_id,
            execution_id,
            project_key: preflight.stored.project_key.clone(),
        },
    );

    let deadline = Instant::now() + limits.max_duration;
    let mut task_state = TaskState::Running;
    let mut repair_attempts = 0_u32;
    let mut action = RuntimeAction::Implement;

    loop {
        if cancellation.is_cancelled() {
            return finish(
                &store,
                &event_tx,
                FinishPlan::new(
                    queued.task_id,
                    preflight.stored.project_key.clone(),
                    execution_id,
                    task_state,
                    TaskState::Cancelled,
                    RuntimeOutcomeKind::Cancelled,
                ),
                output,
            )
            .await;
        }

        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            cancellation.cancel();
            return finish(
                &store,
                &event_tx,
                FinishPlan::new(
                    queued.task_id,
                    preflight.stored.project_key.clone(),
                    execution_id,
                    task_state,
                    TaskState::Failed,
                    RuntimeOutcomeKind::Timeout,
                )
                .failure_category(RuntimeOutcomeKind::Timeout.as_str()),
                output,
            )
            .await;
        };

        let request = RuntimeRequest {
            execution_id,
            task_id: queued.task_id.to_string(),
            handoff: preflight.stored.handoff.clone(),
            trusted_working_dir: preflight.project.local_path().to_path_buf(),
            action: action.clone(),
            profile_id: identity.profile_id.clone(),
        };

        let outcome = match tokio::time::timeout(
            remaining,
            collect_runtime_events(
                runtime.clone(),
                request,
                cancellation.clone(),
                &event_tx,
                queued.task_id,
                execution_id,
                &mut output,
            ),
        )
        .await
        {
            Ok(outcome) => outcome,
            Err(_) => {
                cancellation.cancel();
                RuntimeOutcome::new(RuntimeOutcomeKind::Timeout, "execution deadline exhausted")
            }
        };

        match outcome.kind {
            RuntimeOutcomeKind::Success => {
                task_state = transition(
                    &store,
                    &event_tx,
                    queued.task_id,
                    execution_id,
                    task_state,
                    TaskState::Verifying,
                    "verifying",
                )
                .await
                .unwrap_or(task_state);
                send(
                    &event_tx,
                    CoreExecutionEvent::VerificationStarted {
                        task_id: queued.task_id,
                        execution_id,
                    },
                );
                return finish(
                    &store,
                    &event_tx,
                    FinishPlan::new(
                        queued.task_id,
                        preflight.stored.project_key.clone(),
                        execution_id,
                        task_state,
                        TaskState::Done,
                        RuntimeOutcomeKind::Success,
                    ),
                    output,
                )
                .await;
            }
            RuntimeOutcomeKind::ImplementationFailure | RuntimeOutcomeKind::VerificationFailure => {
                if outcome.kind == RuntimeOutcomeKind::VerificationFailure
                    && task_state == TaskState::Running
                {
                    task_state = transition(
                        &store,
                        &event_tx,
                        queued.task_id,
                        execution_id,
                        task_state,
                        TaskState::Verifying,
                        "verifying",
                    )
                    .await
                    .unwrap_or(task_state);
                    send(
                        &event_tx,
                        CoreExecutionEvent::VerificationStarted {
                            task_id: queued.task_id,
                            execution_id,
                        },
                    );
                }

                if repair_attempts >= limits.max_repair_attempts {
                    return finish(
                        &store,
                        &event_tx,
                        FinishPlan::new(
                            queued.task_id,
                            preflight.stored.project_key.clone(),
                            execution_id,
                            task_state,
                            TaskState::Failed,
                            outcome.kind,
                        )
                        .failure_category(outcome.kind.as_str()),
                        output,
                    )
                    .await;
                }

                repair_attempts += 1;
                task_state = transition(
                    &store,
                    &event_tx,
                    queued.task_id,
                    execution_id,
                    task_state,
                    TaskState::Repairing,
                    "repairing",
                )
                .await
                .unwrap_or(task_state);
                let _ = store
                    .increment_execution_repair_count(&execution_id.to_string())
                    .await;
                send(
                    &event_tx,
                    CoreExecutionEvent::RepairStarted {
                        task_id: queued.task_id,
                        execution_id,
                        attempt: repair_attempts,
                        reason: outcome.kind,
                    },
                );
                action = RuntimeAction::Repair {
                    attempt: repair_attempts,
                    previous_outcome: outcome.kind,
                };
                task_state = transition(
                    &store,
                    &event_tx,
                    queued.task_id,
                    execution_id,
                    task_state,
                    TaskState::Running,
                    "running",
                )
                .await
                .unwrap_or(task_state);
                send(
                    &event_tx,
                    CoreExecutionEvent::RepairAttempted {
                        task_id: queued.task_id,
                        execution_id,
                        attempt: repair_attempts,
                    },
                );
            }
            RuntimeOutcomeKind::NeedsClarification => {
                send(
                    &event_tx,
                    CoreExecutionEvent::ClarificationNeeded {
                        task_id: queued.task_id,
                        execution_id,
                    },
                );
                return finish(
                    &store,
                    &event_tx,
                    FinishPlan::new(
                        queued.task_id,
                        preflight.stored.project_key.clone(),
                        execution_id,
                        task_state,
                        TaskState::NeedsClarification,
                        RuntimeOutcomeKind::NeedsClarification,
                    )
                    .failure_category(RuntimeOutcomeKind::NeedsClarification.as_str()),
                    output,
                )
                .await;
            }
            RuntimeOutcomeKind::NeedsDecision => {
                send(
                    &event_tx,
                    CoreExecutionEvent::DecisionNeeded {
                        task_id: queued.task_id,
                        execution_id,
                    },
                );
                return finish(
                    &store,
                    &event_tx,
                    FinishPlan::new(
                        queued.task_id,
                        preflight.stored.project_key.clone(),
                        execution_id,
                        task_state,
                        TaskState::NeedsDecision,
                        RuntimeOutcomeKind::NeedsDecision,
                    )
                    .failure_category(RuntimeOutcomeKind::NeedsDecision.as_str()),
                    output,
                )
                .await;
            }
            RuntimeOutcomeKind::Cancelled => {
                return finish(
                    &store,
                    &event_tx,
                    FinishPlan::new(
                        queued.task_id,
                        preflight.stored.project_key.clone(),
                        execution_id,
                        task_state,
                        TaskState::Cancelled,
                        RuntimeOutcomeKind::Cancelled,
                    ),
                    output,
                )
                .await;
            }
            RuntimeOutcomeKind::InvalidOutput
            | RuntimeOutcomeKind::RuntimeFailure
            | RuntimeOutcomeKind::Timeout => {
                return finish(
                    &store,
                    &event_tx,
                    FinishPlan::new(
                        queued.task_id,
                        preflight.stored.project_key.clone(),
                        execution_id,
                        task_state,
                        TaskState::Failed,
                        outcome.kind,
                    )
                    .failure_category(outcome.kind.as_str()),
                    output,
                )
                .await;
            }
        }
    }
}

async fn collect_runtime_events<R: Runtime>(
    runtime: Arc<R>,
    request: RuntimeRequest,
    cancellation: CancellationToken,
    event_tx: &broadcast::Sender<CoreExecutionEvent>,
    task_id: TaskId,
    execution_id: ExecutionId,
    output: &mut BoundedOutputBuffer,
) -> RuntimeOutcome {
    let session = match runtime.start(request, cancellation.clone()).await {
        Ok(session) => session,
        Err(error) => {
            return RuntimeOutcome::new(RuntimeOutcomeKind::RuntimeFailure, error.to_string());
        }
    };
    let mut events = session.events;

    loop {
        tokio::select! {
            () = cancellation.cancelled() => {
                return RuntimeOutcome::new(RuntimeOutcomeKind::Cancelled, "execution cancelled");
            }
            event = events.recv() => {
                match event {
                    Some(RuntimeEvent::Output { stream, bytes }) => {
                        let byte_len = bytes.len();
                        output.push(stream.clone(), bytes);
                        send(event_tx, CoreExecutionEvent::RuntimeOutput {
                            task_id,
                            execution_id,
                            stream,
                            byte_len,
                        });
                    }
                    Some(RuntimeEvent::Outcome(outcome)) => return outcome,
                    Some(RuntimeEvent::InvalidOutput { reason }) => {
                        return RuntimeOutcome::new(RuntimeOutcomeKind::InvalidOutput, reason);
                    }
                    None => {
                        return RuntimeOutcome::new(
                            RuntimeOutcomeKind::RuntimeFailure,
                            "runtime event stream ended without an outcome",
                        );
                    }
                }
            }
        }
    }
}

struct Preflight {
    stored: StoredTask,
    project: TrustedProject,
}

async fn preflight(
    relay: &Relay,
    store: &SqliteStore,
    task_id: TaskId,
) -> Result<Preflight, PreflightFailure> {
    let stored = get_stored_task(store, task_id)
        .await
        .map_err(|error| PreflightFailure::Store {
            message: error.to_string(),
        })?
        .ok_or(PreflightFailure::TaskNotFound)?;

    if stored.state != TaskState::Ready {
        return Err(PreflightFailure::TaskNotReady {
            actual: stored.state,
        });
    }

    let project = relay
        .resolve_project(stored.project_key.as_str())
        .map_err(|error| PreflightFailure::ProjectResolution {
            message: error.to_string(),
        })?
        .clone();
    let inspection = crate::inspect_trusted_project_repository(&project);
    if !inspection.is_eligible() {
        let message = match inspection.eligibility {
            RepositoryEligibility::Eligible => "eligible".to_owned(),
            RepositoryEligibility::NotEligible(reason) => format!("{reason:?}"),
        };
        return Err(PreflightFailure::RepositoryNotEligible { message });
    }

    Ok(Preflight { stored, project })
}

fn repository_snapshot(preflight: &Preflight) -> String {
    json!({
        "project_key": preflight.stored.project_key.as_str(),
        "local_path": preflight.project.local_path().display().to_string(),
        "eligibility": "eligible"
    })
    .to_string()
}

async fn transition(
    store: &SqliteStore,
    event_tx: &broadcast::Sender<CoreExecutionEvent>,
    task_id: TaskId,
    execution_id: ExecutionId,
    from: TaskState,
    to: TaskState,
    phase: &str,
) -> Result<TaskState, TaskPersistenceError> {
    store
        .transition_task_and_execution(
            &task_id.to_string(),
            from.as_str(),
            to.as_str(),
            &execution_id.to_string(),
            phase,
        )
        .await?;
    send(
        event_tx,
        CoreExecutionEvent::TaskStateChanged { task_id, from, to },
    );
    Ok(to)
}

struct FinishPlan<'a> {
    task_id: TaskId,
    project_key: ProjectKey,
    execution_id: ExecutionId,
    expected_state: TaskState,
    final_state: TaskState,
    outcome: RuntimeOutcomeKind,
    failure_category: Option<&'a str>,
}

impl<'a> FinishPlan<'a> {
    fn new(
        task_id: TaskId,
        project_key: ProjectKey,
        execution_id: ExecutionId,
        expected_state: TaskState,
        final_state: TaskState,
        outcome: RuntimeOutcomeKind,
    ) -> Self {
        Self {
            task_id,
            project_key,
            execution_id,
            expected_state,
            final_state,
            outcome,
            failure_category: None,
        }
    }

    fn failure_category(mut self, failure_category: &'a str) -> Self {
        self.failure_category = Some(failure_category);
        self
    }
}

async fn finish(
    store: &SqliteStore,
    event_tx: &broadcast::Sender<CoreExecutionEvent>,
    plan: FinishPlan<'_>,
    output: BoundedOutputBuffer,
) -> ExecutionReport {
    let _ = store
        .finish_execution(
            &plan.task_id.to_string(),
            plan.expected_state.as_str(),
            plan.final_state.as_str(),
            &plan.execution_id.to_string(),
            plan.outcome.as_str(),
            plan.failure_category,
        )
        .await;
    if plan.final_state == TaskState::Cancelled {
        send(
            event_tx,
            CoreExecutionEvent::ExecutionCancelled {
                task_id: plan.task_id,
                execution_id: Some(plan.execution_id),
            },
        );
    }
    send(
        event_tx,
        CoreExecutionEvent::ExecutionFinished {
            task_id: plan.task_id,
            execution_id: Some(plan.execution_id),
            final_state: plan.final_state,
            outcome: plan.outcome,
        },
    );

    ExecutionReport {
        task_id: plan.task_id,
        project_key: plan.project_key,
        execution_id: Some(plan.execution_id),
        final_state: Some(plan.final_state),
        outcome: Some(plan.outcome),
        output: output.snapshot(),
    }
}

fn send(event_tx: &broadcast::Sender<CoreExecutionEvent>, event: CoreExecutionEvent) {
    let _ = event_tx.send(event);
}

#[derive(Debug)]
struct BoundedOutputBuffer {
    chunks: VecDeque<BufferedOutput>,
    retained_bytes: usize,
    dropped_bytes: usize,
    dropped_chunks: usize,
    limit: usize,
}

#[derive(Debug)]
struct BufferedOutput {
    _stream: RuntimeOutputStream,
    bytes: Vec<u8>,
}

impl BoundedOutputBuffer {
    fn new(limit: usize) -> Self {
        Self {
            chunks: VecDeque::new(),
            retained_bytes: 0,
            dropped_bytes: 0,
            dropped_chunks: 0,
            limit,
        }
    }

    fn push(&mut self, stream: RuntimeOutputStream, mut bytes: Vec<u8>) {
        if bytes.len() > self.limit {
            let drop_count = bytes.len() - self.limit;
            bytes.drain(..drop_count);
            self.dropped_bytes += drop_count;
            self.dropped_chunks += 1;
        }

        self.retained_bytes += bytes.len();
        self.chunks.push_back(BufferedOutput {
            _stream: stream,
            bytes,
        });

        while self.retained_bytes > self.limit {
            if let Some(front) = self.chunks.pop_front() {
                self.retained_bytes -= front.bytes.len();
                self.dropped_bytes += front.bytes.len();
                self.dropped_chunks += 1;
            } else {
                self.retained_bytes = 0;
                break;
            }
        }
    }

    fn snapshot(&self) -> BoundedOutputSnapshot {
        BoundedOutputSnapshot {
            retained_bytes: self.retained_bytes,
            retained_chunks: self.chunks.len(),
            dropped_bytes: self.dropped_bytes,
            dropped_chunks: self.dropped_chunks,
        }
    }
}

#[derive(Debug, Error)]
pub enum ExecutionSchedulerError {
    #[error("invalid scheduler limit: {name} must be greater than zero")]
    InvalidLimit { name: &'static str },
    #[error("{0}")]
    TaskPersistence(#[from] TaskPersistenceError),
    #[error("task {task_id} was not found")]
    TaskNotFound { task_id: TaskId },
    #[error("task {task_id} is not READY; actual state is {actual}")]
    TaskNotReady { task_id: TaskId, actual: TaskState },
}

#[cfg(test)]
mod tests {
    use super::*;
    use relay_runtime_fake::{FakeRuntime, FakeRuntimeConfig, FakeScenario};
    use std::{
        fs,
        path::PathBuf,
        process::{Command, Output},
    };

    const EXPECTED_REMOTE: &str = "https://github.com/Jacinth091/Relay.git";

    #[tokio::test]
    async fn fake_success_reaches_done_with_execution_record() {
        let fixture = Fixture::new(&[("a", true)]).await;
        let task = fixture.import_task("a", "success").await;
        let runtime = Arc::new(FakeRuntime::default());
        let scheduler = fixture.scheduler(runtime, ExecutionLimits::default());

        let reports = run_tasks(&scheduler, &[task]).await;

        assert_eq!(reports[0].final_state, Some(TaskState::Done));
        assert_task_state(&fixture.store, task, TaskState::Done).await;
        let executions = fixture.executions(task).await;
        assert_eq!(executions.len(), 1);
        assert_eq!(executions[0].exit_outcome.as_deref(), Some("success"));
    }

    #[tokio::test]
    async fn implementation_and_verification_failures_repair_to_success() {
        for (scenario, title) in [
            (
                FakeScenario::ImplementationFailureThenRepairSuccess,
                "implementation repair",
            ),
            (
                FakeScenario::VerificationFailureThenRepairSuccess,
                "verification repair",
            ),
        ] {
            let fixture = Fixture::new(&[("a", true)]).await;
            let task = fixture.import_task("a", title).await;
            let runtime = Arc::new(FakeRuntime::default());
            runtime.set_task_scenario(task.to_string(), scenario);
            let scheduler = fixture.scheduler(runtime, ExecutionLimits::default());

            let reports = run_tasks(&scheduler, &[task]).await;

            assert_eq!(reports[0].final_state, Some(TaskState::Done));
            let executions = fixture.executions(task).await;
            assert_eq!(executions[0].repair_attempt_count, 1);
            assert_eq!(executions[0].exit_outcome.as_deref(), Some("success"));
        }
    }

    #[tokio::test]
    async fn repeated_repair_failure_exhausts_finite_bound() {
        let fixture = Fixture::new(&[("a", true)]).await;
        let task = fixture.import_task("a", "repair exhaustion").await;
        let runtime = Arc::new(FakeRuntime::default());
        runtime.set_task_scenario(task.to_string(), FakeScenario::RepeatedRepairFailure);
        let limits = ExecutionLimits::new(2, 1, 2, Duration::from_secs(5), 64 * 1024).unwrap();
        let scheduler = fixture.scheduler(runtime, limits);

        let reports = run_tasks(&scheduler, &[task]).await;

        assert_eq!(reports[0].final_state, Some(TaskState::Failed));
        assert_eq!(
            reports[0].outcome,
            Some(RuntimeOutcomeKind::ImplementationFailure)
        );
        let executions = fixture.executions(task).await;
        assert_eq!(executions[0].repair_attempt_count, 2);
        assert_eq!(
            executions[0].failure_category.as_deref(),
            Some("implementation_failure")
        );
    }

    #[tokio::test]
    async fn stderr_output_alone_does_not_fail() {
        let fixture = Fixture::new(&[("a", true)]).await;
        let task = fixture.import_task("a", "stderr success").await;
        let runtime = Arc::new(FakeRuntime::default());
        runtime.set_task_scenario(task.to_string(), FakeScenario::StderrThenSuccess);
        let scheduler = fixture.scheduler(runtime, ExecutionLimits::default());
        let mut events = scheduler.subscribe();

        let reports = run_tasks(&scheduler, &[task]).await;

        assert_eq!(reports[0].final_state, Some(TaskState::Done));
        assert!(saw_stderr_output(&mut events).await);
    }

    #[tokio::test]
    async fn slow_streaming_can_succeed_or_timeout_under_total_deadline() {
        let fixture = Fixture::new(&[("a", true)]).await;
        let slow_success = fixture.import_task("a", "slow success").await;
        let timeout = fixture.import_task("a", "slow timeout").await;
        let config = FakeRuntimeConfig {
            stream_delay: Duration::from_millis(15),
            slow_chunks: 5,
            ..FakeRuntimeConfig::default()
        };
        let runtime = Arc::new(FakeRuntime::new(config));
        runtime.set_task_scenario(slow_success.to_string(), FakeScenario::SlowStreaming);
        runtime.set_task_scenario(timeout.to_string(), FakeScenario::SlowStreaming);

        let success_limits =
            ExecutionLimits::new(2, 1, 2, Duration::from_secs(2), 64 * 1024).unwrap();
        let scheduler = fixture.scheduler(runtime.clone(), success_limits);
        let reports = run_tasks(&scheduler, &[slow_success]).await;
        assert_eq!(reports[0].final_state, Some(TaskState::Done));

        let timeout_limits =
            ExecutionLimits::new(2, 1, 2, Duration::from_millis(25), 64 * 1024).unwrap();
        let scheduler = fixture.scheduler(runtime, timeout_limits);
        let reports = run_tasks(&scheduler, &[timeout]).await;
        assert_eq!(reports[0].final_state, Some(TaskState::Failed));
        assert_eq!(reports[0].outcome, Some(RuntimeOutcomeKind::Timeout));
    }

    #[tokio::test]
    async fn clarification_decision_invalid_and_huge_output_are_isolated() {
        for (scenario, expected_state, expected_outcome) in [
            (
                FakeScenario::NeedsClarification,
                TaskState::NeedsClarification,
                RuntimeOutcomeKind::NeedsClarification,
            ),
            (
                FakeScenario::NeedsDecision,
                TaskState::NeedsDecision,
                RuntimeOutcomeKind::NeedsDecision,
            ),
            (
                FakeScenario::InvalidRuntimeOutput,
                TaskState::Failed,
                RuntimeOutcomeKind::InvalidOutput,
            ),
        ] {
            let fixture = Fixture::new(&[("a", true)]).await;
            let task = fixture.import_task("a", "terminal scenario").await;
            let runtime = Arc::new(FakeRuntime::default());
            runtime.set_task_scenario(task.to_string(), scenario);
            let scheduler = fixture.scheduler(runtime, ExecutionLimits::default());

            let reports = run_tasks(&scheduler, &[task]).await;

            assert_eq!(reports[0].final_state, Some(expected_state));
            assert_eq!(reports[0].outcome, Some(expected_outcome));
            assert_task_state(&fixture.store, task, expected_state).await;
        }

        let fixture = Fixture::new(&[("a", true)]).await;
        let task = fixture.import_task("a", "huge output").await;
        let runtime = Arc::new(FakeRuntime::default());
        runtime.set_task_scenario(task.to_string(), FakeScenario::HugeOutput);
        let limits = ExecutionLimits::new(2, 1, 2, Duration::from_secs(5), 4096).unwrap();
        let scheduler = fixture.scheduler(runtime, limits);

        let reports = run_tasks(&scheduler, &[task]).await;

        assert_eq!(reports[0].final_state, Some(TaskState::Done));
        assert!(reports[0].output.retained_bytes <= 4096);
        assert!(reports[0].output.dropped_bytes > 0);
        let executions = fixture.executions(task).await;
        assert!(
            !executions[0]
                .repository_snapshot_json
                .as_deref()
                .unwrap()
                .contains("xxxx")
        );
    }

    #[tokio::test]
    async fn queued_and_running_cancellation_are_isolated() {
        let fixture = Fixture::new(&[("a", true), ("b", true)]).await;
        let queued = fixture.import_task("a", "queued cancel").await;
        let unaffected = fixture.import_task("b", "unaffected").await;
        let runtime = Arc::new(FakeRuntime::default());
        let scheduler = fixture.scheduler(runtime, ExecutionLimits::default());

        scheduler.submit(queued).await.unwrap();
        scheduler.cancel(queued).await.unwrap();
        scheduler.submit(unaffected).await.unwrap();
        let reports = scheduler.run_until_idle().await.unwrap();

        assert_eq!(reports.len(), 1);
        assert_task_state(&fixture.store, queued, TaskState::Cancelled).await;
        assert_task_state(&fixture.store, unaffected, TaskState::Done).await;
        assert!(fixture.executions(queued).await.is_empty());

        let fixture = Fixture::new(&[("a", true), ("b", true)]).await;
        let running = fixture.import_task("a", "running cancel").await;
        let other = fixture.import_task("b", "other project").await;
        let config = FakeRuntimeConfig {
            stream_delay: Duration::from_millis(20),
            slow_chunks: 20,
            ..FakeRuntimeConfig::default()
        };
        let runtime = Arc::new(FakeRuntime::new(config));
        runtime.set_task_scenario(running.to_string(), FakeScenario::SlowStreaming);
        runtime.set_task_scenario(other.to_string(), FakeScenario::SlowStreaming);
        let scheduler = Arc::new(fixture.scheduler(
            runtime,
            ExecutionLimits::new(2, 1, 2, Duration::from_secs(5), 64 * 1024).unwrap(),
        ));
        scheduler.submit(running).await.unwrap();
        scheduler.submit(other).await.unwrap();
        let runner = {
            let scheduler = scheduler.clone();
            tokio::spawn(async move { scheduler.run_until_idle().await.unwrap() })
        };
        wait_for_started(&scheduler, running).await;
        scheduler.cancel(running).await.unwrap();
        let reports = runner.await.unwrap();

        assert!(
            reports.iter().any(|report| report.task_id == running
                && report.final_state == Some(TaskState::Cancelled))
        );
        assert_task_state(&fixture.store, other, TaskState::Done).await;
    }

    #[tokio::test]
    async fn scheduler_honors_project_slots_and_avoids_head_of_line_blocking() {
        let fixture = Fixture::new(&[("a", true), ("b", true)]).await;
        let a1 = fixture.import_task("a", "a1").await;
        let a2 = fixture.import_task("a", "a2").await;
        let b1 = fixture.import_task("b", "b1").await;
        let config = FakeRuntimeConfig {
            stream_delay: Duration::from_millis(25),
            slow_chunks: 4,
            ..FakeRuntimeConfig::default()
        };
        let runtime = Arc::new(FakeRuntime::new(config));
        runtime.set_task_scenario(a1.to_string(), FakeScenario::SlowStreaming);
        runtime.set_task_scenario(b1.to_string(), FakeScenario::SlowStreaming);
        let scheduler = fixture.scheduler(
            runtime,
            ExecutionLimits::new(2, 1, 2, Duration::from_secs(5), 64 * 1024).unwrap(),
        );
        let mut events = scheduler.subscribe();

        scheduler.submit(a1).await.unwrap();
        scheduler.submit(a2).await.unwrap();
        scheduler.submit(b1).await.unwrap();
        let runner = tokio::spawn(async move { scheduler.run_until_idle().await.unwrap() });

        let first_two = first_started_tasks(&mut events, 2).await;
        assert!(first_two.contains(&a1));
        assert!(first_two.contains(&b1));
        assert!(!first_two.contains(&a2));

        let reports = runner.await.unwrap();
        assert_eq!(reports.len(), 3);
    }

    #[tokio::test]
    async fn failure_repair_and_needs_decision_do_not_stop_other_projects() {
        let fixture = Fixture::new(&[("a", true), ("b", true)]).await;
        let failing = fixture.import_task("a", "failing").await;
        let success = fixture.import_task("b", "success").await;
        let runtime = Arc::new(FakeRuntime::default());
        runtime.set_task_scenario(failing.to_string(), FakeScenario::RepeatedRepairFailure);
        let scheduler = fixture.scheduler(runtime, ExecutionLimits::default());

        let reports = run_tasks(&scheduler, &[failing, success]).await;

        assert!(reports.iter().any(
            |report| report.task_id == failing && report.final_state == Some(TaskState::Failed)
        ));
        assert_task_state(&fixture.store, success, TaskState::Done).await;

        let fixture = Fixture::new(&[("a", true), ("b", true)]).await;
        let decision = fixture.import_task("a", "decision").await;
        let unrelated = fixture.import_task("b", "unrelated").await;
        let runtime = Arc::new(FakeRuntime::default());
        runtime.set_task_scenario(decision.to_string(), FakeScenario::NeedsDecision);
        let scheduler = fixture.scheduler(runtime, ExecutionLimits::default());

        let _reports = run_tasks(&scheduler, &[decision, unrelated]).await;

        assert_task_state(&fixture.store, decision, TaskState::NeedsDecision).await;
        assert_task_state(&fixture.store, unrelated, TaskState::Done).await;
    }

    #[tokio::test]
    async fn preflight_blocks_ineligible_repository_and_duplicate_claims() {
        let fixture = Fixture::new(&[("a", false)]).await;
        let task = fixture.import_task("a", "missing remote").await;
        let runtime = Arc::new(FakeRuntime::default());
        let scheduler = fixture.scheduler(runtime.clone(), ExecutionLimits::default());

        let reports = run_tasks(&scheduler, &[task]).await;

        assert_eq!(reports[0].execution_id, None);
        assert_task_state(&fixture.store, task, TaskState::Ready).await;
        assert!(fixture.executions(task).await.is_empty());
        assert!(runtime.observed_requests().is_empty());

        let fixture = Fixture::new(&[("a", true)]).await;
        let task = fixture.import_task("a", "duplicate").await;
        let runtime = Arc::new(FakeRuntime::default());
        let scheduler = fixture.scheduler(
            runtime.clone(),
            ExecutionLimits::new(2, 2, 2, Duration::from_secs(5), 64 * 1024).unwrap(),
        );
        scheduler.submit(task).await.unwrap();
        scheduler.submit(task).await.unwrap();
        let reports = scheduler.run_until_idle().await.unwrap();

        assert_eq!(fixture.executions(task).await.len(), 1);
        assert_eq!(runtime.observed_requests().len(), 1);
        assert_eq!(
            reports
                .iter()
                .filter(|report| report.execution_id.is_some())
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn runtime_receives_only_core_resolved_trusted_working_directory() {
        let fixture = Fixture::new(&[("a", true)]).await;
        let task = fixture
            .import_task_with_extensions(
                "a",
                "extension ignored",
                r#","extensions":{"fake":{"cwd":"C:/attacker","args":["bad"]}}"#,
            )
            .await;
        let runtime = Arc::new(FakeRuntime::default());
        let scheduler = fixture.scheduler(runtime.clone(), ExecutionLimits::default());

        let _reports = run_tasks(&scheduler, &[task]).await;

        let observed = runtime.observed_requests();
        assert_eq!(observed.len(), 1);
        assert_eq!(observed[0].trusted_working_dir, fixture.project_path("a"));
        assert_eq!(observed[0].ignored_extension_count, 1);
    }

    async fn run_tasks<R: Runtime>(
        scheduler: &ExecutionScheduler<R>,
        task_ids: &[TaskId],
    ) -> Vec<ExecutionReport> {
        for task_id in task_ids {
            scheduler.submit(*task_id).await.unwrap();
        }
        scheduler.run_until_idle().await.unwrap()
    }

    async fn assert_task_state(store: &SqliteStore, task_id: TaskId, expected: TaskState) {
        let stored = get_stored_task(store, task_id).await.unwrap().unwrap();
        assert_eq!(stored.state, expected);
    }

    async fn saw_stderr_output(events: &mut broadcast::Receiver<CoreExecutionEvent>) -> bool {
        while let Ok(event) = events.try_recv() {
            if matches!(
                event,
                CoreExecutionEvent::RuntimeOutput {
                    stream: RuntimeOutputStream::Stderr,
                    ..
                }
            ) {
                return true;
            }
        }
        false
    }

    async fn wait_for_started<R: Runtime>(scheduler: &ExecutionScheduler<R>, task_id: TaskId) {
        let mut events = scheduler.subscribe();
        loop {
            if matches!(
                events.recv().await.unwrap(),
                CoreExecutionEvent::ExecutionStarted { task_id: started, .. } if started == task_id
            ) {
                return;
            }
        }
    }

    async fn first_started_tasks(
        events: &mut broadcast::Receiver<CoreExecutionEvent>,
        count: usize,
    ) -> Vec<TaskId> {
        let mut started = Vec::new();
        while started.len() < count {
            if let CoreExecutionEvent::ExecutionStarted { task_id, .. } =
                events.recv().await.unwrap()
            {
                started.push(task_id);
            }
        }
        started
    }

    struct Fixture {
        relay: Relay,
        store: SqliteStore,
        project_paths: HashMap<String, PathBuf>,
    }

    impl Fixture {
        async fn new(projects: &[(&str, bool)]) -> Self {
            let store = SqliteStore::in_memory().await.unwrap();
            let mut registry = crate::ProjectRegistry::empty();
            let mut project_paths = HashMap::new();
            for (key, with_expected_remote) in projects {
                let path = temp_git_repo(key);
                if *with_expected_remote {
                    git(&path, ["remote", "add", "origin", EXPECTED_REMOTE]);
                }
                registry
                    .register(crate::RegisterProject {
                        key: (*key).to_owned(),
                        name: Some((*key).to_owned()),
                        local_path: path.clone(),
                        expected_remote: with_expected_remote.then(|| EXPECTED_REMOTE.to_owned()),
                        default_stack: Some("fake".to_owned()),
                        github: None,
                    })
                    .unwrap();
                project_paths.insert((*key).to_owned(), path);
            }
            Self {
                relay: Relay::with_project_registry(registry),
                store,
                project_paths,
            }
        }

        fn scheduler(
            &self,
            runtime: Arc<FakeRuntime>,
            limits: ExecutionLimits,
        ) -> ExecutionScheduler<FakeRuntime> {
            ExecutionScheduler::new(self.relay.clone(), self.store.clone(), runtime, limits)
        }

        async fn import_task(&self, project: &str, title: &str) -> TaskId {
            self.import_task_with_extensions(project, title, "").await
        }

        async fn import_task_with_extensions(
            &self,
            project: &str,
            title: &str,
            extensions_json: &str,
        ) -> TaskId {
            let json = format!(
                r#"{{
                    "protocol_version": 1,
                    "project": "{project}",
                    "title": "{title}",
                    "stack": "chatgpt-codex",
                    "task": {{"goal": "Run fake runtime."}},
                    "ambiguity_policy": "needs_clarification"
                    {extensions_json}
                }}"#
            );
            let mut relay = self.relay.clone();
            let candidate = relay.preview_local_paste(&json).unwrap();
            let imported = relay.confirm_task_import(candidate).clone();
            crate::durable_state::persist_imported_task(&self.store, &imported)
                .await
                .unwrap();
            imported.id()
        }

        async fn executions(&self, task_id: TaskId) -> Vec<relay_store::ExecutionRecord> {
            self.store
                .list_executions_for_task(&task_id.to_string())
                .await
                .unwrap()
        }

        fn project_path(&self, key: &str) -> PathBuf {
            self.project_paths.get(key).unwrap().clone()
        }
    }

    fn temp_git_repo(name: &str) -> PathBuf {
        let repo = temp_dir(name);
        git(&repo, ["init"]);
        git(
            &repo,
            [
                "-c",
                "user.name=Relay Test",
                "-c",
                "user.email=relay@example.invalid",
                "commit",
                "--allow-empty",
                "-m",
                "initial",
            ],
        );
        repo
    }

    fn temp_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "relay-m6-execution-{name}-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn git<const N: usize>(repo: &std::path::Path, args: [&str; N]) {
        let output = git_output(repo, args);
        assert!(
            output.status.success(),
            "git failed with status {:?}\nstdout:\n{}\nstderr:\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_output<const N: usize>(repo: &std::path::Path, args: [&str; N]) -> Output {
        Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap()
    }
}
