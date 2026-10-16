//! Phase 04: Candidate, Lease, and Worktree Foundation.
//!
//! Provides:
//! - Candidate generation with full repository fingerprinting (`base_head`, `current_head`,
//!   `index_digest`, `tracked_diff_digest`, `relevant_untracked_digest`).
//! - Worktree isolation within authorized checkout worktree boundaries.
//! - Leases with monotonic fencing tokens.
//! - Candidate invalidation verification when repo state diverges.

use std::{
    io,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    Checkout, CheckoutId, ProjectId, RoleAttemptId, RoleInstanceId, TaskId, TrustClassification,
    WorkflowRunId, digest::sha256_digest,
};

#[derive(Debug, Error)]
pub enum CandidateError {
    #[error("git command failed: {0}")]
    Git(String),
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("checkout is not trusted for worktree creation: {0}")]
    UntrustedCheckout(String),
    #[error("unauthorized worktree path outside checkout root: {0}")]
    UnauthorizedWorktreePath(PathBuf),
    #[error("worktree creation failed: {0}")]
    WorktreeCreationFailed(String),
    #[error("worktree cleanup failed: {0}")]
    WorktreeCleanupFailed(String),
    #[error("candidate fingerprint mismatch: expected {expected:?}, observed {observed:?}")]
    FingerprintMismatch {
        expected: Box<CandidateFingerprint>,
        observed: Box<CandidateFingerprint>,
    },
    #[error(
        "lease expired or superseded: current fencing token is {current}, attempted {attempted}"
    )]
    FencingConflict { current: u64, attempted: u64 },
}

/// Computes a hex-encoded SHA-256 digest from bytes.
pub fn compute_digest(bytes: &[u8]) -> String {
    sha256_digest(bytes)
}

fn now_timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_owned())
}

// ---------------------------------------------------------------------------
// Candidate Identifiers and Fingerprint
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CandidateId(pub String);

impl CandidateId {
    pub fn generate() -> Self {
        Self(format!("cand-{}", uuid::Uuid::new_v4().simple()))
    }
}

impl std::fmt::Display for CandidateId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Comprehensive cryptographic/content fingerprint of the repository state covered by a Candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateFingerprint {
    pub base_head: String,
    pub current_head: String,
    pub index_digest: String,
    pub tracked_diff_digest: String,
    pub relevant_untracked_digest: String,
}

impl CandidateFingerprint {
    /// Captures the fingerprint of the git repository at `path`.
    pub fn capture(path: &Path, base_ref: Option<&str>) -> Result<Self, CandidateError> {
        let base_head = match base_ref {
            Some(r) => git_rev_parse(path, r)?,
            None => git_rev_parse(path, "HEAD")?,
        };

        let current_head = git_rev_parse(path, "HEAD")?;

        // Digest of staged changes / index state
        let index_status = git_exec(path, &["diff", "--cached"])?;
        let index_digest = compute_digest(index_status.as_bytes());

        // Digest of tracked unstaged diff from current_head
        let diff_status = git_exec(path, &["diff", "HEAD"])?;
        let tracked_diff_digest = compute_digest(diff_status.as_bytes());

        // Untracked files manifest and content digest (excluding git ignored)
        let untracked_files = git_exec(path, &["ls-files", "--others", "--exclude-standard"])?;
        let mut untracked_entries = Vec::new();
        for file in untracked_files.lines() {
            let file_trimmed = file.trim();
            if file_trimmed.is_empty() {
                continue;
            }
            let file_path = path.join(file_trimmed);
            if file_path.is_file() {
                if let Ok(content) = std::fs::read(&file_path) {
                    untracked_entries.push(format!(
                        "{}:{}",
                        file_trimmed,
                        compute_digest(&content)
                    ));
                }
            }
        }
        untracked_entries.sort();
        let relevant_untracked_digest = compute_digest(untracked_entries.join("\n").as_bytes());

        Ok(Self {
            base_head,
            current_head,
            index_digest,
            tracked_diff_digest,
            relevant_untracked_digest,
        })
    }

    /// Checks if another observed fingerprint matches this fingerprint exactly.
    pub fn matches(&self, other: &Self) -> bool {
        self == other
    }

    /// Verifies against the current filesystem state of the given repository.
    pub fn verify_against_repo(&self, path: &Path) -> Result<bool, CandidateError> {
        let current = Self::capture(path, Some(&self.base_head))?;
        Ok(self.matches(&current))
    }
}

/// An immutable Candidate produced by an implementation role attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    pub id: CandidateId,
    pub project_id: ProjectId,
    pub checkout_id: CheckoutId,
    pub workflow_run_id: WorkflowRunId,
    pub generation: u32,
    pub producer_role_instance_id: RoleInstanceId,
    pub producer_attempt_id: RoleAttemptId,
    pub fingerprint: CandidateFingerprint,
    pub created_at: String,
}

impl Candidate {
    pub fn create(
        project_id: ProjectId,
        checkout_id: CheckoutId,
        workflow_run_id: WorkflowRunId,
        generation: u32,
        producer_role_instance_id: RoleInstanceId,
        producer_attempt_id: RoleAttemptId,
        fingerprint: CandidateFingerprint,
    ) -> Self {
        Self {
            id: CandidateId::generate(),
            project_id,
            checkout_id,
            workflow_run_id,
            generation,
            producer_role_instance_id,
            producer_attempt_id,
            fingerprint,
            created_at: now_timestamp(),
        }
    }
}

// ---------------------------------------------------------------------------
// Worktree Isolation and Management
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeHandle {
    pub id: String,
    pub path: PathBuf,
    pub base_commit: String,
    pub created_at: String,
}

pub struct WorktreeManager;

impl WorktreeManager {
    /// Creates an isolated worktree rooted strictly inside `checkout.allowed_worktree_root`.
    pub fn create_isolated_worktree(
        checkout: &Checkout,
        trust: &TrustClassification,
        worktree_name: &str,
        base_commit: &str,
    ) -> Result<WorktreeHandle, CandidateError> {
        match trust {
            TrustClassification::TrustedAgainstRegistration {
                project_id,
                checkout_id,
            } => {
                if project_id != &checkout.project_id || checkout_id != checkout.id.as_str() {
                    return Err(CandidateError::UntrustedCheckout(format!(
                        "trust classification is for {project_id}/{checkout_id} but checkout is {}/{}",
                        checkout.project_id, checkout.id
                    )));
                }
            }
            TrustClassification::UntrustedDeclaration { reason } => {
                return Err(CandidateError::UntrustedCheckout(reason.clone()));
            }
        }

        let name_path = Path::new(worktree_name);
        if name_path.is_absolute()
            || name_path.components().any(|c| {
                matches!(
                    c,
                    std::path::Component::ParentDir
                        | std::path::Component::RootDir
                        | std::path::Component::Prefix(_)
                )
            })
        {
            return Err(CandidateError::UnauthorizedWorktreePath(
                checkout.allowed_worktree_root.join(worktree_name),
            ));
        }

        let target_path = checkout.allowed_worktree_root.join(worktree_name);
        if !target_path.starts_with(&checkout.allowed_worktree_root) {
            return Err(CandidateError::UnauthorizedWorktreePath(target_path));
        }

        std::fs::create_dir_all(&checkout.allowed_worktree_root)?;

        let target_str = target_path.to_string_lossy();
        // git worktree add --detach <path> <commit>
        let res = git_exec(
            &checkout.canonical_root,
            &["worktree", "add", "--detach", &target_str, base_commit],
        );
        if let Err(err) = res {
            return Err(CandidateError::WorktreeCreationFailed(err.to_string()));
        }

        Ok(WorktreeHandle {
            id: worktree_name.to_owned(),
            path: target_path,
            base_commit: base_commit.to_owned(),
            created_at: now_timestamp(),
        })
    }

    /// Cleans up and removes an isolated worktree.
    pub fn remove_isolated_worktree(
        checkout: &Checkout,
        handle: &WorktreeHandle,
    ) -> Result<(), CandidateError> {
        if !handle.path.starts_with(&checkout.allowed_worktree_root) {
            return Err(CandidateError::UnauthorizedWorktreePath(
                handle.path.clone(),
            ));
        }

        let target_str = handle.path.to_string_lossy();
        let _ = git_exec(
            &checkout.canonical_root,
            &["worktree", "remove", "--force", &target_str],
        );

        if handle.path.exists() {
            let _ = std::fs::remove_dir_all(&handle.path);
        }

        let _ = git_exec(&checkout.canonical_root, &["worktree", "prune"]);

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Leases and Fencing Tokens
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct LeaseId(pub String);

impl LeaseId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn generate() -> Self {
        Self(format!("lease-{}", uuid::Uuid::new_v4().simple()))
    }
}

impl std::fmt::Display for LeaseId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseResource {
    Checkout(CheckoutId),
    Task(TaskId),
    WorkflowRun(WorkflowRunId),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseKind {
    ExclusiveWrite,
    SharedRead,
    Verification,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseHolder {
    pub holder_id: String,
    pub pid: u32,
    pub host: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseState {
    Active,
    Expired,
    Superseded,
    Released,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lease {
    pub id: LeaseId,
    pub resource: LeaseResource,
    pub kind: LeaseKind,
    pub holder: LeaseHolder,
    pub fencing_token: u64,
    pub acquired_at: String,
    pub expires_at: String,
    pub state: LeaseState,
}

impl Lease {
    pub fn new(
        id: LeaseId,
        resource: LeaseResource,
        kind: LeaseKind,
        holder: LeaseHolder,
        fencing_token: u64,
        ttl_seconds: u64,
    ) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self {
            id,
            resource,
            kind,
            holder,
            fencing_token,
            acquired_at: now.to_string(),
            expires_at: (now + ttl_seconds).to_string(),
            state: LeaseState::Active,
        }
    }

    /// Verifies that a caller's fencing token is current, rejecting stale or superseded holders.
    pub fn assert_valid(&self, candidate_fencing_token: u64) -> Result<(), CandidateError> {
        if self.state != LeaseState::Active {
            return Err(CandidateError::FencingConflict {
                current: self.fencing_token,
                attempted: candidate_fencing_token,
            });
        }
        if candidate_fencing_token < self.fencing_token {
            return Err(CandidateError::FencingConflict {
                current: self.fencing_token,
                attempted: candidate_fencing_token,
            });
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Git Helpers
// ---------------------------------------------------------------------------

fn sanitize_git_command(command: &mut Command) {
    command.env_remove("GIT_DIR");
    command.env_remove("GIT_WORK_TREE");
    command.env_remove("GIT_COMMON_DIR");
    command.env_remove("GIT_INDEX_FILE");
    command.env_remove("GIT_OBJECT_DIRECTORY");
    command.env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES");
    command.env_remove("GIT_CONFIG_COUNT");
    command.env_remove("GIT_CONFIG_PARAMETERS");
}

fn git_exec(path: &Path, args: &[&str]) -> Result<String, CandidateError> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(path);
    cmd.args(args);
    sanitize_git_command(&mut cmd);

    let output = cmd.output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CandidateError::Git(format!(
            "git {:?} failed: {}",
            args,
            stderr.trim()
        )));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn git_rev_parse(path: &Path, rev: &str) -> Result<String, CandidateError> {
    git_exec(path, &["rev-parse", rev])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project_identity::CheckoutTrustState;

    fn create_test_repo(dir: &Path) {
        let run = |args: &[&str]| {
            let mut cmd = Command::new("git");
            cmd.arg("-C").arg(dir);
            cmd.args(args);
            sanitize_git_command(&mut cmd);
            let out = cmd.output().expect("git command failed");
            assert!(out.status.success(), "git {:?} failed", args);
        };

        run(&["init"]);
        run(&["config", "user.name", "Relay Test"]);
        run(&["config", "user.email", "relay@example.com"]);
        std::fs::write(dir.join("README.md"), "# Test Repo\n").unwrap();
        run(&["add", "README.md"]);
        run(&["commit", "-m", "initial commit"]);
    }

    fn unique_temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("relay-cand-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn candidate_fingerprint_captures_and_detects_mutation() {
        let repo_dir = unique_temp_dir("fp-mutation");
        create_test_repo(&repo_dir);

        let fp1 = CandidateFingerprint::capture(&repo_dir, None).unwrap();
        assert!(!fp1.current_head.is_empty());

        // Unmodified repo matches
        assert!(fp1.verify_against_repo(&repo_dir).unwrap());

        // Modify tracked file
        std::fs::write(repo_dir.join("README.md"), "# Modified\n").unwrap();
        let fp2 = CandidateFingerprint::capture(&repo_dir, None).unwrap();
        assert_ne!(fp1, fp2);
        assert!(!fp1.verify_against_repo(&repo_dir).unwrap());

        // Revert modification
        std::fs::write(repo_dir.join("README.md"), "# Test Repo\n").unwrap();
        assert!(fp1.verify_against_repo(&repo_dir).unwrap());

        // Add untracked file
        std::fs::write(repo_dir.join("untracked.txt"), "hello").unwrap();
        let fp3 = CandidateFingerprint::capture(&repo_dir, None).unwrap();
        assert_ne!(fp1, fp3);
        assert!(!fp1.verify_against_repo(&repo_dir).unwrap());
    }

    #[test]
    fn worktree_allowed_root_enforcement() {
        let base_dir = unique_temp_dir("worktree-enforcement");
        let canonical_root = base_dir.join("repo");
        let worktree_root = base_dir.join("allowed_worktrees");
        std::fs::create_dir_all(&canonical_root).unwrap();
        std::fs::create_dir_all(&worktree_root).unwrap();

        create_test_repo(&canonical_root);

        let checkout = Checkout::new(
            CheckoutId::generate(),
            ProjectId::parse("test-project").unwrap(),
            canonical_root.clone(),
            None,
            Some(worktree_root.clone()),
            CheckoutTrustState::Trusted,
        )
        .unwrap();

        let trust = TrustClassification::TrustedAgainstRegistration {
            project_id: checkout.project_id.clone(),
            checkout_id: checkout.id.to_string(),
        };

        // 1. Valid isolated worktree within allowed root
        let handle =
            WorktreeManager::create_isolated_worktree(&checkout, &trust, "wt-1", "HEAD").unwrap();
        assert!(handle.path.exists());
        assert!(handle.path.starts_with(&worktree_root));

        // 2. Remove isolated worktree
        WorktreeManager::remove_isolated_worktree(&checkout, &handle).unwrap();
        assert!(!handle.path.exists());

        // 3. Reject worktree path outside allowed root (traversal attempt)
        let traversal_res =
            WorktreeManager::create_isolated_worktree(&checkout, &trust, "../evil-escape", "HEAD");
        assert!(traversal_res.is_err());
    }

    #[test]
    fn lease_fencing_token_supersedes_stale_holders() {
        let lease = Lease::new(
            LeaseId::generate(),
            LeaseResource::Checkout(CheckoutId::generate()),
            LeaseKind::ExclusiveWrite,
            LeaseHolder {
                holder_id: "agent-1".to_owned(),
                pid: 100,
                host: "localhost".to_owned(),
            },
            5,
            300,
        );

        // Current fencing token is valid
        assert!(lease.assert_valid(5).is_ok());

        // Higher fencing token is valid
        assert!(lease.assert_valid(6).is_ok());

        // Stale fencing token (from earlier holder) is strictly rejected
        assert!(matches!(
            lease.assert_valid(4),
            Err(CandidateError::FencingConflict {
                current: 5,
                attempted: 4
            })
        ));
    }

    #[test]
    fn candidate_fingerprint_uses_sha256_digests() {
        let repo_dir = unique_temp_dir("fp-sha256");
        create_test_repo(&repo_dir);

        let fp = CandidateFingerprint::capture(&repo_dir, None).unwrap();
        // SHA-256 hex digests are exactly 64 characters
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
    }
}
