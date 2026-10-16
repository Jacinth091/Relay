//! Phase 13: Privileged Action Executor + Exact Approvals.
//!
//! Adds sensitive Git operations without delegating open-ended privilege to
//! provider roles. A `PrivilegedAction` is a closed, typed enumeration --
//! never an arbitrary command string -- and every approval is bound to the
//! exact canonicalized parameters of one specific action via a SHA-256
//! digest. Changing any parameter, replaying a grant, letting a grant
//! expire, or referencing a request Relay never tracked all fail closed.
//!
//! Automated Git push remains disabled: `PrivilegedAction` deliberately has
//! no push variant (invariant 14 / ADR-0020), consistent with the built-in
//! policy floor's `HardDeny` on `Capability::GitPush`.
//!
//! A provider can only ever *describe* an action in a `reason` string when
//! requesting approval; it cannot construct a valid `ApprovalGrant` itself,
//! and `PrivilegedActionExecutor::execute` only accepts grants bound to a
//! request Relay itself registered -- publishing prose never confers
//! authority.

use std::{
    collections::{HashMap, HashSet},
    path::Path,
    process::Command,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest as Sha2Digest, Sha256};
use thiserror::Error;

use crate::{CheckoutId, ProjectId, RoleAttemptId, TaskId, WorkflowRunId};

fn current_epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ---------------------------------------------------------------------------
// Privileged actions
// ---------------------------------------------------------------------------

/// Closed enumeration of sensitive Git operations Relay may execute on a
/// human's exact approval. There is deliberately no push variant -- v0.1
/// never automatically pushes code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum PrivilegedAction {
    GitBranchCreate {
        checkout_id: CheckoutId,
        branch_name: String,
        from_ref: String,
    },
    GitStash {
        checkout_id: CheckoutId,
        message: Option<String>,
    },
    GitCheckoutSwitch {
        checkout_id: CheckoutId,
        target_ref: String,
    },
    GitRemoteFetch {
        checkout_id: CheckoutId,
        remote_name: String,
    },
    GitTagCreate {
        checkout_id: CheckoutId,
        tag_name: String,
        target_ref: String,
    },
}

// ---------------------------------------------------------------------------
// Parameter digest
// ---------------------------------------------------------------------------

/// SHA-256 digest of an action's exact canonicalized parameters (including
/// which action kind it is). Any change to any field -- or to the action
/// kind itself -- produces a different digest, which is exactly what
/// invalidates an approval bound to the old one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParameterDigest(pub String);

impl ParameterDigest {
    pub fn compute(action: &PrivilegedAction) -> Result<Self, PrivilegedError> {
        // serde_json serializes struct/enum fields in their declared order,
        // so this is a deterministic canonical encoding across processes
        // built from the same `PrivilegedAction` definition.
        let canonical = serde_json::to_vec(action)?;
        let mut hasher = Sha256::new();
        hasher.update(&canonical);
        Ok(Self(to_hex(&hasher.finalize())))
    }
}

impl std::fmt::Display for ParameterDigest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// Approval request & grant
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ApprovalRequestId(pub String);

impl ApprovalRequestId {
    pub fn generate() -> Self {
        Self(format!("apr-{}", uuid::Uuid::new_v4().simple()))
    }
}

impl std::fmt::Display for ApprovalRequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A request for a human to approve exactly one privileged action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub id: ApprovalRequestId,
    pub project_id: ProjectId,
    pub task_id: TaskId,
    pub workflow_run_id: WorkflowRunId,
    pub action: PrivilegedAction,
    pub parameter_digest: ParameterDigest,
    pub reason: String,
    pub requested_by: Option<RoleAttemptId>,
    pub requested_at: u64,
    pub expires_at: u64,
}

impl ApprovalRequest {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        project_id: ProjectId,
        task_id: TaskId,
        workflow_run_id: WorkflowRunId,
        action: PrivilegedAction,
        reason: impl Into<String>,
        requested_by: Option<RoleAttemptId>,
        ttl_seconds: u64,
    ) -> Result<Self, PrivilegedError> {
        let parameter_digest = ParameterDigest::compute(&action)?;
        let now = current_epoch_seconds();
        Ok(Self {
            id: ApprovalRequestId::generate(),
            project_id,
            task_id,
            workflow_run_id,
            action,
            parameter_digest,
            reason: reason.into(),
            requested_by,
            requested_at: now,
            expires_at: now + ttl_seconds,
        })
    }
}

/// A human's exact, single-use authorization to execute the action bound to
/// one specific `ApprovalRequest`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalGrant {
    pub request_id: ApprovalRequestId,
    pub parameter_digest: ParameterDigest,
    pub granted_by: String,
    pub granted_at: u64,
    pub expires_at: u64,
    /// Opaque single-use token. Replay protection is keyed on this value,
    /// not on `request_id` alone, so re-delivery of the identical grant
    /// message is rejected exactly like a forged one.
    pub nonce: String,
}

impl ApprovalGrant {
    /// Issues a grant bound to `request`'s exact parameter digest. Only a
    /// human approval principal can call this -- there is no path from
    /// provider-authored text to a valid `ApprovalGrant`.
    pub fn issue(request: &ApprovalRequest, granted_by: impl Into<String>) -> Self {
        Self {
            request_id: request.id.clone(),
            parameter_digest: request.parameter_digest.clone(),
            granted_by: granted_by.into(),
            granted_at: current_epoch_seconds(),
            expires_at: request.expires_at,
            nonce: format!("nonce-{}", uuid::Uuid::new_v4().simple()),
        }
    }
}

// ---------------------------------------------------------------------------
// Errors & outcomes
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum PrivilegedError {
    #[error("failed to canonicalize action parameters: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("grant references a request Relay never registered")]
    UnknownRequest,
    #[error("grant's parameter digest does not match the requested action")]
    ParameterMismatch,
    #[error("grant has expired")]
    Expired,
    #[error("grant has already been consumed (replay rejected)")]
    AlreadyConsumed,
}

/// Deterministic outcome of one execution attempt. Ambiguity in the
/// underlying external effect is always surfaced as `Reconcile` or
/// `Escalate` -- `execute` never optimistically reports `Succeeded` when it
/// cannot confirm the operation actually applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrivilegedExecutionOutcome {
    Succeeded {
        detail: String,
    },
    /// The operation definitively did not apply (e.g. git exited non-zero
    /// with a clear reason); safe to report as a plain failure.
    Failed {
        reason: String,
    },
    /// Whether the effect applied is unknown but likely safe to re-check
    /// deterministically (e.g. re-run `git rev-parse` for the target ref).
    Reconcile {
        reason: String,
    },
    /// Whether the effect applied is unknown and cannot be safely
    /// reconciled automatically; requires human attention.
    Escalate {
        reason: String,
    },
}

// ---------------------------------------------------------------------------
// Operation runner seam
// ---------------------------------------------------------------------------

/// Result of attempting the actual external effect, before the executor
/// maps it onto `PrivilegedExecutionOutcome`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrivilegedRunOutcome {
    Succeeded { detail: String },
    Failed { reason: String },
    Ambiguous { reason: String, retryable: bool },
}

/// Executes the actual external effect for a `PrivilegedAction`. Kept
/// pluggable (mirroring `ProviderAdapter`/`RuntimeBackplane` elsewhere in
/// this codebase) so approval/replay/digest logic can be tested
/// deterministically against a fake runner, independent of a real Git
/// process.
pub trait PrivilegedOperationRunner: Send + Sync {
    fn run(&self, action: &PrivilegedAction, trusted_cwd: &Path) -> PrivilegedRunOutcome;
}

/// Real runner: explicit executable + argument arrays only, never a shell
/// string built from untrusted text.
pub struct GitOperationRunner;

impl GitOperationRunner {
    pub fn new() -> Self {
        Self
    }

    pub fn arc() -> Arc<Self> {
        Arc::new(Self::new())
    }
}

impl Default for GitOperationRunner {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
enum GitExecError {
    /// Git ran and definitively reported failure.
    CommandFailed(String),
    /// The process could not be spawned/observed cleanly; whether any
    /// partial effect landed is unknown.
    SpawnFailed(String),
}

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

fn git_exec(cwd: &Path, args: &[&str]) -> Result<String, GitExecError> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(cwd);
    cmd.args(args);
    sanitize_git_command(&mut cmd);

    let output = cmd
        .output()
        .map_err(|e| GitExecError::SpawnFailed(format!("failed to spawn git {args:?}: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(GitExecError::CommandFailed(format!(
            "git {args:?} failed: {}",
            stderr.trim()
        )));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

impl PrivilegedOperationRunner for GitOperationRunner {
    fn run(&self, action: &PrivilegedAction, trusted_cwd: &Path) -> PrivilegedRunOutcome {
        let result = match action {
            PrivilegedAction::GitBranchCreate {
                branch_name,
                from_ref,
                ..
            } => git_exec(trusted_cwd, &["branch", branch_name, from_ref]),
            PrivilegedAction::GitStash { message, .. } => match message {
                Some(m) => git_exec(trusted_cwd, &["stash", "push", "-m", m]),
                None => git_exec(trusted_cwd, &["stash", "push"]),
            },
            PrivilegedAction::GitCheckoutSwitch { target_ref, .. } => {
                git_exec(trusted_cwd, &["checkout", target_ref])
            }
            PrivilegedAction::GitRemoteFetch { remote_name, .. } => {
                git_exec(trusted_cwd, &["fetch", remote_name])
            }
            PrivilegedAction::GitTagCreate {
                tag_name,
                target_ref,
                ..
            } => git_exec(trusted_cwd, &["tag", tag_name, target_ref]),
        };

        match result {
            Ok(detail) => PrivilegedRunOutcome::Succeeded { detail },
            Err(GitExecError::CommandFailed(reason)) => PrivilegedRunOutcome::Failed { reason },
            Err(GitExecError::SpawnFailed(reason)) => PrivilegedRunOutcome::Ambiguous {
                reason,
                retryable: true,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Executor
// ---------------------------------------------------------------------------

/// Validates an `ApprovalGrant` against its exact requested action and
/// executes it at most once. Only requests Relay itself registered can ever
/// be executed, so a provider cannot acquire privileged authority merely by
/// publishing text that looks like an approval.
pub struct PrivilegedActionExecutor {
    runner: Arc<dyn PrivilegedOperationRunner>,
    requests: Mutex<HashMap<ApprovalRequestId, ApprovalRequest>>,
    consumed_nonces: Mutex<HashSet<String>>,
}

impl PrivilegedActionExecutor {
    pub fn new(runner: Arc<dyn PrivilegedOperationRunner>) -> Self {
        Self {
            runner,
            requests: Mutex::new(HashMap::new()),
            consumed_nonces: Mutex::new(HashSet::new()),
        }
    }

    /// Registers a request Relay itself created and surfaced for human
    /// approval. `execute` refuses any grant whose `request_id` was never
    /// registered this way.
    pub fn register_request(&self, request: ApprovalRequest) {
        self.requests
            .lock()
            .expect("requests lock poisoned")
            .insert(request.id.clone(), request);
    }

    /// Validates `grant` against `action` and, only if every check passes,
    /// executes it exactly once.
    pub fn execute(
        &self,
        grant: &ApprovalGrant,
        action: &PrivilegedAction,
        trusted_cwd: &Path,
    ) -> Result<PrivilegedExecutionOutcome, PrivilegedError> {
        let request = {
            let requests = self.requests.lock().expect("requests lock poisoned");
            requests
                .get(&grant.request_id)
                .cloned()
                .ok_or(PrivilegedError::UnknownRequest)?
        };

        // The grant must be bound to the request it claims, AND the action
        // about to run must be byte-for-byte the one that was approved.
        let action_digest = ParameterDigest::compute(action)?;
        if grant.parameter_digest != request.parameter_digest
            || action_digest != request.parameter_digest
        {
            return Err(PrivilegedError::ParameterMismatch);
        }

        if current_epoch_seconds() >= grant.expires_at {
            return Err(PrivilegedError::Expired);
        }

        {
            let mut consumed = self.consumed_nonces.lock().expect("nonce lock poisoned");
            if consumed.contains(&grant.nonce) {
                return Err(PrivilegedError::AlreadyConsumed);
            }
            // Marked consumed before running: a failed or ambiguous attempt
            // still burns the grant -- retrying requires a fresh approval,
            // never a second use of the same authorization.
            consumed.insert(grant.nonce.clone());
        }

        Ok(match self.runner.run(action, trusted_cwd) {
            PrivilegedRunOutcome::Succeeded { detail } => {
                PrivilegedExecutionOutcome::Succeeded { detail }
            }
            PrivilegedRunOutcome::Failed { reason } => {
                PrivilegedExecutionOutcome::Failed { reason }
            }
            PrivilegedRunOutcome::Ambiguous {
                reason,
                retryable: true,
            } => PrivilegedExecutionOutcome::Reconcile { reason },
            PrivilegedRunOutcome::Ambiguous {
                reason,
                retryable: false,
            } => PrivilegedExecutionOutcome::Escalate { reason },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_action(branch_name: &str) -> PrivilegedAction {
        PrivilegedAction::GitBranchCreate {
            checkout_id: CheckoutId::parse("chk-1").unwrap(),
            branch_name: branch_name.to_owned(),
            from_ref: "main".to_owned(),
        }
    }

    #[test]
    fn digest_changes_with_any_parameter() {
        let a = ParameterDigest::compute(&sample_action("feature-a")).unwrap();
        let b = ParameterDigest::compute(&sample_action("feature-b")).unwrap();
        let a_again = ParameterDigest::compute(&sample_action("feature-a")).unwrap();

        assert_ne!(a, b);
        assert_eq!(a, a_again);
    }
}
