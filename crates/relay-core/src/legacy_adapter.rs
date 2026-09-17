//! Relay Architecture v2 - Phase 03: Legacy Adapter.
//!
//! Creates the v1 handoff compatibility mapping:
//! ```text
//! HandoffTask v1 + explicit Run
//! -> ApprovedIntent generation 1
//! -> ExecutionGrant
//! -> compatibility WorkflowRun
//! ```
//!
//! Invariants:
//! 1. Run without Ready/approved intent is rejected (fail-closed).
//! 2. No hidden planner role is added to already-approved implementation intent.
//! 3. Semantic Orchestrator is Relay Daemon itself, NOT represented as a provider `RoleInstance`.

use thiserror::Error;

use crate::{
    ApprovedIntent, ConfigSnapshot, ExecutionBudget, ExecutionGrant, ExecutionGrantId,
    RoleDefinitionId, StoredTask, TaskState, WorkflowDefinition, WorkflowDefinitionId,
    WorkflowNodeDefinition, WorkflowRun, WorkflowRunState, WorkflowTransitionDefinition,
};

#[derive(Debug, Error)]
pub enum LegacyAdapterError {
    #[error("task must be in Ready state to execute (current state: {current})")]
    TaskNotReady { current: String },
    #[error("task data error: {0}")]
    TaskData(String),
}

/// The standard v1 compatibility workflow definition:
/// Implementer -> Verifier (no hidden planner role!)
pub fn v1_compatibility_workflow_definition() -> WorkflowDefinition {
    WorkflowDefinition {
        id: WorkflowDefinitionId::new("v1-compatibility-workflow"),
        version: 1,
        nodes: vec![
            WorkflowNodeDefinition {
                id: "implementer".to_owned(),
                role_id: RoleDefinitionId::new("implementer"),
            },
            WorkflowNodeDefinition {
                id: "verifier".to_owned(),
                role_id: RoleDefinitionId::new("verifier"),
            },
        ],
        transitions: vec![WorkflowTransitionDefinition {
            from_node: "implementer".to_owned(),
            to_node: "verifier".to_owned(),
            condition: "CandidateCreated".to_owned(),
        }],
        default_budget: ExecutionBudget::default(),
        content_hash: "v1-default-hash".to_owned(),
    }
}

/// Converts a canonical Ready stored task into an initial ApprovedIntent (generation 1).
///
/// Fails closed if the task is not in Ready state.
pub fn adapt_v1_handoff_to_approved_intent(
    task: &StoredTask,
    approved_by: &str,
) -> Result<ApprovedIntent, LegacyAdapterError> {
    if task.state != TaskState::Ready {
        return Err(LegacyAdapterError::TaskNotReady {
            current: task.state.as_str().to_owned(),
        });
    }

    let handoff = &task.handoff;
    let objective = handoff.task.goal.clone();
    let acceptance_criteria = handoff.task.requirements.clone();
    let constraints = handoff.task.constraints.clone();

    Ok(ApprovedIntent::new(
        task.id,
        1,
        objective,
        acceptance_criteria,
        constraints,
        Vec::new(),
        approved_by.to_owned(),
    ))
}

/// Creates a compatibility workflow run and execution grant for an approved intent.
pub fn create_v1_compatibility_run(
    task: &StoredTask,
    intent: &ApprovedIntent,
    budget: ExecutionBudget,
    granted_by: &str,
) -> Result<(ExecutionGrant, WorkflowRun, ConfigSnapshot), LegacyAdapterError> {
    if task.state != TaskState::Ready {
        return Err(LegacyAdapterError::TaskNotReady {
            current: task.state.as_str().to_owned(),
        });
    }

    let wf_def = v1_compatibility_workflow_definition();
    let snapshot =
        ConfigSnapshot::capture(wf_def.version, budget.clone(), wf_def.content_hash.clone());

    let grant = ExecutionGrant {
        id: ExecutionGrantId::generate(),
        task_id: task.id,
        approved_intent_id: intent.id.clone(),
        approved_intent_generation: intent.generation,
        workflow_definition_id: wf_def.id.clone(),
        config_snapshot_id: snapshot.id.clone(),
        execution_budget: budget,
        allowed_delegation_classes: vec![
            RoleDefinitionId::new("implementer"),
            RoleDefinitionId::new("verifier"),
        ],
        granted_by: granted_by.to_owned(),
        granted_at: intent.approved_at.clone(),
        revoked_at: None,
    };

    let mut run = WorkflowRun::new(task.id, grant.id.clone(), wf_def.id, snapshot.id.clone());
    run.state = WorkflowRunState::Running;

    Ok((grant, run, snapshot))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SubmissionTransport, TaskId};
    use relay_protocol::{AmbiguityPolicy, HandoffTask, TaskSpec};

    fn dummy_stored_task(state: TaskState) -> StoredTask {
        StoredTask {
            id: TaskId::generate(),
            handoff: HandoffTask {
                protocol_version: 1,
                project: "test-project".to_owned(),
                title: "Test Task".to_owned(),
                stack: None,
                source: None,
                task: TaskSpec {
                    goal: "Implement feature X".to_owned(),
                    requirements: vec!["Must be fast".to_owned()],
                    constraints: vec!["No external network".to_owned()],
                    verification: vec!["cargo test".to_owned()],
                },
                ambiguity_policy: AmbiguityPolicy::NeedsClarification,
                extensions: std::collections::BTreeMap::new(),
            },
            project_key: crate::ProjectKey::parse("test-project").unwrap(),
            submission_transport: SubmissionTransport::LocalPaste,
            state,
            created_at: "now".to_owned(),
            updated_at: "now".to_owned(),
        }
    }

    #[test]
    fn run_without_ready_state_is_rejected() {
        let task_running = dummy_stored_task(TaskState::Running);
        let err = adapt_v1_handoff_to_approved_intent(&task_running, "operator");
        assert!(matches!(err, Err(LegacyAdapterError::TaskNotReady { .. })));

        let task_failed = dummy_stored_task(TaskState::Failed);
        let err = adapt_v1_handoff_to_approved_intent(&task_failed, "operator");
        assert!(matches!(err, Err(LegacyAdapterError::TaskNotReady { .. })));
    }

    #[test]
    fn ready_task_adapts_to_generation_1_intent_and_grant() {
        let task_ready = dummy_stored_task(TaskState::Ready);
        let intent = adapt_v1_handoff_to_approved_intent(&task_ready, "operator").unwrap();

        assert_eq!(intent.generation, 1);
        assert_eq!(intent.objective, "Implement feature X");
        assert_eq!(intent.acceptance_criteria, vec!["Must be fast"]);
        assert_eq!(intent.constraints, vec!["No external network"]);

        let (grant, run, snapshot) = create_v1_compatibility_run(
            &task_ready,
            &intent,
            ExecutionBudget::default(),
            "operator",
        )
        .unwrap();

        assert_eq!(grant.approved_intent_generation, 1);
        assert_eq!(grant.task_id, task_ready.id);
        assert_eq!(run.state, WorkflowRunState::Running);
        assert_eq!(snapshot.workflow_version, 1);

        // Verify no planner role exists in compatibility workflow nodes
        let wf = v1_compatibility_workflow_definition();
        assert!(!wf.nodes.iter().any(|n| n.role_id.0.contains("planner")));
        assert!(wf.nodes.iter().any(|n| n.role_id.0 == "implementer"));
        assert!(wf.nodes.iter().any(|n| n.role_id.0 == "verifier"));
    }
}
