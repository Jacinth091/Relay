use std::fmt;

use relay_protocol::{HandoffTask, parse_handoff_task_json};
use relay_store::{NewTaskRecord, StoreError, TaskRecord};
pub use relay_store::{SqliteStore, default_database_path};
use thiserror::Error;

use crate::{ImportedTask, ProjectKey, Relay, SubmissionTransport, TaskId, TaskImportCandidate};

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
    let handoff = task.handoff();
    let source = handoff.source.as_ref();
    let record = NewTaskRecord {
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
    };

    record_to_stored_task(store.insert_task(&record).await?)
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
    store
        .list_tasks()
        .await?
        .into_iter()
        .map(record_to_stored_task)
        .collect()
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ProjectRegistry, RegisterProject};
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
    async fn stored_handoff_json_is_revalidated_on_load() {
        let store = SqliteStore::in_memory().await.unwrap();
        let record = NewTaskRecord {
            id: "bad-json".to_owned(),
            project_key: "relay".to_owned(),
            title: "Bad JSON".to_owned(),
            requested_stack: None,
            submission_transport: "local-paste".to_owned(),
            source_kind: None,
            source_provider: None,
            state: "ready".to_owned(),
            protocol_version: 1,
            handoff_json: r#"{
                "protocol_version": 2,
                "project": "relay",
                "title": "Bad JSON",
                "task": {"goal": "Invalid protocol."},
                "ambiguity_policy": "needs_clarification"
            }"#
            .to_owned(),
        };
        store.insert_task(&record).await.unwrap();

        let error = list_stored_tasks(&store).await.unwrap_err();

        assert!(error.to_string().contains("unsupported protocol version 2"));
    }

    #[tokio::test]
    async fn stored_handoff_with_empty_required_field_is_rejected_on_load() {
        let store = SqliteStore::in_memory().await.unwrap();
        let record = NewTaskRecord {
            id: "empty-title".to_owned(),
            project_key: "relay".to_owned(),
            title: "Stored title".to_owned(),
            requested_stack: None,
            submission_transport: "local-paste".to_owned(),
            source_kind: None,
            source_provider: None,
            state: "ready".to_owned(),
            protocol_version: 1,
            handoff_json: r#"{
                "protocol_version": 1,
                "project": "relay",
                "title": "",
                "task": {"goal": "Invalid empty title."},
                "ambiguity_policy": "needs_clarification"
            }"#
            .to_owned(),
        };
        store.insert_task(&record).await.unwrap();

        let error = list_stored_tasks(&store).await.unwrap_err();

        assert!(error.to_string().contains("title cannot be empty"));
    }

    #[tokio::test]
    async fn stored_project_key_must_match_handoff_project() {
        let store = SqliteStore::in_memory().await.unwrap();
        let record = NewTaskRecord {
            id: "project-mismatch".to_owned(),
            project_key: "relay".to_owned(),
            title: "Mismatch".to_owned(),
            requested_stack: None,
            submission_transport: "local-paste".to_owned(),
            source_kind: None,
            source_provider: None,
            state: "ready".to_owned(),
            protocol_version: 1,
            handoff_json: r#"{
                "protocol_version": 1,
                "project": "other",
                "title": "Mismatch",
                "task": {"goal": "Mismatch."},
                "ambiguity_policy": "needs_clarification"
            }"#
            .to_owned(),
        };
        store.insert_task(&record).await.unwrap();

        let error = list_stored_tasks(&store).await.unwrap_err();

        assert!(error.to_string().contains("does not match handoff project"));
    }

    #[tokio::test]
    async fn stored_title_must_match_handoff_title() {
        let store = SqliteStore::in_memory().await.unwrap();
        let record = NewTaskRecord {
            id: "title-mismatch".to_owned(),
            project_key: "relay".to_owned(),
            title: "Stored title".to_owned(),
            requested_stack: None,
            submission_transport: "local-paste".to_owned(),
            source_kind: None,
            source_provider: None,
            state: "ready".to_owned(),
            protocol_version: 1,
            handoff_json: r#"{
                "protocol_version": 1,
                "project": "relay",
                "title": "Canonical title",
                "task": {"goal": "Mismatch."},
                "ambiguity_policy": "needs_clarification"
            }"#
            .to_owned(),
        };
        store.insert_task(&record).await.unwrap();

        let error = list_stored_tasks(&store).await.unwrap_err();

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
