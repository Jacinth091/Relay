//! Phase 04: Immutable Artifact Store.
//!
//! Provides:
//! - Content-addressed and metadata-tracked immutable artifacts.
//! - Strict provenance (`project_id`, `task_id`, `workflow_run_id`, producer role/attempt, candidate ref).
//! - Trust classification (`ProviderClaim`, `RelayObserved`, `Derived`).
//! - Atomic filesystem writes and checksum verification.

use std::{
    fs::File,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    ProjectId, RoleAttemptId, RoleInstanceId, TaskId, WorkflowRunId,
    candidate::{CandidateId, compute_digest},
};

#[derive(Debug, Error)]
pub enum ArtifactError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("checksum mismatch: expected {expected}, computed {computed}")]
    ChecksumMismatch { expected: String, computed: String },
    #[error("artifact not found: {0}")]
    NotFound(String),
    #[error("artifact directory path invalid: {0}")]
    InvalidStoragePath(PathBuf),
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

fn now_timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_owned())
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ArtifactId(pub String);

impl ArtifactId {
    pub fn generate(kind: &ArtifactKind) -> Self {
        Self(format!(
            "art-{}-{}",
            kind.as_str(),
            uuid::Uuid::new_v4().simple()
        ))
    }
}

impl std::fmt::Display for ArtifactId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Specification,
    ImplementationResult,
    CandidateReference,
    VerificationResult,
    RawLog,
    ReviewResult,
}

impl ArtifactKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Specification => "spec",
            Self::ImplementationResult => "impl-res",
            Self::CandidateReference => "cand-ref",
            Self::VerificationResult => "verif-res",
            Self::RawLog => "raw-log",
            Self::ReviewResult => "review-res",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactTrustClass {
    /// Output claimed by a provider without independent Relay verification.
    ProviderClaim,
    /// Relay engine or daemon direct observation (e.g. exit code, fingerprint, git diff).
    RelayObserved,
    /// Deterministically computed or transformed from trusted artifacts.
    Derived,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactSensitivity {
    Public,
    Internal,
    Confidential,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionClass {
    Ephemeral,
    WorkflowScoped,
    Permanent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactStorageRef {
    pub relative_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct CreateArtifactRequest<'a> {
    pub kind: ArtifactKind,
    pub project_id: ProjectId,
    pub task_id: TaskId,
    pub workflow_run_id: WorkflowRunId,
    pub producer_role_instance_id: Option<RoleInstanceId>,
    pub producer_attempt_id: Option<RoleAttemptId>,
    pub candidate_id: Option<CandidateId>,
    pub trust_class: ArtifactTrustClass,
    pub sensitivity: ArtifactSensitivity,
    pub retention: RetentionClass,
    pub payload: &'a [u8],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactMetadata {
    pub id: ArtifactId,
    pub kind: ArtifactKind,
    pub schema_version: u32,
    pub project_id: ProjectId,
    pub task_id: TaskId,
    pub workflow_run_id: WorkflowRunId,
    pub producer_role_instance_id: Option<RoleInstanceId>,
    pub producer_attempt_id: Option<RoleAttemptId>,
    pub candidate_id: Option<CandidateId>,
    pub trust_class: ArtifactTrustClass,
    pub checksum: String,
    pub size_bytes: u64,
    pub storage_ref: ArtifactStorageRef,
    pub created_at: String,
    pub sensitivity: ArtifactSensitivity,
    pub retention: RetentionClass,
}

pub struct ArtifactStore {
    root: PathBuf,
}

impl ArtifactStore {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, ArtifactError> {
        let root = root.into();
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Atomically stores payload bytes and records immutable metadata.
    pub fn put(
        &self,
        request: CreateArtifactRequest<'_>,
    ) -> Result<ArtifactMetadata, ArtifactError> {
        let id = ArtifactId::generate(&request.kind);
        let checksum = compute_digest(request.payload);
        let size_bytes = request.payload.len() as u64;

        // Content-addressed subpath: <kind>/<id>.blob
        let rel_blob_path = PathBuf::from(request.kind.as_str()).join(format!("{}.blob", id.0));
        let full_blob_path = self.root.join(&rel_blob_path);
        let rel_meta_path =
            PathBuf::from(request.kind.as_str()).join(format!("{}.meta.json", id.0));
        let full_meta_path = self.root.join(&rel_meta_path);

        if let Some(parent) = full_blob_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Atomic write via temp file
        let temp_blob = full_blob_path.with_extension("tmp");
        {
            let mut file = File::create(&temp_blob)?;
            file.write_all(request.payload)?;
            file.flush()?;
        }
        std::fs::rename(temp_blob, &full_blob_path)?;

        let metadata = ArtifactMetadata {
            id,
            kind: request.kind,
            schema_version: 1,
            project_id: request.project_id,
            task_id: request.task_id,
            workflow_run_id: request.workflow_run_id,
            producer_role_instance_id: request.producer_role_instance_id,
            producer_attempt_id: request.producer_attempt_id,
            candidate_id: request.candidate_id,
            trust_class: request.trust_class,
            checksum,
            size_bytes,
            storage_ref: ArtifactStorageRef {
                relative_path: rel_blob_path,
            },
            created_at: now_timestamp(),
            sensitivity: request.sensitivity,
            retention: request.retention,
        };

        let meta_json = serde_json::to_vec_pretty(&metadata)?;
        let temp_meta = full_meta_path.with_extension("tmp");
        {
            let mut file = File::create(&temp_meta)?;
            file.write_all(&meta_json)?;
            file.flush()?;
        }
        std::fs::rename(temp_meta, &full_meta_path)?;

        Ok(metadata)
    }

    /// Reads and verifies payload bytes against recorded checksum.
    pub fn get(&self, metadata: &ArtifactMetadata) -> Result<Vec<u8>, ArtifactError> {
        let full_path = self.root.join(&metadata.storage_ref.relative_path);
        if !full_path.exists() {
            return Err(ArtifactError::NotFound(metadata.id.0.clone()));
        }

        let mut file = File::open(&full_path)?;
        let mut buf = Vec::with_capacity(metadata.size_bytes as usize);
        file.read_to_end(&mut buf)?;

        let computed = compute_digest(&buf);
        if computed != metadata.checksum {
            return Err(ArtifactError::ChecksumMismatch {
                expected: metadata.checksum.clone(),
                computed,
            });
        }

        Ok(buf)
    }

    /// Loads metadata for an artifact by id and kind.
    pub fn get_metadata(
        &self,
        kind: ArtifactKind,
        id: &ArtifactId,
    ) -> Result<ArtifactMetadata, ArtifactError> {
        let meta_path = self
            .root
            .join(kind.as_str())
            .join(format!("{}.meta.json", id.0));
        if !meta_path.exists() {
            return Err(ArtifactError::NotFound(id.0.clone()));
        }

        let mut file = File::open(&meta_path)?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;

        let metadata: ArtifactMetadata = serde_json::from_slice(&buf)?;
        Ok(metadata)
    }

    /// Finds and loads metadata for an artifact by id across all kinds.
    pub fn get_any_metadata(&self, id: &ArtifactId) -> Result<ArtifactMetadata, ArtifactError> {
        let kinds = [
            ArtifactKind::Specification,
            ArtifactKind::ImplementationResult,
            ArtifactKind::CandidateReference,
            ArtifactKind::VerificationResult,
            ArtifactKind::RawLog,
            ArtifactKind::ReviewResult,
        ];
        for k in kinds {
            if let Ok(meta) = self.get_metadata(k, id) {
                return Ok(meta);
            }
        }
        Err(ArtifactError::NotFound(id.0.clone()))
    }

    /// Reads payload for an artifact by id across any kind.
    pub fn read_payload(&self, id: &ArtifactId) -> Result<Vec<u8>, ArtifactError> {
        let meta = self.get_any_metadata(id)?;
        self.get(&meta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("relay-art-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn artifact_store_atomic_put_get_and_tamper_detection() {
        let temp_path = unique_temp_dir("atomic-tamper");
        let store = ArtifactStore::new(&temp_path).unwrap();

        let payload = b"Verification report: all checks passed.";
        let meta = store
            .put(CreateArtifactRequest {
                kind: ArtifactKind::VerificationResult,
                project_id: ProjectId::parse("relay-proj").unwrap(),
                task_id: TaskId::generate(),
                workflow_run_id: WorkflowRunId::generate(),
                producer_role_instance_id: None,
                producer_attempt_id: None,
                candidate_id: None,
                trust_class: ArtifactTrustClass::RelayObserved,
                sensitivity: ArtifactSensitivity::Internal,
                retention: RetentionClass::WorkflowScoped,
                payload,
            })
            .unwrap();

        assert_eq!(meta.size_bytes, payload.len() as u64);
        assert_eq!(meta.kind, ArtifactKind::VerificationResult);

        // Retrieve and verify checksum
        let retrieved = store.get(&meta).unwrap();
        assert_eq!(retrieved, payload);

        // Retrieve metadata by ID
        let loaded_meta = store.get_metadata(meta.kind, &meta.id).unwrap();
        assert_eq!(loaded_meta, meta);

        // Tamper with payload file
        let blob_path = store.root().join(&meta.storage_ref.relative_path);
        std::fs::write(&blob_path, b"Tampered data!").unwrap();

        // Checksum mismatch triggers error
        let err = store.get(&meta).unwrap_err();
        assert!(matches!(err, ArtifactError::ChecksumMismatch { .. }));
    }
}
