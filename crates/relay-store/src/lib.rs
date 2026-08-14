//! SQLite-backed durable storage for Relay.
//!
//! This crate owns migrations and SQL queries. It intentionally uses storage
//! DTOs instead of Relay Core domain types so dependency direction remains
//! from Core into Store, never the reverse.

use std::{
    env,
    path::{Path, PathBuf},
};

use sqlx::{
    Row, SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use thiserror::Error;

const MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTaskRecord {
    pub id: String,
    pub project_key: String,
    pub title: String,
    pub requested_stack: Option<String>,
    pub submission_transport: String,
    pub source_kind: Option<String>,
    pub source_provider: Option<String>,
    pub state: String,
    pub protocol_version: i64,
    pub handoff_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRecord {
    pub id: String,
    pub title: String,
    pub handoff_json: String,
    pub project_key: String,
    pub submission_transport: String,
    pub state: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct SqliteStore {
    pool: SqlitePool,
}

impl SqliteStore {
    /// Opens a SQLite database, creates parent directories, and runs migrations.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|source| {
                StoreError::DatabaseDirectory {
                    path: parent.display().to_string(),
                    source,
                }
            })?;
        }

        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal);
        Self::connect(options).await
    }

    /// Opens an in-memory SQLite database and runs migrations.
    pub async fn in_memory() -> Result<Self, StoreError> {
        Self::connect(
            SqliteConnectOptions::new()
                .filename(":memory:")
                .foreign_keys(true),
        )
        .await
    }

    async fn connect(options: SqliteConnectOptions) -> Result<Self, StoreError> {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        MIGRATOR.run(&pool).await?;

        Ok(Self { pool })
    }

    pub async fn insert_task(&self, task: &NewTaskRecord) -> Result<TaskRecord, StoreError> {
        let mut transaction = self.pool.begin().await?;

        sqlx::query(
            r#"
            INSERT INTO tasks (
                id,
                project_key,
                title,
                requested_stack,
                submission_transport,
                source_kind,
                source_provider,
                state,
                protocol_version,
                handoff_json
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            "#,
        )
        .bind(&task.id)
        .bind(&task.project_key)
        .bind(&task.title)
        .bind(task.requested_stack.as_deref())
        .bind(&task.submission_transport)
        .bind(task.source_kind.as_deref())
        .bind(task.source_provider.as_deref())
        .bind(&task.state)
        .bind(task.protocol_version)
        .bind(&task.handoff_json)
        .execute(&mut *transaction)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO events (task_id, event_kind, event_json)
            VALUES (?1, ?2, ?3)
            "#,
        )
        .bind(&task.id)
        .bind("task_ready")
        .bind(format!(
            r#"{{"submission_transport":"{}"}}"#,
            task.submission_transport
        ))
        .execute(&mut *transaction)
        .await?;

        let task = fetch_task(&mut *transaction, &task.id)
            .await?
            .ok_or_else(|| StoreError::TaskMissingAfterInsert {
                id: task.id.clone(),
            })?;
        transaction.commit().await?;
        Ok(task)
    }

    pub async fn update_task_state(
        &self,
        id: &str,
        expected_state: &str,
        new_state: &str,
    ) -> Result<TaskRecord, StoreError> {
        let result = sqlx::query(
            r#"
            UPDATE tasks
            SET state = ?1,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            WHERE id = ?2
              AND state = ?3
            "#,
        )
        .bind(new_state)
        .bind(id)
        .bind(expected_state)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            let actual_state =
                sqlx::query_scalar::<_, String>("SELECT state FROM tasks WHERE id = ?1")
                    .bind(id)
                    .fetch_optional(&self.pool)
                    .await?;

            return match actual_state {
                Some(actual_state) => Err(StoreError::StateConflict {
                    id: id.to_owned(),
                    expected_state: expected_state.to_owned(),
                    actual_state,
                }),
                None => Err(StoreError::TaskNotFound { id: id.to_owned() }),
            };
        }

        self.get_task(id)
            .await?
            .ok_or_else(|| StoreError::TaskNotFound { id: id.to_owned() })
    }

    pub async fn get_task(&self, id: &str) -> Result<Option<TaskRecord>, StoreError> {
        let row = sqlx::query(
            r#"
            SELECT
                id,
                title,
                handoff_json,
                project_key,
                submission_transport,
                state,
                created_at,
                updated_at
            FROM tasks
            WHERE id = ?1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        row.map(row_to_task_record).transpose()
    }

    pub async fn list_tasks(&self) -> Result<Vec<TaskRecord>, StoreError> {
        let rows = sqlx::query(
            r#"
            SELECT
                id,
                title,
                handoff_json,
                project_key,
                submission_transport,
                state,
                created_at,
                updated_at
            FROM tasks
            ORDER BY created_at, id
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_task_record).collect()
    }

    #[cfg(test)]
    async fn task_event_count(&self, id: &str) -> Result<i64, StoreError> {
        let count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM events WHERE task_id = ?1")
            .bind(id)
            .fetch_one(&self.pool)
            .await?;
        Ok(count)
    }
}

async fn fetch_task<'a>(
    executor: impl sqlx::Executor<'a, Database = sqlx::Sqlite>,
    id: &str,
) -> Result<Option<TaskRecord>, StoreError> {
    let row = sqlx::query(
        r#"
        SELECT
            id,
            title,
            handoff_json,
            project_key,
            submission_transport,
            state,
            created_at,
            updated_at
        FROM tasks
        WHERE id = ?1
        "#,
    )
    .bind(id)
    .fetch_optional(executor)
    .await?;

    row.map(row_to_task_record).transpose()
}

/// Returns Relay's default local durable-state database path.
///
/// `RELAY_DATABASE_PATH` overrides platform defaults and is useful for tests.
pub fn default_database_path() -> PathBuf {
    if let Some(path) = env::var_os("RELAY_DATABASE_PATH") {
        return PathBuf::from(path);
    }

    if cfg!(windows) {
        if let Some(app_data) = env::var_os("APPDATA") {
            return PathBuf::from(app_data).join("Relay").join("state.sqlite3");
        }
    }

    if let Some(state_home) = env::var_os("XDG_STATE_HOME") {
        return PathBuf::from(state_home)
            .join("relay")
            .join("state.sqlite3");
    }

    if let Some(home) = env::var_os("HOME") {
        return PathBuf::from(home)
            .join(".local")
            .join("state")
            .join("relay")
            .join("state.sqlite3");
    }

    PathBuf::from("relay-state.sqlite3")
}

fn row_to_task_record(row: sqlx::sqlite::SqliteRow) -> Result<TaskRecord, StoreError> {
    Ok(TaskRecord {
        id: row.try_get("id")?,
        title: row.try_get("title")?,
        handoff_json: row.try_get("handoff_json")?,
        project_key: row.try_get("project_key")?,
        submission_transport: row.try_get("submission_transport")?,
        state: row.try_get("state")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("failed to create Relay database directory '{path}': {source}")]
    DatabaseDirectory {
        path: String,
        source: std::io::Error,
    },
    #[error("SQLite operation failed: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("SQLite migration failed: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),
    #[error("task {id} was not found after insert")]
    TaskMissingAfterInsert { id: String },
    #[error("task {id} was not found")]
    TaskNotFound { id: String },
    #[error("task {id} state conflict: expected '{expected_state}', found '{actual_state}'")]
    StateConflict {
        id: String,
        expected_state: String,
        actual_state: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    const TASK_JSON: &str = r#"{"protocol_version":1,"project":"relay","title":"Persist imported task","task":{"goal":"Persist accepted local task state."},"ambiguity_policy":"needs_clarification"}"#;

    #[tokio::test]
    async fn migrations_create_expected_tables() {
        let store = SqliteStore::in_memory().await.unwrap();

        let tasks = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'tasks'",
        )
        .fetch_one(&store.pool)
        .await
        .unwrap();

        assert_eq!(tasks, 1);
    }

    #[tokio::test]
    async fn stores_task_records_as_ready_state() {
        let store = SqliteStore::in_memory().await.unwrap();
        let task = task_record("task-1", "Persist imported task");

        let stored = store.insert_task(&task).await.unwrap();

        assert_eq!(stored.id, "task-1");
        assert_eq!(stored.title, "Persist imported task");
        assert_eq!(stored.state, "ready");
        assert_eq!(stored.project_key, "relay");
        assert_eq!(stored.handoff_json, TASK_JSON);
        assert_eq!(stored.submission_transport, "local-paste");
        assert_eq!(store.task_event_count("task-1").await.unwrap(), 1);
    }

    #[tokio::test]
    async fn task_creation_rolls_back_when_initial_event_fails() {
        let store = SqliteStore::in_memory().await.unwrap();
        sqlx::query(
            r#"
            CREATE TRIGGER fail_task_ready_event
            BEFORE INSERT ON events
            WHEN NEW.event_kind = 'task_ready'
            BEGIN
                SELECT RAISE(ABORT, 'forced TaskReady failure');
            END
            "#,
        )
        .execute(&store.pool)
        .await
        .unwrap();

        let error = store
            .insert_task(&task_record("task-rollback", "Rollback task"))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("SQLite operation failed"));
        assert!(store.get_task("task-rollback").await.unwrap().is_none());
        assert_eq!(store.task_event_count("task-rollback").await.unwrap(), 0);
    }

    #[tokio::test]
    async fn persisted_tasks_survive_store_reopen() {
        let database_path = temp_path("relay-state.sqlite3");
        let task = task_record("task-1", "Persist imported task");

        {
            let store = SqliteStore::open(&database_path).await.unwrap();
            store.insert_task(&task).await.unwrap();
        }

        let reopened = SqliteStore::open(&database_path).await.unwrap();
        let loaded = reopened.get_task("task-1").await.unwrap().unwrap();

        assert_eq!(loaded.id, "task-1");
        assert_eq!(loaded.state, "ready");
    }

    #[tokio::test]
    async fn updates_task_state_and_timestamp() {
        let store = SqliteStore::in_memory().await.unwrap();
        let stored = store
            .insert_task(&task_record("task-update", "Update task"))
            .await
            .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(5)).await;

        let updated = store
            .update_task_state("task-update", "ready", "needs_clarification")
            .await
            .unwrap();

        assert_eq!(updated.id, stored.id);
        assert_eq!(updated.state, "needs_clarification");
        assert_ne!(updated.updated_at, stored.updated_at);
    }

    #[tokio::test]
    async fn updated_task_state_survives_reopen() {
        let database_path = temp_path("relay-update-state.sqlite3");

        {
            let store = SqliteStore::open(&database_path).await.unwrap();
            store
                .insert_task(&task_record("task-update", "Update task"))
                .await
                .unwrap();
            store
                .update_task_state("task-update", "ready", "done")
                .await
                .unwrap();
        }

        let reopened = SqliteStore::open(&database_path).await.unwrap();
        let loaded = reopened.get_task("task-update").await.unwrap().unwrap();

        assert_eq!(loaded.state, "done");
    }

    #[tokio::test]
    async fn invalid_task_state_update_is_rejected() {
        let store = SqliteStore::in_memory().await.unwrap();
        store
            .insert_task(&task_record("task-invalid-state", "Invalid update"))
            .await
            .unwrap();

        let error = store
            .update_task_state("task-invalid-state", "ready", "bogus")
            .await
            .unwrap_err();
        let loaded = store.get_task("task-invalid-state").await.unwrap().unwrap();

        assert!(error.to_string().contains("SQLite operation failed"));
        assert_eq!(loaded.state, "ready");
    }

    #[tokio::test]
    async fn missing_task_state_update_is_reported() {
        let store = SqliteStore::in_memory().await.unwrap();

        let error = store
            .update_task_state("missing", "ready", "done")
            .await
            .unwrap_err();

        assert_eq!(error.to_string(), "task missing was not found");
    }

    #[tokio::test]
    async fn stale_task_state_update_is_rejected() {
        let store = SqliteStore::in_memory().await.unwrap();
        store
            .insert_task(&task_record("task-conflict", "Conflict task"))
            .await
            .unwrap();

        let error = store
            .update_task_state("task-conflict", "running", "done")
            .await
            .unwrap_err();
        let loaded = store.get_task("task-conflict").await.unwrap().unwrap();

        assert!(matches!(
            error,
            StoreError::StateConflict {
                expected_state,
                actual_state,
                ..
            } if expected_state == "running" && actual_state == "ready"
        ));
        assert_eq!(loaded.state, "ready");
    }

    #[tokio::test]
    async fn lists_tasks_in_durable_order() {
        let store = SqliteStore::in_memory().await.unwrap();

        for (id, title) in [("task-1", "First task"), ("task-2", "Second task")] {
            store.insert_task(&task_record(id, title)).await.unwrap();
        }

        let titles = store
            .list_tasks()
            .await
            .unwrap()
            .into_iter()
            .map(|task| task.id)
            .collect::<Vec<_>>();

        assert_eq!(titles, ["task-1", "task-2"]);
    }

    fn task_record(id: &str, title: &str) -> NewTaskRecord {
        NewTaskRecord {
            id: id.to_owned(),
            project_key: "relay".to_owned(),
            title: title.to_owned(),
            requested_stack: Some("chatgpt-codex".to_owned()),
            submission_transport: "local-paste".to_owned(),
            source_kind: Some("conversational_ai".to_owned()),
            source_provider: Some("openai".to_owned()),
            state: "ready".to_owned(),
            protocol_version: 1,
            handoff_json: TASK_JSON.to_owned(),
        }
    }

    fn temp_path(name: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("relay-store-{name}-{nanos}"))
    }
}
