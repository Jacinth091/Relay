//! Phase 08: Provider Adapter Contract, Execution Targets, and Role Attempts.
//!
//! Separates provider semantics (what a CLI/agent understands) from runtime
//! hosting (how a process is launched/observed, owned by `RuntimeBackplane`).
//! A `ProviderAdapter` translates a resolved `ExecutionTarget` and a
//! runtime-layer `RoleAttempt` into a concrete `LaunchSpecification`, and
//! normalizes raw `RuntimeObservation` chunks into provider-neutral
//! `ProviderReadiness` facts. It never grants policy, chooses projects, or
//! owns workflow/task semantics -- those remain Relay Core concerns.
//!
//! `RoleAttempt` here is the runtime-layer projection: just enough identity
//! and requirements for provider resolution and launch construction. Relay
//! Core owns the full semantic role/attempt lifecycle (state machine,
//! artifacts, budgets) and maps its own domain model down to this seam.

use std::{collections::BTreeSet, path::PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    EffectKey, LaunchSpecification, ProcessStatus, RuntimeModelSelection, RuntimeObservation,
    RuntimeReasoningSelection,
    role_result::{ResultDefect, observed_role_result},
};

mod codex;
mod mock;

pub use codex::CodexProviderAdapter;
pub use mock::MockProviderAdapter;

// ---------------------------------------------------------------------------
// Identifiers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ProviderId(pub String);

impl ProviderId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl std::fmt::Display for ProviderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RoleAttemptId(pub String);

impl RoleAttemptId {
    pub fn generate(role_instance_id: &str, attempt_no: u32) -> Self {
        Self(format!("{role_instance_id}-att{attempt_no}"))
    }
}

impl std::fmt::Display for RoleAttemptId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// Capabilities & Health
// ---------------------------------------------------------------------------

/// Declares what an installed provider can do, independent of any one attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    /// Semantic role purposes this provider can fulfil (e.g. "implementer", "verifier").
    pub supported_roles: BTreeSet<String>,
    pub filesystem_write: bool,
    pub network: bool,
    pub streaming_output: bool,
    pub cancellation: bool,
    pub supported_models: Vec<String>,
}

impl ProviderCapabilities {
    /// Hard compatibility filter: must support the requested role and any
    /// required capabilities.
    pub fn satisfies(&self, requirements: &ProviderCapabilityRequirements) -> bool {
        self.supported_roles.contains(&requirements.required_role)
            && (!requirements.requires_filesystem_write || self.filesystem_write)
            && (!requirements.requires_network || self.network)
    }
}

/// Capability requirements a role attempt needs a resolved provider to meet.
/// This schema is provider-agnostic: any number of providers may satisfy the
/// same requirements without the workflow/role schema changing.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ProviderCapabilityRequirements {
    pub required_role: String,
    pub requires_filesystem_write: bool,
    pub requires_network: bool,
}

impl ProviderCapabilityRequirements {
    pub fn new(required_role: impl Into<String>) -> Self {
        Self {
            required_role: required_role.into(),
            requires_filesystem_write: false,
            requires_network: false,
        }
    }
}

/// Observable provider health facts. Never a semantic guarantee of success.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderHealthSnapshot {
    Healthy,
    QuotaExhausted,
    AuthRequired,
    Unavailable { reason: String },
}

impl ProviderHealthSnapshot {
    /// True only when the provider is eligible for new attempt assignment.
    pub fn is_eligible(&self) -> bool {
        matches!(self, Self::Healthy)
    }
}

// ---------------------------------------------------------------------------
// Execution Target (immutable per attempt)
// ---------------------------------------------------------------------------

/// Immutable execution target assigned to exactly one `RoleAttempt`.
///
/// Once resolved and assigned, every field is fixed for the lifetime of that
/// attempt: provider pin, effective runtime profile, model specification,
/// and effort/reasoning bounds. A failed attempt never mutates its
/// `ExecutionTarget` in place; a fresh `RoleAttempt` (with its own freshly
/// resolved `ExecutionTarget`) is created instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionTarget {
    pub provider_id: ProviderId,
    pub runtime_profile_id: String,
    pub model: RuntimeModelSelection,
    pub reasoning_effort: RuntimeReasoningSelection,
    pub capability_snapshot: ProviderCapabilities,
    pub provider_adapter_version: String,
}

// ---------------------------------------------------------------------------
// RoleAttempt (runtime-layer view)
// ---------------------------------------------------------------------------

/// Runtime-layer view of a role attempt: just enough for provider resolution
/// and launch construction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleAttempt {
    pub id: RoleAttemptId,
    pub role_instance_id: String,
    pub attempt_no: u32,
    pub required_capabilities: ProviderCapabilityRequirements,
    /// Explicit provider pin, if the operator/policy fixed the provider.
    pub provider_pin: Option<ProviderId>,
    /// Providers already tried and failed for this role instance; fallback
    /// resolution skips these so it never retries a provider that just failed.
    pub excluded_providers: BTreeSet<ProviderId>,
    pub runtime_profile_id: String,
    pub model: RuntimeModelSelection,
    pub reasoning_effort: RuntimeReasoningSelection,
}

impl RoleAttempt {
    pub fn new(
        role_instance_id: impl Into<String>,
        attempt_no: u32,
        required_capabilities: ProviderCapabilityRequirements,
    ) -> Self {
        let role_instance_id = role_instance_id.into();
        Self {
            id: RoleAttemptId::generate(&role_instance_id, attempt_no),
            role_instance_id,
            attempt_no,
            required_capabilities,
            provider_pin: None,
            excluded_providers: BTreeSet::new(),
            runtime_profile_id: String::new(),
            model: RuntimeModelSelection::ProviderDefault,
            reasoning_effort: RuntimeReasoningSelection::ProfileDefault,
        }
    }

    pub fn with_pin(mut self, provider_id: ProviderId) -> Self {
        self.provider_pin = Some(provider_id);
        self
    }

    /// Builds the next attempt for the SAME role instance after a provider
    /// failure or exhaustion. `role_instance_id` never changes across
    /// retries (invariant: retrying an attempt does not create a new
    /// semantic role); only `attempt_no`/`id` advance and the failed
    /// provider is added to the exclusion set so deterministic fallback
    /// moves on to the next eligible candidate. An explicit pin is preserved
    /// unchanged -- provider failure never silently swaps an explicit pin.
    pub fn retry_after_failure(&self, failed_provider: ProviderId) -> Self {
        let mut excluded_providers = self.excluded_providers.clone();
        excluded_providers.insert(failed_provider);
        Self {
            id: RoleAttemptId::generate(&self.role_instance_id, self.attempt_no + 1),
            role_instance_id: self.role_instance_id.clone(),
            attempt_no: self.attempt_no + 1,
            required_capabilities: self.required_capabilities.clone(),
            provider_pin: self.provider_pin.clone(),
            excluded_providers,
            runtime_profile_id: self.runtime_profile_id.clone(),
            model: self.model.clone(),
            reasoning_effort: self.reasoning_effort,
        }
    }
}

// ---------------------------------------------------------------------------
// Launch context & readiness
// ---------------------------------------------------------------------------

/// Trusted launch context supplied by Core. Never carries untrusted shell
/// text; the backplane receives only explicit executable/argument arrays.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchContext {
    pub trusted_cwd: PathBuf,
    pub role_purpose: String,
    pub effect_key: EffectKey,
}

impl LaunchContext {
    pub fn new(
        trusted_cwd: impl Into<PathBuf>,
        role_purpose: impl Into<String>,
        effect_key: EffectKey,
    ) -> Self {
        Self {
            trusted_cwd: trusted_cwd.into(),
            role_purpose: role_purpose.into(),
            effect_key,
        }
    }
}

/// Provider-neutral readiness fact derived from a raw `RuntimeObservation`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderReadiness {
    ReadyForAssignment,
    WaitingForUser,
    ResultPublished,
    ProviderError { reason: String },
}

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("provider is unavailable: {0}")]
    Unavailable(String),
    #[error("required capabilities not supported: {0}")]
    UnsupportedCapability(String),
    #[error("failed to build launch specification: {0}")]
    LaunchBuildFailed(String),
}

// ---------------------------------------------------------------------------
// ProviderAdapter contract
// ---------------------------------------------------------------------------

/// Translates provider-specific launch, readiness, and result behavior.
/// Implementations must be deterministic and side-effect-free with respect
/// to workflow/policy state -- they never grant permissions, choose
/// projects, or advance role/task lifecycle.
pub trait ProviderAdapter: Send + Sync + 'static {
    fn provider_id(&self) -> ProviderId;
    fn capabilities(&self) -> ProviderCapabilities;
    fn health_check(&self) -> ProviderHealthSnapshot;
    fn build_launch(
        &self,
        target: &ExecutionTarget,
        attempt: &RoleAttempt,
        context: &LaunchContext,
    ) -> Result<LaunchSpecification, ProviderError>;
    fn interpret_observation(&self, obs: &RuntimeObservation) -> ProviderReadiness;
}

/// Shared readiness interpretation for adapters driven by a generic
/// `ProcessStatus`-based backplane observation, given an adapter-specific
/// heuristic for detecting a mid-run "waiting for user" marker in stdout.
///
/// Provider exit is never a role result. A process that exited successfully
/// counts as `ResultPublished` only when it actually published a well-formed
/// typed result envelope on stdout; exiting 0 with no envelope (or with an
/// unusable one) is reported as a provider error.
pub(crate) fn interpret_process_observation(
    obs: &RuntimeObservation,
    waiting_for_user: impl FnOnce(&RuntimeObservation) -> bool,
) -> ProviderReadiness {
    match &obs.status {
        ProcessStatus::ExitedSuccess { .. } => match observed_role_result(obs) {
            Ok(_) => ProviderReadiness::ResultPublished,
            Err(ResultDefect::Missing) => ProviderReadiness::ProviderError {
                reason:
                    "provider process exited successfully without publishing a typed role result"
                        .to_owned(),
            },
            Err(ResultDefect::Malformed(reason)) => ProviderReadiness::ProviderError {
                reason: format!("provider process published an unusable role result: {reason}"),
            },
        },
        ProcessStatus::ExitedFailure { exit_code } => ProviderReadiness::ProviderError {
            reason: format!("process exited with failure (exit_code={exit_code:?})"),
        },
        ProcessStatus::Interrupted => ProviderReadiness::ProviderError {
            reason: "process was interrupted".to_owned(),
        },
        ProcessStatus::Lost => ProviderReadiness::ProviderError {
            reason: "process was lost".to_owned(),
        },
        ProcessStatus::Starting => ProviderReadiness::ReadyForAssignment,
        ProcessStatus::Running => {
            if waiting_for_user(obs) {
                ProviderReadiness::WaitingForUser
            } else {
                ProviderReadiness::ReadyForAssignment
            }
        }
    }
}
