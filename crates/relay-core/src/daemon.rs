//! Relay Architecture v2 - Phase 01: Canonical Daemon Ownership,
//! Client Protocol, Durable Effects & Monotonic Events.
//!
//! The Relay Daemon is the single canonical semantic writer for durable state,
//! external effects, and ordered events. Clients (TUI, Web, MCP) communicate
//! with the daemon using typed commands and monotonic event synchronization.

use std::{sync::Arc, time::Duration};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    ApprovedIntentId, ConfigSnapshotId, ExecutionGrantId, ExecutionTarget, Relay, SqliteStore,
    StoredTask, SubmissionTransport, TaskId, TaskState, WorkflowRunId,
    durable_state::list_stored_tasks,
};
use relay_store::{
    CommandRecord, DaemonFence, DaemonLeaseRecord, EventRecord, NewPendingEffectRecord,
    NewWorkflowRunRecord, PendingEffectRecord, StoreError, WorkflowRunStart,
};

pub const DAEMON_PROTOCOL_VERSION: u32 = 2;
pub const DAEMON_PROTOCOL_NAME: &str = "relay.v2";
pub const DEFAULT_LEASE_ID: &str = "relay-daemon-primary";
pub const DEFAULT_LEASE_TTL: Duration = Duration::from_secs(15);
const FROZEN_PROVIDER_ADAPTER_VERSION: &str = "0.1.0";

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
    #[error("daemon lost authority over the database before the command could commit: {0}")]
    AuthorityLost(String),
    #[error("command error: {0}")]
    Command(String),
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("no daemon is currently running")]
    NotRunning,
    #[error("daemon transport error: {0}")]
    Transport(String),
    #[error(
        "daemon state identity mismatch: the client is bound to durable state '{expected}', but the responding daemon serves '{actual}'"
    )]
    StateIdentityMismatch { expected: String, actual: String },
}

/// A client request sent to the Relay Daemon.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", content = "payload")]
pub enum DaemonRequest {
    Handshake {
        client_name: String,
        protocol_version: u32,
        /// Durable state identity the client is bound to. The daemon refuses a
        /// mismatch, so a client can never mutate a database it did not open.
        state_identity: String,
    },
    SubmitCommand {
        command_id: String,
        command: DaemonCommand,
        /// Durable state identity the submitting client is bound to. The daemon
        /// refuses a mismatch before applying anything, so a mutating request
        /// cannot be dispatched without the caller naming the database it means
        /// to write - even if it bypasses the connection wrapper.
        state_identity: String,
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
        /// Durable state identity this daemon is serving, so the client can verify
        /// it reached the daemon for its own database.
        state_identity: String,
    },
    CommandResult {
        command_id: String,
        status: CommandStatus,
        result_json: Option<String>,
    },
    Snapshot(DaemonSnapshot),
    Events(Vec<DaemonEvent>),
    Pong,
    /// The daemon refused the handshake because the client is bound to a
    /// different durable state identity. Carries both identities so the client
    /// can report the mismatch as a typed transport error rather than an opaque
    /// command failure.
    StateIdentityMismatch {
        /// Durable state identity the client asked for.
        client_identity: String,
        /// Durable state identity this daemon actually serves.
        daemon_identity: String,
    },
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
    /// Explicit human Run approval for a durable `Ready` task. Records the v2
    /// workflow run (with the approved intent, grant and snapshot it executes
    /// under), moves the task to `running`, and wakes the workflow driver.
    RunTask { task_id: String, granted_by: String },
}

/// Full daemon state snapshot for client synchronization.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonSnapshot {
    pub latest_event_sequence: i64,
    pub tasks: Vec<DaemonTaskSummary>,
    pub pending_effects: Vec<DaemonPendingEffect>,
    pub fencing_token: i64,
    /// Commands whose payload could not be safely replayed during startup
    /// reconciliation (e.g. corrupted JSON) and were parked instead of
    /// silently left stuck. Should be `0` under healthy operation.
    pub stuck_commands_needing_reconciliation: i64,
}

/// The Relay Daemon server managing the single-writer database lease,
/// durable effect queue, and client command execution.
pub struct DaemonServer {
    store: SqliteStore,
    relay: Arc<Relay>,
    // A std Mutex (not tokio's) because it is only ever held briefly across a
    // plain field read/write, never across an `.await` point. This lets the
    // server be shared behind an `Arc` (request handling and the lease
    // heartbeat run concurrently) while `heartbeat` still only needs `&self`.
    lease_record: std::sync::Mutex<DaemonLeaseRecord>,
    /// This database's durable state identity (migration 0007). Requests bound to
    /// any other identity are refused: a reachable pipe is never accepted as proof
    /// that the responding daemon serves the database the client opened.
    state_identity: String,
    /// `None` while this process is the authoritative lease holder; `Some(reason)`
    /// once it has lost the lease. While it is `Some`, every request fails closed
    /// so a stale daemon cannot keep mutating state it no longer owns.
    authority_lost: std::sync::Mutex<Option<String>>,
    // Wakes the v2 workflow driver when a run is approved. `Notify` stores one
    // permit when no driver is waiting, so an approval that lands mid-drive is
    // never lost (unlike `notify_waiters`).
    workflow_wakeup: Arc<tokio::sync::Notify>,
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
        // The lease is owned by this daemon *instance*, not merely by this
        // process: two daemons started in one process share a pid and hostname,
        // and a process-scoped identity would let the second renew - or a failed
        // second startup release - the first instance's lease.
        let instance = format!("{pid}-{}", uuid::Uuid::new_v4());

        let lease_record = store
            .acquire_or_renew_daemon_lease(lease_id, pid, &host, &instance, ttl)
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

        let state_identity = match store.store_identity().await {
            Ok(identity) => identity,
            Err(err) => {
                // The lease was acquired, but this process cannot serve the
                // database it just claimed. Release the lease rather than pinning
                // it until the TTL elapses; the release is scoped to this exact
                // instance, so it can never delete a successor's row.
                if let Err(release_err) = store.release_daemon_lease(lease_id, pid, &instance).await
                {
                    tracing::warn!(
                        error = %release_err,
                        "failed to release the daemon lease after a failed startup"
                    );
                }
                return Err(DaemonError::Store(err));
            }
        };

        Ok(Self {
            store,
            relay: Arc::new(relay),
            lease_record: std::sync::Mutex::new(lease_record),
            state_identity,
            authority_lost: std::sync::Mutex::new(None),
            workflow_wakeup: Arc::new(tokio::sync::Notify::new()),
        })
    }

    /// Wakes the v2 workflow driver, if one is attached to this daemon.
    ///
    /// Called after a Run approval is durably recorded, so the driver looks at
    /// the run instead of the daemon trying to execute it inline.
    pub fn wake_workflow_driver(&self) {
        self.workflow_wakeup.notify_one();
    }

    /// Handle the workflow driver waits on for new approved runs.
    pub fn workflow_wakeup(&self) -> Arc<tokio::sync::Notify> {
        Arc::clone(&self.workflow_wakeup)
    }

    pub fn store(&self) -> &SqliteStore {
        &self.store
    }

    /// The durable state identity this daemon serves (see
    /// [`relay_store::SqliteStore::store_identity`]).
    pub fn state_identity(&self) -> &str {
        &self.state_identity
    }

    /// Validates a handshake and builds its response.
    ///
    /// This is the single validation path for handshakes: the transport requires
    /// a `HandshakeAck` from here before dispatching any other request on a
    /// connection, and the direct request path uses it too, so both agree on the
    /// protocol version and database identity.
    pub fn handshake_response(
        &self,
        protocol_version: u32,
        state_identity: &str,
    ) -> DaemonResponse {
        if let Some(reason) = self.authority_lost_reason() {
            // Fail closed: a stale instance must not acknowledge a handshake as
            // if it were the live authority. The transport treats any non-ack
            // response as "connection not admitted", so a matching handshake
            // against a lost lease can no longer make a stale daemon look
            // healthy to a caller that treats the ack as proof of authority.
            return DaemonResponse::Error(format!(
                "daemon is no longer authoritative over this database: {reason}"
            ));
        }

        if protocol_version != DAEMON_PROTOCOL_VERSION {
            return DaemonResponse::Error(format!(
                "protocol version mismatch: client requested {protocol_version}, daemon supports {DAEMON_PROTOCOL_VERSION}"
            ));
        }

        if state_identity != self.state_identity {
            return DaemonResponse::StateIdentityMismatch {
                client_identity: state_identity.to_owned(),
                daemon_identity: self.state_identity.clone(),
            };
        }

        DaemonResponse::HandshakeAck {
            daemon_version: env!("CARGO_PKG_VERSION").to_owned(),
            protocol_version: DAEMON_PROTOCOL_VERSION,
            fencing_token: self.fencing_token(),
            state_identity: self.state_identity.clone(),
        }
    }

    /// The durable fence this daemon must still hold for its writes to commit.
    ///
    /// Exposed so a daemon-owned background writer (the effect worker) can
    /// present the same authority as the request path: every fenced store write
    /// refuses a fence that no longer matches the live lease row.
    pub fn fence(&self) -> DaemonFence {
        let record = self
            .lease_record
            .lock()
            .expect("lease record lock poisoned");
        DaemonFence {
            lease_id: record.lease_id.clone(),
            holder_pid: record.holder_pid,
            holder_instance: record.holder_instance.clone(),
            fencing_token: record.fencing_token,
        }
    }

    /// Records that this process has lost authoritative ownership of the daemon
    /// lease. Idempotent: the first reason recorded is the one reported, so the
    /// original cause is what clients and logs see.
    ///
    /// After this, [`DaemonServer::handle_request`] fails closed for every request
    /// and callers are expected to stop the request listener and effect worker, so
    /// a successor can bind the transport and take over.
    pub fn mark_authority_lost(&self, reason: impl Into<String>) {
        let mut authority_lost = self.authority_lost.lock().expect("authority lock poisoned");
        if authority_lost.is_none() {
            *authority_lost = Some(reason.into());
        }
    }

    fn authority_lost_reason(&self) -> Option<String> {
        self.authority_lost
            .lock()
            .expect("authority lock poisoned")
            .clone()
    }

    pub fn lease_id(&self) -> String {
        self.lease_record
            .lock()
            .expect("lease record lock poisoned")
            .lease_id
            .clone()
    }

    pub fn fencing_token(&self) -> i64 {
        self.lease_record
            .lock()
            .expect("lease record lock poisoned")
            .fencing_token
    }

    /// Releases the daemon lease this process holds, so a subsequent process
    /// does not have to wait out the full TTL before it can become the
    /// daemon. Safe to call only after the heartbeat loop has stopped (a
    /// concurrent heartbeat could otherwise re-acquire the row right after
    /// this deletes it).
    pub async fn release_lease(&self) -> Result<(), DaemonError> {
        let lease_id = self.lease_id();
        let instance = self
            .lease_record
            .lock()
            .expect("lease record lock poisoned")
            .holder_instance
            .clone();
        self.store
            .release_daemon_lease(&lease_id, std::process::id() as i64, &instance)
            .await?;
        Ok(())
    }

    /// Renews the active lease heartbeat.
    pub async fn heartbeat(&self, ttl: Duration) -> Result<(), DaemonError> {
        let lease_id = self.lease_id();
        let instance = self
            .lease_record
            .lock()
            .expect("lease record lock poisoned")
            .holder_instance
            .clone();
        let pid = std::process::id() as i64;
        let host = hostname();
        let renewed = self
            .store
            .acquire_or_renew_daemon_lease(&lease_id, pid, &host, &instance, ttl)
            .await?;
        *self
            .lease_record
            .lock()
            .expect("lease record lock poisoned") = renewed;
        Ok(())
    }

    /// Handles a typed client request.
    ///
    /// A mutating [`DaemonRequest::SubmitCommand`] must name the durable state
    /// identity it is bound to; a mismatch is refused before any mutation, so a
    /// caller that bypasses the connection wrapper still cannot write to an
    /// unintended database. The transport additionally refuses any request on a
    /// connection that has not completed a matching handshake.
    pub async fn handle_request(&self, request: DaemonRequest) -> DaemonResponse {
        if let Some(reason) = self.authority_lost_reason() {
            // Fail closed: this process can no longer prove it owns the daemon
            // lease, so it must not execute semantic commands (or claim liveness)
            // on state a successor may already own.
            return DaemonResponse::Error(format!(
                "daemon is no longer authoritative over this database: {reason}"
            ));
        }

        match request {
            DaemonRequest::Handshake {
                client_name: _,
                protocol_version,
                state_identity,
            } => self.handshake_response(protocol_version, &state_identity),
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
                state_identity,
            } => {
                if state_identity != self.state_identity {
                    // Fail closed before any mutation: the caller is not bound to
                    // this daemon's durable state.
                    return DaemonResponse::StateIdentityMismatch {
                        client_identity: state_identity,
                        daemon_identity: self.state_identity.clone(),
                    };
                }
                self.execute_command(command_id, command).await
            }
        }
    }

    async fn build_snapshot(&self) -> Result<DaemonSnapshot, DaemonError> {
        let latest_event_sequence = self.store.latest_event_sequence().await?;
        let tasks = list_stored_tasks(&self.store)
            .await
            .map_err(|e| DaemonError::Command(e.to_string()))?;
        let pending_effects = self.store.list_pending_effects(None).await?;
        let stuck_commands_needing_reconciliation = self
            .store
            .count_commands_with_status("needs_reconciliation")
            .await?;

        Ok(DaemonSnapshot {
            latest_event_sequence,
            tasks: tasks.iter().map(DaemonTaskSummary::from).collect(),
            pending_effects: pending_effects
                .into_iter()
                .map(DaemonPendingEffect::from)
                .collect(),
            fencing_token: self.fencing_token(),
            stuck_commands_needing_reconciliation,
        })
    }

    fn command_type_of(command: &DaemonCommand) -> &'static str {
        match command {
            DaemonCommand::SubmitHandoff { .. } => "submit_handoff",
            DaemonCommand::UpdateTaskState { .. } => "update_task_state",
            DaemonCommand::QueuePendingEffect { .. } => "queue_pending_effect",
            DaemonCommand::CompleteEffect { .. } => "complete_effect",
            DaemonCommand::RunTask { .. } => "run_task",
        }
    }

    async fn execute_command(&self, command_id: String, command: DaemonCommand) -> DaemonResponse {
        let command_type = Self::command_type_of(&command);

        let payload_json = match serde_json::to_string(&command) {
            Ok(json) => json,
            Err(err) => return DaemonResponse::Error(err.to_string()),
        };

        if let Err(err) = self
            .record_command_checked(&command_id, command_type, &payload_json)
            .await
        {
            return DaemonResponse::Error(err.to_string());
        }

        self.resolve_command(&command_id, &command).await
    }

    /// Records a command for idempotency, failing closed if the same command
    /// id was already recorded with a *different* payload (id reuse across
    /// distinct requests must never be silently accepted as a duplicate).
    async fn record_command_checked(
        &self,
        command_id: &str,
        command_type: &str,
        payload_json: &str,
    ) -> Result<(), DaemonError> {
        let is_new = self
            .store
            .record_command_fenced(command_id, command_type, payload_json, &self.fence())
            .await?;

        if !is_new {
            if let Some(existing) = self.store.get_command(command_id).await? {
                if existing.payload_json != payload_json {
                    return Err(DaemonError::Command(format!(
                        "command id {command_id} was already recorded with a different payload"
                    )));
                }
            }
        }

        Ok(())
    }

    /// Resolves a recorded command to a terminal outcome. Terminal commands
    /// (`applied`/`failed`) return their cached result. A non-terminal command is
    /// claimed by exactly one caller, which applies it and records the terminal
    /// result; a concurrent replay of the same command id loses the claim and waits
    /// for that result instead of applying the command a second time.
    ///
    /// The semantic effect and the command's terminal record are two separate
    /// durable writes. If the effect commits but its terminal record does not
    /// (crash, or a failed completion write), the terminal outcome is *not*
    /// reported optimistically: the command is left non-terminal for startup
    /// reconciliation, and the caller is told the resolution could not be
    /// recorded. Replay of an already-committed transition converges on
    /// `applied` (see `apply_command`), so a crash cannot turn a successful
    /// transition into a durable `failed` command.
    async fn resolve_command(&self, command_id: &str, command: &DaemonCommand) -> DaemonResponse {
        let existing = match self.store.get_command(command_id).await {
            Ok(Some(row)) => row,
            Ok(None) => {
                return DaemonResponse::Error(format!(
                    "command {command_id} vanished after being recorded"
                ));
            }
            Err(err) => return DaemonResponse::Error(err.to_string()),
        };

        match existing.status.as_str() {
            "applied" => {
                return DaemonResponse::CommandResult {
                    command_id: command_id.to_owned(),
                    status: CommandStatus::Duplicate,
                    result_json: existing.response_json,
                };
            }
            "failed" => {
                // A cached failure stays a failure. Reporting it as `Duplicate`
                // would let a caller that replays a command id read a failed
                // command as a successful one.
                return DaemonResponse::CommandResult {
                    command_id: command_id.to_owned(),
                    status: CommandStatus::Failed,
                    result_json: existing.response_json,
                };
            }
            _ => {}
        }

        // The claim is fenced with the semantic write below: a daemon that lost
        // the lease must not claim a command after a successor has already run
        // its one-time startup reconciliation, or the command would be stranded
        // with a claim no one will clear until the next restart.
        match self
            .store
            .claim_command_fenced(command_id, &self.fence())
            .await
        {
            Ok(true) => {}
            Ok(false) => return self.await_concurrent_resolution(command_id).await,
            Err(err) => return DaemonResponse::Error(err.to_string()),
        }

        let (status, result_json) = match self.apply_command(command_id, command).await {
            Ok(result_json) => (CommandStatus::Applied, result_json),
            Err(DaemonError::Store(StoreError::DaemonAuthorityLost { .. }))
            | Err(DaemonError::AuthorityLost(_)) => {
                // Authority was lost while the command was in flight. Do not
                // record a terminal result: a successor owns the database now
                // and will reconcile this command. Recording `failed` here
                // would durably mislabel a command that never got to run.
                return DaemonResponse::Error(
                    "daemon lost authority over the database while applying the command; it was not recorded as terminal"
                        .to_owned(),
                );
            }
            Err(err) => (CommandStatus::Failed, Some(err.to_string())),
        };

        let terminal_status = match status {
            CommandStatus::Applied => "applied",
            _ => "failed",
        };
        match self
            .store
            .complete_command_fenced(
                command_id,
                terminal_status,
                result_json.as_deref(),
                &self.fence(),
            )
            .await
        {
            Ok(true) => {}
            Ok(false) => {
                // Another applier recorded this command's terminal result first.
                // Report that committed outcome instead of this call's own, so a
                // caller can never see an outcome that is not the durable one.
                return self.await_concurrent_resolution(command_id).await;
            }
            Err(err) => {
                // Fail closed: the caller must not be told the command succeeded
                // when we could not durably record that outcome. The command stays
                // non-terminal and startup reconciliation will resolve it.
                return DaemonResponse::Error(format!(
                    "command {command_id} was applied but its terminal result could not be recorded: {err}"
                ));
            }
        }

        DaemonResponse::CommandResult {
            command_id: command_id.to_owned(),
            status,
            result_json,
        }
    }

    /// Waits for the caller that holds a command's claim to record the terminal
    /// result, then reports that same result. A duplicate replay therefore observes
    /// the winner's committed outcome - including its `failed` outcome - instead of
    /// racing it, so reported and durable terminal status cannot disagree.
    async fn await_concurrent_resolution(&self, command_id: &str) -> DaemonResponse {
        const MAX_ATTEMPTS: u32 = 200;
        const POLL_INTERVAL: Duration = Duration::from_millis(10);

        for _ in 0..MAX_ATTEMPTS {
            tokio::time::sleep(POLL_INTERVAL).await;

            match self.store.get_command(command_id).await {
                Ok(Some(row)) if matches!(row.status.as_str(), "applied" | "failed") => {
                    let status = if row.status == "applied" {
                        CommandStatus::Duplicate
                    } else {
                        CommandStatus::Failed
                    };
                    return DaemonResponse::CommandResult {
                        command_id: command_id.to_owned(),
                        status,
                        result_json: row.response_json,
                    };
                }
                Ok(_) => {}
                Err(err) => return DaemonResponse::Error(err.to_string()),
            }
        }

        DaemonResponse::Error(format!(
            "command {command_id} is already being applied but has not reached a terminal result"
        ))
    }

    /// Scans for commands stuck in a non-terminal status (e.g. the daemon
    /// crashed between `apply_command` succeeding and `complete_command`
    /// being recorded) and resolves each one. A command whose payload cannot
    /// be deserialized is parked as `needs_reconciliation` with a logged
    /// reason instead of being silently left stuck or guessed at.
    pub async fn reconcile_on_startup(&self) -> Result<(), DaemonError> {
        // Claims held by the previous process died with it. This runs before the
        // request listener accepts connections, so clearing them cannot steal a
        // live claim. It is fenced on this daemon's lease so a stale daemon can
        // never clear a live successor's claims.
        self.store
            .reset_command_claims_fenced(&self.fence())
            .await?;

        let incomplete = self.store.list_incomplete_commands().await?;
        for row in incomplete {
            self.reconcile_one_command(&row).await;
        }
        Ok(())
    }

    async fn reconcile_one_command(&self, row: &CommandRecord) {
        let command: DaemonCommand = match serde_json::from_str(&row.payload_json) {
            Ok(command) => command,
            Err(err) => {
                tracing::error!(
                    command_id = %row.id,
                    error = %err,
                    "daemon startup reconciliation: command payload cannot be safely replayed"
                );
                let reason = format!("payload deserialization failed: {err}");
                let _ = self
                    .store
                    .complete_command_fenced(
                        &row.id,
                        "needs_reconciliation",
                        Some(&reason),
                        &self.fence(),
                    )
                    .await;
                return;
            }
        };

        let response = self.resolve_command(&row.id, &command).await;
        if let DaemonResponse::CommandResult {
            status: CommandStatus::Failed,
            result_json,
            ..
        } = &response
        {
            tracing::error!(
                command_id = %row.id,
                result = ?result_json,
                "daemon startup reconciliation: command re-application failed"
            );
        }
    }

    /// Returns the trusted registry to resolve a submission against.
    ///
    /// When this daemon was built from a config path, the persisted registry is
    /// re-read so registrations made after daemon start take effect immediately.
    /// A registry with no persisted source (test harnesses, explicitly injected
    /// registries) is used as-is.
    fn trusted_registry_for_submission(&self) -> Result<Relay, DaemonError> {
        let Some(config_path) = self.relay.config_path() else {
            return Ok((*self.relay).clone());
        };

        Relay::load_from_config_path(config_path).map_err(|error| {
            DaemonError::Command(format!(
                "cannot refresh the trusted project registry from {}: {error}",
                config_path.display()
            ))
        })
    }

    async fn apply_command(
        &self,
        command_id: &str,
        command: &DaemonCommand,
    ) -> Result<Option<String>, DaemonError> {
        // Every durable write a command performs is fenced on this daemon's
        // lease: if a successor took over between admission and the write, the
        // store refuses the mutation instead of letting a stale daemon commit.
        let fence = self.fence();

        match command {
            DaemonCommand::SubmitHandoff {
                project_key: _,
                handoff_json,
                submission_transport,
            } => {
                let transport =
                    SubmissionTransport::parse(submission_transport).ok_or_else(|| {
                        DaemonError::Command(format!(
                            "unknown submission transport '{submission_transport}'"
                        ))
                    })?;
                // Resolve against the authoritative persisted registry rather than the
                // snapshot captured at daemon start: a project may have been
                // registered since then, and a stale in-memory copy would reject a
                // handoff the trusted config already allows.
                let relay = self.trusted_registry_for_submission()?;
                let candidate = relay
                    .resolve_task_import_candidate(handoff_json, transport)
                    .map_err(|e| DaemonError::Command(e.to_string()))?;

                let imported = candidate
                    .into_imported_task_with_id(TaskId::deterministic_from_command_id(command_id));
                let stored = crate::durable_state::persist_imported_task_if_absent(
                    &self.store,
                    &imported,
                    &fence,
                )
                .await
                .map_err(map_persistence_error)?;

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
                let updated = match self
                    .store
                    .update_task_state_fenced(task_id, expected_state, new_state, &fence)
                    .await
                {
                    Ok(updated) => updated,
                    Err(StoreError::StateConflict { actual_state, .. })
                        if actual_state == *new_state =>
                    {
                        // The transition this command performs is already
                        // durably present (for example, the compare-and-set
                        // committed before a crash but the command's terminal
                        // row was never written). The command is idempotent on
                        // its desired end state, so replaying it converges on
                        // `applied` instead of a false `failed`.
                        return Ok(Some(
                            serde_json::json!({"id": task_id, "state": new_state}).to_string(),
                        ));
                    }
                    Err(err) => return Err(map_store_error(err)),
                };
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
                let queued = self
                    .store
                    .insert_pending_effect_fenced(&record, &fence)
                    .await
                    .map_err(map_store_error)?;
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
                    .record_effect_result_fenced(result_id, effect_id, outcome, result_json, &fence)
                    .await
                    .map_err(map_store_error)?;
                Ok(Some(
                    serde_json::json!({
                        "id": result.id,
                        "effect_id": result.effect_id,
                        "outcome": result.outcome,
                    })
                    .to_string(),
                ))
            }
            DaemonCommand::RunTask {
                task_id,
                granted_by,
            } => {
                let task_id_parsed = TaskId::parse(task_id)
                    .map_err(|e| DaemonError::Command(format!("invalid task id: {e}")))?;
                let stored_task =
                    crate::durable_state::get_stored_task(&self.store, task_id_parsed)
                        .await
                        .map_err(|e| DaemonError::Command(e.to_string()))?
                        .ok_or_else(|| {
                            DaemonError::Command(format!("task '{task_id}' not found"))
                        })?;

                let run_id = WorkflowRunId(format!(
                    "wfrun-{}",
                    deterministic_command_id(command_id, "run")
                ));

                // A recorded approval is identified by the command id, so a
                // replay reports the durable run rather than recomputing (and
                // possibly reinterpreting) what the human approved.
                if let Some(existing_run) = self
                    .store
                    .get_workflow_run(&run_id.0)
                    .await
                    .map_err(map_store_error)?
                {
                    return Ok(Some(
                        serde_json::json!({
                            "task_id": existing_run.task_id,
                            "workflow_run_id": existing_run.id,
                            "execution_grant_id": existing_run.execution_grant_id,
                            "state": existing_run.state,
                        })
                        .to_string(),
                    ));
                }

                if stored_task.state != TaskState::Ready {
                    return Err(DaemonError::Command(format!(
                        "task '{task_id}' is in state '{}', expected 'ready'",
                        stored_task.state.as_str()
                    )));
                }

                let mut intent =
                    crate::adapt_v1_handoff_to_approved_intent(&stored_task, granted_by)
                        .map_err(|e| DaemonError::Command(e.to_string()))?;
                intent.id = ApprovedIntentId(format!(
                    "intent-{}",
                    deterministic_command_id(command_id, "intent")
                ));

                let trusted_relay = self.trusted_registry_for_submission()?;
                let project = trusted_relay
                    .project_registry()
                    .get(stored_task.project_key.as_str())
                    .map_err(|_| {
                        DaemonError::Command(format!(
                            "project '{}' is not registered",
                            stored_task.project_key
                        ))
                    })?;
                let stack = stored_task
                    .handoff
                    .stack
                    .as_deref()
                    .or_else(|| project.default_stack())
                    .ok_or_else(|| {
                        DaemonError::Command("no runtime stack is configured".to_owned())
                    })?;
                let execution_target = execution_target_for_stack(stack)?;

                let default_budget = crate::ExecutionBudget::default();
                let (mut grant, mut run, mut snapshot) =
                    crate::create_v1_compatibility_run_with_target(
                        &stored_task,
                        &intent,
                        execution_target,
                        default_budget,
                        granted_by,
                    )
                    .map_err(|e| DaemonError::Command(e.to_string()))?;

                grant.id = ExecutionGrantId(format!(
                    "grant-{}",
                    deterministic_command_id(command_id, "grant")
                ));
                snapshot.id = ConfigSnapshotId(format!(
                    "cfg-{}",
                    deterministic_command_id(command_id, "snapshot")
                ));
                run.id = run_id.clone();
                run.execution_grant_id = grant.id.clone();
                run.config_snapshot_id = snapshot.id.clone();

                let run_record = NewWorkflowRunRecord {
                    id: run.id.0.clone(),
                    task_id: run.task_id.to_string(),
                    execution_grant_id: run.execution_grant_id.0.clone(),
                    workflow_definition_id: run.workflow_definition_id.0.clone(),
                    state: "running".to_owned(),
                    config_snapshot_id: run.config_snapshot_id.0.clone(),
                };
                let intent_json = serde_json::to_string(&intent)?;
                let grant_json = serde_json::to_string(&grant)?;
                let snapshot_json = serde_json::to_string(&snapshot)?;
                let event_json = serde_json::json!({
                    "task_id": task_id,
                    "workflow_run_id": run.id.0,
                    "execution_grant_id": grant.id.0,
                    "config_snapshot_id": snapshot.id.0,
                    "state": "running",
                })
                .to_string();

                // One fenced transaction: the run and its authority, the task's
                // `ready -> running` transition, and the single start event. A
                // crash leaves either the whole approval or none of it,
                // never a task that is running with no run to execute.
                let started = self
                    .store
                    .start_workflow_run_fenced(
                        &WorkflowRunStart {
                            run: &run_record,
                            approved_intent_json: &intent_json,
                            execution_grant_json: &grant_json,
                            config_snapshot_json: &snapshot_json,
                            task_expected_state: "ready",
                            task_new_state: "running",
                            event_kind: "workflow_run_started",
                            event_json: Some(&event_json),
                        },
                        &fence,
                    )
                    .await
                    // A second Run approval for a task that is no longer `ready`
                    // is a refused command, never lost daemon authority: this
                    // process still owns the lease, the *task* has moved on. The
                    // store's compare-and-set is the guard even when two
                    // approvals with different command ids race past the
                    // `Ready` pre-check above.
                    .map_err(|error| match error {
                        StoreError::StateConflict {
                            id, actual_state, ..
                        } => DaemonError::Command(format!(
                            "task '{id}' is in state '{actual_state}', expected 'ready'; refusing a second Run approval"
                        )),
                        other => map_store_error(other),
                    })?;

                // The run is durable; execution belongs to the workflow driver,
                // never to the command handler.
                self.wake_workflow_driver();

                Ok(Some(
                    serde_json::json!({
                        "task_id": started.task_id,
                        "workflow_run_id": started.id,
                        "execution_grant_id": started.execution_grant_id,
                        "state": started.state,
                    })
                    .to_string(),
                ))
            }
        }
    }
}

/// Client interface for interacting with the canonical Relay Daemon.
/// Enforces handshake negotiation, snapshot synchronization, and monotonic event catch-up.
#[allow(async_fn_in_trait)]
pub trait DaemonClient: Send + Sync {
    /// Performs handshake with the daemon.
    async fn handshake(&self, client_name: &str) -> Result<(String, i64), DaemonError>;

    /// Fetches the latest full state snapshot.
    async fn get_snapshot(&self) -> Result<DaemonSnapshot, DaemonError>;

    /// Fetches monotonic events after the given sequence number.
    async fn get_events_after(
        &self,
        after_sequence: i64,
        limit: i64,
    ) -> Result<Vec<DaemonEvent>, DaemonError>;

    /// Submits a typed command for execution.
    async fn submit_command(
        &self,
        command_id: &str,
        command: DaemonCommand,
    ) -> Result<CommandStatus, DaemonError>;

    /// Sends a ping probe to verify connection liveness.
    async fn ping(&self) -> Result<(), DaemonError>;
}

/// In-process daemon client that calls DaemonServer directly.
/// Used by TUI/Web test harnesses or embedded processes without socket overhead.
pub struct InProcessDaemonClient {
    server: Arc<DaemonServer>,
}

impl InProcessDaemonClient {
    pub fn new(server: Arc<DaemonServer>) -> Self {
        Self { server }
    }
}

impl DaemonClient for InProcessDaemonClient {
    async fn handshake(&self, client_name: &str) -> Result<(String, i64), DaemonError> {
        let resp = self
            .server
            .handle_request(DaemonRequest::Handshake {
                client_name: client_name.to_owned(),
                protocol_version: DAEMON_PROTOCOL_VERSION,
                state_identity: self.server.state_identity().to_owned(),
            })
            .await;

        match resp {
            DaemonResponse::HandshakeAck {
                daemon_version,
                fencing_token,
                ..
            } => Ok((daemon_version, fencing_token)),
            DaemonResponse::Error(err) => Err(DaemonError::Command(err)),
            _ => Err(DaemonError::Command(
                "unexpected handshake response".to_owned(),
            )),
        }
    }

    async fn get_snapshot(&self) -> Result<DaemonSnapshot, DaemonError> {
        let resp = self.server.handle_request(DaemonRequest::GetSnapshot).await;
        match resp {
            DaemonResponse::Snapshot(snapshot) => Ok(snapshot),
            DaemonResponse::Error(err) => Err(DaemonError::Command(err)),
            _ => Err(DaemonError::Command(
                "unexpected snapshot response".to_owned(),
            )),
        }
    }

    async fn get_events_after(
        &self,
        after_sequence: i64,
        limit: i64,
    ) -> Result<Vec<DaemonEvent>, DaemonError> {
        let resp = self
            .server
            .handle_request(DaemonRequest::GetEventsAfter {
                after_sequence,
                limit,
            })
            .await;

        match resp {
            DaemonResponse::Events(events) => Ok(events),
            DaemonResponse::Error(err) => Err(DaemonError::Command(err)),
            _ => Err(DaemonError::Command(
                "unexpected events response".to_owned(),
            )),
        }
    }

    async fn submit_command(
        &self,
        command_id: &str,
        command: DaemonCommand,
    ) -> Result<CommandStatus, DaemonError> {
        let resp = self
            .server
            .handle_request(DaemonRequest::SubmitCommand {
                command_id: command_id.to_owned(),
                command,
                state_identity: self.server.state_identity().to_owned(),
            })
            .await;

        match resp {
            DaemonResponse::CommandResult { status, .. } => Ok(status),
            DaemonResponse::Error(err) => Err(DaemonError::Command(err)),
            _ => Err(DaemonError::Command(
                "unexpected command response".to_owned(),
            )),
        }
    }

    async fn ping(&self) -> Result<(), DaemonError> {
        let resp = self.server.handle_request(DaemonRequest::Ping).await;
        match resp {
            DaemonResponse::Pong => Ok(()),
            DaemonResponse::Error(err) => Err(DaemonError::Command(err)),
            _ => Err(DaemonError::Command("unexpected ping response".to_owned())),
        }
    }
}

/// Preserves the typed "this daemon no longer owns the database" signal so a
/// command refused by the fence is never mistaken for an ordinary failure.
fn map_store_error(error: StoreError) -> DaemonError {
    match error {
        StoreError::DaemonAuthorityLost { lease_id } => DaemonError::AuthorityLost(lease_id),
        other => DaemonError::Store(other),
    }
}

fn map_persistence_error(error: crate::durable_state::TaskPersistenceError) -> DaemonError {
    match error {
        crate::durable_state::TaskPersistenceError::Store(StoreError::DaemonAuthorityLost {
            lease_id,
        }) => DaemonError::AuthorityLost(lease_id),
        other => DaemonError::Command(other.to_string()),
    }
}

/// Deterministic id segment for a Run approval.
///
/// The run, intent, grant and snapshot identities are derived from the approving
/// command id, so replaying that command cannot invent a second run for the same
/// approval.
fn deterministic_command_id(command_id: &str, label: &str) -> String {
    uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_OID,
        format!("{command_id}:{label}").as_bytes(),
    )
    .simple()
    .to_string()
}

fn execution_target_for_stack(stack: &str) -> Result<ExecutionTarget, DaemonError> {
    let (provider_id, runtime_profile_id) = match stack {
        "chatgpt-codex" | "codex" => ("codex", "codex/default"),
        "antigravity" | "agy" => ("antigravity", "antigravity/default"),
        other => {
            return Err(DaemonError::Command(format!(
                "unsupported runtime stack '{other}'"
            )));
        }
    };

    Ok(ExecutionTarget {
        provider_id: provider_id.to_owned(),
        runtime_profile_id: runtime_profile_id.to_owned(),
        provider_adapter_version: FROZEN_PROVIDER_ADAPTER_VERSION.to_owned(),
    })
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
                state_identity: daemon.state_identity().to_owned(),
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
                state_identity: daemon.state_identity().to_owned(),
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
                state_identity: daemon.state_identity().to_owned(),
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
                state_identity: daemon.state_identity().to_owned(),
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
                state_identity: daemon.state_identity().to_owned(),
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
                state_identity: daemon.state_identity().to_owned(),
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

    #[tokio::test]
    async fn handshake_refuses_a_client_bound_to_a_different_database() {
        let store = SqliteStore::in_memory().await.unwrap();
        let other_store = SqliteStore::in_memory().await.unwrap();
        let identity = store.store_identity().await.unwrap();
        let other_identity = other_store.store_identity().await.unwrap();
        assert_ne!(
            identity, other_identity,
            "two databases must never share a durable state identity"
        );

        let daemon = DaemonServer::start(
            store,
            Relay::new(),
            "test-lease-identity",
            Duration::from_secs(60),
        )
        .await
        .unwrap();
        assert_eq!(daemon.state_identity(), identity);

        // A client bound to this daemon's own durable state is accepted.
        let accepted = daemon
            .handle_request(DaemonRequest::Handshake {
                client_name: "relay-tui".to_owned(),
                protocol_version: DAEMON_PROTOCOL_VERSION,
                state_identity: identity.clone(),
            })
            .await;
        match accepted {
            DaemonResponse::HandshakeAck {
                state_identity: acknowledged,
                ..
            } => assert_eq!(acknowledged, identity),
            other => panic!("expected a handshake ack, got {other:?}"),
        }

        // A client bound to a different database is refused, and the refusal names
        // both identities so a host can report it precisely.
        let refused = daemon
            .handle_request(DaemonRequest::Handshake {
                client_name: "relay-tui".to_owned(),
                protocol_version: DAEMON_PROTOCOL_VERSION,
                state_identity: other_identity.clone(),
            })
            .await;
        match refused {
            DaemonResponse::StateIdentityMismatch {
                client_identity,
                daemon_identity,
            } => {
                assert_eq!(client_identity, other_identity);
                assert_eq!(daemon_identity, identity);
            }
            other => panic!("expected a state identity mismatch, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn submission_uses_a_project_registered_after_daemon_start() {
        let store = SqliteStore::in_memory().await.unwrap();
        let config_dir = std::env::temp_dir().join(format!("relay-registry-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&config_dir).unwrap();
        let config_path = config_dir.join("config.toml");
        let repo_path = config_dir.join("repo");
        std::fs::create_dir_all(&repo_path).unwrap();

        // The daemon starts against the persisted trusted registry, which does not
        // know the project yet.
        let relay = Relay::load_from_config_path(&config_path).unwrap();
        let daemon =
            DaemonServer::start(store, relay, "test-lease-registry", Duration::from_secs(60))
                .await
                .unwrap();

        let handoff = |command_id: &str| DaemonRequest::SubmitCommand {
            command_id: command_id.to_owned(),
            command: DaemonCommand::SubmitHandoff {
                project_key: "test-proj".to_owned(),
                handoff_json: TEST_HANDOFF.to_owned(),
                submission_transport: "local-paste".to_owned(),
            },
            state_identity: daemon.state_identity().to_owned(),
        };

        let before = daemon
            .handle_request(handoff("cmd-before-registration"))
            .await;
        match before {
            DaemonResponse::CommandResult { status, .. } => {
                assert_eq!(status, CommandStatus::Failed)
            }
            other => panic!("expected an unregistered project to be rejected, got {other:?}"),
        }

        // The approved Add Project action persists the registration to the trusted
        // config the daemon reads.
        let mut registry = crate::ProjectRegistry::load(&config_path).unwrap();
        registry
            .register(crate::RegisterProject {
                key: "test-proj".to_owned(),
                name: None,
                local_path: repo_path,
                expected_remote: None,
                default_stack: None,
                github: None,
            })
            .unwrap();
        registry.save(&config_path).unwrap();

        // The same daemon instance, without a restart, now accepts the handoff.
        let after = daemon
            .handle_request(handoff("cmd-after-registration"))
            .await;
        match after {
            DaemonResponse::CommandResult { status, .. } => {
                assert_eq!(status, CommandStatus::Applied);
            }
            other => panic!("expected the registered project's handoff to apply, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_daemon_that_lost_its_lease_refuses_every_request() {
        let store = SqliteStore::in_memory().await.unwrap();
        let daemon = DaemonServer::start(
            store,
            test_relay(),
            "test-lease-lost",
            Duration::from_secs(60),
        )
        .await
        .unwrap();

        // While it holds the lease the daemon serves normally.
        assert!(matches!(
            daemon.handle_request(DaemonRequest::Ping).await,
            DaemonResponse::Pong
        ));

        daemon.mark_authority_lost("simulated successor took the lease");

        // Every request - liveness probes and mutations alike - fails closed, so a
        // stale daemon cannot keep mutating state a successor may already own.
        let responses = [
            daemon.handle_request(DaemonRequest::Ping).await,
            daemon.handle_request(DaemonRequest::GetSnapshot).await,
            daemon
                .handle_request(DaemonRequest::SubmitCommand {
                    command_id: "cmd-after-lease-loss".to_owned(),
                    command: DaemonCommand::SubmitHandoff {
                        project_key: "test-proj".to_owned(),
                        handoff_json: TEST_HANDOFF.to_owned(),
                        submission_transport: "local-paste".to_owned(),
                    },
                    state_identity: daemon.state_identity().to_owned(),
                })
                .await,
        ];
        for response in responses {
            match response {
                DaemonResponse::Error(message) => {
                    assert!(message.contains("no longer authoritative"), "{message}");
                }
                other => panic!("expected a fail-closed error, got {other:?}"),
            }
        }

        // The rejected command was refused before it was even recorded.
        assert!(
            daemon
                .store()
                .get_command("cmd-after-lease-loss")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn concurrent_replays_of_one_command_id_converge_on_one_result() {
        let store = SqliteStore::in_memory().await.unwrap();
        let daemon = DaemonServer::start(
            store,
            test_relay(),
            "test-lease-replay",
            Duration::from_secs(60),
        )
        .await
        .unwrap();

        // Seed a task in state `ready` by importing a handoff.
        let seeded = daemon
            .handle_request(DaemonRequest::SubmitCommand {
                command_id: "cmd-seed".to_owned(),
                command: DaemonCommand::SubmitHandoff {
                    project_key: "test-proj".to_owned(),
                    handoff_json: TEST_HANDOFF.to_owned(),
                    submission_transport: "local-paste".to_owned(),
                },
                state_identity: daemon.state_identity().to_owned(),
            })
            .await;
        assert!(matches!(
            seeded,
            DaemonResponse::CommandResult {
                status: CommandStatus::Applied,
                ..
            }
        ));

        let task_id = match daemon.handle_request(DaemonRequest::GetSnapshot).await {
            DaemonResponse::Snapshot(snapshot) => {
                assert_eq!(snapshot.tasks.len(), 1);
                assert_eq!(snapshot.tasks[0].state, "ready");
                snapshot.tasks[0].id.clone()
            }
            other => panic!("expected a snapshot, got {other:?}"),
        };

        let command = DaemonCommand::UpdateTaskState {
            task_id,
            expected_state: "ready".to_owned(),
            new_state: "running".to_owned(),
        };
        let replay = || DaemonRequest::SubmitCommand {
            command_id: "cmd-concurrent".to_owned(),
            command: command.clone(),
            state_identity: daemon.state_identity().to_owned(),
        };

        // Two concurrent submissions of the *same* command id.
        let (first, second) = tokio::join!(
            daemon.handle_request(replay()),
            daemon.handle_request(replay())
        );

        let mut applied = 0;
        let mut duplicate = 0;
        let mut reported = Vec::new();
        for response in [first, second] {
            match response {
                DaemonResponse::CommandResult {
                    status,
                    result_json,
                    ..
                } => {
                    match status {
                        CommandStatus::Applied => applied += 1,
                        CommandStatus::Duplicate => duplicate += 1,
                        other => panic!("unexpected terminal status {other:?}"),
                    }
                    reported.push(result_json);
                }
                other => panic!("expected a command result, got {other:?}"),
            }
        }

        assert_eq!(
            (applied, duplicate),
            (1, 1),
            "exactly one caller applies the command and the other observes its cached result"
        );
        assert_eq!(
            reported[0], reported[1],
            "a duplicate replay must report the committed result, not a raced one"
        );

        // One durable terminal outcome: the command applied once, and the task
        // transitioned exactly once.
        let row = daemon
            .store()
            .get_command("cmd-concurrent")
            .await
            .unwrap()
            .expect("the command must be durably recorded");
        assert_eq!(row.status, "applied");

        match daemon.handle_request(DaemonRequest::GetSnapshot).await {
            DaemonResponse::Snapshot(snapshot) => {
                assert_eq!(snapshot.tasks.len(), 1);
                assert_eq!(snapshot.tasks[0].state, "running");
            }
            other => panic!("expected a snapshot, got {other:?}"),
        }
    }

    /// R18-01: the command ledger is authority-scoped, like the semantic writes.
    /// A daemon that lost the lease must not create, claim, or complete command
    /// rows: otherwise a request admitted before a takeover could claim a command
    /// after the successor's one-time startup reconciliation had already scanned,
    /// leaving it stranded, and a late completion could overwrite the successor's
    /// terminal result. The successor must still resolve the same command id to
    /// exactly one truthful terminal state.
    #[tokio::test]
    async fn a_daemon_that_lost_the_lease_cannot_write_the_command_ledger() {
        let store = SqliteStore::in_memory().await.unwrap();
        let stale = DaemonServer::start(
            store.clone(),
            test_relay(),
            "test-lease-ledger",
            Duration::from_secs(60),
        )
        .await
        .unwrap();

        let request = |identity: &str| DaemonRequest::SubmitCommand {
            command_id: "cmd-stale-ledger".to_owned(),
            command: DaemonCommand::SubmitHandoff {
                project_key: "test-proj".to_owned(),
                handoff_json: TEST_HANDOFF.to_owned(),
                submission_transport: "local-paste".to_owned(),
            },
            state_identity: identity.to_owned(),
        };

        // A successor takes the lease. A different holder identity with a zero TTL
        // is the same stale-takeover trigger the relay-store fence test uses: it
        // moves the holder generation forward and increments the fencing token
        // exactly as a real takeover does. The stale daemon keeps its old snapshot
        // of the lease, so its fence is now behind the durable one.
        let successor = store
            .acquire_or_renew_daemon_lease(
                "test-lease-ledger",
                std::process::id() as i64 + 1,
                "successor-host",
                "successor-instance",
                Duration::ZERO,
            )
            .await
            .unwrap();
        assert_eq!(successor.fencing_token, stale.fencing_token() + 1);

        // The stale daemon cannot even record the command: the ledger write is
        // refused before it commits, so nothing is left for the successor to
        // reconcile or for a late completion to overwrite.
        let refused = stale.handle_request(request(stale.state_identity())).await;
        assert!(
            matches!(refused, DaemonResponse::Error(_)),
            "a daemon that lost the lease must not write the command ledger, got {refused:?}"
        );
        assert!(
            stale
                .store()
                .get_command("cmd-stale-ledger")
                .await
                .unwrap()
                .is_none(),
            "a refused ledger write must not be durably recorded"
        );

        // The successor owns the database now: it resolves the same command id
        // exactly once, through the same fenced ledger the daemon drives, and a
        // second completion cannot overwrite the result it recorded.
        let successor_fence = DaemonFence {
            lease_id: "test-lease-ledger".to_owned(),
            holder_pid: successor.holder_pid,
            holder_instance: successor.holder_instance.clone(),
            fencing_token: successor.fencing_token,
        };
        assert!(
            store
                .record_command_fenced("cmd-stale-ledger", "submit_handoff", "{}", &successor_fence)
                .await
                .unwrap(),
            "the lease owner must be able to record the command"
        );
        assert!(
            store
                .claim_command_fenced("cmd-stale-ledger", &successor_fence)
                .await
                .unwrap(),
            "the lease owner must be able to claim the command"
        );
        assert!(
            store
                .complete_command_fenced(
                    "cmd-stale-ledger",
                    "applied",
                    Some(r#"{"ok":true}"#),
                    &successor_fence
                )
                .await
                .unwrap(),
            "the lease owner must be able to record the terminal result"
        );
        assert!(
            !store
                .complete_command_fenced(
                    "cmd-stale-ledger",
                    "failed",
                    Some(r#"{"ok":false}"#),
                    &successor_fence
                )
                .await
                .unwrap(),
            "a second completion must not overwrite the terminal result"
        );

        let row = store
            .get_command("cmd-stale-ledger")
            .await
            .unwrap()
            .expect("the successor must have recorded the command");
        assert_eq!(row.status, "applied");
        assert_eq!(row.response_json.as_deref(), Some(r#"{"ok":true}"#));
        assert_eq!(
            store
                .count_commands_with_status("needs_reconciliation")
                .await
                .unwrap(),
            0,
            "no command may be left stuck by the takeover"
        );

        // A late request from the stale daemon - a replay of a command the
        // successor already resolved - is refused and cannot change the recorded
        // terminal result.
        let late = stale.handle_request(request(stale.state_identity())).await;
        assert!(
            matches!(late, DaemonResponse::Error(_)),
            "a stale daemon must not touch a command the successor resolved, got {late:?}"
        );
        assert_eq!(
            store
                .get_command("cmd-stale-ledger")
                .await
                .unwrap()
                .expect("the successor's terminal result must still be recorded")
                .status,
            "applied"
        );
    }

    /// R17-01: a mutating request must name the database it means to write; a
    /// caller bound to another database's identity is refused before anything is
    /// applied, even if it bypasses the connection wrapper entirely.
    #[tokio::test]
    async fn submit_command_requires_the_matching_database_identity() {
        let store = SqliteStore::in_memory().await.unwrap();
        let daemon = DaemonServer::start(
            store,
            test_relay(),
            "test-lease-identity",
            Duration::from_secs(60),
        )
        .await
        .unwrap();

        let response = daemon
            .handle_request(DaemonRequest::SubmitCommand {
                command_id: "cmd-foreign-identity".to_owned(),
                command: DaemonCommand::SubmitHandoff {
                    project_key: "test-proj".to_owned(),
                    handoff_json: TEST_HANDOFF.to_owned(),
                    submission_transport: "local-paste".to_owned(),
                },
                state_identity: "some-other-database".to_owned(),
            })
            .await;

        match response {
            DaemonResponse::StateIdentityMismatch {
                client_identity,
                daemon_identity,
            } => {
                assert_eq!(client_identity, "some-other-database");
                assert_eq!(daemon_identity, daemon.state_identity());
            }
            other => panic!("expected an identity mismatch, got {other:?}"),
        }

        // Fail closed means *no* durable effect: nothing was recorded and nothing
        // was imported.
        assert!(
            daemon
                .store()
                .get_command("cmd-foreign-identity")
                .await
                .unwrap()
                .is_none(),
            "a refused command must not be recorded"
        );
        match daemon.handle_request(DaemonRequest::GetSnapshot).await {
            DaemonResponse::Snapshot(snapshot) => assert!(snapshot.tasks.is_empty()),
            other => panic!("expected a snapshot, got {other:?}"),
        }
    }

    /// R17-04: the compare-and-set effect and the command's terminal record are
    /// separate durable writes. When the effect commits but the terminal record
    /// does not, replaying the command must converge on `applied` (the transition
    /// is already present) rather than durably recording a false `failed`.
    #[tokio::test]
    async fn replaying_a_committed_state_transition_reports_applied_not_failed() {
        let store = SqliteStore::in_memory().await.unwrap();
        let daemon = DaemonServer::start(
            store,
            test_relay(),
            "test-lease-replay",
            Duration::from_secs(60),
        )
        .await
        .unwrap();

        let seeded = daemon
            .handle_request(DaemonRequest::SubmitCommand {
                command_id: "cmd-replay-seed".to_owned(),
                command: DaemonCommand::SubmitHandoff {
                    project_key: "test-proj".to_owned(),
                    handoff_json: TEST_HANDOFF.to_owned(),
                    submission_transport: "local-paste".to_owned(),
                },
                state_identity: daemon.state_identity().to_owned(),
            })
            .await;
        assert!(matches!(
            seeded,
            DaemonResponse::CommandResult {
                status: CommandStatus::Applied,
                ..
            }
        ));

        let task_id = match daemon.handle_request(DaemonRequest::GetSnapshot).await {
            DaemonResponse::Snapshot(snapshot) => {
                assert_eq!(snapshot.tasks.len(), 1);
                snapshot.tasks[0].id.clone()
            }
            other => panic!("expected a snapshot, got {other:?}"),
        };

        let transition = DaemonRequest::SubmitCommand {
            command_id: "cmd-replay-transition".to_owned(),
            command: DaemonCommand::UpdateTaskState {
                task_id: task_id.clone(),
                expected_state: "ready".to_owned(),
                new_state: "running".to_owned(),
            },
            state_identity: daemon.state_identity().to_owned(),
        };

        let first = match daemon.handle_request(transition.clone()).await {
            DaemonResponse::CommandResult { status, .. } => status,
            other => panic!("expected a command result, got {other:?}"),
        };
        assert_eq!(first, CommandStatus::Applied);

        // Simulate the crash window: the transition is durably committed, but the
        // command's terminal row was never written and its claim was reset by
        // startup reconciliation.
        daemon
            .store()
            .complete_command("cmd-replay-transition", "received", None)
            .await
            .unwrap();
        daemon.store().reset_command_claims().await.unwrap();

        let replay = match daemon.handle_request(transition).await {
            DaemonResponse::CommandResult { status, .. } => status,
            other => panic!("expected a command result, got {other:?}"),
        };
        assert_eq!(
            replay,
            CommandStatus::Applied,
            "replaying a transition whose effect already committed must report applied, not a false failed"
        );

        match daemon.handle_request(DaemonRequest::GetSnapshot).await {
            DaemonResponse::Snapshot(snapshot) => {
                assert_eq!(snapshot.tasks.len(), 1);
                assert_eq!(snapshot.tasks[0].state, "running");
            }
            other => panic!("expected a snapshot, got {other:?}"),
        }
    }

    /// R17-04: a cached `failed` command must stay a failure on replay. Reporting
    /// it as `Duplicate` would let a caller that reuses a command id read a failed
    /// command as a successful one.
    #[tokio::test]
    async fn a_failed_command_replays_as_failed_not_duplicate() {
        let store = SqliteStore::in_memory().await.unwrap();
        let daemon = DaemonServer::start(
            store,
            test_relay(),
            "test-lease-failed",
            Duration::from_secs(60),
        )
        .await
        .unwrap();

        let seeded = daemon
            .handle_request(DaemonRequest::SubmitCommand {
                command_id: "cmd-failed-seed".to_owned(),
                command: DaemonCommand::SubmitHandoff {
                    project_key: "test-proj".to_owned(),
                    handoff_json: TEST_HANDOFF.to_owned(),
                    submission_transport: "local-paste".to_owned(),
                },
                state_identity: daemon.state_identity().to_owned(),
            })
            .await;
        let task_id = match seeded {
            DaemonResponse::CommandResult { .. } => {
                match daemon.handle_request(DaemonRequest::GetSnapshot).await {
                    DaemonResponse::Snapshot(snapshot) => snapshot.tasks[0].id.clone(),
                    other => panic!("expected a snapshot, got {other:?}"),
                }
            }
            other => panic!("expected a command result, got {other:?}"),
        };

        // The task is `ready`; asking for `running -> done` cannot apply.
        let failing = DaemonRequest::SubmitCommand {
            command_id: "cmd-failed-transition".to_owned(),
            command: DaemonCommand::UpdateTaskState {
                task_id,
                expected_state: "running".to_owned(),
                new_state: "done".to_owned(),
            },
            state_identity: daemon.state_identity().to_owned(),
        };

        let first = match daemon.handle_request(failing.clone()).await {
            DaemonResponse::CommandResult { status, .. } => status,
            other => panic!("expected a command result, got {other:?}"),
        };
        assert_eq!(first, CommandStatus::Failed);

        let replay = match daemon.handle_request(failing).await {
            DaemonResponse::CommandResult { status, .. } => status,
            other => panic!("expected a command result, got {other:?}"),
        };
        assert_eq!(
            replay,
            CommandStatus::Failed,
            "a failed command must not be reported as a successful duplicate"
        );

        let row = daemon
            .store()
            .get_command("cmd-failed-transition")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.status, "failed");
    }
}
