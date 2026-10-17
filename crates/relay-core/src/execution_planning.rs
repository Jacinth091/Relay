//! Deterministic execution planning from user intent.
//!
//! Presentation layers describe *what* should run (project, task reference,
//! optional profile/model/effort preferences); this module owns *how* the run
//! is bounded: profile resolution, stack validation, limit clamping, and
//! permission envelope construction. No LLM reasoning and no provider calls
//! happen here; provider facts enter only as plain-data snapshots supplied by
//! the host (`ExecutionPlanEnvironment`).

use std::time::Duration;

use relay_runtime::{
    EffectiveRuntimeProfile, FilesystemCapability, ReasoningEffort, RuntimeExecutionSelection,
    RuntimeModelCatalog, RuntimeModelSelection, RuntimePermissionEnvelope,
    RuntimeReasoningSelection,
};
use thiserror::Error;

use crate::{
    ExecutionLimits, ExecutionRunPolicy, ProjectKey, StoredTask, TaskId, TaskPersistenceError,
    TrustedProject,
};

/// Upper bound on planned run duration regardless of caller preference.
pub const PLANNED_MAX_DURATION: Duration = Duration::from_secs(1800);

/// Upper bound on planned repair attempts regardless of caller preference.
pub const PLANNED_MAX_REPAIR_ATTEMPTS: u32 = 2;

/// Stacks Relay can plan executions for today.
///
/// Seam: once profiles or the project registry carry per-stack capability
/// metadata, this explicit allowlist should be replaced by capability checks
/// (invariant 15). Until then the allowlist lives here, inside core, where the
/// policy decision belongs.
const SUPPORTED_STACKS: [&str; 4] = ["chatgpt-codex", "codex", "antigravity", "agy"];

/// User intent for one execution run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecutionPlanIntent {
    /// Preferred runtime profile id (e.g. the UI's active profile).
    /// Falls back to the scheduler runtime's own identity when absent.
    pub preferred_profile_id: Option<String>,
    /// Preferred model id; `"default"` means the provider default.
    pub preferred_model: Option<String>,
    /// Preferred reasoning effort override.
    pub preferred_reasoning_effort: Option<ReasoningEffort>,
}

/// Host-provided plain-data snapshots of provider facts.
///
/// Core never queries providers itself; absent snapshots skip the
/// corresponding validation and fall back to conservative defaults, matching
/// behavior when no provider integration is available.
#[derive(Debug, Clone, Default)]
pub struct ExecutionPlanEnvironment {
    /// Effective capability snapshot for the resolved profile, if resolvable
    /// by the host. Drives the permission envelope; absent means the default
    /// (least-privilege) envelope.
    pub effective_profile: Option<EffectiveRuntimeProfile>,
    /// Model catalog of the resolved profile's provider, if discoverable.
    /// Drives cross-provider model/effort validation; absent skips those
    /// checks.
    pub model_catalog: Option<RuntimeModelCatalog>,
}

/// A fully validated, clamped plan ready for `submit_with_policy`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionPlan {
    pub task_id: TaskId,
    pub project_key: ProjectKey,
    pub policy: ExecutionRunPolicy,
}

#[derive(Debug, Error)]
pub enum ExecutionPlanningError {
    #[error("task {task_id} was not found")]
    TaskNotFound { task_id: TaskId },
    #[error(transparent)]
    Store(#[from] TaskPersistenceError),
    #[error("project '{project_key}' is not registered")]
    ProjectNotRegistered { project_key: String },
    #[error("no runtime stack is configured")]
    StackUnconfigured,
    #[error("unsupported runtime stack '{stack}'")]
    UnsupportedStack { stack: String },
    #[error(
        "one-run model '{model}' is not available from provider '{provider}'; \
         it may belong to another provider"
    )]
    ModelNotAvailableFromProvider { model: String, provider: String },
    #[error("one-run reasoning '{effort:?}' is not supported by model '{model_id}'")]
    ReasoningUnsupportedByModel {
        effort: ReasoningEffort,
        model_id: String,
    },
}

/// Deterministically plans an execution run from user intent plus trusted
/// local facts (stored task, registered project).
///
/// The returned plan carries:
/// - the resolved profile and model/reasoning selection;
/// - a permission envelope whose trusted workspace comes **only** from the
///   registered project, and whose write/network capabilities come from the
///   host-provided effective profile snapshot (default envelope when absent);
/// - limits clamped to [`PLANNED_MAX_DURATION`] / [`PLANNED_MAX_REPAIR_ATTEMPTS`]
///   and to the scheduler ceiling.
#[allow(clippy::too_many_arguments)]
pub fn build_execution_plan(
    stored_task: &StoredTask,
    project: &TrustedProject,
    intent: ExecutionPlanIntent,
    limits: &ExecutionLimits,
    environment: &ExecutionPlanEnvironment,
    fallback_profile_id: &str,
) -> Result<ExecutionPlan, ExecutionPlanningError> {
    let profile_id = intent
        .preferred_profile_id
        .clone()
        .unwrap_or_else(|| fallback_profile_id.to_owned());
    let provider = profile_id.split('/').next().unwrap_or_default().to_owned();

    let raw_model = intent
        .preferred_model
        .as_deref()
        .filter(|model| *model != "default");
    let model = match raw_model {
        Some(explicit) => RuntimeModelSelection::Explicit(explicit.to_owned()),
        None => RuntimeModelSelection::ProviderDefault,
    };
    let reasoning_effort = intent
        .preferred_reasoning_effort
        .map(RuntimeReasoningSelection::Explicit)
        .unwrap_or(RuntimeReasoningSelection::ProfileDefault);

    validate_model_selection(
        &environment.model_catalog,
        &provider,
        raw_model,
        reasoning_effort,
    )?;

    validate_stack(stored_task, project)?;

    // The trusted workspace is derived exclusively from the registered
    // project, never from caller-supplied paths.
    let permission_envelope = environment
        .effective_profile
        .as_ref()
        .filter(|effective| effective.profile.id == profile_id)
        .map(|effective| RuntimePermissionEnvelope {
            trusted_workspace: Some(project.local_path().to_path_buf()),
            workspace_write: matches!(
                effective.profile.filesystem,
                FilesystemCapability::WorkspaceWrite
            ),
            network: effective.profile.network,
            additional_writable_roots: effective.profile.extra_writable_roots.clone(),
        })
        .unwrap_or_default();

    Ok(ExecutionPlan {
        task_id: stored_task.id,
        project_key: stored_task.project_key.clone(),
        policy: ExecutionRunPolicy {
            runtime_selection: RuntimeExecutionSelection {
                profile_id,
                model,
                reasoning_effort,
            },
            permission_envelope,
            max_duration: PLANNED_MAX_DURATION.min(limits.max_duration),
            max_repair_attempts: PLANNED_MAX_REPAIR_ATTEMPTS.min(limits.max_repair_attempts),
        },
    })
}

fn validate_model_selection(
    catalog: &Option<RuntimeModelCatalog>,
    provider: &str,
    raw_model: Option<&str>,
    reasoning_effort: RuntimeReasoningSelection,
) -> Result<(), ExecutionPlanningError> {
    let has_override =
        raw_model.is_some() || matches!(reasoning_effort, RuntimeReasoningSelection::Explicit(_));
    if !has_override {
        return Ok(());
    }
    let Some(catalog) = catalog else {
        return Ok(());
    };

    if let Some(model) = raw_model
        && !catalog.available_models.is_empty()
        && !catalog.available_models.iter().any(|m| m.id == model)
    {
        return Err(ExecutionPlanningError::ModelNotAvailableFromProvider {
            model: model.to_owned(),
            provider: provider.to_owned(),
        });
    }

    if let RuntimeReasoningSelection::Explicit(effort) = reasoning_effort {
        let model_id = raw_model.unwrap_or(catalog.default_model.as_str());
        if let Some(model_info) = catalog
            .available_models
            .iter()
            .find(|candidate| candidate.id == model_id)
            && !model_info.supported_reasoning_efforts.is_empty()
            && !model_info.supported_reasoning_efforts.contains(&effort)
        {
            return Err(ExecutionPlanningError::ReasoningUnsupportedByModel {
                effort,
                model_id: model_id.to_owned(),
            });
        }
    }

    Ok(())
}

fn validate_stack(
    stored_task: &StoredTask,
    project: &TrustedProject,
) -> Result<(), ExecutionPlanningError> {
    let stack = stored_task
        .handoff
        .stack
        .as_deref()
        .or_else(|| project.default_stack())
        .ok_or(ExecutionPlanningError::StackUnconfigured)?;
    if !SUPPORTED_STACKS.contains(&stack) {
        return Err(ExecutionPlanningError::UnsupportedStack {
            stack: stack.to_owned(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AmbiguityPolicy, HandoffTask, ProjectKey, SubmissionTransport, TaskState};
    use relay_protocol::TaskSpec;

    fn sample_handoff(stack: Option<&str>) -> HandoffTask {
        HandoffTask {
            protocol_version: 1,
            project: "demo".to_owned(),
            title: "demo task".to_owned(),
            stack: stack.map(str::to_owned),
            source: None,
            task: TaskSpec {
                goal: "Run fake runtime.".to_owned(),
                requirements: Vec::new(),
                constraints: Vec::new(),
                verification: Vec::new(),
            },
            ambiguity_policy: AmbiguityPolicy::NeedsClarification,
            extensions: Default::default(),
        }
    }

    fn stored_task(stack: Option<&str>) -> StoredTask {
        StoredTask {
            id: TaskId::new(),
            handoff: sample_handoff(stack),
            project_key: ProjectKey::parse("demo").unwrap(),
            submission_transport: SubmissionTransport::LocalPaste,
            state: TaskState::Ready,
            created_at: "2026-01-01T00:00:00Z".to_owned(),
            updated_at: "2026-01-01T00:00:00Z".to_owned(),
        }
    }

    fn temp_project(default_stack: Option<&str>) -> TrustedProject {
        let path = std::env::temp_dir().join(format!("relay-plan-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        let mut registry = crate::ProjectRegistry::empty();
        registry
            .register(crate::RegisterProject {
                key: "demo".to_owned(),
                name: None,
                local_path: path,
                expected_remote: None,
                default_stack: default_stack.map(str::to_owned),
                github: None,
            })
            .unwrap();
        registry.get("demo").unwrap().clone()
    }

    fn default_limits() -> ExecutionLimits {
        ExecutionLimits::new(2, 1, 2, Duration::from_secs(4 * 60 * 60), 64 * 1024).unwrap()
    }

    fn codex_effective_profile() -> EffectiveRuntimeProfile {
        EffectiveRuntimeProfile {
            profile: relay_runtime::RuntimeProfile {
                id: "codex/implement".to_owned(),
                provider: "codex".to_owned(),
                display_name: "Codex Implement".to_owned(),
                model: "default".to_owned(),
                reasoning_effort: ReasoningEffort::High,
                filesystem: FilesystemCapability::WorkspaceWrite,
                network: false,
                extra_writable_roots: Vec::new(),
                max_duration: Duration::from_secs(1800),
                max_repair_attempts: 2,
                relay_approval_owner: true,
            },
            provider_config_snapshot: None,
        }
    }

    fn codex_model_catalog() -> RuntimeModelCatalog {
        RuntimeModelCatalog {
            default_model: "gpt-5-codex".to_owned(),
            available_models: vec![relay_runtime::RuntimeModelInfo {
                id: "gpt-5-codex".to_owned(),
                display_name: "GPT-5 Codex".to_owned(),
                supported_reasoning_efforts: vec![
                    ReasoningEffort::Low,
                    ReasoningEffort::Medium,
                    ReasoningEffort::High,
                ],
                default_reasoning_effort: Some(ReasoningEffort::High),
            }],
        }
    }

    #[test]
    fn clamps_limits_to_planning_bounds_and_scheduler_ceiling() {
        let task = stored_task(Some("chatgpt-codex"));
        let project = temp_project(None);
        let intent = ExecutionPlanIntent::default();
        let environment = ExecutionPlanEnvironment::default();

        let plan = build_execution_plan(
            &task,
            &project,
            intent.clone(),
            &default_limits(),
            &environment,
            "codex/implement",
        )
        .unwrap();

        assert_eq!(plan.policy.max_duration, PLANNED_MAX_DURATION);
        assert_eq!(plan.policy.max_repair_attempts, PLANNED_MAX_REPAIR_ATTEMPTS);

        let tight = ExecutionLimits::new(2, 1, 1, Duration::from_secs(60), 64 * 1024).unwrap();
        let plan = build_execution_plan(
            &task,
            &project,
            intent,
            &tight,
            &environment,
            "codex/implement",
        )
        .unwrap();

        assert_eq!(plan.policy.max_duration, Duration::from_secs(60));
        assert_eq!(plan.policy.max_repair_attempts, 1);
    }

    #[test]
    fn rejects_unsupported_and_unconfigured_stacks() {
        let limits = default_limits();
        let environment = ExecutionPlanEnvironment::default();
        let intent = ExecutionPlanIntent::default();

        let unsupported = stored_task(Some("claude-code"));
        let error = build_execution_plan(
            &unsupported,
            &temp_project(None),
            intent.clone(),
            &limits,
            &environment,
            "codex/implement",
        )
        .unwrap_err();
        assert!(
            matches!(
                error,
                ExecutionPlanningError::UnsupportedStack { ref stack }
                    if stack == "claude-code"
            ),
            "unexpected error: {error:?}"
        );

        let unconfigured = stored_task(None);
        let error = build_execution_plan(
            &unconfigured,
            &temp_project(None),
            intent.clone(),
            &limits,
            &environment,
            "codex/implement",
        )
        .unwrap_err();
        assert!(matches!(error, ExecutionPlanningError::StackUnconfigured));

        // The task's own stack takes precedence over the registry default and
        // is validated against the same allowlist.
        let agy_project = temp_project(Some("chatgpt-codex"));
        let plan = build_execution_plan(
            &stored_task(Some("agy")),
            &agy_project,
            intent,
            &limits,
            &environment,
            "antigravity/implement",
        )
        .unwrap();
        assert_eq!(
            plan.policy.runtime_selection.profile_id,
            "antigravity/implement"
        );
    }

    #[test]
    fn rejects_cross_provider_model_leakage_and_unsupported_reasoning() {
        let task = stored_task(Some("chatgpt-codex"));
        let project = temp_project(None);
        let codex_environment = ExecutionPlanEnvironment {
            effective_profile: Some(codex_effective_profile()),
            model_catalog: Some(codex_model_catalog()),
        };
        // The antigravity provider offers a disjoint model set.
        let agy_environment = ExecutionPlanEnvironment {
            effective_profile: None,
            model_catalog: Some(RuntimeModelCatalog {
                default_model: "gemini-3-pro".to_owned(),
                available_models: vec![relay_runtime::RuntimeModelInfo {
                    id: "gemini-3-pro".to_owned(),
                    display_name: "Gemini 3 Pro".to_owned(),
                    supported_reasoning_efforts: vec![ReasoningEffort::Low, ReasoningEffort::High],
                    default_reasoning_effort: Some(ReasoningEffort::High),
                }],
            }),
        };
        let limits = default_limits();

        // A model requested under one provider does not silently travel to a
        // profile resolved for another provider.
        let error = build_execution_plan(
            &task,
            &project,
            ExecutionPlanIntent {
                preferred_profile_id: Some("antigravity/implement".to_owned()),
                preferred_model: Some("gpt-5-codex".to_owned()),
                preferred_reasoning_effort: None,
            },
            &limits,
            &agy_environment,
            "antigravity/implement",
        )
        .unwrap_err();
        assert!(
            matches!(
                error,
                ExecutionPlanningError::ModelNotAvailableFromProvider { ref model, ref provider }
                    if model == "gpt-5-codex" && provider == "antigravity"
            ),
            "unexpected error: {error:?}"
        );

        // Unsupported reasoning effort for the resolved model is rejected.
        let error = build_execution_plan(
            &task,
            &project,
            ExecutionPlanIntent {
                preferred_profile_id: Some("codex/implement".to_owned()),
                preferred_model: Some("gpt-5-codex".to_owned()),
                preferred_reasoning_effort: Some(ReasoningEffort::XHigh),
            },
            &limits,
            &codex_environment,
            "codex/implement",
        )
        .unwrap_err();
        assert!(
            matches!(
                error,
                ExecutionPlanningError::ReasoningUnsupportedByModel {
                    effort: ReasoningEffort::XHigh,
                    ref model_id
                } if model_id == "gpt-5-codex"
            ),
            "unexpected error: {error:?}"
        );

        // Supported selections pass through untouched.
        let plan = build_execution_plan(
            &task,
            &project,
            ExecutionPlanIntent {
                preferred_profile_id: Some("codex/implement".to_owned()),
                preferred_model: Some("gpt-5-codex".to_owned()),
                preferred_reasoning_effort: Some(ReasoningEffort::Medium),
            },
            &limits,
            &codex_environment,
            "codex/implement",
        )
        .unwrap();
        assert_eq!(
            plan.policy.runtime_selection.model.as_option(),
            Some("gpt-5-codex")
        );
        assert_eq!(
            plan.policy.runtime_selection.reasoning_effort,
            RuntimeReasoningSelection::Explicit(ReasoningEffort::Medium)
        );
    }

    #[test]
    fn trusted_workspace_comes_from_registry_not_caller() {
        let task = stored_task(Some("codex"));
        let project = temp_project(Some("codex"));
        let limits = default_limits();

        let plan = build_execution_plan(
            &task,
            &project,
            ExecutionPlanIntent::default(),
            &limits,
            &ExecutionPlanEnvironment {
                effective_profile: Some(codex_effective_profile()),
                model_catalog: None,
            },
            "codex/implement",
        )
        .unwrap();

        assert_eq!(
            plan.policy.permission_envelope.trusted_workspace.as_deref(),
            Some(project.local_path())
        );
        assert!(plan.policy.permission_envelope.workspace_write);
        assert!(!plan.policy.permission_envelope.network);

        // Without a capability snapshot core falls back to the least-privilege
        // default envelope rather than inventing permissions.
        let plan = build_execution_plan(
            &task,
            &project,
            ExecutionPlanIntent::default(),
            &limits,
            &ExecutionPlanEnvironment::default(),
            "codex/implement",
        )
        .unwrap();
        assert_eq!(
            plan.policy.permission_envelope,
            RuntimePermissionEnvelope::default()
        );
        assert_eq!(plan.project_key, task.project_key);
    }
}
