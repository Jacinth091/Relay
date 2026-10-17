//! Phase 03 Tests: Approved Intent, Execution Grant, Workflow, Roles, and Policy.
//!
//! Enforces:
//! 1. Run without Ready/approved intent rejected.
//! 2. Changed scope cannot mutate ApprovedIntent in place (generations).
//! 3. Workflow version/config snapshots remain stable during run.
//! 4. Policy intersection cannot broaden built-in/user hard deny.
//! 5. Semantic Orchestrator is not represented as provider RoleInstance.
//! 6. Attempt retry does not create a new semantic role.
//! 7. Cancellation transition truthfully persists.

use relay_core::{
    Capability, ExecutionBudget, ExecutionTarget, LegacyAdapterError, PolicyCompiler,
    PolicyDisposition, PolicyLayer, RoleDefinitionId, RoleInstance, StoredTask,
    SubmissionTransport, TaskId, TaskState, WorkflowDefinitionId, WorkflowRun, WorkflowRunState,
    adapt_v1_handoff_to_approved_intent, create_v1_compatibility_run,
    v1_compatibility_workflow_definition,
};
use relay_protocol::{AmbiguityPolicy, HandoffTask, TaskSpec};

fn sample_task(state: TaskState) -> StoredTask {
    StoredTask {
        id: TaskId::generate(),
        handoff: HandoffTask {
            protocol_version: 1,
            project: "test-project".to_owned(),
            title: "Test Task".to_owned(),
            stack: None,
            source: None,
            task: TaskSpec {
                goal: "Implement authentication".to_owned(),
                requirements: vec!["Password hashing".to_owned()],
                constraints: vec!["No plaintext passwords".to_owned()],
                verification: vec!["cargo test".to_owned()],
            },
            ambiguity_policy: AmbiguityPolicy::NeedsClarification,
            extensions: std::collections::BTreeMap::new(),
        },
        project_key: relay_core::ProjectKey::parse("test-project").unwrap(),
        submission_transport: SubmissionTransport::LocalPaste,
        state,
        created_at: "now".to_owned(),
        updated_at: "now".to_owned(),
    }
}

#[test]
fn run_without_ready_approved_intent_is_rejected() {
    let non_ready_states = [
        TaskState::Running,
        TaskState::Done,
        TaskState::Failed,
        TaskState::NeedsClarification,
        TaskState::NeedsDecision,
    ];

    for state in non_ready_states {
        let task = sample_task(state);
        let err = adapt_v1_handoff_to_approved_intent(&task, "operator");
        assert!(
            matches!(err, Err(LegacyAdapterError::TaskNotReady { .. })),
            "state {state:?} must not be runnable without Ready state"
        );
    }
}

#[test]
fn changed_scope_cannot_mutate_approved_intent_in_place() {
    let task = sample_task(TaskState::Ready);
    let original = adapt_v1_handoff_to_approved_intent(&task, "operator").unwrap();

    assert_eq!(original.generation, 1);
    let orig_id = original.id.clone();
    let orig_objective = original.objective.clone();

    // User expands scope via new generation
    let next_gen = original.create_next_generation(
        "Implement authentication with OAuth2".to_owned(),
        vec!["Password hashing".to_owned(), "OAuth2 login".to_owned()],
        vec!["No plaintext passwords".to_owned()],
        "operator-2".to_owned(),
    );

    // Generation 2 created
    assert_eq!(next_gen.generation, 2);
    assert_eq!(next_gen.objective, "Implement authentication with OAuth2");

    // Generation 1 completely unchanged
    assert_eq!(original.generation, 1);
    assert_eq!(original.id, orig_id);
    assert_eq!(original.objective, orig_objective);
    assert_ne!(original.id, next_gen.id);
}

#[test]
fn workflow_version_and_config_snapshots_remain_stable_during_run() {
    let task = sample_task(TaskState::Ready);
    let intent = adapt_v1_handoff_to_approved_intent(&task, "operator").unwrap();
    let budget = ExecutionBudget::default();

    let (grant, run, snapshot) =
        create_v1_compatibility_run(&task, &intent, budget, "operator").unwrap();

    assert_eq!(grant.config_snapshot_id, snapshot.id);
    assert_eq!(run.config_snapshot_id, snapshot.id);
    assert_eq!(snapshot.workflow_version, 1);
    assert_eq!(snapshot.budget.max_attempts_per_role, 3);
}

#[test]
fn policy_intersection_cannot_broaden_built_in_or_user_hard_deny() {
    let user_layer = PolicyLayer::new("user").with_rule(
        Capability::NetworkAccess,
        PolicyDisposition::HardDeny,
        "offline safe mode",
    );

    let repo_layer = PolicyLayer::new("repo")
        .with_rule(
            Capability::NetworkAccess,
            PolicyDisposition::Allow,
            "repo wants full internet",
        )
        .with_rule(
            Capability::GitPush,
            PolicyDisposition::Allow,
            "repo wants auto-push",
        );

    let decisions = PolicyCompiler::compile(Some(&user_layer), None, Some(&repo_layer), None, None);

    // 1. Built-in hard deny on GitPush cannot be broadened
    let push_decision = decisions
        .iter()
        .find(|d| d.capability == Capability::GitPush)
        .unwrap();
    assert_eq!(push_decision.disposition, PolicyDisposition::HardDeny);

    // 2. User hard deny on NetworkAccess cannot be broadened by repo
    let net_decision = decisions
        .iter()
        .find(|d| d.capability == Capability::NetworkAccess)
        .unwrap();
    assert_eq!(net_decision.disposition, PolicyDisposition::HardDeny);
}

#[test]
fn semantic_orchestrator_is_not_represented_as_provider_role_instance() {
    let wf = v1_compatibility_workflow_definition();

    // The workflow contains only domain roles (implementer, verifier).
    // The orchestrator is the Relay Daemon, NEVER an LLM role!
    for node in &wf.nodes {
        assert_ne!(node.role_id.0, "orchestrator");
        assert_ne!(node.role_id.0, "manager");
        assert_ne!(node.role_id.0, "planner");
    }
}

#[test]
fn attempt_retry_does_not_create_new_semantic_role() {
    let run = WorkflowRun::new(
        TaskId::generate(),
        relay_core::ExecutionGrantId::generate(),
        WorkflowDefinitionId::new("wf"),
        relay_core::ConfigSnapshotId::generate(),
    );

    let mut role = RoleInstance::new(run.id, RoleDefinitionId::new("implementer"), 1, None);
    let original_role_id = role.id.clone();
    let budget = ExecutionBudget::default();

    let target = ExecutionTarget {
        provider_id: "codex".to_owned(),
        runtime_profile_id: "codex/default".to_owned(),
        provider_adapter_version: "0.1.0".to_owned(),
    };

    // Attempt 1 fails
    let mut att1 = role.spawn_attempt(target.clone(), &budget).unwrap();
    att1.fail("compiler error".to_owned());
    assert_eq!(role.attempts_count, 1);
    assert_eq!(att1.role_instance_id, original_role_id);

    // Attempt 2 (retry)
    let att2 = role.spawn_attempt(target, &budget).unwrap();
    assert_eq!(role.attempts_count, 2);
    assert_eq!(att2.role_instance_id, original_role_id);

    // Semantic role remains one and the same
    assert_eq!(role.id, original_role_id);
}

#[test]
fn cancellation_transition_truthfully_persists() {
    let mut run = WorkflowRun::new(
        TaskId::generate(),
        relay_core::ExecutionGrantId::generate(),
        WorkflowDefinitionId::new("wf"),
        relay_core::ConfigSnapshotId::generate(),
    );

    run.start().unwrap();
    assert_eq!(run.state, WorkflowRunState::Running);

    run.cancel();
    assert_eq!(run.state, WorkflowRunState::Cancelled);
    assert!(run.completed_at.is_some());
    assert!(run.state.is_terminal());
}

#[test]
fn approved_intent_content_hash_covers_all_fields_deterministically() {
    let task_id = TaskId::generate();
    let intent1 = relay_core::ApprovedIntent::new(
        task_id,
        1,
        "Implement feature".to_string(),
        vec!["Criterion A".to_string()],
        vec!["Constraint X".to_string()],
        vec!["Non-goal Z".to_string()],
        "user1".to_string(),
    );

    // Identical fields produce identical hash
    let intent1_dup = relay_core::ApprovedIntent::new(
        task_id,
        1,
        "Implement feature".to_string(),
        vec!["Criterion A".to_string()],
        vec!["Constraint X".to_string()],
        vec!["Non-goal Z".to_string()],
        "user2".to_string(),
    );
    assert_eq!(intent1.content_hash, intent1_dup.content_hash);

    // Changing objective changes hash
    let intent_diff_obj = relay_core::ApprovedIntent::new(
        task_id,
        1,
        "Different objective".to_string(),
        vec!["Criterion A".to_string()],
        vec!["Constraint X".to_string()],
        vec!["Non-goal Z".to_string()],
        "user1".to_string(),
    );
    assert_ne!(intent1.content_hash, intent_diff_obj.content_hash);

    // Changing criteria changes hash
    let intent_diff_crit = relay_core::ApprovedIntent::new(
        task_id,
        1,
        "Implement feature".to_string(),
        vec!["Criterion B".to_string()],
        vec!["Constraint X".to_string()],
        vec!["Non-goal Z".to_string()],
        "user1".to_string(),
    );
    assert_ne!(intent1.content_hash, intent_diff_crit.content_hash);

    // Changing constraints changes hash
    let intent_diff_cons = relay_core::ApprovedIntent::new(
        task_id,
        1,
        "Implement feature".to_string(),
        vec!["Criterion A".to_string()],
        vec!["Constraint Y".to_string()],
        vec!["Non-goal Z".to_string()],
        "user1".to_string(),
    );
    assert_ne!(intent1.content_hash, intent_diff_cons.content_hash);

    // Changing non-goals changes hash
    let intent_diff_ng = relay_core::ApprovedIntent::new(
        task_id,
        1,
        "Implement feature".to_string(),
        vec!["Criterion A".to_string()],
        vec!["Constraint X".to_string()],
        vec!["Non-goal W".to_string()],
        "user1".to_string(),
    );
    assert_ne!(intent1.content_hash, intent_diff_ng.content_hash);

    // Changing generation changes hash
    let intent_diff_gen = relay_core::ApprovedIntent::new(
        task_id,
        2,
        "Implement feature".to_string(),
        vec!["Criterion A".to_string()],
        vec!["Constraint X".to_string()],
        vec!["Non-goal Z".to_string()],
        "user1".to_string(),
    );
    assert_ne!(intent1.content_hash, intent_diff_gen.content_hash);
}

#[test]
fn workflow_run_creation_rejects_invalid_workflow_definitions() {
    let task_id = TaskId::generate();
    let grant_id = relay_core::ExecutionGrantId::generate();
    let snap_id = relay_core::ConfigSnapshotId::generate();

    // 1. Invalid role reference
    let wf_invalid_role = relay_core::WorkflowDefinition {
        id: WorkflowDefinitionId::new("bad-role"),
        version: 1,
        roles: vec![],
        nodes: vec![relay_core::WorkflowNodeDefinition {
            id: "step1".to_string(),
            role_id: relay_core::RoleDefinitionId::new("bogus-role"),
            consumed_artifact_types: vec![],
            produced_artifact_types: vec![],
        }],
        transitions: vec![],
        default_budget: ExecutionBudget::default(),
        content_hash: "hash".to_string(),
    };
    assert!(
        wf_invalid_role
            .create_run(task_id, grant_id.clone(), snap_id.clone())
            .is_err()
    );

    // 2. Unreachable node
    let wf_unreachable = relay_core::WorkflowDefinition {
        id: WorkflowDefinitionId::new("unreachable"),
        version: 1,
        roles: vec![],
        nodes: vec![
            relay_core::WorkflowNodeDefinition {
                id: "start".to_string(),
                role_id: relay_core::RoleDefinitionId::new("implementer"),
                consumed_artifact_types: vec![],
                produced_artifact_types: vec![],
            },
            relay_core::WorkflowNodeDefinition {
                id: "island".to_string(),
                role_id: relay_core::RoleDefinitionId::new("verifier"),
                consumed_artifact_types: vec![],
                produced_artifact_types: vec![],
            },
        ],
        transitions: vec![],
        default_budget: ExecutionBudget::default(),
        content_hash: "hash".to_string(),
    };
    assert!(
        wf_unreachable
            .create_run(task_id, grant_id.clone(), snap_id.clone())
            .is_err()
    );

    // 3. Valid workflow creates run successfully
    let valid_wf = relay_core::v1_compatibility_workflow_definition();
    let run = valid_wf.create_run(task_id, grant_id, snap_id).unwrap();
    assert_eq!(run.state, WorkflowRunState::Pending);
}
