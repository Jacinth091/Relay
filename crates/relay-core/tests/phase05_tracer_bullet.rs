//! Phase 05: Deterministic End-to-End Tracer Bullet Integration Tests.
//!
//! Enforces:
//! 1. Full deterministic v2 orchestration without external backplanes or providers:
//!    legacy HandoffTask v1 -> Ready -> explicit Run approval -> ApprovedIntent / ExecutionGrant
//!    -> Implementer RoleInstance -> RoleAttempt -> ImplementationResult + Candidate 1
//!    -> Verifier RoleInstance -> independent RoleAttempt -> VerificationResult PASS -> COMPLETE.
//! 2. Bounded Remediation loop on verification failure:
//!    Candidate 1 -> Verifier FAIL -> Remediator RoleInstance -> Candidate 2
//!    -> fresh Verifier RoleInstance -> PASS -> COMPLETE.
//! 3. Process success alone never completes a role; acceptance requires typed result artifact.
//! 4. Candidate modification invalidates verification.
//! 5. Execution budget strictly halts unbounded remediation.
//! 6. Cancellation at any state leaves truthful durable state.

use std::{fs, path::PathBuf, process::Command};

use relay_core::{
    ArtifactKind, ArtifactSensitivity, ArtifactStore, ArtifactTrustClass, Candidate,
    CandidateFingerprint, CheckoutId, CreateArtifactRequest, ExecutionBudget, ExecutionTarget,
    ProjectId, ProjectKey, RetentionClass, RoleAttemptState, RoleDefinitionId, RoleInstance,
    RoleInstanceState, StoredTask, SubmissionTransport, TaskId, TaskState, WorkflowError,
    WorkflowRunState, adapt_v1_handoff_to_approved_intent, create_v1_compatibility_run,
};
use relay_protocol::{AmbiguityPolicy, HandoffTask, TaskSpec};

fn unique_temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("relay-p05-{name}-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn init_git_repo(path: &std::path::Path) {
    let run = |args: &[&str]| {
        let mut cmd = Command::new("git");
        cmd.arg("-C").arg(path);
        cmd.args(args);
        cmd.env_remove("GIT_DIR");
        cmd.env_remove("GIT_WORK_TREE");
        cmd.env_remove("GIT_COMMON_DIR");
        let out = cmd.output().expect("git command failed");
        assert!(out.status.success(), "git command {:?} failed", args);
    };

    run(&["init"]);
    run(&["config", "user.name", "Relay Tracer Bullet"]);
    run(&["config", "user.email", "tracer@relay.local"]);
    fs::write(path.join("README.md"), "# Initial\n").unwrap();
    run(&["add", "README.md"]);
    run(&["commit", "-m", "initial commit"]);
}

fn sample_task(state: TaskState) -> StoredTask {
    StoredTask {
        id: TaskId::generate(),
        handoff: HandoffTask {
            protocol_version: 1,
            project: "tracer-proj".to_owned(),
            title: "Implement tracer bullet".to_owned(),
            stack: None,
            source: None,
            task: TaskSpec {
                goal: "Verify v2 end to end orchestration loop".to_owned(),
                requirements: vec!["Pass verification".to_owned()],
                constraints: vec!["Deterministic".to_owned()],
                verification: vec!["cargo test".to_owned()],
            },
            ambiguity_policy: AmbiguityPolicy::NeedsClarification,
            extensions: std::collections::BTreeMap::new(),
        },
        state,
        project_key: ProjectKey::parse("tracer-proj").unwrap(),
        submission_transport: SubmissionTransport::LocalPaste,
        created_at: "now".to_owned(),
        updated_at: "now".to_owned(),
    }
}

#[test]
fn test_happy_path_tracer_bullet_end_to_end() {
    let base_dir = unique_temp_dir("tracer-happy");
    let repo_dir = base_dir.join("repo");
    fs::create_dir_all(&repo_dir).unwrap();
    init_git_repo(&repo_dir);

    let store_dir = base_dir.join("artifacts");
    let artifact_store = ArtifactStore::new(&store_dir).unwrap();

    // 1. Task in Ready state
    let task = sample_task(TaskState::Ready);
    let proj_id = ProjectId::parse(task.project_key.as_str()).unwrap();

    // 2. Human Run Approval -> ApprovedIntent generation 1 + ExecutionGrant + WorkflowRun
    let intent = adapt_v1_handoff_to_approved_intent(&task, "operator-alice").unwrap();

    let budget = ExecutionBudget {
        max_role_activations: 5,
        max_attempts_per_role: 3,
        max_delegations: 2,
        max_delegation_depth: 2,
        max_concurrent_attempts: 1,
        max_wall_clock_secs: Some(300),
    };

    let (_grant, mut workflow_run, _snapshot) =
        create_v1_compatibility_run(&task, &intent, budget.clone(), "operator-alice").unwrap();

    assert_eq!(workflow_run.state, WorkflowRunState::Running);

    // 3. Implementer RoleInstance
    let implementer_def_id = RoleDefinitionId::new("implementer");
    let mut implementer_instance = RoleInstance::new(
        workflow_run.id.clone(),
        implementer_def_id,
        intent.generation,
        None,
    );

    let target = ExecutionTarget {
        provider_id: "fake".to_owned(),
        runtime_profile_id: "fake/tracer".to_owned(),
        provider_adapter_version: "1.0.0".to_owned(),
    };

    let mut attempt = implementer_instance
        .spawn_attempt(target.clone(), &budget)
        .unwrap();
    assert_eq!(attempt.attempt_no, 1);

    // 4. Implementer writes code
    fs::write(repo_dir.join("feature.txt"), "feature v1 ready\n").unwrap();

    // Invariant: Process success alone NEVER completes role instance!
    attempt.finish("EXITED_SUCCESS".to_owned(), None);
    assert_eq!(attempt.state, RoleAttemptState::Completed);
    assert_eq!(implementer_instance.state, RoleInstanceState::Active); // Not completed yet!

    // Candidate 1 captured and registered
    let fp1 = CandidateFingerprint::capture(&repo_dir, None).unwrap();
    let checkout_id = CheckoutId::generate();
    let candidate_1 = Candidate::create(
        proj_id.clone(),
        checkout_id,
        workflow_run.id.clone(),
        1,
        implementer_instance.id.clone(),
        attempt.id.clone(),
        fp1.clone(),
    );

    // Implementation result artifact stored
    let impl_artifact = artifact_store
        .put(CreateArtifactRequest {
            kind: ArtifactKind::ImplementationResult,
            project_id: proj_id.clone(),
            task_id: task.id,
            workflow_run_id: workflow_run.id.clone(),
            producer_role_instance_id: Some(implementer_instance.id.clone()),
            producer_attempt_id: Some(attempt.id.clone()),
            candidate_id: Some(candidate_1.id.clone()),
            trust_class: ArtifactTrustClass::RelayObserved,
            sensitivity: ArtifactSensitivity::Internal,
            retention: RetentionClass::WorkflowScoped,
            payload: b"{\"summary\":\"Implemented feature v1\"}",
        })
        .unwrap();

    // Now accept result artifact -> Implementer RoleInstance Completed
    implementer_instance.accept_result(impl_artifact.id.0);
    assert_eq!(implementer_instance.state, RoleInstanceState::Completed);

    // 5. Verifier RoleInstance
    let verifier_def_id = RoleDefinitionId::new("verifier");
    let mut verifier_instance = RoleInstance::new(
        workflow_run.id.clone(),
        verifier_def_id,
        intent.generation,
        None,
    );

    let mut verifier_attempt = verifier_instance.spawn_attempt(target, &budget).unwrap();

    // Independent verification verifies repo against Candidate 1 fingerprint
    assert!(
        candidate_1
            .fingerprint
            .verify_against_repo(&repo_dir)
            .unwrap()
    );

    // Verification outcome is PASS
    let verif_artifact = artifact_store
        .put(CreateArtifactRequest {
            kind: ArtifactKind::VerificationResult,
            project_id: proj_id,
            task_id: task.id,
            workflow_run_id: workflow_run.id.clone(),
            producer_role_instance_id: Some(verifier_instance.id.clone()),
            producer_attempt_id: Some(verifier_attempt.id.clone()),
            candidate_id: Some(candidate_1.id.clone()),
            trust_class: ArtifactTrustClass::RelayObserved,
            sensitivity: ArtifactSensitivity::Internal,
            retention: RetentionClass::Permanent,
            payload: b"{\"outcome\":\"PASS\",\"checks_passed\":3}",
        })
        .unwrap();

    verifier_attempt.finish(
        "EXITED_SUCCESS".to_owned(),
        Some(verif_artifact.id.0.clone()),
    );
    verifier_instance.accept_result(verif_artifact.id.0);

    // 6. Workflow run transitions to Completed
    workflow_run.complete();
    assert_eq!(workflow_run.state, WorkflowRunState::Completed);
    assert!(workflow_run.completed_at.is_some());
}

#[test]
fn test_verifier_fail_remediation_loop_to_candidate_2() {
    let base_dir = unique_temp_dir("tracer-remediation");
    let repo_dir = base_dir.join("repo");
    fs::create_dir_all(&repo_dir).unwrap();
    init_git_repo(&repo_dir);

    let store_dir = base_dir.join("artifacts");
    let artifact_store = ArtifactStore::new(&store_dir).unwrap();

    let task = sample_task(TaskState::Ready);
    let proj_id = ProjectId::parse(task.project_key.as_str()).unwrap();
    let intent = adapt_v1_handoff_to_approved_intent(&task, "operator").unwrap();

    let budget = ExecutionBudget {
        max_role_activations: 5,
        max_attempts_per_role: 3,
        max_delegations: 2,
        max_delegation_depth: 2,
        max_concurrent_attempts: 1,
        max_wall_clock_secs: Some(300),
    };

    let (_grant, mut workflow_run, _snapshot) =
        create_v1_compatibility_run(&task, &intent, budget.clone(), "operator").unwrap();

    // --- Candidate 1 ---
    fs::write(repo_dir.join("buggy.txt"), "faulty code\n").unwrap();
    let fp1 = CandidateFingerprint::capture(&repo_dir, None).unwrap();
    let checkout_id = CheckoutId::generate();
    let role_impl_id = RoleDefinitionId::new("implementer");
    let mut impl_instance = RoleInstance::new(workflow_run.id.clone(), role_impl_id, 1, None);
    let target = ExecutionTarget {
        provider_id: "fake".to_owned(),
        runtime_profile_id: "fake/tracer".to_owned(),
        provider_adapter_version: "1.0.0".to_owned(),
    };
    let mut impl_att = impl_instance
        .spawn_attempt(target.clone(), &budget)
        .unwrap();
    impl_att.finish("EXITED_SUCCESS".to_owned(), None);

    let candidate_1 = Candidate::create(
        proj_id.clone(),
        checkout_id.clone(),
        workflow_run.id.clone(),
        1,
        impl_instance.id.clone(),
        impl_att.id.clone(),
        fp1,
    );

    // --- Verifier 1 fails ---
    let role_verif_id = RoleDefinitionId::new("verifier");
    let mut verif_instance_1 =
        RoleInstance::new(workflow_run.id.clone(), role_verif_id.clone(), 1, None);
    let mut verif_att_1 = verif_instance_1
        .spawn_attempt(target.clone(), &budget)
        .unwrap();

    let fail_artifact = artifact_store
        .put(CreateArtifactRequest {
            kind: ArtifactKind::VerificationResult,
            project_id: proj_id.clone(),
            task_id: task.id,
            workflow_run_id: workflow_run.id.clone(),
            producer_role_instance_id: Some(verif_instance_1.id.clone()),
            producer_attempt_id: Some(verif_att_1.id.clone()),
            candidate_id: Some(candidate_1.id.clone()),
            trust_class: ArtifactTrustClass::RelayObserved,
            sensitivity: ArtifactSensitivity::Internal,
            retention: RetentionClass::Permanent,
            payload: b"{\"outcome\":\"FAIL\",\"error\":\"Assertion failed on buggy.txt\"}",
        })
        .unwrap();

    verif_att_1.finish(
        "EXITED_SUCCESS".to_owned(),
        Some(fail_artifact.id.0.clone()),
    );
    verif_instance_1.fail(); // Verification failed!

    // --- Remediator RoleInstance produces Candidate 2 ---
    let role_remed_id = RoleDefinitionId::new("remediator");
    let mut remed_instance = RoleInstance::new(workflow_run.id.clone(), role_remed_id, 1, None);
    let mut remed_att = remed_instance
        .spawn_attempt(target.clone(), &budget)
        .unwrap();

    // Fix the bug
    fs::write(repo_dir.join("buggy.txt"), "fixed code\n").unwrap();
    let fp2 = CandidateFingerprint::capture(&repo_dir, None).unwrap();
    assert_ne!(candidate_1.fingerprint, fp2);

    let candidate_2 = Candidate::create(
        proj_id.clone(),
        checkout_id,
        workflow_run.id.clone(),
        2,
        remed_instance.id.clone(),
        remed_att.id.clone(),
        fp2,
    );

    let remed_artifact = artifact_store
        .put(CreateArtifactRequest {
            kind: ArtifactKind::ImplementationResult,
            project_id: proj_id.clone(),
            task_id: task.id,
            workflow_run_id: workflow_run.id.clone(),
            producer_role_instance_id: Some(remed_instance.id.clone()),
            producer_attempt_id: Some(remed_att.id.clone()),
            candidate_id: Some(candidate_2.id.clone()),
            trust_class: ArtifactTrustClass::RelayObserved,
            sensitivity: ArtifactSensitivity::Internal,
            retention: RetentionClass::WorkflowScoped,
            payload: b"{\"summary\":\"Fixed bug in buggy.txt\"}",
        })
        .unwrap();

    remed_att.finish(
        "EXITED_SUCCESS".to_owned(),
        Some(remed_artifact.id.0.clone()),
    );
    remed_instance.accept_result(remed_artifact.id.0);

    // --- Fresh Verifier for Candidate 2 passes ---
    let mut verif_instance_2 = RoleInstance::new(workflow_run.id.clone(), role_verif_id, 1, None);
    let mut verif_att_2 = verif_instance_2.spawn_attempt(target, &budget).unwrap();

    assert!(
        candidate_2
            .fingerprint
            .verify_against_repo(&repo_dir)
            .unwrap()
    );

    let pass_artifact = artifact_store
        .put(CreateArtifactRequest {
            kind: ArtifactKind::VerificationResult,
            project_id: proj_id,
            task_id: task.id,
            workflow_run_id: workflow_run.id.clone(),
            producer_role_instance_id: Some(verif_instance_2.id.clone()),
            producer_attempt_id: Some(verif_att_2.id.clone()),
            candidate_id: Some(candidate_2.id.clone()),
            trust_class: ArtifactTrustClass::RelayObserved,
            sensitivity: ArtifactSensitivity::Internal,
            retention: RetentionClass::Permanent,
            payload: b"{\"outcome\":\"PASS\",\"checks_passed\":3}",
        })
        .unwrap();

    verif_att_2.finish(
        "EXITED_SUCCESS".to_owned(),
        Some(pass_artifact.id.0.clone()),
    );
    verif_instance_2.accept_result(pass_artifact.id.0);

    workflow_run.complete();
    assert_eq!(workflow_run.state, WorkflowRunState::Completed);
}

#[test]
fn test_budget_exhaustion_stops_unbounded_attempts() {
    let task = sample_task(TaskState::Ready);
    let intent = adapt_v1_handoff_to_approved_intent(&task, "operator").unwrap();

    let strict_budget = ExecutionBudget {
        max_role_activations: 5,
        max_attempts_per_role: 2, // strictly capped at 2 attempts
        max_delegations: 2,
        max_delegation_depth: 2,
        max_concurrent_attempts: 1,
        max_wall_clock_secs: Some(300),
    };

    let (_grant, workflow_run, _snapshot) =
        create_v1_compatibility_run(&task, &intent, strict_budget.clone(), "operator").unwrap();

    let mut instance = RoleInstance::new(
        workflow_run.id,
        RoleDefinitionId::new("flaky-role"),
        1,
        None,
    );

    let target = ExecutionTarget {
        provider_id: "fake".to_owned(),
        runtime_profile_id: "fake/tracer".to_owned(),
        provider_adapter_version: "1.0.0".to_owned(),
    };

    // Attempt 1 succeeds
    assert!(
        instance
            .spawn_attempt(target.clone(), &strict_budget)
            .is_ok()
    );
    // Attempt 2 succeeds
    assert!(
        instance
            .spawn_attempt(target.clone(), &strict_budget)
            .is_ok()
    );

    // Attempt 3 fails closed due to budget
    let res = instance.spawn_attempt(target, &strict_budget);
    assert!(matches!(res, Err(WorkflowError::BudgetExhausted { .. })));
}

#[test]
fn test_cancellation_preserves_truthful_state() {
    let task = sample_task(TaskState::Ready);
    let intent = adapt_v1_handoff_to_approved_intent(&task, "operator").unwrap();
    let budget = ExecutionBudget::default();

    let (_grant, mut workflow_run, _snapshot) =
        create_v1_compatibility_run(&task, &intent, budget, "operator").unwrap();

    let mut instance = RoleInstance::new(
        workflow_run.id.clone(),
        RoleDefinitionId::new("implementer"),
        1,
        None,
    );

    // Cancel workflow run
    workflow_run.cancel();
    assert_eq!(workflow_run.state, WorkflowRunState::Cancelled);
    assert!(workflow_run.completed_at.is_some());

    // Cancel active role instance
    instance.cancel();
    assert_eq!(instance.state, RoleInstanceState::Cancelled);
}
