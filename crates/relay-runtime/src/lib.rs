//! Provider-neutral runtime boundary for Relay execution.
//!
//! Runtime implementations receive trusted local execution context from Core.
//! They do not resolve projects, choose repositories, or own task lifecycle.

use std::{
    fmt,
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use relay_protocol::AmbiguityPolicy;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::{Notify, mpsc};
use uuid::Uuid;

pub type RuntimeStartFuture =
    Pin<Box<dyn Future<Output = Result<RuntimeSession, RuntimeError>> + Send + 'static>>;

/// Relay-owned execution identity, independent of task and runtime session IDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ExecutionId(Uuid);

impl ExecutionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn parse(input: impl AsRef<str>) -> Result<Self, uuid::Error> {
        Uuid::parse_str(input.as_ref()).map(Self)
    }
}

impl Default for ExecutionId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for ExecutionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeIdentity {
    pub runtime_id: String,
    pub profile_id: String,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeCapabilities {
    pub streaming_output: bool,
    pub cancellation: bool,
    pub deterministic_outcome: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeAction {
    Implement,
    Repair {
        attempt: u32,
        previous_outcome: RuntimeOutcomeKind,
    },
}

#[derive(Debug, Clone)]
pub struct RuntimeRequest {
    pub execution_id: ExecutionId,
    pub task_id: String,
    pub intent: RuntimeTaskIntent,
    pub trusted_working_dir: PathBuf,
    pub action: RuntimeAction,
    pub profile_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeTaskIntent {
    pub title: String,
    pub goal: String,
    pub requirements: Vec<String>,
    pub constraints: Vec<String>,
    pub verification: Vec<String>,
    pub requested_stack: Option<String>,
    pub ambiguity_policy: AmbiguityPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeOutputStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeEvent {
    Output {
        stream: RuntimeOutputStream,
        bytes: Vec<u8>,
    },
    Outcome(RuntimeOutcome),
    InvalidOutput {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeOutcome {
    pub kind: RuntimeOutcomeKind,
    pub summary: String,
}

impl RuntimeOutcome {
    pub fn new(kind: RuntimeOutcomeKind, summary: impl Into<String>) -> Self {
        Self {
            kind,
            summary: summary.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeOutcomeKind {
    Success,
    ImplementationFailure,
    VerificationFailure,
    NeedsClarification,
    NeedsDecision,
    Cancelled,
    InvalidOutput,
    RuntimeFailure,
    Timeout,
}

impl RuntimeOutcomeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::ImplementationFailure => "implementation_failure",
            Self::VerificationFailure => "verification_failure",
            Self::NeedsClarification => "needs_clarification",
            Self::NeedsDecision => "needs_decision",
            Self::Cancelled => "cancelled",
            Self::InvalidOutput => "invalid_output",
            Self::RuntimeFailure => "runtime_failure",
            Self::Timeout => "timeout",
        }
    }
}

#[derive(Debug)]
pub struct RuntimeSession {
    pub session_id: String,
    pub events: mpsc::Receiver<RuntimeEvent>,
}

pub trait Runtime: Send + Sync + 'static {
    fn identity(&self) -> RuntimeIdentity;
    fn capabilities(&self) -> RuntimeCapabilities;
    fn start(&self, request: RuntimeRequest, cancellation: CancellationToken)
    -> RuntimeStartFuture;
}

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("runtime failed before event stream started: {0}")]
    StartFailed(String),
}

#[derive(Debug, Clone, Default)]
pub struct CancellationToken {
    inner: Arc<CancellationInner>,
}

#[derive(Debug, Default)]
struct CancellationInner {
    cancelled: AtomicBool,
    notify: Notify,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        if !self.inner.cancelled.swap(true, Ordering::SeqCst) {
            self.inner.notify.notify_waiters();
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    pub async fn cancelled(&self) {
        if self.is_cancelled() {
            return;
        }
        self.inner.notify.notified().await;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeDeadline {
    pub max_duration: Duration,
}

mod activity;
mod integration;
mod profile;
mod router;
pub mod sanitizer;

pub use activity::*;
pub use integration::*;
pub use profile::*;
pub use router::*;
pub use sanitizer::*;
