//! Relay Architecture v2 - Phase 01: Canonical Daemon Ownership,
//! Client Protocol, Durable Effects & Monotonic Events.
//!
//! The Relay Daemon is the single canonical semantic writer for durable state,
//! external effects, and ordered events. Clients (TUI, Web, MCP) communicate
//! with the daemon using typed commands and monotonic event synchronization.

use std::{sync::Arc, time::Duration};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{Relay, SqliteStore, StoredTask, durable_state::list_stored_tasks};
use relay_store::{
    DaemonLeaseRecord, EventRecord, NewPendingEffectRecord, PendingEffectRecord, StoreError,
};

pub const DAEMON_PROTOCOL_VERSION: u32 = 1;
pub const DAEMON_PROTOCOL_NAME: &str = "relay.v2";
pub const DEFAULT_LEASE_ID: &str = "relay-daemon-primary";
pub const DEFAULT_LEASE_TTL: Duration = Duration::from_secs(15);

#[derive(Debug, Error)]
pub enum DaemonError {
    #[error(
        "protocol version mismatch: client requested {client_version}, daemon supports {daemon_version}"
    )]
    ProtocolVersionMismatch {
        client_version: u32,
        daemon_version: u32,
    },
    #[error("daemon lease conflict: {0}")]
    LeaseHeld(String),
    #[error("store error: {0}")]
    Store(#[from] StoreError),
    #[error("command error: {0}")]
    Command(String),
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

/// A client request sent to the Relay Daemon.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", content = "payload")]
pub enum DaemonRequest {
    Handshake {
        client_name: String,
        protocol_version: u32,
    },
    SubmitCommand {
        command_id: String,
        command: DaemonCommand,
    },
    GetSnapshot,
    GetEventsAfter {
        after_sequence: i64,
        limit: i64,
    },
    Ping,
}

/// Daemon response sent back to a client.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", content = "payload")]
pub enum DaemonResponse {
    HandshakeAck {
        daemon_version: String,
        protocol_version: u32,
        fencing_token: i64,
    },
    CommandResult {
        command_id: String,
        status: CommandStatus,
        result_json: Option<String>,
    },
    Snapshot(DaemonSnapshot),
    Events(Vec<DaemonEvent>),
    Pong,
    Error(String),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum CommandStatus {
    Applied,
    Duplicate,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonEvent {
    pub sequence: i64,
    pub task_id: Option<String>,
    pub execution_id: Option<String>,
    pub event_kind: String,
    pub event_json: Option<String>,
    pub created_at: String,
}

impl From<EventRecord> for DaemonEvent {
    fn from(r: EventRecord) -> Self {
        Self {
            sequence: r.sequence,
            task_id: r.task_id,
            execution_id: r.execution_id,
            event_kind: r.event_kind,
            event_json: r.event_json,
            created_at: r.created_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonPendingEffect {
    pub id: String,
    pub effect_type: String,
    pub idempotency_key: String,
    pub payload_json: String,
    pub state: String,
    pub attempts: i64,
    pub last_error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl From<PendingEffectRecord> for DaemonPendingEffect {
    fn from(r: PendingEffectRecord) -> Self {
        Self {
            id: r.id,
            effect_type: r.effect_type,
            idempotency_key: r.idempotency_key,
            payload_json: r.payload_json,
            state: r.state,
            attempts: r.attempts,
            last_error: r.last_error,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonTaskSummary {
    pub id: String,
    pub project_key: String,
    pub title: String,
    pub submission_transport: String,
    pub state: String,
    pub created_at: String,
    pub updated_at: String,
}

impl From<&StoredTask> for DaemonTaskSummary {
    fn from(t: &StoredTask) -> Self {
        Self {
            id: t.id.to_string(),
            project_key: t.project_key.as_str().to_owned(),
            title: t.handoff.title.clone(),
            submission_transport: t.submission_transport.as_str().to_owned(),
            state: t.state.as_str().to_owned(),
            created_at: t.created_at.clone(),
            updated_at: t.updated_at.clone(),
        }
    }
}

/// Commands that perform semantic state transitions or schedule effects.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "data")]
pub enum DaemonCommand {
    SubmitHandoff {
        project_key: String,
        handoff_json: String,
        submission_transport: String,
    },
    UpdateTaskState {
        task_id: String,
        expected_state: String,
        new_state: String,
    },
    QueuePendingEffect {
        effect_id: String,
        effect_type: String,
        idempotency_key: String,
        payload_json: String,
    },
    CompleteEffect {
        result_id: String,
        effect_id: String,
        outcome: String,
        result_json: String,
    },
}

/// Full daemon state snapshot for client synchronization.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonSnapshot {
    pub latest_event_sequence: i64,
    pub tasks: Vec<DaemonTaskSummary>,
    pub pending_effects: Vec<DaemonPendingEffect>,
    pub fencing_token: i64,
}

/// The Relay Daemon server managing the single-writer database lease,
/// durable effect queue, and client command execution.
pub struct DaemonServer {
    store: SqliteStore,
    relay: Arc<Relay>,
    lease_record: DaemonLeaseRecord,
}

impl DaemonServer {
    /// Bootstraps the daemon server, acquiring the exclusive database lease.
    pub async fn start(
        store: SqliteStore,
        relay: Relay,
        lease_id: &str,
        ttl: Duration,
    ) -> Result<Self, DaemonError> {
        let pid = std::process::id() as i64;
        let host = hostname();

        let lease_record = store
            .acquire_or_renew_daemon_lease(lease_id, pid, &host, ttl)
            .await
            .map_err(|err| match err {
                StoreError::DaemonLeaseHeld {
                    lease_id,
                    holder_pid,
                    holder_host,
                } => DaemonError::LeaseHeld(format!(
                    "lease '{lease_id}' held by pid {holder_pid} on {holder_host}"
                )),
                other => DaemonError::Store(other),
            })?;

        Ok(Self {
            store,
            relay: Arc::new(relay),
            lease_record,
        })
    }

    pub fn store(&self) -> &SqliteStore {
        &self.store
    }

    pub fn fencing_token(&self) -> i64 {
        self.lease_record.fencing_token
    }

    /// Renews the active lease heartbeat.
    pub async fn heartbeat(&mut self, ttl: Duration) -> Result<(), DaemonError> {
        let pid = std::process::id() as i64;
        let host = hostname();
        self.lease_record = self
            .store
            .acquire_or_renew_daemon_lease(&self.lease_record.lease_id, pid, &host, ttl)
            .await?;
        Ok(())
    }

    /// Handles a typed client request.
    pub async fn handle_request(&self, request: DaemonRequest) -> DaemonResponse {
        match request {
            DaemonRequest::Handshake {
                client_name: _,
                protocol_version,
            } => {
                if protocol_version != DAEMON_PROTOCOL_VERSION {
                    return DaemonResponse::Error(format!(
                        "protocol version mismatch: client requested {protocol_version}, daemon supports {DAEMON_PROTOCOL_VERSION}"
                    ));
                }
                DaemonResponse::HandshakeAck {
                    daemon_version: env!("CARGO_PKG_VERSION").to_owned(),
                    protocol_version: DAEMON_PROTOCOL_VERSION,
                    fencing_token: self.lease_record.fencing_token,
                }
            }
            DaemonRequest::Ping => DaemonResponse::Pong,
            DaemonRequest::GetSnapshot => match self.build_snapshot().await {
                Ok(snapshot) => DaemonResponse::Snapshot(snapshot),
                Err(err) => DaemonResponse::Error(err.to_string()),
            },
            DaemonRequest::GetEventsAfter {
                after_sequence,
                limit,
            } => match self.store.get_events_after(after_sequence, limit).await {
                Ok(events) => {
                    DaemonResponse::Events(events.into_iter().map(DaemonEvent::from).collect())
                }
                Err(err) => DaemonResponse::Error(err.to_string()),
            },
            DaemonRequest::SubmitCommand {
                command_id,
                command,
            } => self.execute_command(command_id, command).await,
        }
    }

    async fn build_snapshot(&self) -> Result<DaemonSnapshot, DaemonError> {
        let latest_event_sequence = self.store.latest_event_sequence().await?;
        let tasks = list_stored_tasks(&self.store)
            .await
            .map_err(|e| DaemonError::Command(e.to_string()))?;
        let pending_effects = self.store.list_pending_effects(None).await?;

        Ok(DaemonSnapshot {
            latest_event_sequence,
            tasks: tasks.iter().map(DaemonTaskSummary::from).collect(),
            pending_effects: pending_effects
                .into_iter()
                .map(DaemonPendingEffect::from)
                .collect(),
            fencing_token: self.lease_record.fencing_token,
        })
    }

    async fn execute_command(&self, command_id: String, command: DaemonCommand) -> DaemonResponse {
        let command_type = match &command {
            DaemonCommand::SubmitHandoff { .. } => "submit_handoff",
            DaemonCommand::UpdateTaskState { .. } => "update_task_state",
            DaemonCommand::QueuePendingEffect { .. } => "queue_pending_effect",
            DaemonCommand::CompleteEffect { .. } => "complete_effect",
        };

        let payload_json = match serde_json::to_string(&command) {
            Ok(json) => json,
            Err(err) => return DaemonResponse::Error(err.to_string()),
        };

        // Record command for idempotency
        let is_new = match self
            .store
            .record_command(&command_id, command_type, &payload_json)
            .await
        {
            Ok(is_new) => is_new,
            Err(err) => return DaemonResponse::Error(err.to_string()),
        };

        if !is_new {
            // Already recorded, fetch existing result
            if let Ok(Some(existing)) = self.store.get_command(&command_id).await {
                return DaemonResponse::CommandResult {
                    command_id,
                    status: CommandStatus::Duplicate,
                    result_json: existing.response_json,
                };
            }
        }

        // Execute command intent
        match self.apply_command(&command).await {
            Ok(result_json) => {
                let _ = self
                    .store
                    .complete_command(&command_id, "applied", result_json.as_deref())
                    .await;
                DaemonResponse::CommandResult {
                    command_id,
                    status: CommandStatus::Applied,
                    result_json,
                }
            }
            Err(err) => {
                let err_str = err.to_string();
                let _ = self
                    .store
                    .complete_command(&command_id, "failed", Some(&err_str))
                    .await;
                DaemonResponse::CommandResult {
                    command_id,
                    status: CommandStatus::Failed,
                    result_json: Some(err_str),
                }
            }
        }
    }

    async fn apply_command(&self, command: &DaemonCommand) -> Result<Option<String>, DaemonError> {
        match command {
            DaemonCommand::SubmitHandoff {
                project_key: _,
                handoff_json,
                submission_transport,
            } => {
                let candidate = match submission_transport.as_str() {
                    "relay-mcp" => self.relay.preview_mcp_submission(handoff_json),
                    "local-file" => self.relay.preview_local_file(handoff_json),
                    _ => self.relay.preview_local_paste(handoff_json),
                }
                .map_err(|e| DaemonError::Command(e.to_string()))?;

                let imported = candidate.into_imported_task();
                let stored = crate::durable_state::persist_imported_task(&self.store, &imported)
                    .await
                    .map_err(|e| DaemonError::Command(e.to_string()))?;

                Ok(Some(
                    serde_json::json!({
                        "id": stored.id.to_string(),
                        "state": stored.state.as_str(),
                        "project_key": stored.project_key.as_str(),
                    })
                    .to_string(),
                ))
            }
            DaemonCommand::UpdateTaskState {
                task_id,
                expected_state,
                new_state,
            } => {
                let updated = self
                    .store
                    .update_task_state(task_id, expected_state, new_state)
                    .await?;
                Ok(Some(
                    serde_json::json!({
                        "id": updated.id,
                        "state": updated.state,
                    })
                    .to_string(),
                ))
            }
            DaemonCommand::QueuePendingEffect {
                effect_id,
                effect_type,
                idempotency_key,
                payload_json,
            } => {
                let record = NewPendingEffectRecord {
                    id: effect_id.clone(),
                    effect_type: effect_type.clone(),
                    idempotency_key: idempotency_key.clone(),
                    payload_json: payload_json.clone(),
                };
                let queued = self.store.insert_pending_effect(&record).await?;
                Ok(Some(serde_json::to_string(&DaemonPendingEffect::from(
                    queued,
                ))?))
            }
            DaemonCommand::CompleteEffect {
                result_id,
                effect_id,
                outcome,
                result_json,
            } => {
                let result = self
                    .store
                    .record_effect_result(result_id, effect_id, outcome, result_json)
                    .await?;
                Ok(Some(
                    serde_json::json!({
                        "id": result.id,
                        "effect_id": result.effect_id,
                        "outcome": result.outcome,
                    })
                    .to_string(),
                ))
            }
        }
    }
}

fn hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "localhost".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use uuid::Uuid;

    fn test_relay() -> Relay {
        let mut registry = crate::ProjectRegistry::empty();
        let path = std::env::temp_dir().join("relay-daemon-project");
        std::fs::create_dir_all(&path).unwrap();
        registry
            .register(crate::RegisterProject {
                key: "test-proj".to_owned(),
                name: None,
                local_path: path,
                expected_remote: None,
                default_stack: None,
                github: None,
            })
            .unwrap();
        Relay::with_project_registry(registry)
    }

    const TEST_HANDOFF: &str = r#"{
        "protocol_version": 1,
        "project": "test-proj",
        "title": "Daemon Test Task",
        "task": { "goal": "Verify daemon command and event handling" },
        "ambiguity_policy": "needs_clarification"
    }"#;

    #[tokio::test]
    async fn handshake_protocol_negotiation_and_mismatch() {
        let store = SqliteStore::in_memory().await.unwrap();
        let daemon =
            DaemonServer::start(store, Relay::new(), "test-lease", Duration::from_secs(60))
                .await
                .unwrap();

        // Valid handshake
        let res = daemon
            .handle_request(DaemonRequest::Handshake {
                client_name: "relay-tui".to_owned(),
                protocol_version: DAEMON_PROTOCOL_VERSION,
            })
            .await;

        match res {
            DaemonResponse::HandshakeAck {
                protocol_version,
                fencing_token,
                ..
            } => {
                assert_eq!(protocol_version, DAEMON_PROTOCOL_VERSION);
                assert_eq!(fencing_token, 1);
            }
            other => panic!("unexpected handshake response: {other:?}"),
        }

        // Mismatched version
        let bad_res = daemon
            .handle_request(DaemonRequest::Handshake {
                client_name: "old-client".to_owned(),
                protocol_version: 999,
            })
            .await;

        assert!(matches!(bad_res, DaemonResponse::Error(_)));
    }

    #[tokio::test]
    async fn daemon_command_execution_idempotency_and_snapshot() {
        let store = SqliteStore::in_memory().await.unwrap();
        let daemon =
            DaemonServer::start(store, test_relay(), "test-lease-2", Duration::from_secs(60))
                .await
                .unwrap();

        let cmd_id = Uuid::new_v4().to_string();
        let command = DaemonCommand::SubmitHandoff {
            project_key: "test-proj".to_owned(),
            handoff_json: TEST_HANDOFF.to_owned(),
            submission_transport: "local-paste".to_owned(),
        };

        // First execution -> Applied
        let res1 = daemon
            .handle_request(DaemonRequest::SubmitCommand {
                command_id: cmd_id.clone(),
                command: command.clone(),
            })
            .await;

        match res1 {
            DaemonResponse::CommandResult {
                command_id, status, ..
            } => {
                assert_eq!(command_id, cmd_id);
                assert_eq!(status, CommandStatus::Applied);
            }
            other => panic!("unexpected command result: {other:?}"),
        }

        // Duplicate command -> Duplicate status
        let res2 = daemon
            .handle_request(DaemonRequest::SubmitCommand {
                command_id: cmd_id.clone(),
                command,
            })
            .await;

        match res2 {
            DaemonResponse::CommandResult {
                command_id, status, ..
            } => {
                assert_eq!(command_id, cmd_id);
                assert_eq!(status, CommandStatus::Duplicate);
            }
            other => panic!("unexpected duplicate command result: {other:?}"),
        }

        // Snapshot reflects created task and monotonic events
        let snap_res = daemon.handle_request(DaemonRequest::GetSnapshot).await;
        match snap_res {
            DaemonResponse::Snapshot(snapshot) => {
                assert_eq!(snapshot.tasks.len(), 1);
                assert_eq!(snapshot.tasks[0].title, "Daemon Test Task");
                assert!(snapshot.latest_event_sequence >= 1);
            }
            other => panic!("unexpected snapshot result: {other:?}"),
        }

        // Events after 0 returns the task_ready event
        let events_res = daemon
            .handle_request(DaemonRequest::GetEventsAfter {
                after_sequence: 0,
                limit: 10,
            })
            .await;

        match events_res {
            DaemonResponse::Events(events) => {
                assert!(!events.is_empty());
                assert_eq!(events[0].sequence, 1);
                assert_eq!(events[0].event_kind, "task_ready");
            }
            other => panic!("unexpected events result: {other:?}"),
        }
    }

    #[tokio::test]
    async fn pending_effect_queue_and_completion_flow() {
        let store = SqliteStore::in_memory().await.unwrap();
        let daemon =
            DaemonServer::start(store, test_relay(), "test-lease-3", Duration::from_secs(60))
                .await
                .unwrap();

        let eff_cmd_id = Uuid::new_v4().to_string();
        let queue_cmd = DaemonCommand::QueuePendingEffect {
            effect_id: "eff-100".to_owned(),
            effect_type: "process_spawn".to_owned(),
            idempotency_key: "launch-token-1".to_owned(),
            payload_json: r#"{"process":"herdr"}"#.to_owned(),
        };

        let q_res = daemon
            .handle_request(DaemonRequest::SubmitCommand {
                command_id: eff_cmd_id,
                command: queue_cmd,
            })
            .await;

        assert!(matches!(
            q_res,
            DaemonResponse::CommandResult {
                status: CommandStatus::Applied,
                ..
            }
        ));

        // Complete effect
        let comp_cmd_id = Uuid::new_v4().to_string();
        let comp_cmd = DaemonCommand::CompleteEffect {
            result_id: "res-100".to_owned(),
            effect_id: "eff-100".to_owned(),
            outcome: "success".to_owned(),
            result_json: r#"{"exit_code":0}"#.to_owned(),
        };

        let c_res = daemon
            .handle_request(DaemonRequest::SubmitCommand {
                command_id: comp_cmd_id,
                command: comp_cmd,
            })
            .await;

        assert!(matches!(
            c_res,
            DaemonResponse::CommandResult {
                status: CommandStatus::Applied,
                ..
            }
        ));

        // Snapshot shows effect is completed
        let snap_res = daemon.handle_request(DaemonRequest::GetSnapshot).await;
        if let DaemonResponse::Snapshot(snapshot) = snap_res {
            assert_eq!(snapshot.pending_effects.len(), 1);
            assert_eq!(snapshot.pending_effects[0].state, "completed");
        } else {
            panic!("expected snapshot");
        }
    }
}
