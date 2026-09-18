//! Provider-agnostic Relay core.
//!
//! Presentation-framework types do not belong in this crate.

pub mod artifact;
pub mod candidate;
pub mod daemon;
pub mod delegation;
mod durable_state;
pub mod exchange;
mod execution;
mod execution_planning;
pub mod legacy_adapter;
pub mod policy;
pub mod portable_config;
pub mod privileged;
pub mod project_identity;
mod project_registry;
pub mod recovery_v2;
mod repository_safety;
pub mod retention;
pub mod scheduler_v2;
mod task_ingress;
pub mod workflow;

use std::path::Path;

pub use relay_protocol::{AmbiguityPolicy, HandoffTask};
pub use relay_runtime::{
    ReasoningEffort, RuntimeExecutionSelection, RuntimeModelSelection, RuntimeReasoningSelection,
};

pub use artifact::{
    ArtifactError, ArtifactId, ArtifactKind, ArtifactMetadata, ArtifactSensitivity,
    ArtifactStorageRef, ArtifactStore, ArtifactTrustClass, CreateArtifactRequest, RetentionClass,
};
pub use candidate::{
    Candidate, CandidateError, CandidateFingerprint, CandidateId, Lease, LeaseHolder, LeaseId,
    LeaseKind, LeaseResource, LeaseState, WorktreeHandle, WorktreeManager, compute_digest,
};
pub use daemon::{
    CommandStatus, DAEMON_PROTOCOL_NAME, DAEMON_PROTOCOL_VERSION, DEFAULT_LEASE_ID,
    DEFAULT_LEASE_TTL, DaemonClient, DaemonCommand, DaemonError, DaemonEvent, DaemonPendingEffect,
    DaemonRequest, DaemonResponse, DaemonServer, DaemonSnapshot, DaemonTaskSummary,
    InProcessDaemonClient,
};
pub use delegation::{
    BudgetAccounting, BudgetError, ConsultationRequest, DelegationError, DelegationManager,
    DelegationMode, DelegationOutcome, DelegationRequest, IndependenceRequirement,
    RoleDependencyGraph, RoleDependencyNode, RoleDependencyState,
};
pub use durable_state::{
    SqliteStore, StoredTask, TaskPersistenceError, TaskState, default_database_path,
    get_stored_task, list_stored_tasks, list_stored_tasks_with_quarantined,
    update_stored_task_state,
};
pub use exchange::{
    ExchangeError, ExchangeMessage, ExchangeMessageId, ExchangeMessageKind, ExchangeOperation,
    IssueCapabilityRequest, PublishResultRequest, RelayExchange, RoleInbox, RoleSessionCapability,
    RoleSessionToken, SendMessageRequest,
};
pub use execution::{
    BoundedOutputSnapshot, CoreExecutionEvent, ExecutionLimits, ExecutionReport,
    ExecutionRunPolicy, ExecutionScheduler, ExecutionSchedulerError, PreflightFailure,
    RetainedOutputChunk, SchedulerSnapshot, SchedulerTaskSnapshot, SchedulerTaskStatus,
};
pub use execution_planning::{
    ExecutionPlan, ExecutionPlanEnvironment, ExecutionPlanIntent, ExecutionPlanningError,
    PLANNED_MAX_DURATION, PLANNED_MAX_REPAIR_ATTEMPTS, build_execution_plan,
};
pub use legacy_adapter::{
    LegacyAdapterError, adapt_v1_handoff_to_approved_intent, create_v1_compatibility_run,
    v1_compatibility_workflow_definition,
};
pub use policy::{
    Capability, EnforcementClass, PolicyCompiler, PolicyDecision, PolicyDisposition, PolicyLayer,
    built_in_safety_floor,
};
pub use portable_config::{
    POLICIES_CONFIG_FILE, PROJECT_CONFIG_FILE, PortableConfigError, PortableRepositoryConfig,
    RELAY_DIR, RawPoliciesToml, RawRestrictionsSection, TrustClassification,
};
pub use privileged::{
    ApprovalGrant, ApprovalRequest, ApprovalRequestId, GitOperationRunner, ParameterDigest,
    PrivilegedAction, PrivilegedActionExecutor, PrivilegedError, PrivilegedExecutionOutcome,
    PrivilegedOperationRunner, PrivilegedRunOutcome,
};
pub use project_identity::{
    Checkout, CheckoutId, CheckoutTrustState, DeclaredProjectIdentity, ProjectId,
    ProjectIdentityError, RemoteFingerprint, TrustedProjectRegistration,
};
pub use project_registry::{
    ExpectedRemote, GitHubRepositoryMetadata, ProjectKey, ProjectRegistry, ProjectRegistryError,
    RegisterProject, TrustedProject, default_config_path,
};
pub use recovery_v2::{
    AttemptReconciliation, CandidateIntegrity, RecoveryCoordinator, RecoveryError, RecoveryReport,
};
pub use repository_safety::{
    BranchState, GitInspectionOperation, NormalizedRemote, RepositoryEligibility,
    RepositoryInspection, RepositorySafetyFailure, RepositoryStatus,
    inspect_trusted_project_repository,
};
pub use retention::{CleanupReport, RetentionError, RetentionManager, RetentionPolicy};
pub use scheduler_v2::{
    ActiveExecutionSlot, QueuedWorkItem, SchedulePriority, SchedulerV2, SchedulerV2Config,
    SchedulerV2Error,
};
pub use task_ingress::{
    ImportedTask, MAX_LOCAL_TASK_INPUT_BYTES, SubmissionTransport, TaskId, TaskImportCandidate,
    TaskIngressError,
};
pub use workflow::{
    ApprovedIntent, ApprovedIntentId, ClarificationRequest, ConfigSnapshot, ConfigSnapshotId,
    DecisionRequest, ExecutionBudget, ExecutionGrant, ExecutionGrantId, ExecutionTarget,
    RoleAttempt, RoleAttemptId, RoleAttemptState, RoleDefinition, RoleDefinitionId, RoleInstance,
    RoleInstanceId, RoleInstanceState, RoleWriteMode, ScopeExpansionRequest, WorkflowDefinition,
    WorkflowDefinitionId, WorkflowError, WorkflowNodeDefinition, WorkflowRun, WorkflowRunId,
    WorkflowRunState, WorkflowTransitionDefinition,
};

/// Minimal Relay application state.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Relay {
    project_registry: ProjectRegistry,
    reviewed_task: Option<ImportedTask>,
}

impl Relay {
    /// Creates Relay with deterministic empty state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates Relay from an already validated project registry.
    pub fn with_project_registry(project_registry: ProjectRegistry) -> Self {
        Self {
            project_registry,
            reviewed_task: None,
        }
    }

    /// Loads Relay's trusted local project registry from a TOML config path.
    pub fn load_from_config_path(path: impl AsRef<Path>) -> Result<Self, ProjectRegistryError> {
        Ok(Self::with_project_registry(ProjectRegistry::load(path)?))
    }

    /// Returns the number of configured projects currently visible to Relay.
    pub fn configured_project_count(&self) -> usize {
        self.project_registry.len()
    }

    pub fn project_registry(&self) -> &ProjectRegistry {
        &self.project_registry
    }

    pub fn project_registry_mut(&mut self) -> &mut ProjectRegistry {
        &mut self.project_registry
    }

    pub fn list_projects(&self) -> impl ExactSizeIterator<Item = &TrustedProject> {
        self.project_registry.list()
    }

    pub fn resolve_project(
        &self,
        key: impl AsRef<str>,
    ) -> Result<&TrustedProject, ProjectRegistryError> {
        self.project_registry.get(key)
    }

    pub fn inspect_project_repository(
        &self,
        key: impl AsRef<str>,
    ) -> Result<RepositoryInspection, ProjectRegistryError> {
        let project = self.resolve_project(key)?;
        Ok(inspect_trusted_project_repository(project))
    }

    pub fn register_project(
        &mut self,
        request: RegisterProject,
    ) -> Result<&TrustedProject, ProjectRegistryError> {
        self.project_registry.register(request)
    }

    pub fn save_project_registry(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<(), ProjectRegistryError> {
        self.project_registry.save(path)
    }
}

/// Returns the Relay Core crate version.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initializes_with_deterministic_empty_state() {
        let relay = Relay::new();

        assert_eq!(relay.configured_project_count(), 0);
        assert_eq!(relay, Relay::default());
    }

    #[test]
    fn exposes_crate_version() {
        assert_eq!(version(), "0.1.0");
    }
}
