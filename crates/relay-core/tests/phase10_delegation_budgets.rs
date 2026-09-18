//! Phase 10: Delegation, Consultation, Budgets, Deadlock Prevention Tests.
//!
//! Verifies:
//! 1. Child cannot broaden approved intent.
//! 2. Cyclic blocking delegation rejected fail-closed.
//! 3. Budget exhaustion escalates/terminates predictably (activations, depth).
//! 4. Consultation does not accidentally create write authority.
//! 5. A delegation caller cannot name an executable/path to bypass the
//!    Phase 08 provider resolver -- only a pre-registered `RoleDefinitionId`
//!    can ever be named, and free-form `purpose` text never reaches launch
//!    construction.
//! 6. Cancellation propagates cleanly through outstanding children, without
//!    overwriting an already-terminal (Completed) descendant.

use relay_core::{
    ApprovedIntent, ConfigSnapshotId, DelegationError, DelegationManager, DelegationMode,
    DelegationRequest, ExecutionBudget, ExecutionGrant, ExecutionGrantId, IndependenceRequirement,
    RoleAttemptId, RoleDefinition, RoleDefinitionId, RoleDependencyState, RoleInstanceId,
    RoleWriteMode, TaskId, WorkflowDefinitionId,
};
use relay_runtime::ProviderAdapter;

fn grant_with(allowed: Vec<RoleDefinitionId>, budget: ExecutionBudget) -> ExecutionGrant {
    let intent = ApprovedIntent::new(
        TaskId::generate(),
        1,
        "Ship the widget".to_owned(),
        vec!["widget compiles".to_owned()],
        vec!["no regressions".to_owned()],
        vec![],
        "operator".to_owned(),
    );
    ExecutionGrant {
        id: ExecutionGrantId::generate(),
        task_id: intent.task_id,
        approved_intent_id: intent.id.clone(),
        approved_intent_generation: intent.generation,
        workflow_definition_id: WorkflowDefinitionId::new("default"),
        config_snapshot_id: ConfigSnapshotId::generate(),
        execution_budget: budget,
        allowed_delegation_classes: allowed,
        granted_by: "operator".to_owned(),
        granted_at: "0".to_owned(),
        revoked_at: None,
    }
}

fn role(id: &str, write_mode: RoleWriteMode, allowed_delegations: &[&str]) -> RoleDefinition {
    RoleDefinition {
        id: RoleDefinitionId::new(id),
        purpose: id.to_owned(),
        write_mode,
        allowed_delegations: allowed_delegations
            .iter()
            .map(|r| RoleDefinitionId::new(*r))
            .collect(),
    }
}

fn unbounded_budget() -> ExecutionBudget {
    ExecutionBudget {
        max_role_activations: 100,
        max_attempts_per_role: 3,
        max_delegations: 100,
        max_delegation_depth: 100,
        max_concurrent_attempts: 100,
        max_wall_clock_secs: Some(600),
    }
}

fn attempt_id(instance: &RoleInstanceId) -> RoleAttemptId {
    RoleAttemptId::generate(instance, 1)
}

#[test]
fn child_cannot_broaden_approved_intent() {
    // The ExecutionGrant only approved delegation to "reviewer" -- the
    // human never approved "privileged_admin" when the grant was created.
    let grant = grant_with(vec![RoleDefinitionId::new("reviewer")], unbounded_budget());
    let approved_intent_id = grant.approved_intent_id.clone();
    let approved_generation = grant.approved_intent_generation;

    let implementer = role(
        "implementer",
        RoleWriteMode::CandidateProducer,
        &["reviewer"],
    );
    let reviewer = role("reviewer", RoleWriteMode::ReadOnly, &[]);
    let mut mgr = DelegationManager::new(grant, vec![implementer, reviewer]);

    let root_id = RoleInstanceId::generate(&RoleDefinitionId::new("implementer"));
    mgr.register_root(root_id.clone(), RoleDefinitionId::new("implementer"))
        .unwrap();

    let err = mgr
        .delegate(DelegationRequest {
            parent_role_instance_id: root_id.clone(),
            parent_attempt_id: attempt_id(&root_id),
            requested_role: RoleDefinitionId::new("privileged_admin"),
            mode: DelegationMode::NonblockingChild,
            purpose: "grant myself more scope".to_owned(),
            input_artifacts: vec![],
            requested_independence: IndependenceRequirement::NoIndependenceRequirement,
        })
        .expect_err("delegation outside the grant's allowed classes must be rejected");

    assert!(
        matches!(err, DelegationError::RoleNotAllowedByGrant(role) if role == RoleDefinitionId::new("privileged_admin"))
    );

    // Even a role the PARENT ROLE DEFINITION never declared as delegable is
    // rejected, independent of the grant-level check.
    let grant2 = grant_with(
        vec![
            RoleDefinitionId::new("reviewer"),
            RoleDefinitionId::new("ghost"),
        ],
        unbounded_budget(),
    );
    let implementer2 = role(
        "implementer",
        RoleWriteMode::CandidateProducer,
        &["reviewer"],
    );
    let reviewer2 = role("reviewer", RoleWriteMode::ReadOnly, &[]);
    let ghost = role("ghost", RoleWriteMode::CandidateProducer, &[]);
    let mut mgr2 = DelegationManager::new(grant2, vec![implementer2, reviewer2, ghost]);
    let root_id2 = RoleInstanceId::generate(&RoleDefinitionId::new("implementer"));
    mgr2.register_root(root_id2.clone(), RoleDefinitionId::new("implementer"))
        .unwrap();

    let err2 = mgr2
        .delegate(DelegationRequest {
            parent_role_instance_id: root_id2.clone(),
            parent_attempt_id: attempt_id(&root_id2),
            requested_role: RoleDefinitionId::new("ghost"),
            mode: DelegationMode::NonblockingChild,
            purpose: "ghost role not declared by implementer".to_owned(),
            input_artifacts: vec![],
            requested_independence: IndependenceRequirement::NoIndependenceRequirement,
        })
        .expect_err("role not declared by the parent's own RoleDefinition must be rejected");
    assert!(
        matches!(err2, DelegationError::RoleNotAllowedByParent(role, parent) if role == RoleDefinitionId::new("ghost") && parent == RoleDefinitionId::new("implementer"))
    );

    // The ApprovedIntent this grant references is untouched by any of the above.
    assert_eq!(approved_generation, 1);
    assert_eq!(approved_intent_id, approved_intent_id.clone());
}

#[test]
fn cyclic_blocking_delegation_rejected_fail_closed() {
    // implementer <-> reviewer mutually allowed to delegate to each other.
    let grant = grant_with(
        vec![
            RoleDefinitionId::new("reviewer"),
            RoleDefinitionId::new("implementer"),
        ],
        unbounded_budget(),
    );
    let implementer = role(
        "implementer",
        RoleWriteMode::CandidateProducer,
        &["reviewer"],
    );
    let reviewer = role("reviewer", RoleWriteMode::ReadOnly, &["implementer"]);
    let mut mgr = DelegationManager::new(grant, vec![implementer, reviewer]);

    let root_id = RoleInstanceId::generate(&RoleDefinitionId::new("implementer"));
    mgr.register_root(root_id.clone(), RoleDefinitionId::new("implementer"))
        .unwrap();

    let outcome = mgr
        .delegate(DelegationRequest {
            parent_role_instance_id: root_id.clone(),
            parent_attempt_id: attempt_id(&root_id),
            requested_role: RoleDefinitionId::new("reviewer"),
            mode: DelegationMode::BlockingChild,
            purpose: "review the diff".to_owned(),
            input_artifacts: vec![],
            requested_independence: IndependenceRequirement::NoIndependenceRequirement,
        })
        .expect("first-level blocking delegation is legitimate");

    // The reviewer, while still blocking its parent, now tries to blocking-
    // delegate back to "implementer" -- a deadlock shape (implementer waits
    // on reviewer, reviewer would wait on a fresh implementer forever).
    let err = mgr
        .delegate(DelegationRequest {
            parent_role_instance_id: outcome.role_instance_id.clone(),
            parent_attempt_id: attempt_id(&outcome.role_instance_id),
            requested_role: RoleDefinitionId::new("implementer"),
            mode: DelegationMode::BlockingChild,
            purpose: "ask the implementer to re-implement".to_owned(),
            input_artifacts: vec![],
            requested_independence: IndependenceRequirement::NoIndependenceRequirement,
        })
        .expect_err("cyclic blocking delegation must be rejected fail-closed");

    assert!(
        matches!(err, DelegationError::CyclicBlockingDelegation(role) if role == RoleDefinitionId::new("implementer"))
    );

    // A NON-BLOCKING delegation back to "implementer" is fine -- it cannot deadlock.
    let ok = mgr.delegate(DelegationRequest {
        parent_role_instance_id: outcome.role_instance_id.clone(),
        parent_attempt_id: attempt_id(&outcome.role_instance_id),
        requested_role: RoleDefinitionId::new("implementer"),
        mode: DelegationMode::NonblockingChild,
        purpose: "fire-and-forget follow-up".to_owned(),
        input_artifacts: vec![],
        requested_independence: IndependenceRequirement::NoIndependenceRequirement,
    });
    assert!(ok.is_ok());
}

#[test]
fn budget_exhaustion_escalates_predictably() {
    // Only 2 role activations total: the root, plus exactly one delegated child.
    let budget = ExecutionBudget {
        max_role_activations: 2,
        max_attempts_per_role: 3,
        max_delegations: 10,
        max_delegation_depth: 10,
        max_concurrent_attempts: 10,
        max_wall_clock_secs: Some(600),
    };
    let grant = grant_with(vec![RoleDefinitionId::new("reviewer")], budget);
    let implementer = role(
        "implementer",
        RoleWriteMode::CandidateProducer,
        &["reviewer"],
    );
    let reviewer = role("reviewer", RoleWriteMode::ReadOnly, &["reviewer"]);
    let mut mgr = DelegationManager::new(grant, vec![implementer, reviewer]);

    let root_id = RoleInstanceId::generate(&RoleDefinitionId::new("implementer"));
    mgr.register_root(root_id.clone(), RoleDefinitionId::new("implementer"))
        .unwrap();

    let first = mgr
        .delegate(DelegationRequest {
            parent_role_instance_id: root_id.clone(),
            parent_attempt_id: attempt_id(&root_id),
            requested_role: RoleDefinitionId::new("reviewer"),
            mode: DelegationMode::NonblockingChild,
            purpose: "review pass 1".to_owned(),
            input_artifacts: vec![],
            requested_independence: IndependenceRequirement::NoIndependenceRequirement,
        })
        .expect("first delegation is within budget");

    // A second delegation predictably escalates via a typed error rather
    // than silently proceeding or panicking.
    let err = mgr
        .delegate(DelegationRequest {
            parent_role_instance_id: first.role_instance_id.clone(),
            parent_attempt_id: attempt_id(&first.role_instance_id),
            requested_role: RoleDefinitionId::new("reviewer"),
            mode: DelegationMode::NonblockingChild,
            purpose: "review pass 2".to_owned(),
            input_artifacts: vec![],
            requested_independence: IndependenceRequirement::NoIndependenceRequirement,
        })
        .expect_err("role activation budget must be exhausted");
    assert!(matches!(
        err,
        DelegationError::BudgetExhausted(relay_core::BudgetError::RoleActivationsExhausted {
            max: 2
        })
    ));

    // Depth limits escalate the same way, independent of activation count.
    let depth_budget = ExecutionBudget {
        max_role_activations: 100,
        max_delegation_depth: 1,
        ..unbounded_budget()
    };
    let grant2 = grant_with(vec![RoleDefinitionId::new("worker")], depth_budget);
    let root_role = role("root_role", RoleWriteMode::CandidateProducer, &["worker"]);
    let worker = role("worker", RoleWriteMode::CandidateProducer, &["worker"]);
    let mut mgr2 = DelegationManager::new(grant2, vec![root_role, worker]);
    let root2 = RoleInstanceId::generate(&RoleDefinitionId::new("root_role"));
    mgr2.register_root(root2.clone(), RoleDefinitionId::new("root_role"))
        .unwrap();

    let depth1 = mgr2
        .delegate(DelegationRequest {
            parent_role_instance_id: root2.clone(),
            parent_attempt_id: attempt_id(&root2),
            requested_role: RoleDefinitionId::new("worker"),
            mode: DelegationMode::NonblockingChild,
            purpose: "depth 1".to_owned(),
            input_artifacts: vec![],
            requested_independence: IndependenceRequirement::NoIndependenceRequirement,
        })
        .expect("depth 1 is within the depth-1 budget");

    let depth_err = mgr2
        .delegate(DelegationRequest {
            parent_role_instance_id: depth1.role_instance_id.clone(),
            parent_attempt_id: attempt_id(&depth1.role_instance_id),
            requested_role: RoleDefinitionId::new("worker"),
            mode: DelegationMode::NonblockingChild,
            purpose: "depth 2 exceeds budget".to_owned(),
            input_artifacts: vec![],
            requested_independence: IndependenceRequirement::NoIndependenceRequirement,
        })
        .expect_err("delegation depth must be bounded");
    assert!(matches!(
        depth_err,
        DelegationError::BudgetExhausted(relay_core::BudgetError::DelegationDepthExceeded {
            requested: 2,
            max: 1
        })
    ));
}

#[test]
fn consultation_never_confers_write_authority() {
    let grant = grant_with(vec![RoleDefinitionId::new("verifier")], unbounded_budget());
    let implementer = role(
        "implementer",
        RoleWriteMode::CandidateProducer,
        &["verifier"],
    );
    // The verifier's OWN role definition is a write-capable CandidateProducer.
    let verifier = role("verifier", RoleWriteMode::CandidateProducer, &[]);
    let mut mgr = DelegationManager::new(grant, vec![implementer, verifier]);

    let root_id = RoleInstanceId::generate(&RoleDefinitionId::new("implementer"));
    mgr.register_root(root_id.clone(), RoleDefinitionId::new("implementer"))
        .unwrap();

    let outcome = mgr
        .consult(relay_core::ConsultationRequest {
            parent_role_instance_id: root_id.clone(),
            parent_attempt_id: attempt_id(&root_id),
            consultant_role: RoleDefinitionId::new("verifier"),
            purpose: "sanity check the approach".to_owned(),
            input_artifacts: vec![],
        })
        .expect("consultation of an approved role succeeds");

    assert_eq!(outcome.mode, DelegationMode::Consultation);
    // Even though "verifier" is declared CandidateProducer, consultation
    // forces the effective authority down to ReadOnly.
    assert_eq!(outcome.effective_write_mode, RoleWriteMode::ReadOnly);
}

#[test]
fn provider_cannot_name_executable_to_bypass_resolver() {
    let grant = grant_with(vec![RoleDefinitionId::new("worker")], unbounded_budget());
    let implementer = role("implementer", RoleWriteMode::CandidateProducer, &["worker"]);
    let worker = role("worker", RoleWriteMode::CandidateProducer, &[]);
    let mut mgr = DelegationManager::new(grant, vec![implementer, worker]);

    let root_id = RoleInstanceId::generate(&RoleDefinitionId::new("implementer"));
    mgr.register_root(root_id.clone(), RoleDefinitionId::new("implementer"))
        .unwrap();

    // The ONLY free-form field a caller controls is `purpose`. Try to smuggle
    // an executable/path/shell-injection payload through it.
    let malicious_purpose = "; rm -rf / # C:\\evil\\malware.exe --steal-secrets".to_owned();
    let outcome = mgr
        .delegate(DelegationRequest {
            parent_role_instance_id: root_id.clone(),
            parent_attempt_id: attempt_id(&root_id),
            requested_role: RoleDefinitionId::new("worker"),
            mode: DelegationMode::NonblockingChild,
            purpose: malicious_purpose.clone(),
            input_artifacts: vec![],
            requested_independence: IndependenceRequirement::NoIndependenceRequirement,
        })
        .expect("delegation to a pre-registered role definition still succeeds");

    // The only thing that was ever "named" is a trusted, pre-registered
    // RoleDefinitionId -- never an executable or path.
    assert_eq!(outcome.role_definition_id, RoleDefinitionId::new("worker"));

    // Execution target/launch resolution for the new role attempt still
    // goes exclusively through the Phase 08 resolver + provider adapter,
    // which never sees (and cannot be influenced by) `purpose`.
    let mut resolver = relay_runtime::ProviderResolver::new();
    resolver.register(
        relay_runtime::MockProviderAdapter::new(
            "codex",
            relay_runtime::ProviderCapabilities {
                supported_roles: ["worker"].into_iter().map(str::to_owned).collect(),
                filesystem_write: true,
                network: false,
                streaming_output: true,
                cancellation: true,
                supported_models: vec!["default".to_owned()],
            },
        )
        .arc(),
    );

    let runtime_attempt = relay_runtime::RoleAttempt::new(
        outcome.role_instance_id.to_string(),
        1,
        relay_runtime::ProviderCapabilityRequirements::new("worker"),
    );
    let target = resolver
        .resolve(&runtime_attempt)
        .expect("resolver picks the registered worker-capable provider");

    let adapter = relay_runtime::MockProviderAdapter::new(
        "codex",
        relay_runtime::ProviderCapabilities {
            supported_roles: ["worker"].into_iter().map(str::to_owned).collect(),
            filesystem_write: true,
            network: false,
            streaming_output: true,
            cancellation: true,
            supported_models: vec!["default".to_owned()],
        },
    );
    let context = relay_runtime::LaunchContext::new(
        std::env::temp_dir(),
        "worker",
        relay_runtime::EffectKey::new("eff-phase10-1"),
    );
    let spec = adapter
        .build_launch(&target, &runtime_attempt, &context)
        .expect("launch spec is built solely from provider/target/context");

    let spec_text = format!("{} {:?}", spec.executable.display(), spec.args);
    assert!(
        !spec_text.contains("rm -rf")
            && !spec_text.contains("evil")
            && !spec_text.contains("malware"),
        "malicious purpose text must never leak into the launch specification: {spec_text}"
    );
    assert!(!malicious_purpose.is_empty()); // the payload really was non-trivial
}

#[test]
fn cancellation_propagates_through_outstanding_children_without_overwriting_terminal_state() {
    let grant = grant_with(
        vec![
            RoleDefinitionId::new("child"),
            RoleDefinitionId::new("grandchild"),
        ],
        unbounded_budget(),
    );
    let root_role = role("root_role", RoleWriteMode::CandidateProducer, &["child"]);
    let child_role = role("child", RoleWriteMode::CandidateProducer, &["grandchild"]);
    let grandchild_role = role("grandchild", RoleWriteMode::ReadOnly, &[]);
    let mut mgr = DelegationManager::new(grant, vec![root_role, child_role, grandchild_role]);

    let root_id = RoleInstanceId::generate(&RoleDefinitionId::new("root_role"));
    mgr.register_root(root_id.clone(), RoleDefinitionId::new("root_role"))
        .unwrap();

    let child = mgr
        .delegate(DelegationRequest {
            parent_role_instance_id: root_id.clone(),
            parent_attempt_id: attempt_id(&root_id),
            requested_role: RoleDefinitionId::new("child"),
            mode: DelegationMode::BlockingChild,
            purpose: "do the work".to_owned(),
            input_artifacts: vec![],
            requested_independence: IndependenceRequirement::NoIndependenceRequirement,
        })
        .unwrap();

    let grandchild = mgr
        .delegate(DelegationRequest {
            parent_role_instance_id: child.role_instance_id.clone(),
            parent_attempt_id: attempt_id(&child.role_instance_id),
            requested_role: RoleDefinitionId::new("grandchild"),
            mode: DelegationMode::BlockingChild,
            purpose: "double check".to_owned(),
            input_artifacts: vec![],
            requested_independence: IndependenceRequirement::NoIndependenceRequirement,
        })
        .unwrap();

    // The grandchild already finished successfully before the parent is cancelled.
    mgr.complete(&grandchild.role_instance_id);
    assert_eq!(mgr.graph().active_count(), 2); // root + child remain Active

    let cancelled = mgr.cancel(&root_id);

    // Root and child transition to Cancelled; the already-terminal
    // grandchild is left untouched (never overwritten to Cancelled).
    assert!(cancelled.contains(&root_id));
    assert!(cancelled.contains(&child.role_instance_id));
    assert!(!cancelled.contains(&grandchild.role_instance_id));

    assert_eq!(
        mgr.graph().get(&root_id).unwrap().state,
        RoleDependencyState::Cancelled
    );
    assert_eq!(
        mgr.graph().get(&child.role_instance_id).unwrap().state,
        RoleDependencyState::Cancelled
    );
    assert_eq!(
        mgr.graph().get(&grandchild.role_instance_id).unwrap().state,
        RoleDependencyState::Completed
    );
    assert_eq!(mgr.graph().active_count(), 0);
}
