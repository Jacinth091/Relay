//! Phase 09: Relay Exchange and Role-Session Capabilities.
//!
//! Provides:
//! - Typed, durable, role-scoped communication layer (`ExchangeMessage`, `RoleInbox`).
//! - Ephemeral attempt-scoped capabilities (`RoleSessionCapability`, `RoleSessionToken`).
//! - Cross-task and cross-project isolation enforcement.
//! - Structured result publication with schema and provenance validation.
//! - Materialized session projection fallback for non-tool-calling CLIs.

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    ArtifactError, ArtifactId, ArtifactKind, ArtifactMetadata, ArtifactSensitivity, ArtifactStore,
    ArtifactTrustClass, CheckoutId, CreateArtifactRequest, ProjectId, RetentionClass,
    RoleAttemptId, RoleInstanceId, TaskId, WorkflowRunId, candidate::CandidateId,
};

#[derive(Debug, Error)]
pub enum ExchangeError {
    #[error("unauthorized capability: {0}")]
    Unauthorized(String),
    #[error("capability expired at {0}")]
    CapabilityExpired(u64),
    #[error("capability revoked")]
    CapabilityRevoked,
    #[error("operation not permitted by capability: {0:?}")]
    OperationDenied(ExchangeOperation),
    #[error("cross-task access denied: capability task {cap_task}, requested task {req_task}")]
    CrossTaskAccessDenied { cap_task: String, req_task: String },
    #[error(
        "cross-project access denied: capability project {cap_proj}, requested project {req_proj}"
    )]
    CrossProjectAccessDenied { cap_proj: String, req_proj: String },
    #[error("scope expansion attempted via caller-supplied parameters")]
    ScopeWideningRejected,
    #[error("result validation failed: {0}")]
    ResultInvalid(String),
    #[error("artifact store error: {0}")]
    Artifact(#[from] ArtifactError),
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

fn current_epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn now_iso_timestamp() -> String {
    current_epoch_seconds().to_string()
}

/// Operations that a role attempt can execute via the Exchange interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExchangeOperation {
    ReadInbox,
    SendMessage,
    PublishResult,
    GetArtifact,
    DelegateRole,
    ConsultRole,
}

/// Opaque high-entropy capability token passed by an executing attempt.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RoleSessionToken(pub String);

impl RoleSessionToken {
    pub fn generate() -> Self {
        Self(format!("rst-{}", uuid::Uuid::new_v4().simple()))
    }
}

impl std::fmt::Display for RoleSessionToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Attempt-scoped capability authorizing role operations within strict task/project bounds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleSessionCapability {
    pub token: RoleSessionToken,
    pub project_id: ProjectId,
    pub checkout_id: CheckoutId,
    pub task_id: TaskId,
    pub workflow_run_id: WorkflowRunId,
    pub role_instance_id: RoleInstanceId,
    pub role_attempt_id: RoleAttemptId,
    pub allowed_operations: HashSet<ExchangeOperation>,
    pub issued_at: u64,
    pub expires_at: u64,
    pub is_revoked: bool,
}

impl RoleSessionCapability {
    pub fn is_active(&self, now: u64) -> Result<(), ExchangeError> {
        if self.is_revoked {
            return Err(ExchangeError::CapabilityRevoked);
        }
        if now >= self.expires_at {
            return Err(ExchangeError::CapabilityExpired(self.expires_at));
        }
        Ok(())
    }

    pub fn check_permission(&self, op: ExchangeOperation, now: u64) -> Result<(), ExchangeError> {
        self.is_active(now)?;
        if !self.allowed_operations.contains(&op) {
            return Err(ExchangeError::OperationDenied(op));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ExchangeMessageId(pub String);

impl ExchangeMessageId {
    pub fn generate() -> Self {
        Self(format!("msg-{}", uuid::Uuid::new_v4().simple()))
    }
}

impl std::fmt::Display for ExchangeMessageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExchangeMessageKind {
    Question,
    Answer,
    Progress,
    Clarification,
    CorrectionNotice,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExchangeMessage {
    pub id: ExchangeMessageId,
    pub project_id: ProjectId,
    pub task_id: TaskId,
    pub workflow_run_id: WorkflowRunId,
    pub from_role_attempt_id: RoleAttemptId,
    pub to_role_attempt_id: Option<RoleAttemptId>,
    pub kind: ExchangeMessageKind,
    pub payload: serde_json::Value,
    pub attached_artifacts: Vec<ArtifactId>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleInbox {
    pub role_attempt_id: RoleAttemptId,
    pub task_id: TaskId,
    pub project_id: ProjectId,
    pub messages: Vec<ExchangeMessage>,
    pub visible_artifacts: Vec<ArtifactMetadata>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendMessageRequest {
    pub to_role_attempt_id: Option<RoleAttemptId>,
    pub kind: ExchangeMessageKind,
    pub payload: serde_json::Value,
    pub attached_artifacts: Vec<ArtifactId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishResultRequest {
    pub kind: ArtifactKind,
    pub candidate_id: Option<CandidateId>,
    pub trust_class: ArtifactTrustClass,
    pub sensitivity: ArtifactSensitivity,
    pub retention: RetentionClass,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueCapabilityRequest {
    pub project_id: ProjectId,
    pub checkout_id: CheckoutId,
    pub task_id: TaskId,
    pub workflow_run_id: WorkflowRunId,
    pub role_instance_id: RoleInstanceId,
    pub role_attempt_id: RoleAttemptId,
    pub allowed_operations: HashSet<ExchangeOperation>,
    pub ttl_seconds: u64,
}

/// The Relay Exchange orchestrates role communication and scoped capability enforcement.
pub struct RelayExchange {
    artifact_store: Arc<ArtifactStore>,
    capabilities: Arc<Mutex<HashMap<RoleSessionToken, RoleSessionCapability>>>,
    messages: Arc<Mutex<Vec<ExchangeMessage>>>,
    visible_artifact_mounts: Arc<Mutex<HashMap<RoleAttemptId, HashSet<ArtifactId>>>>,
}

impl RelayExchange {
    pub fn new(artifact_store: Arc<ArtifactStore>) -> Self {
        Self {
            artifact_store,
            capabilities: Arc::new(Mutex::new(HashMap::new())),
            messages: Arc::new(Mutex::new(Vec::new())),
            visible_artifact_mounts: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Issues a high-entropy, short-lived, attempt-scoped capability token.
    pub fn issue_capability(&self, req: IssueCapabilityRequest) -> RoleSessionCapability {
        let token = RoleSessionToken::generate();
        let now = current_epoch_seconds();
        let capability = RoleSessionCapability {
            token: token.clone(),
            project_id: req.project_id,
            checkout_id: req.checkout_id,
            task_id: req.task_id,
            workflow_run_id: req.workflow_run_id,
            role_instance_id: req.role_instance_id,
            role_attempt_id: req.role_attempt_id,
            allowed_operations: req.allowed_operations,
            issued_at: now,
            expires_at: now + req.ttl_seconds,
            is_revoked: false,
        };

        self.capabilities
            .lock()
            .unwrap()
            .insert(token, capability.clone());
        capability
    }

    /// Revokes an active capability token immediately (e.g. at attempt end or cancellation).
    pub fn revoke_capability(&self, token: &RoleSessionToken) -> bool {
        let mut caps = self.capabilities.lock().unwrap();
        if let Some(cap) = caps.get_mut(token) {
            cap.is_revoked = true;
            true
        } else {
            false
        }
    }

    /// Revokes all capabilities issued for a specific role attempt (e.g. upon attempt termination/cancellation).
    pub fn revoke_all_for_attempt(&self, role_attempt_id: &RoleAttemptId) -> usize {
        let mut caps = self.capabilities.lock().unwrap();
        let mut count = 0;
        for cap in caps.values_mut() {
            if &cap.role_attempt_id == role_attempt_id && !cap.is_revoked {
                cap.is_revoked = true;
                count += 1;
            }
        }
        count
    }

    /// Revokes all capabilities whose TTL has expired.
    pub fn revoke_expired_capabilities(&self) -> usize {
        let mut caps = self.capabilities.lock().unwrap();
        let now = current_epoch_seconds();
        let mut count = 0;
        for cap in caps.values_mut() {
            if now >= cap.expires_at && !cap.is_revoked {
                cap.is_revoked = true;
                count += 1;
            }
        }
        count
    }

    /// Revokes all capabilities across all attempts (e.g. upon daemon crash recovery or shutdown).
    pub fn revoke_all(&self) -> usize {
        let mut caps = self.capabilities.lock().unwrap();
        let mut count = 0;
        for cap in caps.values_mut() {
            if !cap.is_revoked {
                cap.is_revoked = true;
                count += 1;
            }
        }
        count
    }

    /// Mounts an artifact as visible input for a specific role attempt.
    pub fn mount_artifact_for_attempt(
        &self,
        role_attempt_id: &RoleAttemptId,
        artifact_id: ArtifactId,
    ) {
        self.visible_artifact_mounts
            .lock()
            .unwrap()
            .entry(role_attempt_id.clone())
            .or_default()
            .insert(artifact_id);
    }

    /// Validates capability token and verifies requested operation permission.
    pub fn validate_capability(
        &self,
        token: &RoleSessionToken,
        op: ExchangeOperation,
    ) -> Result<RoleSessionCapability, ExchangeError> {
        let caps = self.capabilities.lock().unwrap();
        let cap = caps
            .get(token)
            .ok_or_else(|| ExchangeError::Unauthorized("unknown capability token".to_string()))?;

        cap.check_permission(op, current_epoch_seconds())?;
        Ok(cap.clone())
    }

    /// Reads the role-scoped inbox. Strictly isolated to the capability's task and project.
    pub fn get_inbox(&self, token: &RoleSessionToken) -> Result<RoleInbox, ExchangeError> {
        let cap = self.validate_capability(token, ExchangeOperation::ReadInbox)?;

        // Fetch messages addressed to this attempt, or broadcast in this task
        let messages = self.messages.lock().unwrap();
        let filtered_messages: Vec<ExchangeMessage> = messages
            .iter()
            .filter(|m| {
                m.task_id == cap.task_id
                    && m.project_id == cap.project_id
                    && (m.to_role_attempt_id.as_ref() == Some(&cap.role_attempt_id)
                        || m.to_role_attempt_id.is_none())
            })
            .cloned()
            .collect();
        drop(messages);

        // Fetch mounted visible artifacts
        let mounts = self.visible_artifact_mounts.lock().unwrap();
        let visible_ids = mounts
            .get(&cap.role_attempt_id)
            .cloned()
            .unwrap_or_default();
        drop(mounts);

        let mut visible_artifacts = Vec::new();
        for id in visible_ids {
            if let Ok(meta) = self.artifact_store.get_any_metadata(&id) {
                // Confirm cross-task isolation check
                if meta.task_id == cap.task_id && meta.project_id == cap.project_id {
                    visible_artifacts.push(meta);
                }
            }
        }

        Ok(RoleInbox {
            role_attempt_id: cap.role_attempt_id,
            task_id: cap.task_id,
            project_id: cap.project_id,
            messages: filtered_messages,
            visible_artifacts,
        })
    }

    /// Sends a typed exchange message. Must not cross task or project boundaries.
    pub fn send_message(
        &self,
        token: &RoleSessionToken,
        req: SendMessageRequest,
    ) -> Result<ExchangeMessageId, ExchangeError> {
        let cap = self.validate_capability(token, ExchangeOperation::SendMessage)?;

        let msg_id = ExchangeMessageId::generate();
        let message = ExchangeMessage {
            id: msg_id.clone(),
            project_id: cap.project_id,
            task_id: cap.task_id,
            workflow_run_id: cap.workflow_run_id,
            from_role_attempt_id: cap.role_attempt_id,
            to_role_attempt_id: req.to_role_attempt_id,
            kind: req.kind,
            payload: req.payload,
            attached_artifacts: req.attached_artifacts,
            created_at: now_iso_timestamp(),
        };

        self.messages.lock().unwrap().push(message);
        Ok(msg_id)
    }

    /// Publishes a structured result artifact.
    /// Rejects malformed results, validates candidate references, and stores atomically.
    pub fn publish_result(
        &self,
        token: &RoleSessionToken,
        req: PublishResultRequest,
    ) -> Result<ArtifactId, ExchangeError> {
        let cap = self.validate_capability(token, ExchangeOperation::PublishResult)?;

        // 1. Semantic and schema validation
        if req.payload.is_empty() {
            return Err(ExchangeError::ResultInvalid(
                "payload cannot be empty".to_string(),
            ));
        }

        // Must be parseable JSON
        let parsed_json: serde_json::Value = serde_json::from_slice(&req.payload)
            .map_err(|e| ExchangeError::ResultInvalid(format!("malformed JSON result: {}", e)))?;

        if !parsed_json.is_object() {
            return Err(ExchangeError::ResultInvalid(
                "result payload must be a JSON object".to_string(),
            ));
        }

        // Verification results must reference a candidate
        if req.kind == ArtifactKind::VerificationResult && req.candidate_id.is_none() {
            return Err(ExchangeError::ResultInvalid(
                "verification result must bind to an explicit candidate_id".to_string(),
            ));
        }

        // 2. Persist into content-addressed immutable artifact store
        let metadata = self.artifact_store.put(CreateArtifactRequest {
            kind: req.kind,
            project_id: cap.project_id,
            task_id: cap.task_id,
            workflow_run_id: cap.workflow_run_id,
            producer_role_instance_id: Some(cap.role_instance_id),
            producer_attempt_id: Some(cap.role_attempt_id),
            candidate_id: req.candidate_id,
            trust_class: req.trust_class,
            sensitivity: req.sensitivity,
            retention: req.retention,
            payload: &req.payload,
        })?;

        Ok(metadata.id)
    }

    /// Retrieves an artifact by ID, strictly enforcing cross-task and cross-project isolation.
    pub fn get_artifact(
        &self,
        token: &RoleSessionToken,
        artifact_id: &ArtifactId,
    ) -> Result<(ArtifactMetadata, Vec<u8>), ExchangeError> {
        let cap = self.validate_capability(token, ExchangeOperation::GetArtifact)?;

        let meta = self.artifact_store.get_any_metadata(artifact_id)?;

        // Enforce cross-project isolation
        if meta.project_id != cap.project_id {
            return Err(ExchangeError::CrossProjectAccessDenied {
                cap_proj: cap.project_id.to_string(),
                req_proj: meta.project_id.to_string(),
            });
        }

        // Enforce cross-task isolation
        if meta.task_id != cap.task_id {
            return Err(ExchangeError::CrossTaskAccessDenied {
                cap_task: cap.task_id.to_string(),
                req_task: meta.task_id.to_string(),
            });
        }

        // Read payload
        let payload = self.artifact_store.read_payload(artifact_id)?;
        Ok((meta, payload))
    }

    /// Projects an ephemeral materialized session on disk for CLIs that cannot call tools.
    /// INVARIANT: Does NOT leak the secret capability token to disk.
    pub fn project_materialized_session(
        &self,
        token: &RoleSessionToken,
        base_dir: &Path,
    ) -> Result<PathBuf, ExchangeError> {
        let cap = self.validate_capability(token, ExchangeOperation::ReadInbox)?;

        let session_dir = base_dir.join(cap.role_attempt_id.to_string());
        let input_dir = session_dir.join("input");
        let outbox_dir = session_dir.join("outbox");

        fs::create_dir_all(&input_dir)?;
        fs::create_dir_all(&outbox_dir)?;

        // 1. SESSION.json (identity, permissions, NO secret tokens)
        let session_info = serde_json::json!({
            "project_id": cap.project_id.to_string(),
            "checkout_id": cap.checkout_id.to_string(),
            "task_id": cap.task_id.to_string(),
            "workflow_run_id": cap.workflow_run_id.to_string(),
            "role_instance_id": cap.role_instance_id.to_string(),
            "role_attempt_id": cap.role_attempt_id.to_string(),
            "allowed_operations": cap.allowed_operations,
            "expires_at": cap.expires_at,
        });
        fs::write(
            session_dir.join("SESSION.json"),
            serde_json::to_string_pretty(&session_info)?,
        )?;

        // 2. INBOX.json
        let inbox = self.get_inbox(token)?;
        fs::write(
            session_dir.join("INBOX.json"),
            serde_json::to_string_pretty(&inbox)?,
        )?;

        // 3. Populate input/ directory with visible artifact payloads
        for art in &inbox.visible_artifacts {
            if let Ok((_, payload)) = self.get_artifact(token, &art.id) {
                let filename = format!("{}.json", art.id.0);
                let _ = fs::write(input_dir.join(filename), payload);
            }
        }

        Ok(session_dir)
    }

    /// Ingests and validates a published result written by a provider into `outbox/result.json`.
    pub fn consume_materialized_outbox(
        &self,
        token: &RoleSessionToken,
        base_dir: &Path,
    ) -> Result<ArtifactId, ExchangeError> {
        let cap = self.validate_capability(token, ExchangeOperation::PublishResult)?;
        let result_file = base_dir
            .join(cap.role_attempt_id.to_string())
            .join("outbox")
            .join("result.json");

        if !result_file.exists() {
            return Err(ExchangeError::ResultInvalid(
                "outbox/result.json does not exist".to_string(),
            ));
        }

        let payload = fs::read(&result_file)?;
        let publish_req: PublishResultRequest = serde_json::from_slice(&payload).map_err(|e| {
            ExchangeError::ResultInvalid(format!("invalid outbox JSON schema: {}", e))
        })?;

        let artifact_id = self.publish_result(token, publish_req)?;

        // Clean up outbox file after successful publication
        let _ = fs::remove_file(result_file);
        Ok(artifact_id)
    }
}
