//! Phase 15: Retention Management and Auditable Resource Cleanup.
//!
//! Provides:
//! - Configurable retention policies for attempt worktrees, ephemeral session projections, and artifacts.
//! - Safe pruning of stale or completed worktrees without touching active executions.
//! - Pruning of expired ephemeral session projections on disk (never deleting unexpired or active sessions).
//! - Artifact garbage collection that respects `RetentionClass` (protecting Permanent and WorkflowScoped artifacts).
//! - Auditable structured cleanup reporting.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    ArtifactError, ArtifactKind, ArtifactMetadata, ArtifactStore, RelayExchange, RetentionClass,
    WorktreeHandle,
};

#[derive(Debug, Error)]
pub enum RetentionError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("artifact store error: {0}")]
    Artifact(#[from] ArtifactError),
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

fn current_epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Retention policy specifying lifetimes for ephemeral resources.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionPolicy {
    /// Maximum age in seconds for inactive/orphaned worktree directories.
    pub attempt_worktree_ttl_seconds: u64,
    /// Maximum age in seconds for projected session directories.
    pub ephemeral_projection_ttl_seconds: u64,
    /// Maximum age in seconds for Ephemeral-class artifacts.
    pub artifact_ephemeral_ttl_seconds: u64,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            attempt_worktree_ttl_seconds: 3600,
            ephemeral_projection_ttl_seconds: 600,
            artifact_ephemeral_ttl_seconds: 86400,
        }
    }
}

/// Structured, auditable report produced after executing a retention cycle.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupReport {
    pub worktrees_pruned: usize,
    pub projections_cleaned: usize,
    pub artifacts_collected: usize,
    pub bytes_freed: u64,
    pub tokens_revoked: usize,
}

/// Retention and garbage collection coordinator.
pub struct RetentionManager {
    policy: RetentionPolicy,
}

impl RetentionManager {
    pub fn new(policy: RetentionPolicy) -> Self {
        Self { policy }
    }

    pub fn policy(&self) -> &RetentionPolicy {
        &self.policy
    }

    /// Prunes worktrees from `worktree_root` that are NOT present in `active_worktrees`.
    pub fn prune_isolated_worktrees(
        &self,
        worktree_root: &Path,
        active_worktrees: &[&WorktreeHandle],
    ) -> Result<usize, RetentionError> {
        if !worktree_root.exists() {
            return Ok(0);
        }

        let active_paths: Vec<PathBuf> = active_worktrees.iter().map(|h| h.path.clone()).collect();

        let mut pruned = 0;
        for entry in fs::read_dir(worktree_root)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() && !active_paths.iter().any(|active| active == &path) {
                fs::remove_dir_all(&path)?;
                pruned += 1;
            }
        }

        Ok(pruned)
    }

    /// Scans the projection root directory and removes expired materialized sessions.
    /// Uses `SESSION.json` metadata `expires_at` timestamp.
    pub fn clean_expired_projections(
        &self,
        projection_root: &Path,
    ) -> Result<usize, RetentionError> {
        if !projection_root.exists() {
            return Ok(0);
        }

        let now = current_epoch_seconds();
        let mut cleaned = 0;

        for entry in fs::read_dir(projection_root)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                let session_json_path = path.join("SESSION.json");
                if session_json_path.exists() {
                    let should_clean = match fs::read_to_string(&session_json_path) {
                        Ok(content) => {
                            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                                val.get("expires_at")
                                    .and_then(|e| e.as_u64())
                                    .map(|exp| now >= exp)
                                    .unwrap_or(false)
                            } else {
                                false
                            }
                        }
                        Err(_) => false,
                    };

                    if should_clean {
                        fs::remove_dir_all(&path)?;
                        cleaned += 1;
                    }
                }
            }
        }

        Ok(cleaned)
    }

    /// Collects expired `Ephemeral` artifacts from `ArtifactStore`.
    /// INVARIANT: Never removes `WorkflowScoped` or `Permanent` artifacts.
    pub fn collect_expired_artifacts(
        &self,
        artifact_store: &ArtifactStore,
    ) -> Result<(usize, u64), RetentionError> {
        let store_root = artifact_store.root();
        if !store_root.exists() {
            return Ok((0, 0));
        }

        let now = current_epoch_seconds();
        let ttl = self.policy.artifact_ephemeral_ttl_seconds;
        let mut collected = 0;
        let mut bytes_freed = 0;

        let kinds = [
            ArtifactKind::Specification,
            ArtifactKind::ImplementationResult,
            ArtifactKind::CandidateReference,
            ArtifactKind::VerificationResult,
            ArtifactKind::RawLog,
            ArtifactKind::ReviewResult,
        ];

        for kind in &kinds {
            let kind_dir = store_root.join(kind.as_str());
            if !kind_dir.exists() {
                continue;
            }

            for entry in fs::read_dir(&kind_dir)? {
                let entry = entry?;
                let meta_path = entry.path();
                if meta_path.is_file() && meta_path.to_string_lossy().ends_with(".meta.json") {
                    let meta_content = match fs::read_to_string(&meta_path) {
                        Ok(c) => c,
                        Err(_) => continue,
                    };

                    let meta: ArtifactMetadata = match serde_json::from_str(&meta_content) {
                        Ok(m) => m,
                        Err(_) => continue,
                    };

                    // Only collect Ephemeral artifacts
                    if meta.retention != RetentionClass::Ephemeral {
                        continue;
                    }

                    let created_secs: u64 = meta.created_at.parse().unwrap_or(0);
                    if now.saturating_sub(created_secs) >= ttl {
                        let blob_path = store_root.join(&meta.storage_ref.relative_path);
                        if blob_path.exists() {
                            if let Ok(meta_file) = fs::metadata(&blob_path) {
                                bytes_freed += meta_file.len();
                            }
                            let _ = fs::remove_file(&blob_path);
                        }
                        let _ = fs::remove_file(&meta_path);
                        collected += 1;
                    }
                }
            }
        }

        Ok((collected, bytes_freed))
    }

    /// Revokes expired tokens in the exchange.
    pub fn revoke_expired_tokens(&self, exchange: &RelayExchange) -> usize {
        exchange.revoke_expired_capabilities()
    }

    /// Executes a complete retention cycle across all resources.
    pub fn run_retention_cycle(
        &self,
        worktree_root: Option<&Path>,
        active_worktrees: &[&WorktreeHandle],
        projection_root: Option<&Path>,
        artifact_store: Option<&ArtifactStore>,
        exchange: Option<&RelayExchange>,
    ) -> Result<CleanupReport, RetentionError> {
        let worktrees_pruned = if let Some(root) = worktree_root {
            self.prune_isolated_worktrees(root, active_worktrees)?
        } else {
            0
        };

        let projections_cleaned = if let Some(root) = projection_root {
            self.clean_expired_projections(root)?
        } else {
            0
        };

        let (artifacts_collected, bytes_freed) = if let Some(store) = artifact_store {
            self.collect_expired_artifacts(store)?
        } else {
            (0, 0)
        };

        let tokens_revoked = if let Some(ex) = exchange {
            self.revoke_expired_tokens(ex)
        } else {
            0
        };

        Ok(CleanupReport {
            worktrees_pruned,
            projections_cleaned,
            artifacts_collected,
            bytes_freed,
            tokens_revoked,
        })
    }
}
