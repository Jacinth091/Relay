//! Phase 04 Integration Tests: Candidate, Worktree, Lease, and Artifact Foundation.
//!
//! Enforces:
//! 1. Candidate N generation with full repo fingerprint (`base_head`, `current_head`, `index_digest`, `tracked_diff_digest`, `relevant_untracked_digest`).
//! 2. Worktree isolation strictly inside allowed checkout boundary; attempts to escape outside root are rejected fail-closed.
//! 3. Worktree cleanup removes the worktree and prunes git references.
//! 4. Candidate verification invalidation: manual untracked or tracked file mutations break verification against observed fingerprint.
//! 5. Monotonic lease fencing tokens supersede earlier holders and block stale concurrent writes.
//! 6. Immutable Artifact store guarantees atomic write, checksum integrity, and tamper detection.

use std::{fs, path::PathBuf, process::Command};

use relay_core::{
    ArtifactKind, ArtifactSensitivity, ArtifactStore, ArtifactTrustClass, Candidate,
    CandidateError, CandidateFingerprint, Checkout, CheckoutId, CheckoutTrustState, Lease,
    LeaseHolder, LeaseId, LeaseKind, LeaseResource, ProjectId, RetentionClass, RoleAttemptId,
    RoleInstanceId, WorkflowRunId, WorktreeManager,
};

fn unique_temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("relay-p04-{name}-{}", uuid::Uuid::new_v4()));
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
    run(&["config", "user.name", "Relay Test"]);
    run(&["config", "user.email", "test@relay.local"]);
    fs::write(path.join("file1.txt"), "hello world\n").unwrap();
    run(&["add", "file1.txt"]);
    run(&["commit", "-m", "initial commit"]);
}

#[test]
fn test_candidate_fingerprint_and_invalidation() {
    let repo_dir = unique_temp_dir("cand-invalidation");
    init_git_repo(&repo_dir);

    // 1. Capture initial candidate fingerprint
    let fp = CandidateFingerprint::capture(&repo_dir, None).unwrap();
    assert!(!fp.current_head.is_empty());
    assert_eq!(fp.base_head, fp.current_head);
    assert_eq!(fp.index_digest.len(), 64);
    assert_eq!(fp.tracked_diff_digest.len(), 64);
    assert_eq!(fp.relevant_untracked_digest.len(), 64);
    assert!(fp.index_digest.chars().all(|c| c.is_ascii_hexdigit()));
    assert!(
        fp.tracked_diff_digest
            .chars()
            .all(|c| c.is_ascii_hexdigit())
    );
    assert!(
        fp.relevant_untracked_digest
            .chars()
            .all(|c| c.is_ascii_hexdigit())
    );

    let proj_id = ProjectId::parse("test-project").unwrap();
    let checkout_id = CheckoutId::generate();
    let run_id = WorkflowRunId::generate();
    let role_id = relay_core::RoleDefinitionId("implementer".to_owned());
    let instance_id = RoleInstanceId::generate(&role_id);
    let attempt_id = RoleAttemptId::generate(&instance_id, 1);

    let candidate = Candidate::create(
        proj_id,
        checkout_id,
        run_id,
        1,
        instance_id,
        attempt_id,
        fp.clone(),
    );

    assert_eq!(candidate.generation, 1);
    assert!(
        candidate
            .fingerprint
            .verify_against_repo(&repo_dir)
            .unwrap()
    );

    // 2. Modify tracked file without commit -> verification fails
    fs::write(repo_dir.join("file1.txt"), "mutated content\n").unwrap();
    assert!(
        !candidate
            .fingerprint
            .verify_against_repo(&repo_dir)
            .unwrap()
    );

    // 3. Restore tracked file -> verification passes again
    fs::write(repo_dir.join("file1.txt"), "hello world\n").unwrap();
    assert!(
        candidate
            .fingerprint
            .verify_against_repo(&repo_dir)
            .unwrap()
    );

    // 4. Add unexpected untracked file -> verification fails
    fs::write(repo_dir.join("rogue.sh"), "#!/bin/sh\necho evil\n").unwrap();
    assert!(
        !candidate
            .fingerprint
            .verify_against_repo(&repo_dir)
            .unwrap()
    );

    // 5. Remove untracked file -> verification passes again
    fs::remove_file(repo_dir.join("rogue.sh")).unwrap();
    assert!(
        candidate
            .fingerprint
            .verify_against_repo(&repo_dir)
            .unwrap()
    );
}

#[test]
fn test_worktree_isolation_and_boundary_enforcement() {
    let base_dir = unique_temp_dir("worktree-isolation");
    let repo_dir = base_dir.join("repo");
    let wt_root = base_dir.join("worktrees");
    fs::create_dir_all(&repo_dir).unwrap();
    fs::create_dir_all(&wt_root).unwrap();
    init_git_repo(&repo_dir);

    let checkout = Checkout::new(
        CheckoutId::generate(),
        ProjectId::parse("test-project").unwrap(),
        repo_dir.clone(),
        None,
        Some(wt_root.clone()),
        CheckoutTrustState::Trusted,
    )
    .unwrap();
    let trust = relay_core::TrustClassification::TrustedAgainstRegistration {
        project_id: checkout.project_id.clone(),
        checkout_id: checkout.id.to_string(),
    };

    // 1. Create valid isolated worktree
    let handle =
        WorktreeManager::create_isolated_worktree(&checkout, &trust, "attempt-1", "HEAD").unwrap();
    assert!(handle.path.exists());
    assert!(handle.path.starts_with(&wt_root));

    // Inspect files inside worktree
    assert!(handle.path.join("file1.txt").exists());

    // 2. Clean up worktree
    WorktreeManager::remove_isolated_worktree(&checkout, &handle).unwrap();
    assert!(!handle.path.exists());

    // 3. Directory traversal attempt must be rejected
    let traversal_err =
        WorktreeManager::create_isolated_worktree(&checkout, &trust, "../../outside", "HEAD");
    assert!(traversal_err.is_err());
}

#[test]
fn test_lease_monotonic_fencing_tokens() {
    let lease = Lease::new(
        LeaseId::generate(),
        LeaseResource::Checkout(CheckoutId::generate()),
        LeaseKind::ExclusiveWrite,
        LeaseHolder {
            holder_id: "agent-active".to_owned(),
            pid: 1234,
            host: "relay-daemon".to_owned(),
        },
        10,
        60,
    );

    // Current token is 10
    assert!(lease.assert_valid(10).is_ok());
    // Advanced token is valid
    assert!(lease.assert_valid(11).is_ok());

    // Stale token (e.g. 9 or earlier) is rejected
    let res = lease.assert_valid(9);
    assert!(matches!(
        res,
        Err(CandidateError::FencingConflict {
            current: 10,
            attempted: 9
        })
    ));
}

#[test]
fn test_immutable_artifact_store_provenance_and_tamper_detection() {
    let store_root = unique_temp_dir("artifacts");
    let store = ArtifactStore::new(&store_root).unwrap();

    let payload = b"{\"outcome\":\"PASS\",\"tests_passed\":42}";
    let proj_id = ProjectId::parse("test-project").unwrap();
    let task_id = relay_core::TaskId::generate();
    let run_id = WorkflowRunId::generate();

    let meta = store
        .put(relay_core::CreateArtifactRequest {
            kind: ArtifactKind::VerificationResult,
            project_id: proj_id,
            task_id,
            workflow_run_id: run_id,
            producer_role_instance_id: None,
            producer_attempt_id: None,
            candidate_id: None,
            trust_class: ArtifactTrustClass::RelayObserved,
            sensitivity: ArtifactSensitivity::Internal,
            retention: RetentionClass::Permanent,
            payload,
        })
        .unwrap();

    assert_eq!(meta.size_bytes, payload.len() as u64);
    assert_eq!(meta.checksum.len(), 64);
    assert!(meta.checksum.chars().all(|c| c.is_ascii_hexdigit()));

    // Fetch and verify content
    let retrieved = store.get(&meta).unwrap();
    assert_eq!(retrieved, payload);

    // Tamper with underlying blob
    let blob_path = store.root().join(&meta.storage_ref.relative_path);
    fs::write(&blob_path, b"{\"outcome\":\"PASS\",\"tests_passed\":999}").unwrap();

    // Verification fails on tamper
    let err = store.get(&meta).unwrap_err();
    assert!(matches!(
        err,
        relay_core::ArtifactError::ChecksumMismatch { .. }
    ));
}
