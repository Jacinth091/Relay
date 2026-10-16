//! Phase 15: Integrated Recovery, Retention, and Security Truthfulness Tests.
//!
//! Verifies:
//! 1. Daemon crash recovery: pending effect reconciliation, lease fencing verification, and capability revocation.
//! 2. Candidate mutation during verification invalidates candidate truthfully.
//! 3. Retention manager: prunes orphaned worktrees, removes expired projections, collects expired Ephemeral artifacts, preserves WorkflowScoped artifacts.
//! 4. Process-loss truthfulness: terminated/lost processes fail explicitly without fake success.
//! 5. Approval racing parameter change fails closed.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use relay_core::{
    ApprovalGrant, ApprovalRequest, ArtifactKind, ArtifactSensitivity, ArtifactStore,
    ArtifactTrustClass, Candidate, CandidateFingerprint, CandidateId, CandidateIntegrity,
    CheckoutId, CreateArtifactRequest, GitOperationRunner, PrivilegedAction,
    PrivilegedActionExecutor, PrivilegedError, ProjectId, RecoveryCoordinator, RecoveryError,
    RelayExchange, RetentionClass, RetentionManager, RetentionPolicy, RoleAttemptId,
    RoleInstanceId, SqliteStore, TaskId, WorkflowRunId, WorktreeHandle,
};

fn unique_temp_dir(suffix: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("relay_phase15_{}_{}", suffix, uuid::Uuid::new_v4()));
    fs::create_dir_all(&path).expect("failed to create temp dir");
    path
}

fn init_git_repo(path: &Path) {
    let run_git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(path)
            .output()
            .expect("failed to execute git");
        assert!(
            output.status.success(),
            "git command failed: {:?}",
            String::from_utf8_lossy(&output.stderr)
        );
    };

    run_git(&["init"]);
    run_git(&["config", "user.name", "Relay Test"]);
    run_git(&["config", "user.email", "relay-test@example.com"]);
    fs::write(path.join("README.md"), "# Test Repo\n").unwrap();
    run_git(&["add", "README.md"]);
    run_git(&["commit", "-m", "initial commit"]);
}

#[tokio::test]
async fn test_daemon_crash_pending_effect_reconciliation() {
    let store = SqliteStore::in_memory().await.unwrap();

    // 1. Enqueue a pending effect left in-flight when daemon crashed
    let record = relay_store::NewPendingEffectRecord {
        id: "eff-crash-1".to_owned(),
        effect_type: "checkout_create".to_owned(),
        idempotency_key: "idem-1".to_owned(),
        payload_json: "{\"checkout\":\"main\"}".to_owned(),
    };
    store.insert_pending_effect(&record).await.unwrap();

    // 2. Startup recovery with stale fencing token fails
    let stale_res = RecoveryCoordinator::reconcile_startup(&store, None, 1, 2).await;
    assert!(matches!(
        stale_res,
        Err(RecoveryError::StaleFencingToken { .. })
    ));

    // 3. Startup recovery with valid higher fencing token reconciles orphaned effect
    let report = RecoveryCoordinator::reconcile_startup(&store, None, 3, 2)
        .await
        .expect("recovery succeeded");

    assert_eq!(report.fencing_token, 3);
    assert_eq!(report.pending_effects_reconciled, 1);
    assert_eq!(report.orphaned_effects_failed, 1);

    // Invariant: Orphaned effect has a recorded failure state (not faked success)
    let effect = store
        .get_pending_effect("eff-crash-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(effect.state, "failed");
}

#[test]
fn test_candidate_mutation_during_verification_invalidates_candidate() {
    let repo_dir = unique_temp_dir("cand_mut");
    init_git_repo(&repo_dir);

    // Generate Candidate from clean repo
    let fp = CandidateFingerprint::capture(&repo_dir, None).unwrap();
    let role_inst = RoleInstanceId("role-imp-1".to_owned());
    let attempt_id = RoleAttemptId::generate(&role_inst, 1);
    let candidate = Candidate::create(
        ProjectId::parse("proj-1").unwrap(),
        CheckoutId::parse("chk-1").unwrap(),
        WorkflowRunId("run-1".to_owned()),
        1,
        role_inst,
        attempt_id,
        fp,
    );

    // 1. Clean repo matches candidate
    let res = RecoveryCoordinator::verify_candidate_integrity(&candidate, &repo_dir, None).unwrap();
    assert_eq!(res, CandidateIntegrity::Valid);

    // 2. Mutate working tree (e.g. unapproved file modification during verification)
    fs::write(repo_dir.join("TAMPERED.txt"), "rogue content").unwrap();

    let res_mut =
        RecoveryCoordinator::verify_candidate_integrity(&candidate, &repo_dir, None).unwrap();
    assert!(matches!(res_mut, CandidateIntegrity::Mutated { .. }));
}

#[tokio::test]
async fn test_retention_cycle_pruning_and_artifact_collection() {
    let root = unique_temp_dir("retention_test");
    let worktree_dir = root.join("worktrees");
    let projection_dir = root.join("projections");
    let artifact_dir = root.join("artifacts");

    fs::create_dir_all(&worktree_dir).unwrap();
    fs::create_dir_all(&projection_dir).unwrap();

    let artifact_store = ArtifactStore::new(&artifact_dir).unwrap();
    let exchange = RelayExchange::new(Arc::new(ArtifactStore::new(&artifact_dir).unwrap()));

    // 1. Worktrees: active vs orphaned
    let active_wt = worktree_dir.join("wt-active");
    let orphaned_wt = worktree_dir.join("wt-orphaned");
    fs::create_dir_all(&active_wt).unwrap();
    fs::create_dir_all(&orphaned_wt).unwrap();

    let active_handle = WorktreeHandle {
        id: "wt-active".to_owned(),
        path: active_wt.clone(),
        base_commit: "HEAD".to_owned(),
        created_at: "0".to_owned(),
    };

    // 2. Session projections: active vs expired
    let expired_proj = projection_dir.join("role-att-expired");
    let active_proj = projection_dir.join("role-att-active");
    fs::create_dir_all(&expired_proj).unwrap();
    fs::create_dir_all(&active_proj).unwrap();

    // Expired session (expires_at = 0)
    fs::write(
        expired_proj.join("SESSION.json"),
        serde_json::json!({ "expires_at": 0 }).to_string(),
    )
    .unwrap();

    // Active session (expires_at = far in future)
    fs::write(
        active_proj.join("SESSION.json"),
        serde_json::json!({ "expires_at": 9999999999u64 }).to_string(),
    )
    .unwrap();

    // 3. Artifacts: Ephemeral vs WorkflowScoped
    let meta_eph = artifact_store
        .put(CreateArtifactRequest {
            kind: ArtifactKind::RawLog,
            project_id: ProjectId::parse("p1").unwrap(),
            task_id: TaskId::generate(),
            workflow_run_id: WorkflowRunId("w1".to_owned()),
            producer_role_instance_id: None,
            producer_attempt_id: None,
            candidate_id: None,
            trust_class: ArtifactTrustClass::RelayObserved,
            sensitivity: ArtifactSensitivity::Internal,
            retention: RetentionClass::Ephemeral,
            payload: b"ephemeral raw log data",
        })
        .unwrap();

    let meta_scoped = artifact_store
        .put(CreateArtifactRequest {
            kind: ArtifactKind::VerificationResult,
            project_id: ProjectId::parse("p1").unwrap(),
            task_id: TaskId::generate(),
            workflow_run_id: WorkflowRunId("w1".to_owned()),
            producer_role_instance_id: None,
            producer_attempt_id: None,
            candidate_id: Some(CandidateId::generate()),
            trust_class: ArtifactTrustClass::RelayObserved,
            sensitivity: ArtifactSensitivity::Internal,
            retention: RetentionClass::WorkflowScoped,
            payload: b"{\"passed\": true}",
        })
        .unwrap();

    // Configure retention policy with 0s TTL so all ephemeral items are eligible
    let policy = RetentionPolicy {
        attempt_worktree_ttl_seconds: 0,
        ephemeral_projection_ttl_seconds: 0,
        artifact_ephemeral_ttl_seconds: 0,
    };
    let manager = RetentionManager::new(policy);

    let report = manager
        .run_retention_cycle(
            Some(&worktree_dir),
            &[&active_handle],
            Some(&projection_dir),
            Some(&artifact_store),
            Some(&exchange),
        )
        .expect("retention cycle succeeded");

    // Assertions
    assert_eq!(report.worktrees_pruned, 1);
    assert!(!orphaned_wt.exists(), "orphaned worktree must be pruned");
    assert!(active_wt.exists(), "active worktree must be preserved");

    assert_eq!(report.projections_cleaned, 1);
    assert!(!expired_proj.exists(), "expired projection must be removed");
    assert!(active_proj.exists(), "active projection must be preserved");

    assert_eq!(report.artifacts_collected, 1);
    // Ephemeral artifact was collected
    assert!(
        artifact_store.get(&meta_eph).is_err(),
        "ephemeral artifact should be collected"
    );
    // WorkflowScoped artifact remains intact!
    assert!(
        artifact_store.get(&meta_scoped).is_ok(),
        "workflow-scoped verification artifact must be preserved"
    );
}

#[test]
fn test_process_loss_truthfulness() {
    let role_inst = RoleInstanceId("role-verifier-1".to_owned());
    let attempt_id = RoleAttemptId::generate(&role_inst, 1);
    let outcome = RecoveryCoordinator::reconcile_lost_process(&attempt_id, "SIGKILL by host OOM");

    match outcome {
        relay_core::AttemptReconciliation::FailedProcessLost { reason } => {
            assert!(reason.contains("terminated unexpectedly"));
            assert!(reason.contains("SIGKILL"));
        }
        _ => panic!("expected FailedProcessLost, never optimistic success"),
    }
}

#[test]
fn test_approval_parameter_change_invalidates_grant() {
    let action_a = PrivilegedAction::GitBranchCreate {
        checkout_id: CheckoutId::parse("chk-1").unwrap(),
        branch_name: "feature/safe-branch".to_owned(),
        from_ref: "HEAD".to_owned(),
    };

    let req = ApprovalRequest::new(
        ProjectId::parse("proj-1").unwrap(),
        TaskId::generate(),
        WorkflowRunId("run-1".to_owned()),
        action_a.clone(),
        "operator approved safe-branch",
        None,
        300,
    )
    .unwrap();

    let grant = ApprovalGrant::issue(&req, "human-operator");

    // Different action (modified branch name)
    let action_mutated = PrivilegedAction::GitBranchCreate {
        checkout_id: CheckoutId::parse("chk-1").unwrap(),
        branch_name: "feature/attacker-branch".to_owned(),
        from_ref: "HEAD".to_owned(),
    };

    // Attempting to execute action_mutated with grant for action_a must fail closed
    let executor = PrivilegedActionExecutor::new(Arc::new(GitOperationRunner::new()));
    executor.register_request(req);
    let res = executor.execute(&grant, &action_mutated, Path::new("."));

    assert!(matches!(res, Err(PrivilegedError::ParameterMismatch)));
}
