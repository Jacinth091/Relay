//! Phase 06: RuntimeBackplane Seam.
//!
//! Provides the deep provider-neutral execution hosting seam:
//! - `provision`: launches or prepares process context from provenanced request.
//! - `observe`: captures output stream, activity, and terminal states.
//! - `send`: transmits input or control messages to the running attempt.
//! - `interrupt`: requests clean cancellation/termination of the process tree.
//! - `dispose`: reaps resources and ensures process tree death.
//! - `snapshot`: reports backplane-wide capacity and active allocations.
//! - `reconcile`: reconciles durable bindings against live host state after restart.

use std::{collections::HashMap, future::Future, path::PathBuf, pin::Pin};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BackplaneError {
    #[error("backplane io error: {0}")]
    Io(String),
    #[error("process launch failed: {0}")]
    LaunchFailed(String),
    #[error("unauthorized execution directory: {0}")]
    UnauthorizedCwd(PathBuf),
    #[error("handle not found: {0}")]
    HandleNotFound(String),
    #[error("process already exited")]
    ProcessAlreadyExited,
    #[error("binding reconciliation failed: {0}")]
    ReconciliationFailed(String),
    #[error("unsupported backplane operation: {0}")]
    Unsupported(String),
}

/// Backplane identification.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BackplaneId(pub String);

impl BackplaneId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl std::fmt::Display for BackplaneId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Stable effect key identifying an external provisioning action.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct EffectKey(pub String);

impl EffectKey {
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }
}

/// Explicit launch specification. The backplane does NOT receive arbitrary untrusted shell strings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchSpecification {
    pub executable: PathBuf,
    pub args: Vec<String>,
}

impl LaunchSpecification {
    pub fn new(executable: impl Into<PathBuf>, args: Vec<String>) -> Self {
        Self {
            executable: executable.into(),
            args,
        }
    }
}

/// Sanitized environment variables passed to the execution process.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SanitizedEnvironment {
    pub vars: HashMap<String, String>,
}

impl SanitizedEnvironment {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.vars.insert(key.into(), value.into());
    }
}

/// Explicit metadata associated with a runtime attempt.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RuntimeMetadata {
    pub state_root_id: Option<String>,
    pub project_id: Option<String>,
    pub checkout_id: Option<String>,
    pub task_id: Option<String>,
    pub workflow_run_id: Option<String>,
    pub role_instance_id: Option<String>,
    pub role_attempt_id: String,
    pub effect_key: EffectKey,
}

impl RuntimeMetadata {
    pub fn new(role_attempt_id: impl Into<String>, effect_key: EffectKey) -> Self {
        Self {
            role_attempt_id: role_attempt_id.into(),
            effect_key,
            ..Default::default()
        }
    }
}

/// Provisioning request sent to a RuntimeBackplane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeProvisionRequest {
    pub effect_key: EffectKey,
    pub role_attempt_id: String,
    pub trusted_cwd: PathBuf,
    pub launch: LaunchSpecification,
    pub environment: SanitizedEnvironment,
    pub output_byte_limit: usize,
    #[serde(default)]
    pub metadata: Option<RuntimeMetadata>,
}

impl RuntimeProvisionRequest {
    pub fn new(
        effect_key: EffectKey,
        role_attempt_id: impl Into<String>,
        trusted_cwd: impl Into<PathBuf>,
        launch: LaunchSpecification,
        environment: SanitizedEnvironment,
        output_byte_limit: usize,
    ) -> Self {
        Self {
            effect_key,
            role_attempt_id: role_attempt_id.into(),
            trusted_cwd: trusted_cwd.into(),
            launch,
            environment,
            output_byte_limit,
            metadata: None,
        }
    }

    pub fn with_metadata(mut self, metadata: RuntimeMetadata) -> Self {
        self.metadata = Some(metadata);
        self
    }
}

/// Unique identifier for a runtime binding.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RuntimeBindingId(pub String);

impl RuntimeBindingId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl std::fmt::Display for RuntimeBindingId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Lifecycle state of a runtime binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeBindingState {
    Attached,
    Running,
    ExitedCleanly { exit_code: Option<i32> },
    ExitedFailed { exit_code: Option<i32> },
    Interrupted,
    Missing,
    Orphaned,
    Mismatched { reason: String },
    Ambiguous { reason: String },
    Disposed,
}

/// Mapping from Relay role attempt identity to runtime backplane handle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeBinding {
    pub id: RuntimeBindingId,
    pub role_attempt_id: String,
    pub backplane_id: BackplaneId,
    pub runtime_handle: RuntimeHandle,
    pub created_at: String,
    pub last_reconciled_at: String,
    pub state: RuntimeBindingState,
    pub metadata: Option<RuntimeMetadata>,
}

/// An opaque physical runtime handle returned by a backplane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeHandle {
    pub handle_id: String,
    pub backplane_id: BackplaneId,
    pub pid: Option<u32>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessStatus {
    Starting,
    Running,
    ExitedSuccess { exit_code: i32 },
    ExitedFailure { exit_code: Option<i32> },
    Interrupted,
    Lost,
}

/// Live observation chunk from the backplane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeObservation {
    pub handle_id: String,
    pub status: ProcessStatus,
    pub stdout_chunk: Vec<u8>,
    pub stderr_chunk: Vec<u8>,
    pub total_bytes_captured: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendReceipt {
    pub bytes_written: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterruptReceipt {
    pub initiated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisposeReceipt {
    pub reaped: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackplaneSnapshot {
    pub backplane_id: BackplaneId,
    pub active_processes: usize,
    pub capacity_slots: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconcileOutcome {
    Alive { pid: u32 },
    CleanlyExited { exit_code: Option<i32> },
    FailedExit { exit_code: Option<i32> },
    ResourceNotFound,
    Mismatched { reason: String },
}

pub type BackplaneFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, BackplaneError>> + Send + 'a>>;

/// The foundational interface decoupled from Herdr and providers.
pub trait RuntimeBackplane: Send + Sync + 'static {
    fn id(&self) -> BackplaneId;
    fn provision<'a>(&'a self, req: RuntimeProvisionRequest) -> BackplaneFuture<'a, RuntimeHandle>;
    fn observe<'a>(&'a self, handle: &'a RuntimeHandle) -> BackplaneFuture<'a, RuntimeObservation>;
    fn send<'a>(
        &'a self,
        handle: &'a RuntimeHandle,
        input: &'a [u8],
    ) -> BackplaneFuture<'a, SendReceipt>;
    fn interrupt<'a>(&'a self, handle: &'a RuntimeHandle) -> BackplaneFuture<'a, InterruptReceipt>;
    fn dispose<'a>(&'a self, handle: &'a RuntimeHandle) -> BackplaneFuture<'a, DisposeReceipt>;
    fn snapshot<'a>(&'a self) -> BackplaneFuture<'a, BackplaneSnapshot>;
    fn reconcile<'a>(&'a self, handle: &'a RuntimeHandle) -> BackplaneFuture<'a, ReconcileOutcome>;
    fn reconcile_binding<'a>(
        &'a self,
        binding: &'a RuntimeBinding,
    ) -> BackplaneFuture<'a, ReconcileOutcome> {
        self.reconcile(&binding.runtime_handle)
    }
}
