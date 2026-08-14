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
    pub ignored_extension_count: usize,
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
                ignored_extension_count: request.handoff.extensions.len(),
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
