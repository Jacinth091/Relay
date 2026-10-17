//! Phase 15: Integrated Recovery and Security Truthfulness.
//!
//! Enforces:
//! - Daemon crash recovery: pending effect reconciliation, lease fencing validation, capability revocation.
//! - Candidate integrity verification: any mutation to tracked diff, index, or untracked files invalidates candidate.
//! - Process-loss truthfulness: terminated or unreachable processes fail explicitly without optimistic success.
//! - Approval parameter change race protection: parameter change invalidates prior approval grant.
//! - Monotonic recovery reports for auditing.

use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    Candidate, CandidateFingerprint, RelayExchange, RoleAttemptId, SqliteStore,
    candidate::CandidateError,
};

#[derive(Debug, Error)]
pub enum RecoveryError {
    #[error("store error: {0}")]
    Store(#[from] relay_store::StoreError),
    #[error("candidate verification error: {0}")]
    Candidate(#[from] CandidateError),
    #[error("stale lease fencing token: expected > {expected}, got {actual}")]
    StaleFencingToken { expected: i64, actual: i64 },
    #[error("integrity violation: {0}")]
    IntegrityViolation(String),
}

/// Outcome of verifying a Candidate's working tree integrity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CandidateIntegrity {
    /// Repository state matches the candidate fingerprint identically.
    Valid,
    /// Repository state has diverged, invalidating the candidate.
    Mutated { reason: String },
}

/// Truthful status of an in-flight role attempt after a daemon or process disruption.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttemptReconciliation {
    StillActive,
    FailedProcessLost { reason: String },
    FailedMalformedResult { reason: String },
}

/// Auditable report produced upon completing recovery reconciliation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryReport {
    pub fencing_token: i64,
    pub pending_effects_reconciled: usize,
    pub orphaned_effects_failed: usize,
    pub tokens_revoked: usize,
    pub candidates_checked: usize,
    pub candidates_invalidated: usize,
}

/// Recovery and truthfulness coordinator.
pub struct RecoveryCoordinator;

impl RecoveryCoordinator {
    /// Verifies that the repository at `path` matches `candidate.fingerprint` precisely.
    /// Invariant: Any mutation to index, head commit, tracked diff, or untracked files
    /// causes immediate invalidation.
    pub fn verify_candidate_integrity(
        candidate: &Candidate,
        repo_path: &Path,
        base_ref: Option<&str>,
    ) -> Result<CandidateIntegrity, RecoveryError> {
        let current_fp = CandidateFingerprint::capture(repo_path, base_ref)?;

        if current_fp.base_head != candidate.fingerprint.base_head {
            return Ok(CandidateIntegrity::Mutated {
                reason: format!(
                    "base head moved: expected {}, found {}",
                    candidate.fingerprint.base_head, current_fp.base_head
                ),
            });
        }

        if current_fp.current_head != candidate.fingerprint.current_head {
            return Ok(CandidateIntegrity::Mutated {
                reason: format!(
                    "current head commit moved: expected {}, found {}",
                    candidate.fingerprint.current_head, current_fp.current_head
                ),
            });
        }

        if current_fp.index_digest != candidate.fingerprint.index_digest {
            return Ok(CandidateIntegrity::Mutated {
                reason: "index staged changes modified after candidate generation".to_string(),
            });
        }

        if current_fp.tracked_diff_digest != candidate.fingerprint.tracked_diff_digest {
            return Ok(CandidateIntegrity::Mutated {
                reason: "tracked working tree modified after candidate generation".to_string(),
            });
        }

        if current_fp.relevant_untracked_digest != candidate.fingerprint.relevant_untracked_digest {
            return Ok(CandidateIntegrity::Mutated {
                reason: "untracked files modified after candidate generation".to_string(),
            });
        }

        Ok(CandidateIntegrity::Valid)
    }

    /// Performs recovery reconciliation across the daemon store, exchange capabilities,
    /// and active leases.
    ///
    /// Not on the daemon's startup path: the daemon runs
    /// [`crate::daemon::DaemonServer::reconcile_on_startup`], whose writes are fenced on
    /// the daemon lease. This module is exercised by its phase tests only, and its effect
    /// writes here are unfenced; any future caller that runs inside a daemon process must
    /// fence them on that process's lease exactly like the effect worker does.
    pub async fn reconcile_startup(
        store: &SqliteStore,
        exchange: Option<&RelayExchange>,
        current_fencing_token: i64,
        last_known_fencing_token: i64,
    ) -> Result<RecoveryReport, RecoveryError> {
        // 1. Lease fencing token must be strictly monotonic
        if current_fencing_token <= last_known_fencing_token {
            return Err(RecoveryError::StaleFencingToken {
                expected: last_known_fencing_token,
                actual: current_fencing_token,
            });
        }

        // 2. Reconcile in-flight pending effects from database
        let pending = store.list_pending_effects(Some("pending")).await?;
        let pending_effects_reconciled = pending.len();
        let mut orphaned_effects_failed = 0;

        // Any pending effect that was not completed before daemon restart
        // must be truthfully recorded as failed, never optimistically marked complete.
        for effect in &pending {
            store
                .record_effect_result(
                    &format!("recovery-res-{}", effect.id),
                    &effect.id,
                    "failure",
                    "{\"error\":\"orphaned in-flight effect during daemon crash\"}",
                )
                .await?;
            orphaned_effects_failed += 1;
        }

        // 3. Revoke all stale capability tokens to prevent zombie executions
        let tokens_revoked = if let Some(ex) = exchange {
            ex.revoke_all()
        } else {
            0
        };

        Ok(RecoveryReport {
            fencing_token: current_fencing_token,
            pending_effects_reconciled,
            orphaned_effects_failed,
            tokens_revoked,
            candidates_checked: 0,
            candidates_invalidated: 0,
        })
    }

    /// Reconciles an attempt that failed to produce a result or whose process was lost.
    /// INVARIANT: Truthful failure reporting, never faking success.
    pub fn reconcile_lost_process(
        attempt_id: &RoleAttemptId,
        reason: &str,
    ) -> AttemptReconciliation {
        AttemptReconciliation::FailedProcessLost {
            reason: format!(
                "attempt '{}' process terminated unexpectedly: {}",
                attempt_id, reason
            ),
        }
    }
}
