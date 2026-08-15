//! Provider-agnostic Relay core.
//!
//! Presentation-framework types do not belong in this crate.

mod durable_state;
mod execution;
mod project_registry;
mod repository_safety;
mod task_ingress;

use std::path::Path;

pub use relay_protocol::{AmbiguityPolicy, HandoffTask};

pub use durable_state::{
    SqliteStore, StoredTask, TaskPersistenceError, TaskState, default_database_path,
    get_stored_task, list_stored_tasks, update_stored_task_state,
};
pub use execution::{
    BoundedOutputSnapshot, CoreExecutionEvent, ExecutionLimits, ExecutionReport,
    ExecutionRunPolicy, ExecutionScheduler, ExecutionSchedulerError, PreflightFailure,
    RetainedOutputChunk, SchedulerSnapshot, SchedulerTaskSnapshot, SchedulerTaskStatus,
};
pub use project_registry::{
    ExpectedRemote, GitHubRepositoryMetadata, ProjectKey, ProjectRegistry, ProjectRegistryError,
    RegisterProject, TrustedProject, default_config_path,
};
pub use repository_safety::{
    BranchState, GitInspectionOperation, NormalizedRemote, RepositoryEligibility,
    RepositoryInspection, RepositorySafetyFailure, RepositoryStatus,
    inspect_trusted_project_repository,
};
pub use task_ingress::{
    ImportedTask, SubmissionTransport, TaskId, TaskImportCandidate, TaskIngressError,
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
