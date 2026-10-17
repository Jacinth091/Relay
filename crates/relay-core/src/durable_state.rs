use std::fmt;

use relay_protocol::{HandoffTask, parse_handoff_task_json};
use relay_store::{
    NewResourceLeaseRecord, NewTaskRecord, ResourceLeaseRecord, StoreError, TaskRecord,
};
pub use relay_store::{SqliteStore, default_database_path};
use thiserror::Error;

use crate::{
    CheckoutId, ImportedTask, Lease, LeaseHolder, LeaseId, LeaseKind, LeaseResource, LeaseState,
    ProjectKey, Relay, SubmissionTransport, TaskId, TaskImportCandidate, WorkflowRunId,
};

/// Durable Relay task state stored locally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Received,
    Ready,
    Running,
    Verifying,
    Repairing,
    NeedsClarification,
    NeedsDecision,
    Resolved,
    Done,
    Failed,
    Cancelled,
}

impl TaskState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Received => "received",
            Self::Ready => "ready",
            Self::Running => "running",
            Self::Verifying => "verifying",
            Self::Repairing => "repairing",
            Self::NeedsClarification => "needs_clarification",
            Self::NeedsDecision => "needs_decision",
            Self::Resolved => "resolved",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    fn parse(input: &str) -> Result<Self, TaskPersistenceError> {
        match input {
            "received" => Ok(Self::Received),
            "ready" => Ok(Self::Ready),
            "running" => Ok(Self::Running),
            "verifying" => Ok(Self::Verifying),
            "repairing" => Ok(Self::Repairing),
            "needs_clarification" => Ok(Self::NeedsClarification),
            "needs_decision" => Ok(Self::NeedsDecision),
            "resolved" => Ok(Self::Resolved),
            "done" => Ok(Self::Done),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            state => Err(TaskPersistenceError::UnknownTaskState {
                state: state.to_owned(),
            }),
        }
    }
}

impl fmt::Display for TaskState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A canonical task record loaded from durable state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredTask {
    pub id: TaskId,
    pub handoff: HandoffTask,
    pub project_key: ProjectKey,
    pub submission_transport: SubmissionTransport,
    pub state: TaskState,
    pub created_at: String,
    pub updated_at: String,
}

pub(crate) async fn persist_imported_task(
    store: &SqliteStore,
    task: &ImportedTask,
) -> Result<StoredTask, TaskPersistenceError> {
    record_to_stored_task(store.insert_task(&new_task_record(task)?).await?)
}

/// Persists an imported task idempotently: replaying the same task id (e.g.
/// a daemon command retried after a crash) returns the existing row instead
/// of erroring or duplicating the `task_ready` event.
///
/// Fenced on the daemon lease, because the only caller is the daemon's handoff
/// submission path: a submission admitted before authority was lost must not
/// land after a successor took the lease.
pub(crate) async fn persist_imported_task_if_absent(
    store: &SqliteStore,
    task: &ImportedTask,
    fence: &relay_store::DaemonFence,
) -> Result<StoredTask, TaskPersistenceError> {
    record_to_stored_task(
        store
            .insert_task_if_absent_fenced(&new_task_record(task)?, fence)
            .await?,
    )
}

fn new_task_record(task: &ImportedTask) -> Result<NewTaskRecord, TaskPersistenceError> {
    let handoff = task.handoff();
    let source = handoff.source.as_ref();
    Ok(NewTaskRecord {
        id: task.id().to_string(),
        project_key: task.project_key().as_str().to_owned(),
        title: handoff.title.clone(),
        requested_stack: handoff.stack.clone(),
        submission_transport: task.submission_transport().as_str().to_owned(),
        source_kind: source.map(|source| source.kind.clone()),
        source_provider: source.map(|source| source.provider.clone()),
        state: TaskState::Ready.as_str().to_owned(),
        protocol_version: i64::from(handoff.protocol_version),
        handoff_json: serde_json::to_string(handoff)?,
    })
}

pub async fn get_stored_task(
    store: &SqliteStore,
    id: TaskId,
) -> Result<Option<StoredTask>, TaskPersistenceError> {
    store
        .get_task(&id.to_string())
        .await?
        .map(record_to_stored_task)
        .transpose()
}

pub async fn list_stored_tasks(
    store: &SqliteStore,
) -> Result<Vec<StoredTask>, TaskPersistenceError> {
    Ok(list_stored_tasks_with_quarantined(store).await?.0)
}

/// Lists stored tasks, quarantining records that fail revalidation.
///
/// A single corrupt row must not hide every task from the user, so rows that
/// cannot be revalidated are skipped instead of failing the whole listing.
/// The number of quarantined records is returned alongside the parsed tasks
/// so callers can surface partial corruption.
pub async fn list_stored_tasks_with_quarantined(
    store: &SqliteStore,
) -> Result<(Vec<StoredTask>, usize), TaskPersistenceError> {
    let mut tasks = Vec::new();
    let mut quarantined = 0usize;

    for record in store.list_tasks().await? {
        match record_to_stored_task(record) {
            Ok(task) => tasks.push(task),
            Err(_) => quarantined += 1,
        }
    }

    Ok((tasks, quarantined))
}

pub async fn update_stored_task_state(
    store: &SqliteStore,
    id: TaskId,
    expected_state: TaskState,
    new_state: TaskState,
) -> Result<StoredTask, TaskPersistenceError> {
    record_to_stored_task(
        store
            .update_task_state(&id.to_string(), expected_state.as_str(), new_state.as_str())
            .await?,
    )
}

impl Relay {
    pub async fn confirm_task_import_with_store(
        &mut self,
        store: &SqliteStore,
        candidate: TaskImportCandidate,
    ) -> Result<StoredTask, TaskPersistenceError> {
        let imported = candidate.into_imported_task();
        let stored = persist_imported_task(store, &imported).await?;
        self.accept_imported_task(imported);
        Ok(stored)
    }
}

fn record_to_stored_task(record: TaskRecord) -> Result<StoredTask, TaskPersistenceError> {
    let handoff = parse_handoff_task_json(&record.handoff_json)?;
    let stored_project_key = ProjectKey::parse(&record.project_key)?;
    let handoff_project_key = ProjectKey::parse(&handoff.project)?;
    if stored_project_key != handoff_project_key {
        return Err(TaskPersistenceError::StoredProjectMismatch {
            id: record.id,
            stored_project_key,
            handoff_project_key,
        });
    }
    if record.title != handoff.title {
        return Err(TaskPersistenceError::StoredTitleMismatch {
            id: record.id,
            stored_title: record.title,
            handoff_title: handoff.title,
        });
    }

    Ok(StoredTask {
        id: TaskId::parse(&record.id).map_err(|source| TaskPersistenceError::InvalidTaskId {
            input: record.id,
            source,
        })?,
        handoff,
        project_key: stored_project_key,
        submission_transport: parse_submission_transport(&record.submission_transport)?,
        state: TaskState::parse(&record.state)?,
        created_at: record.created_at,
        updated_at: record.updated_at,
    })
}

fn parse_submission_transport(input: &str) -> Result<SubmissionTransport, TaskPersistenceError> {
    match input {
        "local-file" => Ok(SubmissionTransport::LocalFile),
        "local-paste" => Ok(SubmissionTransport::LocalPaste),
        "relay-mcp" => Ok(SubmissionTransport::RelayMcp),
        transport => Err(TaskPersistenceError::UnknownSubmissionTransport {
            transport: transport.to_owned(),
        }),
    }
}

#[derive(Debug, Error)]
pub enum TaskPersistenceError {
    #[error("{0}")]
    Store(#[from] StoreError),
    #[error("failed to serialize or parse task payload: {0}")]
    TaskJson(#[from] serde_json::Error),
    #[error("{0}")]
    Protocol(#[from] relay_protocol::ProtocolError),
    #[error("invalid stored task id '{input}': {source}")]
    InvalidTaskId { input: String, source: uuid::Error },
    #[error("unknown stored submission transport '{transport}'")]
    UnknownSubmissionTransport { transport: String },
    #[error("unknown stored task state '{state}'")]
    UnknownTaskState { state: String },
    #[error(
        "stored task {id} project key '{stored_project_key}' does not match handoff project '{handoff_project_key}'"
    )]
    StoredProjectMismatch {
        id: String,
        stored_project_key: ProjectKey,
        handoff_project_key: ProjectKey,
    },
    #[error(
        "stored task {id} title '{stored_title}' does not match handoff title '{handoff_title}'"
    )]
    StoredTitleMismatch {
        id: String,
        stored_title: String,
        handoff_title: String,
    },
    #[error("{0}")]
    ProjectRegistry(#[from] crate::ProjectRegistryError),
    #[error("{0}")]
    ProjectIdentity(#[from] crate::project_identity::ProjectIdentityError),
    #[error("unknown stored lease resource type '{resource_type}'")]
    UnknownLeaseResourceType { resource_type: String },
    #[error("unknown stored lease kind '{lease_kind}'")]
    UnknownLeaseKind { lease_kind: String },
    #[error("unknown stored lease state '{state}'")]
    UnknownLeaseState { state: String },
}

pub fn record_to_lease(record: ResourceLeaseRecord) -> Result<Lease, TaskPersistenceError> {
    let resource = match record.resource_type.as_str() {
        "checkout" => LeaseResource::Checkout(CheckoutId::parse(&record.resource_id)?),
        "task" => LeaseResource::Task(TaskId::parse(&record.resource_id).map_err(|source| {
            TaskPersistenceError::InvalidTaskId {
                input: record.resource_id,
                source,
            }
        })?),
        "workflow_run" => LeaseResource::WorkflowRun(WorkflowRunId(record.resource_id)),
        other => {
            return Err(TaskPersistenceError::UnknownLeaseResourceType {
                resource_type: other.to_string(),
            });
        }
    };

    let kind = match record.lease_kind.as_str() {
        "exclusive_write" => LeaseKind::ExclusiveWrite,
        "shared_read" => LeaseKind::SharedRead,
        "verification" => LeaseKind::Verification,
        other => {
            return Err(TaskPersistenceError::UnknownLeaseKind {
                lease_kind: other.to_string(),
            });
        }
    };

    let state = match record.state.as_str() {
        "active" => LeaseState::Active,
        "expired" => LeaseState::Expired,
        "superseded" => LeaseState::Superseded,
        "released" => LeaseState::Released,
        other => {
            return Err(TaskPersistenceError::UnknownLeaseState {
                state: other.to_string(),
            });
        }
    };

    Ok(Lease {
        id: LeaseId::new(record.id),
        resource,
        kind,
        holder: LeaseHolder {
            holder_id: record.holder_id,
            pid: record.holder_pid as u32,
            host: record.holder_host,
        },
        fencing_token: record.fencing_token as u64,
        acquired_at: record.acquired_at,
        expires_at: record.expires_at,
        state,
    })
}

pub async fn acquire_durable_lease(
    store: &SqliteStore,
    id: &LeaseId,
    resource: &LeaseResource,
    kind: &LeaseKind,
    holder: &LeaseHolder,
    ttl_seconds: u64,
) -> Result<Lease, TaskPersistenceError> {
    let (resource_type, resource_id) = match resource {
        LeaseResource::Checkout(chk) => ("checkout".to_string(), chk.to_string()),
        LeaseResource::Task(tsk) => ("task".to_string(), tsk.to_string()),
        LeaseResource::WorkflowRun(wfr) => ("workflow_run".to_string(), wfr.to_string()),
    };

    let lease_kind_str = match kind {
        LeaseKind::ExclusiveWrite => "exclusive_write".to_string(),
        LeaseKind::SharedRead => "shared_read".to_string(),
        LeaseKind::Verification => "verification".to_string(),
    };

    // Canonical durable-lease expiry representation: Unix epoch seconds as text.
    // The store compares expiry as an integer against the current epoch second
    // (`CAST(expires_at AS INTEGER)`), and `Lease::assert_valid` parses this field
    // as epoch seconds. Only this wrapper writes the column; any other
    // representation (for example an ISO date string) is read as that string's
    // leading integer and would appear already expired, not "tolerated". If this
    // changes, both the store comparison and `assert_valid` must change with it.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let expires_at = (now + ttl_seconds).to_string();

    let new_rec = NewResourceLeaseRecord {
        id: id.to_string(),
        resource_type,
        resource_id,
        lease_kind: lease_kind_str,
        holder_id: holder.holder_id.clone(),
        holder_pid: holder.pid as i64,
        holder_host: holder.host.clone(),
        expires_at,
    };

    let record = store.acquire_resource_lease(&new_rec).await?;
    record_to_lease(record)
}

pub async fn release_durable_lease(
    store: &SqliteStore,
    lease_id: &LeaseId,
    holder_id: &str,
) -> Result<(), TaskPersistenceError> {
    store
        .release_resource_lease(&lease_id.to_string(), holder_id)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CandidateError, ProjectRegistry, RegisterProject};
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    const TASK_JSON: &str = r#"{
        "protocol_version": 1,
        "project": "Relay",
        "title": "Persist imported task",
        "stack": "chatgpt-codex",
        "source": {
            "kind": "conversational_ai",
            "provider": "openai"
        },
        "task": {
            "goal": "Persist accepted local task state.",
            "requirements": ["Store canonical payload."],
            "constraints": ["Do not execute."],
            "verification": ["Reload from SQLite."]
        },
        "ambiguity_policy": "needs_clarification"
    }"#;

    #[tokio::test]
    async fn durable_lease_expiry_is_reclaimed_once_ttl_elapses() {
        // Regression for PR #16 review finding R16-01: this wrapper writes
        // `expires_at` as Unix epoch seconds, but the store evaluated expiry as
        // SQLite date text, so an elapsed durable lease was never detected and a
        // crashed holder permanently pinned the resource.
        let store = SqliteStore::in_memory().await.unwrap();
        let resource = LeaseResource::WorkflowRun(WorkflowRunId("wfr-r16-01".to_owned()));
        let kind = LeaseKind::ExclusiveWrite;
        let holder_one = LeaseHolder {
            holder_id: "holder-one".to_string(),
            pid: std::process::id(),
            host: "localhost".to_string(),
        };
        let holder_two = LeaseHolder {
            holder_id: "holder-two".to_string(),
            pid: std::process::id(),
            host: "localhost".to_string(),
        };

        // ttl_seconds = 0 => already expired when the next holder asks for it.
        let first = acquire_durable_lease(
            &store,
            &LeaseId::new("lease-r16-01-a"),
            &resource,
            &kind,
            &holder_one,
            0,
        )
        .await
        .unwrap();
        assert_eq!(first.fencing_token, 1);
        assert_eq!(first.state, LeaseState::Active);
        assert!(
            first.expires_at.parse::<u64>().is_ok(),
            "durable lease expiry must be canonical Unix epoch seconds, got {}",
            first.expires_at
        );

        // A competing holder reclaims the elapsed lease with a monotonic token.
        let second = acquire_durable_lease(
            &store,
            &LeaseId::new("lease-r16-01-b"),
            &resource,
            &kind,
            &holder_two,
            3600,
        )
        .await
        .unwrap();
        assert_eq!(second.fencing_token, 2, "fencing token must stay monotonic");
        assert_eq!(second.state, LeaseState::Active);

        // The elapsed lease no longer validates; the reclaimed one does.
        assert!(matches!(
            first.assert_valid(first.fencing_token),
            Err(CandidateError::LeaseExpired { .. })
        ));
        assert!(second.assert_valid(second.fencing_token).is_ok());
    }

    #[tokio::test]
    async fn durable_lease_with_live_ttl_still_blocks_competing_holder() {
        let store = SqliteStore::in_memory().await.unwrap();
        let resource = LeaseResource::WorkflowRun(WorkflowRunId("wfr-r16-02".to_owned()));
        let kind = LeaseKind::ExclusiveWrite;
        let holder_one = LeaseHolder {
            holder_id: "holder-one".to_string(),
            pid: std::process::id(),
            host: "localhost".to_string(),
        };
        let holder_two = LeaseHolder {
            holder_id: "holder-two".to_string(),
            pid: std::process::id(),
            host: "localhost".to_string(),
        };

        acquire_durable_lease(
            &store,
            &LeaseId::new("lease-r16-02-a"),
            &resource,
            &kind,
            &holder_one,
            3600,
        )
        .await
        .unwrap();

        let err = acquire_durable_lease(
            &store,
            &LeaseId::new("lease-r16-02-b"),
            &resource,
            &kind,
            &holder_two,
            3600,
        )
        .await
        .unwrap_err();
        assert!(matches!(
            err,
            TaskPersistenceError::Store(StoreError::ResourceLeaseHeld { .. })
        ));
    }

    #[tokio::test]
    async fn stores_imported_task_as_canonical_ready_state() {
        let store = SqliteStore::in_memory().await.unwrap();
        let mut relay = relay_with_project("relay");
        let candidate = relay.preview_local_paste(TASK_JSON).unwrap();
        let imported = relay.confirm_task_import(candidate).clone();

        let stored = persist_imported_task(&store, &imported).await.unwrap();

        assert_eq!(stored.id, imported.id());
        assert_eq!(stored.state, TaskState::Ready);
        assert_eq!(stored.project_key.as_str(), "relay");
        assert_eq!(&stored.handoff, imported.handoff());
        assert_eq!(stored.submission_transport, SubmissionTransport::LocalPaste);
    }

    #[tokio::test]
    async fn persisted_tasks_survive_store_reopen() {
        let database_path = temp_path("relay-state.sqlite3");
        let mut relay = relay_with_project("relay");
        let candidate = relay.preview_local_paste(TASK_JSON).unwrap();
        let imported = relay.confirm_task_import(candidate).clone();

        {
            let store = SqliteStore::open(&database_path).await.unwrap();
            persist_imported_task(&store, &imported).await.unwrap();
        }

        let reopened = SqliteStore::open(&database_path).await.unwrap();
        let loaded = get_stored_task(&reopened, imported.id())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(loaded.id, imported.id());
        assert_eq!(loaded.handoff.title, "Persist imported task");
        assert_eq!(loaded.state, TaskState::Ready);
    }

    #[tokio::test]
    async fn lists_tasks_in_durable_order() {
        let store = SqliteStore::in_memory().await.unwrap();
        let mut relay = relay_with_project("relay");

        for title in ["First task", "Second task"] {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            let json = TASK_JSON.replace("Persist imported task", title);
            let candidate = relay.preview_local_paste(&json).unwrap();
            let imported = relay.confirm_task_import(candidate).clone();
            persist_imported_task(&store, &imported).await.unwrap();
        }

        let titles = list_stored_tasks(&store)
            .await
            .unwrap()
            .into_iter()
            .map(|task| task.handoff.title)
            .collect::<Vec<_>>();

        assert_eq!(titles, ["First task", "Second task"]);
    }

    #[tokio::test]
    async fn import_confirmation_persists_before_review_state_updates() {
        let store = SqliteStore::in_memory().await.unwrap();
        let mut relay = relay_with_project("relay");
        let candidate = relay.preview_local_paste(TASK_JSON).unwrap();

        let stored = relay
            .confirm_task_import_with_store(&store, candidate)
            .await
            .unwrap();

        assert_eq!(stored.state, TaskState::Ready);
        assert_eq!(relay.reviewed_task().unwrap().id(), stored.id);
        assert!(get_stored_task(&store, stored.id).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn updates_task_state_through_typed_core_adapter() {
        let store = SqliteStore::in_memory().await.unwrap();
        let mut relay = relay_with_project("relay");
        let candidate = relay.preview_local_paste(TASK_JSON).unwrap();
        let imported = relay.confirm_task_import(candidate).clone();
        persist_imported_task(&store, &imported).await.unwrap();

        let updated =
            update_stored_task_state(&store, imported.id(), TaskState::Ready, TaskState::Done)
                .await
                .unwrap();

        assert_eq!(updated.state, TaskState::Done);
        assert_eq!(updated.id, imported.id());
    }

    #[tokio::test]
    async fn stale_task_state_update_is_rejected_by_typed_adapter() {
        let store = SqliteStore::in_memory().await.unwrap();
        let mut relay = relay_with_project("relay");
        let candidate = relay.preview_local_paste(TASK_JSON).unwrap();
        let imported = relay.confirm_task_import(candidate).clone();
        persist_imported_task(&store, &imported).await.unwrap();

        let error =
            update_stored_task_state(&store, imported.id(), TaskState::Running, TaskState::Done)
                .await
                .unwrap_err();
        let loaded = get_stored_task(&store, imported.id())
            .await
            .unwrap()
            .unwrap();

        assert!(error.to_string().contains("state conflict"));
        assert_eq!(loaded.state, TaskState::Ready);
    }

    #[tokio::test]
    async fn mcp_submission_persists_with_relay_mcp_transport_and_round_trips() {
        let store = SqliteStore::in_memory().await.unwrap();
        let mut relay = relay_with_project("relay");
        let candidate = relay.preview_mcp_submission(TASK_JSON).unwrap();

        let stored = relay
            .confirm_task_import_with_store(&store, candidate)
            .await
            .unwrap();

        assert_eq!(stored.submission_transport, SubmissionTransport::RelayMcp);
        assert_eq!(stored.state, TaskState::Ready);

        let loaded = get_stored_task(&store, stored.id).await.unwrap().unwrap();
        assert_eq!(loaded.submission_transport, SubmissionTransport::RelayMcp);
    }

    #[test]
    fn unknown_stored_state_is_rejected() {
        let record = task_record(
            "550e8400-e29b-41d4-a716-446655440000",
            "Persist imported task",
            "ready-ish",
            TASK_JSON,
        );

        let error = record_to_stored_task(record).unwrap_err();

        assert!(matches!(
            error,
            TaskPersistenceError::UnknownTaskState { state } if state == "ready-ish"
        ));
    }

    #[tokio::test]
    async fn listing_skips_corrupt_row_and_returns_good_row() {
        let store = SqliteStore::in_memory().await.unwrap();
        let mut relay = relay_with_project("relay");
        let candidate = relay.preview_local_paste(TASK_JSON).unwrap();
        let imported = relay.confirm_task_import(candidate).clone();
        let good = persist_imported_task(&store, &imported).await.unwrap();

        let corrupt = NewTaskRecord {
            id: "corrupt-json".to_owned(),
            project_key: "relay".to_owned(),
            title: "Corrupt JSON".to_owned(),
            requested_stack: None,
            submission_transport: "local-paste".to_owned(),
            source_kind: None,
            source_provider: None,
            state: "ready".to_owned(),
            protocol_version: 1,
            handoff_json: "{not valid json".to_owned(),
        };
        store.insert_task(&corrupt).await.unwrap();

        let tasks = list_stored_tasks(&store).await.unwrap();
        let (tasks_with_count, quarantined) =
            list_stored_tasks_with_quarantined(&store).await.unwrap();

        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, good.id);
        assert_eq!(tasks_with_count.len(), 1);
        assert_eq!(tasks_with_count[0].id, good.id);
        assert_eq!(quarantined, 1);
    }

    #[test]
    fn stored_handoff_json_is_revalidated_on_load() {
        let record = task_record(
            "550e8400-e29b-41d4-a716-446655440000",
            "Bad JSON",
            "ready",
            r#"{
                "protocol_version": 2,
                "project": "relay",
                "title": "Bad JSON",
                "task": {"goal": "Invalid protocol."},
                "ambiguity_policy": "needs_clarification"
            }"#,
        );

        let error = record_to_stored_task(record).unwrap_err();

        assert!(error.to_string().contains("unsupported protocol version 2"));
    }

    #[test]
    fn stored_handoff_with_empty_required_field_is_rejected_on_load() {
        let record = task_record(
            "550e8400-e29b-41d4-a716-446655440001",
            "Stored title",
            "ready",
            r#"{
                "protocol_version": 1,
                "project": "relay",
                "title": "",
                "task": {"goal": "Invalid empty title."},
                "ambiguity_policy": "needs_clarification"
            }"#,
        );

        let error = record_to_stored_task(record).unwrap_err();

        assert!(error.to_string().contains("title cannot be empty"));
    }

    #[test]
    fn stored_project_key_must_match_handoff_project() {
        let record = task_record(
            "550e8400-e29b-41d4-a716-446655440002",
            "Mismatch",
            "ready",
            r#"{
                "protocol_version": 1,
                "project": "other",
                "title": "Mismatch",
                "task": {"goal": "Mismatch."},
                "ambiguity_policy": "needs_clarification"
            }"#,
        );

        let error = record_to_stored_task(record).unwrap_err();

        assert!(error.to_string().contains("does not match handoff project"));
    }

    #[test]
    fn stored_title_must_match_handoff_title() {
        let record = task_record(
            "550e8400-e29b-41d4-a716-446655440003",
            "Stored title",
            "ready",
            r#"{
                "protocol_version": 1,
                "project": "relay",
                "title": "Canonical title",
                "task": {"goal": "Mismatch."},
                "ambiguity_policy": "needs_clarification"
            }"#,
        );

        let error = record_to_stored_task(record).unwrap_err();

        assert!(error.to_string().contains("does not match handoff title"));
    }

    fn task_record(id: &str, title: &str, state: &str, handoff_json: &str) -> TaskRecord {
        TaskRecord {
            id: id.to_owned(),
            title: title.to_owned(),
            handoff_json: handoff_json.to_owned(),
            project_key: "relay".to_owned(),
            submission_transport: "local-paste".to_owned(),
            state: state.to_owned(),
            created_at: "2026-08-14T00:00:00.000Z".to_owned(),
            updated_at: "2026-08-14T00:00:00.000Z".to_owned(),
        }
    }

    fn relay_with_project(key: &str) -> Relay {
        let dir = temp_dir("project");
        let mut registry = ProjectRegistry::empty();
        registry
            .register(RegisterProject {
                key: key.to_owned(),
                name: Some("Relay".to_owned()),
                local_path: dir,
                expected_remote: None,
                default_stack: Some("chatgpt-codex".to_owned()),
                github: None,
            })
            .unwrap();
        Relay::with_project_registry(registry)
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let path = temp_path(name);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn temp_path(name: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("relay-core-durable-state-{name}-{nanos}"))
    }
}
