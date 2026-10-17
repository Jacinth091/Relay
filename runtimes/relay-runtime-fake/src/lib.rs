//! Deterministic local fake runtime for Relay Core integration tests.
//!
//! Scenario selection is local configuration. Handoff text and extensions are
//! intentionally ignored for runtime behavior.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use relay_runtime::{
    CancellationToken, Runtime, RuntimeCapabilities, RuntimeError, RuntimeEvent, RuntimeIdentity,
    RuntimeOutcome, RuntimeOutcomeKind, RuntimeOutputStream, RuntimeRequest, RuntimeSession,
    RuntimeStartFuture,
};
use tokio::{sync::mpsc, time::sleep};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FakeScenario {
    Success,
    ImplementationFailureThenRepairSuccess,
    StderrThenSuccess,
    SlowStreaming,
    VerificationFailureThenRepairSuccess,
    RepeatedRepairFailure,
    NeedsClarification,
    NeedsDecision,
    InvalidRuntimeOutput,
    HugeOutput,
}

#[derive(Debug, Clone)]
pub struct FakeRuntimeConfig {
    pub default_scenario: FakeScenario,
    pub scenarios_by_task_id: BTreeMap<String, FakeScenario>,
    pub stream_delay: Duration,
    pub slow_chunks: usize,
    pub huge_chunks: usize,
    pub chunk_size: usize,
}

impl Default for FakeRuntimeConfig {
    fn default() -> Self {
        Self {
            default_scenario: FakeScenario::Success,
            scenarios_by_task_id: BTreeMap::new(),
            stream_delay: Duration::from_millis(10),
            slow_chunks: 8,
            huge_chunks: 256,
            chunk_size: 1024,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FakeRuntime {
    config: Arc<Mutex<FakeRuntimeConfig>>,
    observed_requests: Arc<Mutex<Vec<ObservedRuntimeRequest>>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedRuntimeRequest {
    pub task_id: String,
    pub trusted_working_dir: std::path::PathBuf,
    pub action: String,
    pub intent_title: String,
}

impl FakeRuntime {
    pub fn new(config: FakeRuntimeConfig) -> Self {
        Self {
            config: Arc::new(Mutex::new(config)),
            observed_requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn with_default_scenario(scenario: FakeScenario) -> Self {
        let config = FakeRuntimeConfig {
            default_scenario: scenario,
            ..FakeRuntimeConfig::default()
        };
        Self::new(config)
    }

    pub fn set_task_scenario(&self, task_id: impl Into<String>, scenario: FakeScenario) {
        self.config
            .lock()
            .expect("fake runtime config mutex poisoned")
            .scenarios_by_task_id
            .insert(task_id.into(), scenario);
    }

    pub fn observed_requests(&self) -> Vec<ObservedRuntimeRequest> {
        self.observed_requests
            .lock()
            .expect("fake runtime observed request mutex poisoned")
            .clone()
    }
}

impl Default for FakeRuntime {
    fn default() -> Self {
        Self::new(FakeRuntimeConfig::default())
    }
}

impl Runtime for FakeRuntime {
    fn identity(&self) -> RuntimeIdentity {
        RuntimeIdentity {
            runtime_id: "fake".to_owned(),
            profile_id: "fake/default".to_owned(),
            display_name: "Relay Fake Runtime".to_owned(),
        }
    }

    fn capabilities(&self) -> RuntimeCapabilities {
        RuntimeCapabilities {
            streaming_output: true,
            cancellation: true,
            deterministic_outcome: true,
            permission_support: relay_runtime::RuntimePermissionSupport::PreapprovedOnly,
        }
    }

    fn start(
        &self,
        request: RuntimeRequest,
        cancellation: CancellationToken,
    ) -> RuntimeStartFuture {
        let config = self
            .config
            .lock()
            .expect("fake runtime config mutex poisoned")
            .clone();
        let scenario = config
            .scenarios_by_task_id
            .get(&request.task_id)
            .copied()
            .unwrap_or(config.default_scenario);
        self.observed_requests
            .lock()
            .expect("fake runtime observed request mutex poisoned")
            .push(ObservedRuntimeRequest {
                task_id: request.task_id.clone(),
                trusted_working_dir: request.trusted_working_dir.clone(),
                action: format!("{:?}", request.action),
                intent_title: request.intent.title.clone(),
            });

        Box::pin(async move {
            let (tx, rx) = mpsc::channel(16);
            let session_id = format!("fake-{}", request.execution_id);
            tokio::spawn(async move {
                run_scenario(tx, scenario, request, cancellation, config).await;
            });

            Ok(RuntimeSession {
                session_id,
                events: rx,
                control: None,
            })
        })
    }
}

async fn run_scenario(
    tx: mpsc::Sender<RuntimeEvent>,
    scenario: FakeScenario,
    request: RuntimeRequest,
    cancellation: CancellationToken,
    config: FakeRuntimeConfig,
) {
    let outcome = match scenario {
        FakeScenario::Success => {
            send_stdout(&tx, "fake implementation complete\n").await;
            RuntimeOutcomeKind::Success
        }
        FakeScenario::ImplementationFailureThenRepairSuccess => match request.action {
            relay_runtime::RuntimeAction::Implement => RuntimeOutcomeKind::ImplementationFailure,
            relay_runtime::RuntimeAction::Repair { .. } => RuntimeOutcomeKind::Success,
        },
        FakeScenario::StderrThenSuccess => {
            send_stderr(&tx, "warning: fake stderr\n").await;
            RuntimeOutcomeKind::Success
        }
        FakeScenario::SlowStreaming => {
            for index in 0..config.slow_chunks {
                if cancellation.is_cancelled() {
                    finish(&tx, RuntimeOutcomeKind::Cancelled).await;
                    return;
                }
                send_stdout(&tx, &format!("slow chunk {index}\n")).await;
                sleep(config.stream_delay).await;
            }
            RuntimeOutcomeKind::Success
        }
        FakeScenario::VerificationFailureThenRepairSuccess => match request.action {
            relay_runtime::RuntimeAction::Implement => RuntimeOutcomeKind::VerificationFailure,
            relay_runtime::RuntimeAction::Repair { .. } => RuntimeOutcomeKind::Success,
        },
        FakeScenario::RepeatedRepairFailure => RuntimeOutcomeKind::ImplementationFailure,
        FakeScenario::NeedsClarification => RuntimeOutcomeKind::NeedsClarification,
        FakeScenario::NeedsDecision => RuntimeOutcomeKind::NeedsDecision,
        FakeScenario::InvalidRuntimeOutput => {
            let _ = tx
                .send(RuntimeEvent::InvalidOutput {
                    reason: "fake invalid output".to_owned(),
                })
                .await;
            return;
        }
        FakeScenario::HugeOutput => {
            let chunk = "x".repeat(config.chunk_size);
            for _ in 0..config.huge_chunks {
                if cancellation.is_cancelled() {
                    finish(&tx, RuntimeOutcomeKind::Cancelled).await;
                    return;
                }
                send_stdout(&tx, &chunk).await;
            }
            RuntimeOutcomeKind::Success
        }
    };

    if cancellation.is_cancelled() {
        finish(&tx, RuntimeOutcomeKind::Cancelled).await;
    } else {
        finish(&tx, outcome).await;
    }
}

async fn send_stdout(tx: &mpsc::Sender<RuntimeEvent>, text: &str) {
    let _ = tx
        .send(RuntimeEvent::Output {
            stream: RuntimeOutputStream::Stdout,
            bytes: text.as_bytes().to_vec(),
        })
        .await;
}

async fn send_stderr(tx: &mpsc::Sender<RuntimeEvent>, text: &str) {
    let _ = tx
        .send(RuntimeEvent::Output {
            stream: RuntimeOutputStream::Stderr,
            bytes: text.as_bytes().to_vec(),
        })
        .await;
}

async fn finish(tx: &mpsc::Sender<RuntimeEvent>, kind: RuntimeOutcomeKind) {
    let _ = tx
        .send(RuntimeEvent::Outcome(RuntimeOutcome::new(
            kind,
            format!("fake outcome: {}", kind.as_str()),
        )))
        .await;
}

#[allow(dead_code)]
fn _runtime_error_type_is_public(error: RuntimeError) -> RuntimeError {
    error
}

// ---------------------------------------------------------------------------
// Phase 06: RuntimeBackplane implementation for FakeRuntime
// ---------------------------------------------------------------------------

use relay_runtime::{
    BackplaneError, BackplaneFuture, BackplaneId, BackplaneSnapshot, DisposeReceipt,
    InterruptReceipt, ProcessStatus, ReconcileOutcome, RuntimeBackplane, RuntimeHandle,
    RuntimeObservation, RuntimeProvisionRequest, SendReceipt,
};

#[derive(Clone, Default)]
pub struct FakeBackplane {
    inner: FakeRuntime,
    active_handles: Arc<Mutex<HashMap<String, RuntimeProvisionRequest>>>,
    sent_inputs: Arc<Mutex<HashMap<String, Vec<u8>>>>,
}

use std::collections::HashMap;

impl FakeBackplane {
    pub fn new() -> Self {
        Self {
            inner: FakeRuntime::default(),
            active_handles: Arc::new(Mutex::new(HashMap::new())),
            sent_inputs: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn inner(&self) -> &FakeRuntime {
        &self.inner
    }

    pub fn get_sent_input(&self, handle_id: &str) -> Option<Vec<u8>> {
        self.sent_inputs.lock().unwrap().get(handle_id).cloned()
    }
}

impl RuntimeBackplane for FakeBackplane {
    fn id(&self) -> BackplaneId {
        BackplaneId::new("fake-backplane")
    }

    fn provision<'a>(&'a self, req: RuntimeProvisionRequest) -> BackplaneFuture<'a, RuntimeHandle> {
        Box::pin(async move {
            let handle_id = format!("fake-h-{}", req.role_attempt_id);
            let handle = RuntimeHandle {
                handle_id: handle_id.clone(),
                backplane_id: self.id(),
                pid: Some(42),
                created_at: "now".to_owned(),
            };
            self.active_handles.lock().unwrap().insert(handle_id, req);
            Ok(handle)
        })
    }

    fn observe<'a>(&'a self, handle: &'a RuntimeHandle) -> BackplaneFuture<'a, RuntimeObservation> {
        Box::pin(async move {
            let active = self.active_handles.lock().unwrap();
            if active.contains_key(&handle.handle_id) {
                Ok(RuntimeObservation {
                    handle_id: handle.handle_id.clone(),
                    status: ProcessStatus::ExitedSuccess { exit_code: 0 },
                    stdout_chunk: b"fake backplane execution output\n".to_vec(),
                    stderr_chunk: Vec::new(),
                    total_bytes_captured: 32,
                })
            } else {
                Err(BackplaneError::HandleNotFound(handle.handle_id.clone()))
            }
        })
    }

    fn send<'a>(
        &'a self,
        handle: &'a RuntimeHandle,
        input: &'a [u8],
    ) -> BackplaneFuture<'a, SendReceipt> {
        Box::pin(async move {
            self.sent_inputs
                .lock()
                .unwrap()
                .entry(handle.handle_id.clone())
                .or_default()
                .extend_from_slice(input);
            Ok(SendReceipt {
                bytes_written: input.len(),
            })
        })
    }

    fn interrupt<'a>(
        &'a self,
        _handle: &'a RuntimeHandle,
    ) -> BackplaneFuture<'a, InterruptReceipt> {
        Box::pin(async move { Ok(InterruptReceipt { initiated: true }) })
    }

    fn dispose<'a>(&'a self, handle: &'a RuntimeHandle) -> BackplaneFuture<'a, DisposeReceipt> {
        Box::pin(async move {
            let mut active = self.active_handles.lock().unwrap();
            let reaped = active.remove(&handle.handle_id).is_some();
            Ok(DisposeReceipt { reaped })
        })
    }

    fn snapshot<'a>(&'a self) -> BackplaneFuture<'a, BackplaneSnapshot> {
        Box::pin(async move {
            let count = self.active_handles.lock().unwrap().len();
            Ok(BackplaneSnapshot {
                backplane_id: self.id(),
                active_processes: count,
                capacity_slots: 100,
            })
        })
    }

    fn reconcile<'a>(&'a self, handle: &'a RuntimeHandle) -> BackplaneFuture<'a, ReconcileOutcome> {
        Box::pin(async move {
            let active = self.active_handles.lock().unwrap();
            if active.contains_key(&handle.handle_id) {
                Ok(ReconcileOutcome::Alive { pid: 42 })
            } else {
                Ok(ReconcileOutcome::ResourceNotFound)
            }
        })
    }
}
