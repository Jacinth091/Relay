//! SQLite-backed durable storage for Relay.
//!
//! This crate owns migrations and SQL queries. It intentionally uses storage
//! DTOs instead of Relay Core domain types so dependency direction remains
//! from Core into Store, never the reverse.

use std::{
    env,
    future::Future,
    path::{Path, PathBuf},
    time::Duration,
};

use sqlx::{
    Row, SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use thiserror::Error;

const MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Default SQLite busy timeout applied to every Relay connection.
const DEFAULT_BUSY_TIMEOUT: Duration = Duration::from_millis(5000);

/// Bounded retry budget applied before transient SQLite lock errors surface.
const WRITE_RETRY_ATTEMPTS: usize = 5;
const WRITE_RETRY_BASE_BACKOFF: Duration = Duration::from_millis(20);
const WRITE_RETRY_MAX_BACKOFF: Duration = Duration::from_millis(160);

/// Bounded retry budget applied when two Relay processes race to apply the
/// same migration while opening a shared database for the first time.
const MIGRATION_RACE_ATTEMPTS: usize = 5;
const MIGRATION_RACE_BACKOFF: Duration = Duration::from_millis(50);

// Primary SQLite result codes treated as transient lock contention. Extended
// codes (for example SQLITE_BUSY_SNAPSHOT) share the low byte.
const SQLITE_BUSY_PRIMARY_CODE: i32 = 5;
const SQLITE_LOCKED_PRIMARY_CODE: i32 = 6;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewExecutionRecord {
    pub id: String,
    pub task_id: String,
    pub runtime_profile: String,
    pub phase: String,
    pub repository_snapshot_json: Option<String>,
    pub runtime_metadata_json: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionRecord {
    pub id: String,
    pub task_id: String,
    pub runtime_profile: String,
    pub phase: String,
    pub repair_attempt_count: i64,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub exit_outcome: Option<String>,
    pub failure_category: Option<String>,
    pub repository_snapshot_json: Option<String>,
    pub runtime_metadata_json: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRecord {
    pub id: String,
    pub command_type: String,
    pub payload_json: String,
    pub status: String,
    pub response_json: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPendingEffectRecord {
    pub id: String,
    pub effect_type: String,
    pub idempotency_key: String,
    pub payload_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingEffectRecord {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectResultRecord {
    pub id: String,
    pub effect_id: String,
    pub outcome: String,
    pub result_json: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRecord {
    pub sequence: i64,
    pub task_id: Option<String>,
    pub execution_id: Option<String>,
    pub event_kind: String,
    pub event_json: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonLeaseRecord {
    pub lease_id: String,
    pub holder_pid: i64,
    pub holder_host: String,
    /// The individual daemon instance that owns this lease. Two daemons in one
    /// process share a pid and hostname; renewal and release match this too, so
    /// neither can act as the other.
    pub holder_instance: String,
    pub acquired_at: String,
    pub heartbeat_at: String,
    pub fencing_token: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewResourceLeaseRecord {
    pub id: String,
    pub resource_type: String,
    pub resource_id: String,
    pub lease_kind: String,
    pub holder_id: String,
    pub holder_pid: i64,
    pub holder_host: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceLeaseRecord {
    pub id: String,
    pub resource_type: String,
    pub resource_id: String,
    pub lease_kind: String,
    pub holder_id: String,
    pub holder_pid: i64,
    pub holder_host: String,
    pub fencing_token: i64,
    pub acquired_at: String,
    pub expires_at: String,
    pub state: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewWorkflowRunRecord {
    pub id: String,
    pub task_id: String,
    pub execution_grant_id: String,
    pub workflow_definition_id: String,
    pub state: String,
    pub config_snapshot_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRunRecord {
    pub id: String,
    pub task_id: String,
    pub execution_grant_id: String,
    pub workflow_definition_id: String,
    pub state: String,
    pub config_snapshot_id: String,
    pub started_at: String,
    pub completed_at: Option<String>,
    /// The approved intent this run executes, as JSON. Empty only for rows that
    /// predate authority capture (migration 0012).
    pub approved_intent_json: String,
    /// The execution grant derived from `approved_intent_json`, as JSON.
    pub execution_grant_json: String,
    /// The frozen configuration snapshot the run was approved under, as JSON.
    pub config_snapshot_json: String,
}

/// Everything one atomic Run approval writes: the workflow run (including the
/// authority it executes under), the task's `ready -> running` transition, and
/// the single `workflow_run_started` event.
///
/// These belong to one transaction because they are one fact. Split into
/// separate writes, a crash between them leaves a run whose task is still
/// `ready` (or a task marked `running` with no run), and a replay of the same
/// command cannot tell whether it is fixing that state or duplicating it.
#[derive(Debug, Clone, Copy)]
pub struct WorkflowRunStart<'a> {
    pub run: &'a NewWorkflowRunRecord,
    pub approved_intent_json: &'a str,
    pub execution_grant_json: &'a str,
    pub config_snapshot_json: &'a str,
    pub task_expected_state: &'a str,
    pub task_new_state: &'a str,
    pub event_kind: &'a str,
    pub event_json: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRoleInstanceRecord {
    pub id: String,
    pub workflow_run_id: String,
    pub role_definition_id: String,
    pub generation: i64,
    pub state: String,
    pub parent_role_instance_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleInstanceRecord {
    pub id: String,
    pub workflow_run_id: String,
    pub role_definition_id: String,
    pub generation: i64,
    pub state: String,
    pub parent_role_instance_id: Option<String>,
    pub accepted_result_artifact: Option<String>,
    pub attempts_count: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRoleAttemptRecord {
    pub id: String,
    pub role_instance_id: String,
    pub attempt_no: i64,
    pub execution_target_json: String,
    pub state: String,
    pub lease_id: Option<String>,
    pub fencing_token: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleAttemptRecord {
    pub id: String,
    pub role_instance_id: String,
    pub attempt_no: i64,
    pub execution_target_json: String,
    pub state: String,
    pub lease_id: Option<String>,
    pub fencing_token: Option<i64>,
    pub runtime_outcome: Option<String>,
    pub result_artifact_id: Option<String>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub runtime_handle_id: Option<String>,
    pub runtime_backplane_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct SqliteStore {
    pool: SqlitePool,
}

impl SqliteStore {
    /// Opens a SQLite database, creates parent directories, and runs migrations.
    ///
    /// Every connection sets WAL journal mode, a 5000 ms busy timeout, and
    /// foreign key enforcement so two Relay processes (TUI, serve-mcp) can
    /// share the same database file safely.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        Self::open_with_busy_timeout(path, DEFAULT_BUSY_TIMEOUT).await
    }

    /// Opens a SQLite database like [`SqliteStore::open`] with an explicit
    /// busy timeout.
    pub async fn open_with_busy_timeout(
        path: impl AsRef<Path>,
        busy_timeout: Duration,
    ) -> Result<Self, StoreError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|source| {
                StoreError::DatabaseDirectory {
                    path: parent.display().to_string(),
                    source,
                }
            })?;
        }

        let options = Self::connect_options(path)
            .create_if_missing(true)
            .busy_timeout(busy_timeout);
        Self::connect(options).await
    }

    /// Opens an in-memory SQLite database and runs migrations.
    pub async fn in_memory() -> Result<Self, StoreError> {
        Self::connect(Self::connect_options(":memory:").busy_timeout(DEFAULT_BUSY_TIMEOUT)).await
    }

    /// Read access to the underlying connection pool for inspection queries.
    ///
    /// Intended for integration tests and operational verification; all schema
    /// mutations must go through the typed store APIs.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Returns this database's durable state identity: a random value generated
    /// once, inside the database, when the schema was created (migration 0007).
    ///
    /// Two stores opened on the same database file return the same identity; two
    /// different databases - including two [`SqliteStore::in_memory`] stores - never
    /// do. The daemon transport compares it during its handshake so that a client
    /// can only ever mutate the durable state it was opened against, instead of
    /// trusting that some responding pipe must be serving the right database.
    pub async fn store_identity(&self) -> Result<String, StoreError> {
        let identity =
            sqlx::query_scalar::<_, String>("SELECT identity FROM store_identity WHERE id = 1")
                .fetch_one(&self.pool)
                .await?;
        Ok(identity)
    }

    fn connect_options(filename: impl AsRef<Path>) -> SqliteConnectOptions {
        SqliteConnectOptions::new()
            .filename(filename)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
    }

    async fn connect(options: SqliteConnectOptions) -> Result<Self, StoreError> {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        run_migrations(&pool).await?;

        Ok(Self { pool })
    }

    pub async fn insert_task(&self, task: &NewTaskRecord) -> Result<TaskRecord, StoreError> {
        retry_on_transient_lock(|| self.insert_task_once(task)).await
    }

    async fn insert_task_once(&self, task: &NewTaskRecord) -> Result<TaskRecord, StoreError> {
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

    /// Inserts a task only if its id is not already present, returning the
    /// existing row unchanged on a replay instead of erroring or duplicating
    /// the `task_ready` event. Used by daemon command replay so that
    /// resuming a non-terminal command after a crash is idempotent.
    pub async fn insert_task_if_absent(
        &self,
        task: &NewTaskRecord,
    ) -> Result<TaskRecord, StoreError> {
        retry_on_transient_lock(|| self.insert_task_if_absent_once(task, None)).await
    }

    /// Fenced variant used by the daemon's handoff submission path.
    pub async fn insert_task_if_absent_fenced(
        &self,
        task: &NewTaskRecord,
        fence: &DaemonFence,
    ) -> Result<TaskRecord, StoreError> {
        retry_on_transient_lock(|| self.insert_task_if_absent_once(task, Some(fence))).await
    }

    async fn insert_task_if_absent_once(
        &self,
        task: &NewTaskRecord,
        fence: Option<&DaemonFence>,
    ) -> Result<TaskRecord, StoreError> {
        let mut transaction = self.pool.begin().await?;

        if let Some(fence) = fence {
            assert_daemon_fence(&mut *transaction, fence).await?;
        }

        let result = sqlx::query(
            r#"
            INSERT OR IGNORE INTO tasks (
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

        if result.rows_affected() > 0 {
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
        }

        let task = fetch_task(&mut *transaction, &task.id)
            .await?
            .ok_or_else(|| StoreError::TaskMissingAfterInsert {
                id: task.id.clone(),
            })?;
        transaction.commit().await?;
        Ok(task)
    }

    /// Append a bounded, structured event without exposing the SQL event table
    /// to Core callers. Event payloads are caller-validated JSON strings.
    pub async fn append_event(
        &self,
        task_id: Option<&str>,
        execution_id: Option<&str>,
        event_kind: &str,
        event_json: Option<&str>,
    ) -> Result<(), StoreError> {
        retry_on_transient_lock(|| {
            self.append_event_once(task_id, execution_id, event_kind, event_json)
        })
        .await
    }

    /// Fenced variant of [`SqliteStore::append_event`] used by the daemon-owned
    /// v2 workflow driver, whose writes are not command-handler writes.
    ///
    /// The authority assertion shares the event's transaction, so a stale
    /// daemon cannot append a workflow event after a successor took the lease.
    pub async fn append_event_fenced(
        &self,
        task_id: Option<&str>,
        execution_id: Option<&str>,
        event_kind: &str,
        event_json: Option<&str>,
        fence: &DaemonFence,
    ) -> Result<(), StoreError> {
        retry_on_transient_lock(|| {
            self.append_event_fenced_once(task_id, execution_id, event_kind, event_json, fence)
        })
        .await
    }

    async fn append_event_fenced_once(
        &self,
        task_id: Option<&str>,
        execution_id: Option<&str>,
        event_kind: &str,
        event_json: Option<&str>,
        fence: &DaemonFence,
    ) -> Result<(), StoreError> {
        let mut transaction = self.pool.begin().await?;
        assert_daemon_fence(&mut *transaction, fence).await?;
        insert_event(
            &mut transaction,
            task_id,
            execution_id,
            event_kind,
            event_json,
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    async fn append_event_once(
        &self,
        task_id: Option<&str>,
        execution_id: Option<&str>,
        event_kind: &str,
        event_json: Option<&str>,
    ) -> Result<(), StoreError> {
        let mut transaction = self.pool.begin().await?;
        insert_event(
            &mut transaction,
            task_id,
            execution_id,
            event_kind,
            event_json,
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn update_task_state(
        &self,
        id: &str,
        expected_state: &str,
        new_state: &str,
    ) -> Result<TaskRecord, StoreError> {
        retry_on_transient_lock(|| self.update_task_state_once(id, expected_state, new_state, None))
            .await
    }

    /// Fenced variant used by the daemon command path: the transition is
    /// refused unless this process still holds `fence` at the moment the write
    /// commits, so a command admitted before authority was lost cannot land a
    /// stale state change after a successor took the lease.
    pub async fn update_task_state_fenced(
        &self,
        id: &str,
        expected_state: &str,
        new_state: &str,
        fence: &DaemonFence,
    ) -> Result<TaskRecord, StoreError> {
        retry_on_transient_lock(|| {
            self.update_task_state_once(id, expected_state, new_state, Some(fence))
        })
        .await
    }

    async fn update_task_state_once(
        &self,
        id: &str,
        expected_state: &str,
        new_state: &str,
        fence: Option<&DaemonFence>,
    ) -> Result<TaskRecord, StoreError> {
        let mut transaction = self.pool.begin().await?;

        if let Some(fence) = fence {
            assert_daemon_fence(&mut *transaction, fence).await?;
        }

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
        .execute(&mut *transaction)
        .await?;

        if result.rows_affected() == 0 {
            let actual_state =
                sqlx::query_scalar::<_, String>("SELECT state FROM tasks WHERE id = ?1")
                    .bind(id)
                    .fetch_optional(&mut *transaction)
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

        let task = fetch_task(&mut *transaction, id)
            .await?
            .ok_or_else(|| StoreError::TaskNotFound { id: id.to_owned() })?;
        transaction.commit().await?;
        Ok(task)
    }

    pub async fn claim_ready_task_for_execution(
        &self,
        execution: &NewExecutionRecord,
    ) -> Result<(TaskRecord, ExecutionRecord), StoreError> {
        retry_on_transient_lock(|| self.claim_ready_task_for_execution_once(execution)).await
    }

    async fn claim_ready_task_for_execution_once(
        &self,
        execution: &NewExecutionRecord,
    ) -> Result<(TaskRecord, ExecutionRecord), StoreError> {
        let mut transaction = self.pool.begin().await?;

        let result = sqlx::query(
            r#"
            UPDATE tasks
            SET state = 'running',
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            WHERE id = ?1
              AND state = 'ready'
            "#,
        )
        .bind(&execution.task_id)
        .execute(&mut *transaction)
        .await?;

        if result.rows_affected() == 0 {
            let actual_state =
                sqlx::query_scalar::<_, String>("SELECT state FROM tasks WHERE id = ?1")
                    .bind(&execution.task_id)
                    .fetch_optional(&mut *transaction)
                    .await?;

            return match actual_state {
                Some(actual_state) => Err(StoreError::StateConflict {
                    id: execution.task_id.clone(),
                    expected_state: "ready".to_owned(),
                    actual_state,
                }),
                None => Err(StoreError::TaskNotFound {
                    id: execution.task_id.clone(),
                }),
            };
        }

        sqlx::query(
            r#"
            INSERT INTO executions (
                id,
                task_id,
                runtime_profile,
                phase,
                repository_snapshot_json,
                runtime_metadata_json
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
        )
        .bind(&execution.id)
        .bind(&execution.task_id)
        .bind(&execution.runtime_profile)
        .bind(&execution.phase)
        .bind(execution.repository_snapshot_json.as_deref())
        .bind(execution.runtime_metadata_json.as_deref())
        .execute(&mut *transaction)
        .await?;

        insert_event(
            &mut transaction,
            Some(&execution.task_id),
            Some(&execution.id),
            "execution_started",
            Some(r#"{"phase":"running"}"#),
        )
        .await?;

        let task = fetch_task(&mut *transaction, &execution.task_id)
            .await?
            .ok_or_else(|| StoreError::TaskNotFound {
                id: execution.task_id.clone(),
            })?;
        let execution = fetch_execution(&mut *transaction, &execution.id)
            .await?
            .ok_or_else(|| StoreError::ExecutionNotFound {
                id: execution.id.clone(),
            })?;
        transaction.commit().await?;
        Ok((task, execution))
    }

    pub async fn get_execution(&self, id: &str) -> Result<Option<ExecutionRecord>, StoreError> {
        fetch_execution(&self.pool, id).await
    }

    pub async fn list_executions_for_task(
        &self,
        task_id: &str,
    ) -> Result<Vec<ExecutionRecord>, StoreError> {
        let rows = sqlx::query(
            r#"
            SELECT
                id,
                task_id,
                runtime_profile,
                phase,
                repair_attempt_count,
                started_at,
                ended_at,
                exit_outcome,
                failure_category,
                repository_snapshot_json,
                runtime_metadata_json
            FROM executions
            WHERE task_id = ?1
            ORDER BY started_at, id
            "#,
        )
        .bind(task_id)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_execution_record).collect()
    }

    pub async fn update_execution_phase(
        &self,
        execution_id: &str,
        phase: &str,
    ) -> Result<ExecutionRecord, StoreError> {
        retry_on_transient_lock(|| self.update_execution_phase_once(execution_id, phase)).await
    }

    async fn update_execution_phase_once(
        &self,
        execution_id: &str,
        phase: &str,
    ) -> Result<ExecutionRecord, StoreError> {
        let result = sqlx::query(
            r#"
            UPDATE executions
            SET phase = ?1
            WHERE id = ?2
            "#,
        )
        .bind(phase)
        .bind(execution_id)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(StoreError::ExecutionNotFound {
                id: execution_id.to_owned(),
            });
        }

        self.get_execution(execution_id)
            .await?
            .ok_or_else(|| StoreError::ExecutionNotFound {
                id: execution_id.to_owned(),
            })
    }

    pub async fn transition_task_and_execution(
        &self,
        task_id: &str,
        expected_task_state: &str,
        new_task_state: &str,
        execution_id: &str,
        new_execution_phase: &str,
    ) -> Result<(TaskRecord, ExecutionRecord), StoreError> {
        retry_on_transient_lock(|| {
            self.transition_task_and_execution_once(
                task_id,
                expected_task_state,
                new_task_state,
                execution_id,
                new_execution_phase,
            )
        })
        .await
    }

    async fn transition_task_and_execution_once(
        &self,
        task_id: &str,
        expected_task_state: &str,
        new_task_state: &str,
        execution_id: &str,
        new_execution_phase: &str,
    ) -> Result<(TaskRecord, ExecutionRecord), StoreError> {
        let mut transaction = self.pool.begin().await?;

        let result = sqlx::query(
            r#"
            UPDATE tasks
            SET state = ?1,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            WHERE id = ?2
              AND state = ?3
            "#,
        )
        .bind(new_task_state)
        .bind(task_id)
        .bind(expected_task_state)
        .execute(&mut *transaction)
        .await?;

        if result.rows_affected() == 0 {
            let actual_state =
                sqlx::query_scalar::<_, String>("SELECT state FROM tasks WHERE id = ?1")
                    .bind(task_id)
                    .fetch_optional(&mut *transaction)
                    .await?;

            return match actual_state {
                Some(actual_state) => Err(StoreError::StateConflict {
                    id: task_id.to_owned(),
                    expected_state: expected_task_state.to_owned(),
                    actual_state,
                }),
                None => Err(StoreError::TaskNotFound {
                    id: task_id.to_owned(),
                }),
            };
        }

        let execution_result = sqlx::query(
            r#"
            UPDATE executions
            SET phase = ?1
            WHERE id = ?2
              AND task_id = ?3
            "#,
        )
        .bind(new_execution_phase)
        .bind(execution_id)
        .bind(task_id)
        .execute(&mut *transaction)
        .await?;

        if execution_result.rows_affected() == 0 {
            return Err(StoreError::ExecutionNotFound {
                id: execution_id.to_owned(),
            });
        }

        insert_event(
            &mut transaction,
            Some(task_id),
            Some(execution_id),
            "task_state_changed",
            Some(&format!(
                r#"{{"from":"{expected_task_state}","to":"{new_task_state}"}}"#
            )),
        )
        .await?;

        let task = fetch_task(&mut *transaction, task_id)
            .await?
            .ok_or_else(|| StoreError::TaskNotFound {
                id: task_id.to_owned(),
            })?;
        let execution = fetch_execution(&mut *transaction, execution_id)
            .await?
            .ok_or_else(|| StoreError::ExecutionNotFound {
                id: execution_id.to_owned(),
            })?;
        transaction.commit().await?;
        Ok((task, execution))
    }

    pub async fn increment_execution_repair_count(
        &self,
        execution_id: &str,
    ) -> Result<ExecutionRecord, StoreError> {
        retry_on_transient_lock(|| self.increment_execution_repair_count_once(execution_id)).await
    }

    async fn increment_execution_repair_count_once(
        &self,
        execution_id: &str,
    ) -> Result<ExecutionRecord, StoreError> {
        let result = sqlx::query(
            r#"
            UPDATE executions
            SET repair_attempt_count = repair_attempt_count + 1
            WHERE id = ?1
            "#,
        )
        .bind(execution_id)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(StoreError::ExecutionNotFound {
                id: execution_id.to_owned(),
            });
        }

        self.get_execution(execution_id)
            .await?
            .ok_or_else(|| StoreError::ExecutionNotFound {
                id: execution_id.to_owned(),
            })
    }

    pub async fn transition_task_and_increment_repair_count(
        &self,
        task_id: &str,
        expected_task_state: &str,
        execution_id: &str,
    ) -> Result<(TaskRecord, ExecutionRecord), StoreError> {
        retry_on_transient_lock(|| {
            self.transition_task_and_increment_repair_count_once(
                task_id,
                expected_task_state,
                execution_id,
            )
        })
        .await
    }

    async fn transition_task_and_increment_repair_count_once(
        &self,
        task_id: &str,
        expected_task_state: &str,
        execution_id: &str,
    ) -> Result<(TaskRecord, ExecutionRecord), StoreError> {
        let mut transaction = self.pool.begin().await?;

        let result = sqlx::query(
            r#"
            UPDATE tasks
            SET state = 'repairing',
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            WHERE id = ?1
              AND state = ?2
            "#,
        )
        .bind(task_id)
        .bind(expected_task_state)
        .execute(&mut *transaction)
        .await?;

        if result.rows_affected() == 0 {
            let actual_state =
                sqlx::query_scalar::<_, String>("SELECT state FROM tasks WHERE id = ?1")
                    .bind(task_id)
                    .fetch_optional(&mut *transaction)
                    .await?;

            return match actual_state {
                Some(actual_state) => Err(StoreError::StateConflict {
                    id: task_id.to_owned(),
                    expected_state: expected_task_state.to_owned(),
                    actual_state,
                }),
                None => Err(StoreError::TaskNotFound {
                    id: task_id.to_owned(),
                }),
            };
        }

        let execution_result = sqlx::query(
            r#"
            UPDATE executions
            SET phase = 'repairing',
                repair_attempt_count = repair_attempt_count + 1
            WHERE id = ?1
              AND task_id = ?2
            "#,
        )
        .bind(execution_id)
        .bind(task_id)
        .execute(&mut *transaction)
        .await?;

        if execution_result.rows_affected() == 0 {
            return Err(StoreError::ExecutionNotFound {
                id: execution_id.to_owned(),
            });
        }

        insert_event(
            &mut transaction,
            Some(task_id),
            Some(execution_id),
            "repair_started",
            Some(r#"{"phase":"repairing"}"#),
        )
        .await?;

        let task = fetch_task(&mut *transaction, task_id)
            .await?
            .ok_or_else(|| StoreError::TaskNotFound {
                id: task_id.to_owned(),
            })?;
        let execution = fetch_execution(&mut *transaction, execution_id)
            .await?
            .ok_or_else(|| StoreError::ExecutionNotFound {
                id: execution_id.to_owned(),
            })?;
        transaction.commit().await?;
        Ok((task, execution))
    }

    pub async fn finish_execution(
        &self,
        task_id: &str,
        expected_task_state: &str,
        terminal_task_state: &str,
        execution_id: &str,
        exit_outcome: &str,
        failure_category: Option<&str>,
    ) -> Result<(TaskRecord, ExecutionRecord), StoreError> {
        retry_on_transient_lock(|| {
            self.finish_execution_once(
                task_id,
                expected_task_state,
                terminal_task_state,
                execution_id,
                exit_outcome,
                failure_category,
            )
        })
        .await
    }

    async fn finish_execution_once(
        &self,
        task_id: &str,
        expected_task_state: &str,
        terminal_task_state: &str,
        execution_id: &str,
        exit_outcome: &str,
        failure_category: Option<&str>,
    ) -> Result<(TaskRecord, ExecutionRecord), StoreError> {
        let mut transaction = self.pool.begin().await?;

        let result = sqlx::query(
            r#"
            UPDATE tasks
            SET state = ?1,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            WHERE id = ?2
              AND state = ?3
            "#,
        )
        .bind(terminal_task_state)
        .bind(task_id)
        .bind(expected_task_state)
        .execute(&mut *transaction)
        .await?;

        if result.rows_affected() == 0 {
            let actual_state =
                sqlx::query_scalar::<_, String>("SELECT state FROM tasks WHERE id = ?1")
                    .bind(task_id)
                    .fetch_optional(&mut *transaction)
                    .await?;

            return match actual_state {
                Some(actual_state) => Err(StoreError::StateConflict {
                    id: task_id.to_owned(),
                    expected_state: expected_task_state.to_owned(),
                    actual_state,
                }),
                None => Err(StoreError::TaskNotFound {
                    id: task_id.to_owned(),
                }),
            };
        }

        let execution_result = sqlx::query(
            r#"
            UPDATE executions
            SET phase = 'finished',
                ended_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                exit_outcome = ?1,
                failure_category = ?2
            WHERE id = ?3
              AND task_id = ?4
            "#,
        )
        .bind(exit_outcome)
        .bind(failure_category)
        .bind(execution_id)
        .bind(task_id)
        .execute(&mut *transaction)
        .await?;

        if execution_result.rows_affected() == 0 {
            return Err(StoreError::ExecutionNotFound {
                id: execution_id.to_owned(),
            });
        }

        insert_event(
            &mut transaction,
            Some(task_id),
            Some(execution_id),
            "execution_finished",
            Some(&format!(
                r#"{{"task_state":"{terminal_task_state}","exit_outcome":"{exit_outcome}"}}"#
            )),
        )
        .await?;

        let task = fetch_task(&mut *transaction, task_id)
            .await?
            .ok_or_else(|| StoreError::TaskNotFound {
                id: task_id.to_owned(),
            })?;
        let execution = fetch_execution(&mut *transaction, execution_id)
            .await?
            .ok_or_else(|| StoreError::ExecutionNotFound {
                id: execution_id.to_owned(),
            })?;
        transaction.commit().await?;
        Ok((task, execution))
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

    /// Records a command for idempotency, without an authority check.
    ///
    /// The daemon must use [`SqliteStore::record_command_fenced`] instead: this
    /// form exists for store-level tests that seed command rows directly.
    pub async fn record_command(
        &self,
        id: &str,
        command_type: &str,
        payload_json: &str,
    ) -> Result<bool, StoreError> {
        retry_on_transient_lock(|| self.record_command_once(id, command_type, payload_json, None))
            .await
    }

    /// Fenced variant used by the daemon command path.
    ///
    /// The insert is refused unless this process still holds `fence` at the
    /// moment it commits, so a command admitted before authority was lost
    /// cannot create a durable command row after a successor took the lease.
    /// The command ledger is part of the daemon's canonical state, so it is
    /// fenced on the same lease as the semantic mutations.
    pub async fn record_command_fenced(
        &self,
        id: &str,
        command_type: &str,
        payload_json: &str,
        fence: &DaemonFence,
    ) -> Result<bool, StoreError> {
        retry_on_transient_lock(|| {
            self.record_command_once(id, command_type, payload_json, Some(fence))
        })
        .await
    }

    async fn record_command_once(
        &self,
        id: &str,
        command_type: &str,
        payload_json: &str,
        fence: Option<&DaemonFence>,
    ) -> Result<bool, StoreError> {
        let mut transaction = self.pool.begin().await?;

        if let Some(fence) = fence {
            assert_daemon_fence(&mut *transaction, fence).await?;
        }

        let res = sqlx::query(
            r#"
            INSERT OR IGNORE INTO commands (id, command_type, payload_json, status)
            VALUES (?1, ?2, ?3, 'received')
            "#,
        )
        .bind(id)
        .bind(command_type)
        .bind(payload_json)
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;
        Ok(res.rows_affected() > 0)
    }

    /// Lists commands that have not reached a terminal status (`applied` or
    /// `failed`), for startup reconciliation after a crash or restart.
    pub async fn list_incomplete_commands(&self) -> Result<Vec<CommandRecord>, StoreError> {
        let rows = sqlx::query(
            r#"
            SELECT id, command_type, payload_json, status, response_json, created_at, updated_at
            FROM commands
            WHERE status IN ('received', 'needs_reconciliation')
            ORDER BY created_at ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|r| {
                Ok(CommandRecord {
                    id: r.try_get("id")?,
                    command_type: r.try_get("command_type")?,
                    payload_json: r.try_get("payload_json")?,
                    status: r.try_get("status")?,
                    response_json: r.try_get("response_json")?,
                    created_at: r.try_get("created_at")?,
                    updated_at: r.try_get("updated_at")?,
                })
            })
            .collect()
    }

    /// Counts commands currently in the given status, for operator/test
    /// visibility (e.g. how many are stuck `needs_reconciliation`).
    pub async fn count_commands_with_status(&self, status: &str) -> Result<i64, StoreError> {
        let count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM commands WHERE status = ?1")
            .bind(status)
            .fetch_one(&self.pool)
            .await?;
        Ok(count)
    }

    pub async fn get_command(&self, id: &str) -> Result<Option<CommandRecord>, StoreError> {
        let row = sqlx::query(
            r#"
            SELECT id, command_type, payload_json, status, response_json, created_at, updated_at
            FROM commands
            WHERE id = ?1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        row.map(|r| {
            Ok(CommandRecord {
                id: r.try_get("id")?,
                command_type: r.try_get("command_type")?,
                payload_json: r.try_get("payload_json")?,
                status: r.try_get("status")?,
                response_json: r.try_get("response_json")?,
                created_at: r.try_get("created_at")?,
                updated_at: r.try_get("updated_at")?,
            })
        })
        .transpose()
    }

    /// Rewrites a command's status row, without an authority check and without
    /// the terminal-status guard of [`SqliteStore::complete_command_fenced`].
    ///
    /// The daemon must use `complete_command_fenced` instead. This form exists for
    /// store-level tests that reproduce crash windows by rewinding a row to a
    /// non-terminal status.
    pub async fn complete_command(
        &self,
        id: &str,
        status: &str,
        response_json: Option<&str>,
    ) -> Result<(), StoreError> {
        retry_on_transient_lock(|| self.complete_command_once(id, status, response_json, None))
            .await
            .map(|_| ())
    }

    /// Fenced variant used by the daemon command path.
    ///
    /// The write is refused unless this process still holds `fence` when it
    /// commits, and it is additionally guarded on the command still being
    /// non-terminal, so neither a stale daemon nor a duplicated completion can
    /// overwrite a terminal result that another applier already recorded.
    ///
    /// Returns `true` when this call recorded the terminal result, and `false`
    /// when the command had already reached a terminal status - in which case
    /// nothing is written and the existing terminal row is authoritative. A
    /// command id that does not exist at all is still a typed error.
    pub async fn complete_command_fenced(
        &self,
        id: &str,
        status: &str,
        response_json: Option<&str>,
        fence: &DaemonFence,
    ) -> Result<bool, StoreError> {
        retry_on_transient_lock(|| {
            self.complete_command_once(id, status, response_json, Some(fence))
        })
        .await
    }

    async fn complete_command_once(
        &self,
        id: &str,
        status: &str,
        response_json: Option<&str>,
        fence: Option<&DaemonFence>,
    ) -> Result<bool, StoreError> {
        let mut transaction = self.pool.begin().await?;

        if let Some(fence) = fence {
            assert_daemon_fence(&mut *transaction, fence).await?;
        }

        // The fenced (daemon) path never overwrites a terminal row: `received`
        // and `needs_reconciliation` are the only completable statuses, so a late
        // duplicate cannot turn a recorded `applied` into `failed`. The unfenced
        // path is relied on by crash-window tests that deliberately rewind a row,
        // so it keeps its unconditional rewrite.
        let query = if fence.is_some() {
            r#"
            UPDATE commands
            SET status = ?1,
                response_json = ?2,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            WHERE id = ?3
              AND status IN ('received', 'needs_reconciliation')
            "#
        } else {
            r#"
            UPDATE commands
            SET status = ?1,
                response_json = ?2,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            WHERE id = ?3
            "#
        };

        let res = sqlx::query(query)
            .bind(status)
            .bind(response_json)
            .bind(id)
            .execute(&mut *transaction)
            .await?;

        if res.rows_affected() == 0 {
            // Distinguish "no such command" (fail closed) from "already
            // terminal" (another applier won; its result stands).
            let exists =
                sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM commands WHERE id = ?1")
                    .bind(id)
                    .fetch_one(&mut *transaction)
                    .await?
                    > 0;
            transaction.commit().await?;
            if exists {
                return Ok(false);
            }
            return Err(StoreError::CommandNotFound { id: id.to_owned() });
        }

        transaction.commit().await?;
        Ok(true)
    }

    /// Atomically claims an unfinished command for execution.
    ///
    /// Returns `true` only for the caller that won the claim, so concurrent replays
    /// of the same command id resolve through a single applier and a single terminal
    /// result instead of racing each other into inconsistent state.
    ///
    /// The daemon must use [`SqliteStore::claim_command_fenced`]: this form has no
    /// authority check.
    pub async fn claim_command(&self, id: &str) -> Result<bool, StoreError> {
        retry_on_transient_lock(|| self.claim_command_once(id, None)).await
    }

    /// Fenced variant used by the daemon command path.
    ///
    /// A claim is refused unless this process still holds `fence` when it
    /// commits. Without this, a daemon that lost the lease could claim a command
    /// *after* the successor's one-time startup reconciliation had already run,
    /// stranding that command until the next restart.
    pub async fn claim_command_fenced(
        &self,
        id: &str,
        fence: &DaemonFence,
    ) -> Result<bool, StoreError> {
        retry_on_transient_lock(|| self.claim_command_once(id, Some(fence))).await
    }

    async fn claim_command_once(
        &self,
        id: &str,
        fence: Option<&DaemonFence>,
    ) -> Result<bool, StoreError> {
        let mut transaction = self.pool.begin().await?;

        if let Some(fence) = fence {
            assert_daemon_fence(&mut *transaction, fence).await?;
        }

        let res = sqlx::query(
            r#"
            UPDATE commands
            SET claimed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            WHERE id = ?1
              AND claimed_at IS NULL
              AND status IN ('received', 'needs_reconciliation')
            "#,
        )
        .bind(id)
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;
        Ok(res.rows_affected() > 0)
    }

    /// Clears command claims left behind by a crashed process.
    ///
    /// Only safe during startup reconciliation, before the request listener
    /// accepts connections; a live applier holds its claim precisely so a
    /// concurrent replay cannot steal it.
    ///
    /// The daemon must use [`SqliteStore::reset_command_claims_fenced`]: this form
    /// has no authority check.
    pub async fn reset_command_claims(&self) -> Result<u64, StoreError> {
        retry_on_transient_lock(|| self.reset_command_claims_once(None)).await
    }

    /// Fenced variant used by the daemon startup reconciliation.
    ///
    /// The reset is refused unless this process still holds `fence`, so a stale
    /// daemon cannot clear a successor's claims.
    pub async fn reset_command_claims_fenced(
        &self,
        fence: &DaemonFence,
    ) -> Result<u64, StoreError> {
        retry_on_transient_lock(|| self.reset_command_claims_once(Some(fence))).await
    }

    async fn reset_command_claims_once(
        &self,
        fence: Option<&DaemonFence>,
    ) -> Result<u64, StoreError> {
        let mut transaction = self.pool.begin().await?;

        if let Some(fence) = fence {
            assert_daemon_fence(&mut *transaction, fence).await?;
        }

        let res = sqlx::query(
            r#"
            UPDATE commands
            SET claimed_at = NULL
            WHERE claimed_at IS NOT NULL
              AND status IN ('received', 'needs_reconciliation')
            "#,
        )
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;
        Ok(res.rows_affected())
    }

    pub async fn insert_pending_effect(
        &self,
        effect: &NewPendingEffectRecord,
    ) -> Result<PendingEffectRecord, StoreError> {
        retry_on_transient_lock(|| self.insert_pending_effect_once(effect, None)).await
    }

    /// Fenced variant used by the daemon's `QueuePendingEffect` command path.
    pub async fn insert_pending_effect_fenced(
        &self,
        effect: &NewPendingEffectRecord,
        fence: &DaemonFence,
    ) -> Result<PendingEffectRecord, StoreError> {
        retry_on_transient_lock(|| self.insert_pending_effect_once(effect, Some(fence))).await
    }

    async fn insert_pending_effect_once(
        &self,
        effect: &NewPendingEffectRecord,
        fence: Option<&DaemonFence>,
    ) -> Result<PendingEffectRecord, StoreError> {
        let mut transaction = self.pool.begin().await?;

        if let Some(fence) = fence {
            assert_daemon_fence(&mut *transaction, fence).await?;
        }

        // OR IGNORE makes replaying the same QueuePendingEffect command (same
        // effect id) after a crash idempotent instead of erroring.
        sqlx::query(
            r#"
            INSERT OR IGNORE INTO pending_effects (id, effect_type, idempotency_key, payload_json, state)
            VALUES (?1, ?2, ?3, ?4, 'pending')
            "#,
        )
        .bind(&effect.id)
        .bind(&effect.effect_type)
        .bind(&effect.idempotency_key)
        .bind(&effect.payload_json)
        .execute(&mut *transaction)
        .await?;

        let row = sqlx::query(
            r#"
            SELECT id, effect_type, idempotency_key, payload_json, state, attempts, last_error, created_at, updated_at
            FROM pending_effects
            WHERE id = ?1
            "#,
        )
        .bind(&effect.id)
        .fetch_optional(&mut *transaction)
        .await?;

        let record = row
            .map(row_to_pending_effect_record)
            .transpose()?
            .ok_or_else(|| StoreError::PendingEffectNotFound {
                id: effect.id.clone(),
            })?;
        transaction.commit().await?;
        Ok(record)
    }

    pub async fn get_pending_effect(
        &self,
        id: &str,
    ) -> Result<Option<PendingEffectRecord>, StoreError> {
        let row = sqlx::query(
            r#"
            SELECT id, effect_type, idempotency_key, payload_json, state, attempts, last_error, created_at, updated_at
            FROM pending_effects
            WHERE id = ?1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        row.map(row_to_pending_effect_record).transpose()
    }

    pub async fn get_pending_effect_by_idempotency_key(
        &self,
        idempotency_key: &str,
    ) -> Result<Option<PendingEffectRecord>, StoreError> {
        let row = sqlx::query(
            r#"
            SELECT id, effect_type, idempotency_key, payload_json, state, attempts, last_error, created_at, updated_at
            FROM pending_effects
            WHERE idempotency_key = ?1
            "#,
        )
        .bind(idempotency_key)
        .fetch_optional(&self.pool)
        .await?;

        row.map(row_to_pending_effect_record).transpose()
    }

    pub async fn list_pending_effects(
        &self,
        state: Option<&str>,
    ) -> Result<Vec<PendingEffectRecord>, StoreError> {
        let rows = if let Some(st) = state {
            sqlx::query(
                r#"
                SELECT id, effect_type, idempotency_key, payload_json, state, attempts, last_error, created_at, updated_at
                FROM pending_effects
                WHERE state = ?1
                ORDER BY created_at ASC
                "#,
            )
            .bind(st)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                r#"
                SELECT id, effect_type, idempotency_key, payload_json, state, attempts, last_error, created_at, updated_at
                FROM pending_effects
                ORDER BY created_at ASC
                "#,
            )
            .fetch_all(&self.pool)
            .await?
        };

        rows.into_iter().map(row_to_pending_effect_record).collect()
    }

    /// Lists effects stuck `in_flight` for at least `stale_seconds`, for the
    /// effect worker to reconcile after a crash (nothing resumed them).
    /// Comparison happens in SQL against the store's own ISO-8601 timestamps,
    /// which sort lexicographically in time order, rather than parsing
    /// timestamps back out in application code.
    pub async fn list_stale_in_flight_effects(
        &self,
        stale_seconds: i64,
    ) -> Result<Vec<PendingEffectRecord>, StoreError> {
        let rows = sqlx::query(
            r#"
            SELECT id, effect_type, idempotency_key, payload_json, state, attempts, last_error, created_at, updated_at
            FROM pending_effects
            WHERE state = 'in_flight'
              AND updated_at <= strftime('%Y-%m-%dT%H:%M:%fZ', 'now', ?1 || ' seconds')
            ORDER BY updated_at ASC
            "#,
        )
        .bind(format!("-{stale_seconds}"))
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_pending_effect_record).collect()
    }

    pub async fn update_effect_state(
        &self,
        id: &str,
        state: &str,
        last_error: Option<&str>,
    ) -> Result<(), StoreError> {
        retry_on_transient_lock(|| self.update_effect_state_once(id, state, last_error, None, None))
            .await
            .map(|_| ())
    }

    /// Fenced variant used by the daemon's own effect worker.
    ///
    /// The write is refused (`DaemonAuthorityLost`) once a successor holds the
    /// lease, and it only applies while the effect is still in
    /// `expected_state`. A worker that read a `pending` effect and scanned its
    /// bag only after a successor settled it must not regress the newer result,
    /// so the guard is on the same transaction as the mutation. Returns `false`
    /// when the effect has moved on (the worker no longer owns the transition);
    /// an effect that does not exist is still a typed error.
    pub async fn update_effect_state_fenced(
        &self,
        id: &str,
        state: &str,
        last_error: Option<&str>,
        expected_state: &str,
        fence: &DaemonFence,
    ) -> Result<bool, StoreError> {
        retry_on_transient_lock(|| {
            self.update_effect_state_once(id, state, last_error, Some(expected_state), Some(fence))
        })
        .await
    }

    async fn update_effect_state_once(
        &self,
        id: &str,
        state: &str,
        last_error: Option<&str>,
        expected_state: Option<&str>,
        fence: Option<&DaemonFence>,
    ) -> Result<bool, StoreError> {
        let mut transaction = self.pool.begin().await?;

        if let Some(fence) = fence {
            assert_daemon_fence(&mut *transaction, fence).await?;
        }

        let res = sqlx::query(
            r#"
            UPDATE pending_effects
            SET state = ?1,
                attempts = attempts + 1,
                last_error = ?2,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            WHERE id = ?3
              AND (?4 IS NULL OR state = ?4)
            "#,
        )
        .bind(state)
        .bind(last_error)
        .bind(id)
        .bind(expected_state)
        .execute(&mut *transaction)
        .await?;

        if res.rows_affected() == 0 {
            if expected_state.is_none() {
                return Err(StoreError::PendingEffectNotFound { id: id.to_owned() });
            }
            // The guarded write matched nothing: either the effect moved on
            // under us (a successor settled or re-claimed it), or it never
            // existed. Only the second case is an error.
            let exists = sqlx::query("SELECT 1 FROM pending_effects WHERE id = ?1")
                .bind(id)
                .fetch_optional(&mut *transaction)
                .await?
                .is_some();
            if !exists {
                return Err(StoreError::PendingEffectNotFound { id: id.to_owned() });
            }
            transaction.commit().await?;
            return Ok(false);
        }

        transaction.commit().await?;
        Ok(true)
    }

    pub async fn record_effect_result(
        &self,
        id: &str,
        effect_id: &str,
        outcome: &str,
        result_json: &str,
    ) -> Result<EffectResultRecord, StoreError> {
        retry_on_transient_lock(|| {
            self.record_effect_result_once(id, effect_id, outcome, result_json, None)
        })
        .await
    }

    /// Fenced variant used by the daemon's `CompleteEffect` command path.
    pub async fn record_effect_result_fenced(
        &self,
        id: &str,
        effect_id: &str,
        outcome: &str,
        result_json: &str,
        fence: &DaemonFence,
    ) -> Result<EffectResultRecord, StoreError> {
        retry_on_transient_lock(|| {
            self.record_effect_result_once(id, effect_id, outcome, result_json, Some(fence))
        })
        .await
    }

    /// Fenced variant used by the daemon's own effect worker: records the
    /// handler's result and settles the effect, but only while this instance
    /// still owns the claim (`in_flight`).
    ///
    /// Both writes are one transaction, guarded by the lease fence and by the
    /// effect still being in-flight, so a worker result that lands after a
    /// successor already settled the effect is refused as a whole: it cannot
    /// flip a `completed` effect back to `failed`, and it cannot add a second,
    /// contradictory result row. Returns `false` when the claim was lost; a
    /// missing effect row is the same case, not an error, because the
    /// transition this worker wanted to own no longer exists to own.
    pub async fn settle_effect_from_worker_fenced(
        &self,
        id: &str,
        effect_id: &str,
        outcome: &str,
        result_json: &str,
        fence: &DaemonFence,
    ) -> Result<bool, StoreError> {
        retry_on_transient_lock(|| {
            self.settle_effect_from_worker_once(id, effect_id, outcome, result_json, fence)
        })
        .await
    }

    async fn settle_effect_from_worker_once(
        &self,
        id: &str,
        effect_id: &str,
        outcome: &str,
        result_json: &str,
        fence: &DaemonFence,
    ) -> Result<bool, StoreError> {
        let mut transaction = self.pool.begin().await?;

        assert_daemon_fence(&mut *transaction, fence).await?;

        let res = sqlx::query(
            r#"
            UPDATE pending_effects
            SET state = CASE WHEN ?1 = 'success' THEN 'completed' ELSE 'failed' END,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            WHERE id = ?2
              AND state = 'in_flight'
            "#,
        )
        .bind(outcome)
        .bind(effect_id)
        .execute(&mut *transaction)
        .await?;

        if res.rows_affected() == 0 {
            // This instance no longer owns the effect's transition: someone else
            // settled or re-claimed it after this worker read it. Record no
            // result, so no contradictory outcome is left behind.
            transaction.rollback().await?;
            return Ok(false);
        }

        sqlx::query(
            r#"
            INSERT OR IGNORE INTO effect_results (id, effect_id, outcome, result_json)
            VALUES (?1, ?2, ?3, ?4)
            "#,
        )
        .bind(id)
        .bind(effect_id)
        .bind(outcome)
        .bind(result_json)
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;
        Ok(true)
    }

    async fn record_effect_result_once(
        &self,
        id: &str,
        effect_id: &str,
        outcome: &str,
        result_json: &str,
        fence: Option<&DaemonFence>,
    ) -> Result<EffectResultRecord, StoreError> {
        let mut transaction = self.pool.begin().await?;

        if let Some(fence) = fence {
            assert_daemon_fence(&mut *transaction, fence).await?;
        }

        // OR IGNORE makes replaying the same CompleteEffect command (same
        // result id) after a crash idempotent: the subsequent SELECT below
        // still finds the row committed by the original attempt.
        sqlx::query(
            r#"
            INSERT OR IGNORE INTO effect_results (id, effect_id, outcome, result_json)
            VALUES (?1, ?2, ?3, ?4)
            "#,
        )
        .bind(id)
        .bind(effect_id)
        .bind(outcome)
        .bind(result_json)
        .execute(&mut *transaction)
        .await?;

        sqlx::query(
            r#"
            UPDATE pending_effects
            SET state = CASE WHEN ?1 = 'success' THEN 'completed' ELSE 'failed' END,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            WHERE id = ?2
            "#,
        )
        .bind(outcome)
        .bind(effect_id)
        .execute(&mut *transaction)
        .await?;

        let row = sqlx::query(
            r#"
            SELECT id, effect_id, outcome, result_json, created_at
            FROM effect_results
            WHERE id = ?1
            "#,
        )
        .bind(id)
        .fetch_one(&mut *transaction)
        .await?;

        let record = EffectResultRecord {
            id: row.try_get("id")?,
            effect_id: row.try_get("effect_id")?,
            outcome: row.try_get("outcome")?,
            result_json: row.try_get("result_json")?,
            created_at: row.try_get("created_at")?,
        };

        transaction.commit().await?;
        Ok(record)
    }

    pub async fn get_events_after(
        &self,
        after_sequence: i64,
        limit: i64,
    ) -> Result<Vec<EventRecord>, StoreError> {
        let rows = sqlx::query(
            r#"
            SELECT id, task_id, execution_id, event_kind, event_json, created_at
            FROM events
            WHERE id > ?1
            ORDER BY id ASC
            LIMIT ?2
            "#,
        )
        .bind(after_sequence)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                Ok(EventRecord {
                    sequence: row.try_get("id")?,
                    task_id: row.try_get("task_id")?,
                    execution_id: row.try_get("execution_id")?,
                    event_kind: row.try_get("event_kind")?,
                    event_json: row.try_get("event_json")?,
                    created_at: row.try_get("created_at")?,
                })
            })
            .collect()
    }

    pub async fn latest_event_sequence(&self) -> Result<i64, StoreError> {
        let max_id = sqlx::query_scalar::<_, Option<i64>>("SELECT MAX(id) FROM events")
            .fetch_one(&self.pool)
            .await?;

        Ok(max_id.unwrap_or(0))
    }

    /// Acquires or renews the daemon lease for one daemon *instance*.
    ///
    /// `holder_instance` identifies the individual instance, not the process:
    /// renewal requires the row to belong to this exact instance, so a second
    /// daemon started inside the same process is refused (`DaemonLeaseHeld`)
    /// while the first is live instead of silently sharing its fencing token.
    /// A row whose owner is stale (elapsed TTL) is still reclaimable by any
    /// successor, which increments the fencing token.
    pub async fn acquire_or_renew_daemon_lease(
        &self,
        lease_id: &str,
        holder_pid: i64,
        holder_host: &str,
        holder_instance: &str,
        ttl: Duration,
    ) -> Result<DaemonLeaseRecord, StoreError> {
        retry_on_transient_lock(|| {
            self.acquire_or_renew_daemon_lease_once(
                lease_id,
                holder_pid,
                holder_host,
                holder_instance,
                ttl,
            )
        })
        .await
    }

    async fn acquire_or_renew_daemon_lease_once(
        &self,
        lease_id: &str,
        holder_pid: i64,
        holder_host: &str,
        holder_instance: &str,
        ttl: Duration,
    ) -> Result<DaemonLeaseRecord, StoreError> {
        let mut transaction = self.pool.begin().await?;

        let existing = sqlx::query(
            r#"
            SELECT lease_id, holder_pid, holder_host, holder_instance, acquired_at, heartbeat_at, fencing_token,
                   CAST((strftime('%s', 'now') - strftime('%s', heartbeat_at)) AS INTEGER) as elapsed_secs
            FROM daemon_leases
            WHERE lease_id = ?1
            "#,
        )
        .bind(lease_id)
        .fetch_optional(&mut *transaction)
        .await?;

        if let Some(row) = existing {
            let current_pid: i64 = row.try_get("holder_pid")?;
            let current_host: String = row.try_get("holder_host")?;
            let current_instance: String = row.try_get("holder_instance")?;
            let elapsed_secs: Option<i64> = row.try_get("elapsed_secs")?;

            if current_pid == holder_pid
                && current_host == holder_host
                && current_instance == holder_instance
            {
                // Heartbeat renewal: this is the instance that already owns the row.
                sqlx::query(
                    r#"
                    UPDATE daemon_leases
                    SET heartbeat_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                    WHERE lease_id = ?1
                    "#,
                )
                .bind(lease_id)
                .execute(&mut *transaction)
                .await?;
            } else {
                let ttl_secs = ttl.as_secs() as i64;
                let is_stale = elapsed_secs.is_none_or(|s| s >= ttl_secs);

                if is_stale {
                    // A successor takes over the row - a different holder, or a
                    // second instance in the same process whose predecessor is
                    // gone. The fencing token advances so every write admitted
                    // under the previous owner is refused.
                    sqlx::query(
                        r#"
                        UPDATE daemon_leases
                        SET holder_pid = ?1,
                            holder_host = ?2,
                            holder_instance = ?3,
                            acquired_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                            heartbeat_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                            fencing_token = fencing_token + 1
                        WHERE lease_id = ?4
                        "#,
                    )
                    .bind(holder_pid)
                    .bind(holder_host)
                    .bind(holder_instance)
                    .bind(lease_id)
                    .execute(&mut *transaction)
                    .await?;
                } else {
                    return Err(StoreError::DaemonLeaseHeld {
                        lease_id: lease_id.to_string(),
                        holder_pid: current_pid,
                        holder_host: current_host,
                    });
                }
            }
        } else {
            sqlx::query(
                r#"
                INSERT INTO daemon_leases (lease_id, holder_pid, holder_host, holder_instance, fencing_token)
                VALUES (?1, ?2, ?3, ?4, 1)
                "#,
            )
            .bind(lease_id)
            .bind(holder_pid)
            .bind(holder_host)
            .bind(holder_instance)
            .execute(&mut *transaction)
            .await?;
        }

        let updated = sqlx::query(
            r#"
            SELECT lease_id, holder_pid, holder_host, holder_instance, acquired_at, heartbeat_at, fencing_token
            FROM daemon_leases
            WHERE lease_id = ?1
            "#,
        )
        .bind(lease_id)
        .fetch_one(&mut *transaction)
        .await?;

        let record = DaemonLeaseRecord {
            lease_id: updated.try_get("lease_id")?,
            holder_pid: updated.try_get("holder_pid")?,
            holder_host: updated.try_get("holder_host")?,
            holder_instance: updated.try_get("holder_instance")?,
            acquired_at: updated.try_get("acquired_at")?,
            heartbeat_at: updated.try_get("heartbeat_at")?,
            fencing_token: updated.try_get("fencing_token")?,
        };

        transaction.commit().await?;
        Ok(record)
    }

    /// Releases the daemon lease held by one daemon *instance*.
    ///
    /// Scoped to the exact instance that acquired it, so a failed or superseded
    /// instance can never delete the row of a live successor - not even a
    /// successor in the same process, where a pid-only delete would match.
    pub async fn release_daemon_lease(
        &self,
        lease_id: &str,
        holder_pid: i64,
        holder_instance: &str,
    ) -> Result<(), StoreError> {
        retry_on_transient_lock(|| {
            self.release_daemon_lease_once(lease_id, holder_pid, holder_instance)
        })
        .await
    }

    async fn release_daemon_lease_once(
        &self,
        lease_id: &str,
        holder_pid: i64,
        holder_instance: &str,
    ) -> Result<(), StoreError> {
        sqlx::query(
            r#"
            DELETE FROM daemon_leases
            WHERE lease_id = ?1
              AND holder_pid = ?2
              AND holder_instance = ?3
            "#,
        )
        .bind(lease_id)
        .bind(holder_pid)
        .bind(holder_instance)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Acquires a durable lease on a protected resource with monotonic fencing token allocation.
    ///
    /// If an active lease already exists on the resource:
    /// - If held by the same holder, updates expiry and returns the existing lease.
    /// - If expired (current time >= expires_at), marks the existing lease 'expired',
    ///   increments the fencing token monotonically, and inserts the new active lease.
    /// - If active and unexpired, returns `StoreError::ResourceLeaseHeld`.
    ///
    /// If no prior lease exists for the resource, inserts with fencing_token = 1.
    pub async fn acquire_resource_lease(
        &self,
        new_lease: &NewResourceLeaseRecord,
    ) -> Result<ResourceLeaseRecord, StoreError> {
        retry_on_transient_lock(|| self.acquire_resource_lease_once(new_lease)).await
    }

    async fn acquire_resource_lease_once(
        &self,
        new_lease: &NewResourceLeaseRecord,
    ) -> Result<ResourceLeaseRecord, StoreError> {
        let mut transaction = self.pool.begin().await?;

        // 1. Find the most recent lease for this resource (active or otherwise) to determine max fencing token
        let latest_token_row = sqlx::query(
            r#"
            SELECT COALESCE(MAX(fencing_token), 0) as max_token
            FROM resource_leases
            WHERE resource_type = ?1 AND resource_id = ?2
            "#,
        )
        .bind(&new_lease.resource_type)
        .bind(&new_lease.resource_id)
        .fetch_one(&mut *transaction)
        .await?;
        let max_token: i64 = latest_token_row.try_get("max_token")?;

        // 2. Check if there is currently an active lease on this resource.
        //
        // `expires_at` holds Unix epoch seconds — the canonical representation written
        // by relay-core's durable lease wrapper (`durable_state::acquire_durable_lease`)
        // — so expiry is compared as integers. Evaluating this with
        // `strftime('%s', expires_at)` previously returned NULL for epoch seconds, which
        // made every lease look unexpired and permanently blocked reclamation.
        //
        // A row whose expiry is not numeric casts to 0 and therefore reads as expired
        // and reclaimable. Such a row can only be produced out of band (no Relay writer
        // emits it), and reclaiming it grants no authority: the new holder gets a
        // strictly higher fencing token, and the domain layer
        // (`candidate::Lease::assert_valid`) treats an unparseable expiry as invalid
        // rather than as valid forever.
        let active_row = sqlx::query(
            r#"
            SELECT id, resource_type, resource_id, lease_kind, holder_id, holder_pid, holder_host,
                   fencing_token, acquired_at, expires_at, state,
                   CAST(strftime('%s', 'now') AS INTEGER) - CAST(expires_at AS INTEGER) as expired_diff
            FROM resource_leases
            WHERE resource_type = ?1 AND resource_id = ?2 AND state = 'active'
            ORDER BY fencing_token DESC
            LIMIT 1
            "#,
        )
        .bind(&new_lease.resource_type)
        .bind(&new_lease.resource_id)
        .fetch_optional(&mut *transaction)
        .await?;

        if let Some(row) = active_row {
            let active_id: String = row.try_get("id")?;
            let active_holder: String = row.try_get("holder_id")?;
            let active_expires: String = row.try_get("expires_at")?;
            let expired_diff: Option<i64> = row.try_get("expired_diff")?;
            let is_expired = expired_diff.is_some_and(|diff| diff >= 0);

            if active_holder == new_lease.holder_id && !is_expired {
                // Same holder renewing / extending active lease
                sqlx::query(
                    r#"
                    UPDATE resource_leases
                    SET expires_at = ?1
                    WHERE id = ?2
                    "#,
                )
                .bind(&new_lease.expires_at)
                .bind(&active_id)
                .execute(&mut *transaction)
                .await?;

                let updated = sqlx::query(
                    r#"
                    SELECT id, resource_type, resource_id, lease_kind, holder_id, holder_pid, holder_host,
                           fencing_token, acquired_at, expires_at, state
                    FROM resource_leases
                    WHERE id = ?1
                    "#,
                )
                .bind(&active_id)
                .fetch_one(&mut *transaction)
                .await?;

                let record = row_to_resource_lease_record(updated)?;
                transaction.commit().await?;
                return Ok(record);
            } else if is_expired {
                // Mark earlier lease as expired
                sqlx::query(
                    r#"
                    UPDATE resource_leases
                    SET state = 'expired'
                    WHERE id = ?1
                    "#,
                )
                .bind(&active_id)
                .execute(&mut *transaction)
                .await?;
            } else {
                // Still active and held by someone else
                return Err(StoreError::ResourceLeaseHeld {
                    resource_type: new_lease.resource_type.clone(),
                    resource_id: new_lease.resource_id.clone(),
                    holder_id: active_holder,
                    expires_at: active_expires,
                });
            }
        }

        // Allocate monotonically incremented fencing token
        let next_token = max_token + 1;

        sqlx::query(
            r#"
            INSERT INTO resource_leases (
                id, resource_type, resource_id, lease_kind,
                holder_id, holder_pid, holder_host,
                fencing_token, expires_at, state
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'active')
            "#,
        )
        .bind(&new_lease.id)
        .bind(&new_lease.resource_type)
        .bind(&new_lease.resource_id)
        .bind(&new_lease.lease_kind)
        .bind(&new_lease.holder_id)
        .bind(new_lease.holder_pid)
        .bind(&new_lease.holder_host)
        .bind(next_token)
        .bind(&new_lease.expires_at)
        .execute(&mut *transaction)
        .await?;

        let inserted = sqlx::query(
            r#"
            SELECT id, resource_type, resource_id, lease_kind, holder_id, holder_pid, holder_host,
                   fencing_token, acquired_at, expires_at, state
            FROM resource_leases
            WHERE id = ?1
            "#,
        )
        .bind(&new_lease.id)
        .fetch_one(&mut *transaction)
        .await?;

        let record = row_to_resource_lease_record(inserted)?;
        transaction.commit().await?;
        Ok(record)
    }

    /// Releases a resource lease, setting its state to 'released'.
    pub async fn release_resource_lease(
        &self,
        lease_id: &str,
        holder_id: &str,
    ) -> Result<(), StoreError> {
        retry_on_transient_lock(|| self.release_resource_lease_once(lease_id, holder_id)).await
    }

    async fn release_resource_lease_once(
        &self,
        lease_id: &str,
        holder_id: &str,
    ) -> Result<(), StoreError> {
        let result = sqlx::query(
            r#"
            UPDATE resource_leases
            SET state = 'released'
            WHERE id = ?1 AND holder_id = ?2 AND state = 'active'
            "#,
        )
        .bind(lease_id)
        .bind(holder_id)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            // Check if lease exists
            let exists =
                sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM resource_leases WHERE id = ?1")
                    .bind(lease_id)
                    .fetch_one(&self.pool)
                    .await?;

            if exists == 0 {
                return Err(StoreError::ResourceLeaseNotFound {
                    id: lease_id.to_string(),
                });
            }
        }

        Ok(())
    }

    /// Fetches a resource lease record by its ID.
    pub async fn get_resource_lease(
        &self,
        lease_id: &str,
    ) -> Result<Option<ResourceLeaseRecord>, StoreError> {
        let row = sqlx::query(
            r#"
            SELECT id, resource_type, resource_id, lease_kind, holder_id, holder_pid, holder_host,
                   fencing_token, acquired_at, expires_at, state
            FROM resource_leases
            WHERE id = ?1
            "#,
        )
        .bind(lease_id)
        .fetch_optional(&self.pool)
        .await?;

        row.map(row_to_resource_lease_record).transpose()
    }

    /// Fetches the currently active lease for a resource, if any.
    pub async fn get_active_resource_lease(
        &self,
        resource_type: &str,
        resource_id: &str,
    ) -> Result<Option<ResourceLeaseRecord>, StoreError> {
        let row = sqlx::query(
            r#"
            SELECT id, resource_type, resource_id, lease_kind, holder_id, holder_pid, holder_host,
                   fencing_token, acquired_at, expires_at, state
            FROM resource_leases
            WHERE resource_type = ?1 AND resource_id = ?2 AND state = 'active'
            ORDER BY fencing_token DESC
            LIMIT 1
            "#,
        )
        .bind(resource_type)
        .bind(resource_id)
        .fetch_optional(&self.pool)
        .await?;

        row.map(row_to_resource_lease_record).transpose()
    }

    // -----------------------------------------------------------------------
    // Wave B / R12-02: durable v2 workflow runs, role instances and attempts.
    //
    // Every write has a `*_fenced` variant that calls `assert_daemon_fence` as
    // the first statement of its own transaction, so a write admitted before
    // authority was lost cannot land after a successor took the daemon lease.
    // The unfenced variants are authority-unchecked; they exist only for
    // store-level test seeding and must not be used by the daemon-owned Run
    // path (the same convention as the unfenced command-ledger variants).
    // -----------------------------------------------------------------------

    /// Authority-unchecked insert of a workflow run. Returns the existing row
    /// unchanged on a replay.
    pub async fn insert_workflow_run_if_absent(
        &self,
        record: &NewWorkflowRunRecord,
    ) -> Result<WorkflowRunRecord, StoreError> {
        retry_on_transient_lock(|| self.insert_workflow_run_if_absent_once(record, None)).await
    }

    /// Fenced variant used by the daemon-owned Run path.
    pub async fn insert_workflow_run_if_absent_fenced(
        &self,
        record: &NewWorkflowRunRecord,
        fence: &DaemonFence,
    ) -> Result<WorkflowRunRecord, StoreError> {
        retry_on_transient_lock(|| self.insert_workflow_run_if_absent_once(record, Some(fence)))
            .await
    }

    async fn insert_workflow_run_if_absent_once(
        &self,
        record: &NewWorkflowRunRecord,
        fence: Option<&DaemonFence>,
    ) -> Result<WorkflowRunRecord, StoreError> {
        let mut transaction = self.pool.begin().await?;

        if let Some(fence) = fence {
            assert_daemon_fence(&mut *transaction, fence).await?;
        }

        sqlx::query(
            r#"
            INSERT OR IGNORE INTO workflow_runs (
                id, task_id, execution_grant_id, workflow_definition_id, state, config_snapshot_id
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
        )
        .bind(&record.id)
        .bind(&record.task_id)
        .bind(&record.execution_grant_id)
        .bind(&record.workflow_definition_id)
        .bind(&record.state)
        .bind(&record.config_snapshot_id)
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;

        self.get_workflow_run(&record.id)
            .await?
            .ok_or_else(|| StoreError::ExecutionNotFound {
                id: record.id.clone(),
            })
    }

    pub async fn get_workflow_run(
        &self,
        id: &str,
    ) -> Result<Option<WorkflowRunRecord>, StoreError> {
        let row = sqlx::query(
            r#"
            SELECT id, task_id, execution_grant_id, workflow_definition_id, state,
                   config_snapshot_id, started_at, completed_at,
                   approved_intent_json, execution_grant_json, config_snapshot_json
            FROM workflow_runs
            WHERE id = ?1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        row.map(row_to_workflow_run_record).transpose()
    }

    /// Start of a Run approval: insert the run with its authority, transition
    /// the task `ready -> running`, and append the single start event, all in
    /// one fenced transaction.
    ///
    /// Convergence: on a replay the run already exists and the task is already
    /// `running`, so the transition matches its desired end state, no second
    /// event is appended, and the committed run is returned. A task in any other
    /// state is a genuine conflict and refuses the write, which also rolls back
    /// the run insert.
    pub async fn start_workflow_run_fenced(
        &self,
        start: &WorkflowRunStart<'_>,
        fence: &DaemonFence,
    ) -> Result<WorkflowRunRecord, StoreError> {
        retry_on_transient_lock(|| self.start_workflow_run_once(start, fence)).await
    }

    async fn start_workflow_run_once(
        &self,
        start: &WorkflowRunStart<'_>,
        fence: &DaemonFence,
    ) -> Result<WorkflowRunRecord, StoreError> {
        let mut transaction = self.pool.begin().await?;

        assert_daemon_fence(&mut *transaction, fence).await?;

        let inserted = sqlx::query(
            r#"
            INSERT OR IGNORE INTO workflow_runs (
                id, task_id, execution_grant_id, workflow_definition_id, state,
                config_snapshot_id, approved_intent_json, execution_grant_json,
                config_snapshot_json
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            "#,
        )
        .bind(&start.run.id)
        .bind(&start.run.task_id)
        .bind(&start.run.execution_grant_id)
        .bind(&start.run.workflow_definition_id)
        .bind(&start.run.state)
        .bind(&start.run.config_snapshot_id)
        .bind(start.approved_intent_json)
        .bind(start.execution_grant_json)
        .bind(start.config_snapshot_json)
        .execute(&mut *transaction)
        .await?;

        let first_start = inserted.rows_affected() == 1;

        let transitioned = sqlx::query(
            r#"
            UPDATE tasks
            SET state = ?1,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            WHERE id = ?2
              AND state = ?3
            "#,
        )
        .bind(start.task_new_state)
        .bind(&start.run.task_id)
        .bind(start.task_expected_state)
        .execute(&mut *transaction)
        .await?;

        if transitioned.rows_affected() == 0 {
            let actual_state =
                sqlx::query_scalar::<_, String>("SELECT state FROM tasks WHERE id = ?1")
                    .bind(&start.run.task_id)
                    .fetch_optional(&mut *transaction)
                    .await?;

            match actual_state {
                // Already converged: a replay of a committed approval.
                Some(actual_state) if actual_state == start.task_new_state => {}
                Some(actual_state) => {
                    return Err(StoreError::StateConflict {
                        id: start.run.task_id.clone(),
                        expected_state: start.task_expected_state.to_owned(),
                        actual_state,
                    });
                }
                None => {
                    return Err(StoreError::TaskNotFound {
                        id: start.run.task_id.clone(),
                    });
                }
            }
        }

        // Exactly one start event per run: only the call that created the run
        // appends it, and that call's insert, transition and event share this
        // transaction.
        if first_start {
            insert_event(
                &mut transaction,
                Some(&start.run.task_id),
                Some(&start.run.id),
                start.event_kind,
                start.event_json,
            )
            .await?;
        }

        transaction.commit().await?;

        self.get_workflow_run(&start.run.id)
            .await?
            .ok_or_else(|| StoreError::ExecutionNotFound {
                id: start.run.id.clone(),
            })
    }

    /// Authority-unchecked workflow-run state transition (store tests only).
    pub async fn update_workflow_run_state(
        &self,
        id: &str,
        state: &str,
    ) -> Result<WorkflowRunRecord, StoreError> {
        retry_on_transient_lock(|| self.update_workflow_run_state_once(id, state, None)).await
    }

    /// Fenced variant used by the daemon-owned Run path.
    pub async fn update_workflow_run_state_fenced(
        &self,
        id: &str,
        state: &str,
        fence: &DaemonFence,
    ) -> Result<WorkflowRunRecord, StoreError> {
        retry_on_transient_lock(|| self.update_workflow_run_state_once(id, state, Some(fence)))
            .await
    }

    async fn update_workflow_run_state_once(
        &self,
        id: &str,
        state: &str,
        fence: Option<&DaemonFence>,
    ) -> Result<WorkflowRunRecord, StoreError> {
        let mut transaction = self.pool.begin().await?;

        if let Some(fence) = fence {
            assert_daemon_fence(&mut *transaction, fence).await?;
        }

        let result = sqlx::query(
            r#"
            UPDATE workflow_runs
            SET state = ?1,
                completed_at = CASE
                    WHEN ?1 IN ('completed', 'failed', 'cancelled') THEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                    ELSE completed_at
                END
            WHERE id = ?2
            "#,
        )
        .bind(state)
        .bind(id)
        .execute(&mut *transaction)
        .await?;

        if result.rows_affected() == 0 {
            return Err(StoreError::ExecutionNotFound { id: id.to_owned() });
        }

        transaction.commit().await?;

        self.get_workflow_run(id)
            .await?
            .ok_or_else(|| StoreError::ExecutionNotFound { id: id.to_owned() })
    }

    /// Authority-unchecked insert of a role instance. Returns the existing row
    /// unchanged on a replay.
    pub async fn insert_role_instance_if_absent(
        &self,
        record: &NewRoleInstanceRecord,
    ) -> Result<RoleInstanceRecord, StoreError> {
        retry_on_transient_lock(|| self.insert_role_instance_if_absent_once(record, None)).await
    }

    /// Fenced variant used by the daemon-owned Run path.
    pub async fn insert_role_instance_if_absent_fenced(
        &self,
        record: &NewRoleInstanceRecord,
        fence: &DaemonFence,
    ) -> Result<RoleInstanceRecord, StoreError> {
        retry_on_transient_lock(|| self.insert_role_instance_if_absent_once(record, Some(fence)))
            .await
    }

    async fn insert_role_instance_if_absent_once(
        &self,
        record: &NewRoleInstanceRecord,
        fence: Option<&DaemonFence>,
    ) -> Result<RoleInstanceRecord, StoreError> {
        let mut transaction = self.pool.begin().await?;

        if let Some(fence) = fence {
            assert_daemon_fence(&mut *transaction, fence).await?;
        }

        sqlx::query(
            r#"
            INSERT OR IGNORE INTO role_instances (
                id, workflow_run_id, role_definition_id, generation, state, parent_role_instance_id
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
        )
        .bind(&record.id)
        .bind(&record.workflow_run_id)
        .bind(&record.role_definition_id)
        .bind(record.generation)
        .bind(&record.state)
        .bind(record.parent_role_instance_id.as_deref())
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;

        self.get_role_instance(&record.id)
            .await?
            .ok_or_else(|| StoreError::ExecutionNotFound {
                id: record.id.clone(),
            })
    }

    pub async fn get_role_instance(
        &self,
        id: &str,
    ) -> Result<Option<RoleInstanceRecord>, StoreError> {
        let row = sqlx::query(
            r#"
            SELECT id, workflow_run_id, role_definition_id, generation, state,
                   parent_role_instance_id, accepted_result_artifact, attempts_count,
                   created_at, updated_at
            FROM role_instances
            WHERE id = ?1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        row.map(row_to_role_instance_record).transpose()
    }

    /// Authority-unchecked role-instance update (store tests only).
    pub async fn update_role_instance(
        &self,
        id: &str,
        state: &str,
        accepted_result_artifact: Option<&str>,
        attempts_count: i64,
    ) -> Result<RoleInstanceRecord, StoreError> {
        retry_on_transient_lock(|| {
            self.update_role_instance_once(
                id,
                state,
                accepted_result_artifact,
                attempts_count,
                None,
            )
        })
        .await
    }

    /// Fenced variant used by the daemon-owned Run path.
    pub async fn update_role_instance_fenced(
        &self,
        id: &str,
        state: &str,
        accepted_result_artifact: Option<&str>,
        attempts_count: i64,
        fence: &DaemonFence,
    ) -> Result<RoleInstanceRecord, StoreError> {
        retry_on_transient_lock(|| {
            self.update_role_instance_once(
                id,
                state,
                accepted_result_artifact,
                attempts_count,
                Some(fence),
            )
        })
        .await
    }

    async fn update_role_instance_once(
        &self,
        id: &str,
        state: &str,
        accepted_result_artifact: Option<&str>,
        attempts_count: i64,
        fence: Option<&DaemonFence>,
    ) -> Result<RoleInstanceRecord, StoreError> {
        let mut transaction = self.pool.begin().await?;

        if let Some(fence) = fence {
            assert_daemon_fence(&mut *transaction, fence).await?;
        }

        let result = sqlx::query(
            r#"
            UPDATE role_instances
            SET state = ?1,
                accepted_result_artifact = COALESCE(?2, accepted_result_artifact),
                attempts_count = ?3,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            WHERE id = ?4
            "#,
        )
        .bind(state)
        .bind(accepted_result_artifact)
        .bind(attempts_count)
        .bind(id)
        .execute(&mut *transaction)
        .await?;

        if result.rows_affected() == 0 {
            return Err(StoreError::ExecutionNotFound { id: id.to_owned() });
        }

        transaction.commit().await?;

        self.get_role_instance(id)
            .await?
            .ok_or_else(|| StoreError::ExecutionNotFound { id: id.to_owned() })
    }

    /// Authority-unchecked insert of a role attempt. Returns the existing row
    /// unchanged on a replay.
    pub async fn insert_role_attempt_if_absent(
        &self,
        record: &NewRoleAttemptRecord,
    ) -> Result<RoleAttemptRecord, StoreError> {
        retry_on_transient_lock(|| self.insert_role_attempt_if_absent_once(record, None)).await
    }

    /// Fenced variant used by the daemon-owned Run path.
    pub async fn insert_role_attempt_if_absent_fenced(
        &self,
        record: &NewRoleAttemptRecord,
        fence: &DaemonFence,
    ) -> Result<RoleAttemptRecord, StoreError> {
        retry_on_transient_lock(|| self.insert_role_attempt_if_absent_once(record, Some(fence)))
            .await
    }

    async fn insert_role_attempt_if_absent_once(
        &self,
        record: &NewRoleAttemptRecord,
        fence: Option<&DaemonFence>,
    ) -> Result<RoleAttemptRecord, StoreError> {
        let mut transaction = self.pool.begin().await?;

        if let Some(fence) = fence {
            assert_daemon_fence(&mut *transaction, fence).await?;
        }

        sqlx::query(
            r#"
            INSERT OR IGNORE INTO role_attempts (
                id, role_instance_id, attempt_no, execution_target_json, state,
                lease_id, fencing_token, started_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
            "#,
        )
        .bind(&record.id)
        .bind(&record.role_instance_id)
        .bind(record.attempt_no)
        .bind(&record.execution_target_json)
        .bind(&record.state)
        .bind(record.lease_id.as_deref())
        .bind(record.fencing_token)
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;

        self.get_role_attempt(&record.id)
            .await?
            .ok_or_else(|| StoreError::ExecutionNotFound {
                id: record.id.clone(),
            })
    }

    pub async fn get_role_attempt(
        &self,
        id: &str,
    ) -> Result<Option<RoleAttemptRecord>, StoreError> {
        let row = sqlx::query(
            r#"
            SELECT id, role_instance_id, attempt_no, execution_target_json, state,
                   lease_id, fencing_token, runtime_outcome, result_artifact_id,
                   started_at, ended_at, runtime_handle_id, runtime_backplane_id,
                   created_at, updated_at
            FROM role_attempts
            WHERE id = ?1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        row.map(row_to_role_attempt_record).transpose()
    }

    /// Records the write lease and fencing token an attempt was admitted
    /// under, so the durable attempt shows which lease generation it ran with.
    ///
    /// Authority-unchecked (store tests only); the daemon path uses
    /// [`SqliteStore::set_role_attempt_lease_fenced`].
    pub async fn set_role_attempt_lease(
        &self,
        id: &str,
        lease_id: &str,
        fencing_token: i64,
    ) -> Result<(), StoreError> {
        retry_on_transient_lock(|| {
            self.set_role_attempt_lease_once(id, lease_id, fencing_token, None)
        })
        .await
    }

    /// Fenced variant used by the daemon-owned Run path.
    pub async fn set_role_attempt_lease_fenced(
        &self,
        id: &str,
        lease_id: &str,
        fencing_token: i64,
        fence: &DaemonFence,
    ) -> Result<(), StoreError> {
        retry_on_transient_lock(|| {
            self.set_role_attempt_lease_once(id, lease_id, fencing_token, Some(fence))
        })
        .await
    }

    async fn set_role_attempt_lease_once(
        &self,
        id: &str,
        lease_id: &str,
        fencing_token: i64,
        fence: Option<&DaemonFence>,
    ) -> Result<(), StoreError> {
        let mut transaction = self.pool.begin().await?;

        if let Some(fence) = fence {
            assert_daemon_fence(&mut *transaction, fence).await?;
        }

        let result = sqlx::query(
            r#"
            UPDATE role_attempts
            SET lease_id = ?1,
                fencing_token = ?2,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            WHERE id = ?3
            "#,
        )
        .bind(lease_id)
        .bind(fencing_token)
        .bind(id)
        .execute(&mut *transaction)
        .await?;

        if result.rows_affected() == 0 {
            return Err(StoreError::ExecutionNotFound { id: id.to_owned() });
        }

        transaction.commit().await?;
        Ok(())
    }

    /// Records the backplane handle a role attempt was launched as.
    ///
    /// A restarted daemon uses this to ask the owning backplane about the
    /// process it actually launched, instead of guessing a handle identity.
    ///
    /// Authority-unchecked (store tests only); the daemon path uses
    /// [`SqliteStore::set_role_attempt_runtime_handle_fenced`].
    pub async fn set_role_attempt_runtime_handle(
        &self,
        id: &str,
        handle_id: &str,
        backplane_id: &str,
    ) -> Result<(), StoreError> {
        retry_on_transient_lock(|| {
            self.set_role_attempt_runtime_handle_once(id, handle_id, backplane_id, None)
        })
        .await
    }

    /// Fenced variant used by the daemon-owned Run path.
    pub async fn set_role_attempt_runtime_handle_fenced(
        &self,
        id: &str,
        handle_id: &str,
        backplane_id: &str,
        fence: &DaemonFence,
    ) -> Result<(), StoreError> {
        retry_on_transient_lock(|| {
            self.set_role_attempt_runtime_handle_once(id, handle_id, backplane_id, Some(fence))
        })
        .await
    }

    async fn set_role_attempt_runtime_handle_once(
        &self,
        id: &str,
        handle_id: &str,
        backplane_id: &str,
        fence: Option<&DaemonFence>,
    ) -> Result<(), StoreError> {
        let mut transaction = self.pool.begin().await?;

        if let Some(fence) = fence {
            assert_daemon_fence(&mut *transaction, fence).await?;
        }

        let result = sqlx::query(
            r#"
            UPDATE role_attempts
            SET runtime_handle_id = ?1,
                runtime_backplane_id = ?2,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            WHERE id = ?3
            "#,
        )
        .bind(handle_id)
        .bind(backplane_id)
        .bind(id)
        .execute(&mut *transaction)
        .await?;

        if result.rows_affected() == 0 {
            return Err(StoreError::ExecutionNotFound { id: id.to_owned() });
        }

        transaction.commit().await?;
        Ok(())
    }

    /// Authority-unchecked role-attempt update (store tests only).
    pub async fn update_role_attempt(
        &self,
        id: &str,
        state: &str,
        runtime_outcome: Option<&str>,
        result_artifact_id: Option<&str>,
    ) -> Result<RoleAttemptRecord, StoreError> {
        retry_on_transient_lock(|| {
            self.update_role_attempt_once(id, state, runtime_outcome, result_artifact_id, None)
        })
        .await
    }

    /// Fenced variant used by the daemon-owned Run path.
    pub async fn update_role_attempt_fenced(
        &self,
        id: &str,
        state: &str,
        runtime_outcome: Option<&str>,
        result_artifact_id: Option<&str>,
        fence: &DaemonFence,
    ) -> Result<RoleAttemptRecord, StoreError> {
        retry_on_transient_lock(|| {
            self.update_role_attempt_once(
                id,
                state,
                runtime_outcome,
                result_artifact_id,
                Some(fence),
            )
        })
        .await
    }

    async fn update_role_attempt_once(
        &self,
        id: &str,
        state: &str,
        runtime_outcome: Option<&str>,
        result_artifact_id: Option<&str>,
        fence: Option<&DaemonFence>,
    ) -> Result<RoleAttemptRecord, StoreError> {
        let mut transaction = self.pool.begin().await?;

        if let Some(fence) = fence {
            assert_daemon_fence(&mut *transaction, fence).await?;
        }

        let result = sqlx::query(
            r#"
            UPDATE role_attempts
            SET state = ?1,
                runtime_outcome = COALESCE(?2, runtime_outcome),
                result_artifact_id = COALESCE(?3, result_artifact_id),
                ended_at = CASE
                    WHEN ?1 IN ('completed', 'failed', 'cancelled', 'lost') THEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                    ELSE ended_at
                END,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            WHERE id = ?4
            "#,
        )
        .bind(state)
        .bind(runtime_outcome)
        .bind(result_artifact_id)
        .bind(id)
        .execute(&mut *transaction)
        .await?;

        if result.rows_affected() == 0 {
            return Err(StoreError::ExecutionNotFound { id: id.to_owned() });
        }

        transaction.commit().await?;

        self.get_role_attempt(id)
            .await?
            .ok_or_else(|| StoreError::ExecutionNotFound { id: id.to_owned() })
    }

    pub async fn list_active_workflow_runs(&self) -> Result<Vec<WorkflowRunRecord>, StoreError> {
        let rows = sqlx::query(
            r#"
            SELECT id, task_id, execution_grant_id, workflow_definition_id, state,
                   config_snapshot_id, started_at, completed_at,
                   approved_intent_json, execution_grant_json, config_snapshot_json
            FROM workflow_runs
            WHERE state IN ('pending', 'running')
            ORDER BY started_at ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_workflow_run_record).collect()
    }

    pub async fn list_role_instances_for_run(
        &self,
        workflow_run_id: &str,
    ) -> Result<Vec<RoleInstanceRecord>, StoreError> {
        let rows = sqlx::query(
            r#"
            SELECT id, workflow_run_id, role_definition_id, generation, state,
                   parent_role_instance_id, accepted_result_artifact, attempts_count,
                   created_at, updated_at
            FROM role_instances
            WHERE workflow_run_id = ?1
            ORDER BY created_at ASC
            "#,
        )
        .bind(workflow_run_id)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_role_instance_record).collect()
    }

    pub async fn list_role_attempts_for_instance(
        &self,
        role_instance_id: &str,
    ) -> Result<Vec<RoleAttemptRecord>, StoreError> {
        let rows = sqlx::query(
            r#"
            SELECT id, role_instance_id, attempt_no, execution_target_json, state,
                   lease_id, fencing_token, runtime_outcome, result_artifact_id,
                   started_at, ended_at, runtime_handle_id, runtime_backplane_id,
                   created_at, updated_at
            FROM role_attempts
            WHERE role_instance_id = ?1
            ORDER BY attempt_no ASC
            "#,
        )
        .bind(role_instance_id)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_role_attempt_record).collect()
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

/// Runs a write operation, retrying transient SQLite lock contention with
/// bounded exponential backoff before surfacing a typed error.
///
/// Each attempt re-runs the whole operation (transactions are never resumed
/// mid-flight), so a retry always starts from a clean transaction boundary.
async fn retry_on_transient_lock<T, F, Fut>(mut operation: F) -> Result<T, StoreError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, StoreError>>,
{
    let mut attempt = 1usize;
    loop {
        match operation().await {
            Err(error) if is_transient_lock_error(&error) => {
                if attempt >= WRITE_RETRY_ATTEMPTS {
                    return Err(match error {
                        StoreError::Sqlx(source) => StoreError::DatabaseBusy {
                            attempts: attempt as u32,
                            source,
                        },
                        other => other,
                    });
                }
                let backoff = WRITE_RETRY_BASE_BACKOFF
                    .saturating_mul(2u32.pow(attempt as u32 - 1))
                    .min(WRITE_RETRY_MAX_BACKOFF);
                tokio::time::sleep(backoff).await;
                attempt += 1;
            }
            other => return other,
        }
    }
}

fn is_transient_lock_error(error: &StoreError) -> bool {
    let StoreError::Sqlx(sqlx::Error::Database(database_error)) = error else {
        return false;
    };
    database_error
        .code()
        .and_then(|code| code.parse::<i32>().ok())
        .is_some_and(|code| {
            matches!(
                code & 0xff,
                SQLITE_BUSY_PRIMARY_CODE | SQLITE_LOCKED_PRIMARY_CODE
            )
        })
}

fn map_migration_error(error: sqlx::migrate::MigrateError) -> StoreError {
    match error {
        // The database was written by a different Relay build; fail closed.
        sqlx::migrate::MigrateError::VersionMissing(version)
        | sqlx::migrate::MigrateError::VersionMismatch(version)
        | sqlx::migrate::MigrateError::VersionNotPresent(version) => {
            StoreError::SchemaVersionMismatch { version }
        }
        sqlx::migrate::MigrateError::Dirty(version) => StoreError::SchemaMigrationDirty { version },
        other => StoreError::Migration(other),
    }
}

/// Runs migrations, tolerating benign cross-process races.
///
/// Relay processes (TUI, `relay daemon`, serve-mcp) may open a database that
/// has never been migrated at the same moment. sqlx serializes each
/// migration's DDL in its own transaction but not the applied-version
/// bookkeeping across processes, so the loser of the race can surface either
/// symptom:
///
/// - a UNIQUE constraint on `_sqlx_migrations.version`, when both processes
///   raced the version bookkeeping; or
/// - `... already exists`, when the loser read the migration history before the
///   winner committed, then blocked on the winner's write lock and finally
///   re-ran DDL the winner had committed in the meantime. Migrations
///   deliberately avoid `IF NOT EXISTS` so that a genuinely partial schema
///   cannot be silently accepted.
///
/// In both cases the winner has finished applying that migration, so a bounded
/// retry re-reads the completed history and succeeds without re-running DDL.
/// Every other failure - including a real schema conflict that reproduces on
/// every attempt - fails closed.
async fn run_migrations(pool: &SqlitePool) -> Result<(), StoreError> {
    let mut attempt = 1usize;

    loop {
        match MIGRATOR.run(pool).await {
            Ok(()) => return Ok(()),
            Err(error) => {
                if attempt >= MIGRATION_RACE_ATTEMPTS || !is_migration_race(&error) {
                    return Err(map_migration_error(error));
                }

                // relay-store deliberately avoids a tracing dependency; the
                // retry is observable only as normal latency.
                tokio::time::sleep(MIGRATION_RACE_BACKOFF * attempt as u32).await;
                attempt += 1;
            }
        }
    }
}

/// Recognizes the two symptoms of two processes racing to apply the same
/// migration (see [`run_migrations`]).
fn is_migration_race(error: &sqlx::migrate::MigrateError) -> bool {
    let display = error.to_string();

    (display.contains("_sqlx_migrations") && display.contains("UNIQUE constraint"))
        || display.contains("already exists")
        // A migration that adds a column (0002, 0008, 0009) fails with this
        // message - not "already exists" - when a concurrent first-time
        // migrator applied it first. Same race, same recovery.
        || display.contains("duplicate column name")
}

async fn insert_event(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    task_id: Option<&str>,
    execution_id: Option<&str>,
    event_kind: &str,
    event_json: Option<&str>,
) -> Result<(), StoreError> {
    sqlx::query(
        r#"
        INSERT INTO events (task_id, execution_id, event_kind, event_json)
        VALUES (?1, ?2, ?3, ?4)
        "#,
    )
    .bind(task_id)
    .bind(execution_id)
    .bind(event_kind)
    .bind(event_json)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// Asserts that the daemon lease row still belongs to this daemon instance at
/// the expected fencing token. Evaluated as the first statement of the same
/// transaction as the mutation it guards, so it cannot be satisfied by a
/// lease that was valid when the command was admitted but lost before the
/// write committed.
///
/// The instance identity is part of the comparison: a pid and token alone cannot
/// distinguish two daemons sharing one process, nor a row re-acquired at the same
/// pid after a release (where the token restarts at 1).
async fn assert_daemon_fence<'a>(
    executor: impl sqlx::Executor<'a, Database = sqlx::Sqlite>,
    fence: &DaemonFence,
) -> Result<(), StoreError> {
    let row = sqlx::query(
        r#"
        SELECT holder_pid, holder_instance, fencing_token
        FROM daemon_leases
        WHERE lease_id = ?1
        "#,
    )
    .bind(&fence.lease_id)
    .fetch_optional(executor)
    .await?;

    let still_holds = match row {
        Some(row) => {
            row.try_get::<i64, _>("holder_pid")? == fence.holder_pid
                && row.try_get::<String, _>("holder_instance")? == fence.holder_instance
                && row.try_get::<i64, _>("fencing_token")? == fence.fencing_token
        }
        None => false,
    };

    if still_holds {
        Ok(())
    } else {
        Err(StoreError::DaemonAuthorityLost {
            lease_id: fence.lease_id.clone(),
        })
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

async fn fetch_execution<'a>(
    executor: impl sqlx::Executor<'a, Database = sqlx::Sqlite>,
    id: &str,
) -> Result<Option<ExecutionRecord>, StoreError> {
    let row = sqlx::query(
        r#"
        SELECT
            id,
            task_id,
            runtime_profile,
            phase,
            repair_attempt_count,
            started_at,
            ended_at,
            exit_outcome,
            failure_category,
            repository_snapshot_json,
            runtime_metadata_json
        FROM executions
        WHERE id = ?1
        "#,
    )
    .bind(id)
    .fetch_optional(executor)
    .await?;

    row.map(row_to_execution_record).transpose()
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

fn row_to_execution_record(row: sqlx::sqlite::SqliteRow) -> Result<ExecutionRecord, StoreError> {
    Ok(ExecutionRecord {
        id: row.try_get("id")?,
        task_id: row.try_get("task_id")?,
        runtime_profile: row.try_get("runtime_profile")?,
        phase: row.try_get("phase")?,
        repair_attempt_count: row.try_get("repair_attempt_count")?,
        started_at: row.try_get("started_at")?,
        ended_at: row.try_get("ended_at")?,
        exit_outcome: row.try_get("exit_outcome")?,
        failure_category: row.try_get("failure_category")?,
        repository_snapshot_json: row.try_get("repository_snapshot_json")?,
        runtime_metadata_json: row.try_get("runtime_metadata_json")?,
    })
}

fn row_to_pending_effect_record(
    row: sqlx::sqlite::SqliteRow,
) -> Result<PendingEffectRecord, StoreError> {
    Ok(PendingEffectRecord {
        id: row.try_get("id")?,
        effect_type: row.try_get("effect_type")?,
        idempotency_key: row.try_get("idempotency_key")?,
        payload_json: row.try_get("payload_json")?,
        state: row.try_get("state")?,
        attempts: row.try_get("attempts")?,
        last_error: row.try_get("last_error")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn row_to_resource_lease_record(
    row: sqlx::sqlite::SqliteRow,
) -> Result<ResourceLeaseRecord, StoreError> {
    Ok(ResourceLeaseRecord {
        id: row.try_get("id")?,
        resource_type: row.try_get("resource_type")?,
        resource_id: row.try_get("resource_id")?,
        lease_kind: row.try_get("lease_kind")?,
        holder_id: row.try_get("holder_id")?,
        holder_pid: row.try_get("holder_pid")?,
        holder_host: row.try_get("holder_host")?,
        fencing_token: row.try_get("fencing_token")?,
        acquired_at: row.try_get("acquired_at")?,
        expires_at: row.try_get("expires_at")?,
        state: row.try_get("state")?,
    })
}

fn row_to_workflow_run_record(
    row: sqlx::sqlite::SqliteRow,
) -> Result<WorkflowRunRecord, StoreError> {
    Ok(WorkflowRunRecord {
        id: row.try_get("id")?,
        task_id: row.try_get("task_id")?,
        execution_grant_id: row.try_get("execution_grant_id")?,
        workflow_definition_id: row.try_get("workflow_definition_id")?,
        state: row.try_get("state")?,
        config_snapshot_id: row.try_get("config_snapshot_id")?,
        started_at: row.try_get("started_at")?,
        completed_at: row.try_get("completed_at")?,
        approved_intent_json: row.try_get("approved_intent_json")?,
        execution_grant_json: row.try_get("execution_grant_json")?,
        config_snapshot_json: row.try_get("config_snapshot_json")?,
    })
}

fn row_to_role_instance_record(
    row: sqlx::sqlite::SqliteRow,
) -> Result<RoleInstanceRecord, StoreError> {
    Ok(RoleInstanceRecord {
        id: row.try_get("id")?,
        workflow_run_id: row.try_get("workflow_run_id")?,
        role_definition_id: row.try_get("role_definition_id")?,
        generation: row.try_get("generation")?,
        state: row.try_get("state")?,
        parent_role_instance_id: row.try_get("parent_role_instance_id")?,
        accepted_result_artifact: row.try_get("accepted_result_artifact")?,
        attempts_count: row.try_get("attempts_count")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn row_to_role_attempt_record(
    row: sqlx::sqlite::SqliteRow,
) -> Result<RoleAttemptRecord, StoreError> {
    Ok(RoleAttemptRecord {
        id: row.try_get("id")?,
        role_instance_id: row.try_get("role_instance_id")?,
        attempt_no: row.try_get("attempt_no")?,
        execution_target_json: row.try_get("execution_target_json")?,
        state: row.try_get("state")?,
        lease_id: row.try_get("lease_id")?,
        fencing_token: row.try_get("fencing_token")?,
        runtime_outcome: row.try_get("runtime_outcome")?,
        result_artifact_id: row.try_get("result_artifact_id")?,
        started_at: row.try_get("started_at")?,
        ended_at: row.try_get("ended_at")?,
        runtime_handle_id: row.try_get("runtime_handle_id")?,
        runtime_backplane_id: row.try_get("runtime_backplane_id")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

/// Proof that a caller may still write on behalf of a daemon lease.
///
/// A daemon mutates durable state inside a transaction that first asserts the
/// `daemon_leases` row still belongs to this exact daemon instance at this exact
/// fencing token. If a successor has taken the lease (which increments the
/// token), the assertion fails and the mutation is refused, so a command
/// admitted before authority was lost cannot commit after a successor took
/// over.
///
/// The instance identity is part of the fence, not just the process: two daemon
/// instances inside one process share a pid and hostname, and a released row can
/// be re-acquired at the same pid with the token restarting at 1, so a
/// pid-plus-token fence alone would let a previous instance's write match a
/// successor's row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonFence {
    pub lease_id: String,
    pub holder_pid: i64,
    pub holder_instance: String,
    pub fencing_token: i64,
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
    #[error("execution {id} was not found")]
    ExecutionNotFound { id: String },
    #[error("command {id} was not found")]
    CommandNotFound { id: String },
    #[error("pending effect {id} was not found")]
    PendingEffectNotFound { id: String },
    #[error("daemon lease '{lease_id}' is held by pid {holder_pid} on {holder_host}")]
    DaemonLeaseHeld {
        lease_id: String,
        holder_pid: i64,
        holder_host: String,
    },
    #[error(
        "daemon lease '{lease_id}' is no longer held by this process at the expected fencing token; refusing a stale write"
    )]
    DaemonAuthorityLost { lease_id: String },
    #[error(
        "resource lease on {resource_type}:{resource_id} is active and held by {holder_id} (expires {expires_at})"
    )]
    ResourceLeaseHeld {
        resource_type: String,
        resource_id: String,
        holder_id: String,
        expires_at: String,
    },
    #[error("resource lease '{id}' not found")]
    ResourceLeaseNotFound { id: String },
    #[error("task {id} state conflict: expected '{expected_state}', found '{actual_state}'")]
    StateConflict {
        id: String,
        expected_state: String,
        actual_state: String,
    },
    #[error(
        "database stayed locked after {attempts} attempts (WAL busy timeout exceeded): {source}"
    )]
    DatabaseBusy { attempts: u32, source: sqlx::Error },
    #[error(
        "database schema was written by a different Relay build (unexpected migration version {version}); refusing to open"
    )]
    SchemaVersionMismatch { version: i64 },
    #[error(
        "database schema is partially migrated (dirty migration version {version}); refusing to open"
    )]
    SchemaMigrationDirty { version: i64 },
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
    async fn file_database_connections_set_concurrency_pragmas() {
        let database_path = temp_path("relay-pragmas.sqlite3");
        let store = SqliteStore::open(&database_path).await.unwrap();

        let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        let busy_timeout: i64 = sqlx::query_scalar("PRAGMA busy_timeout")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&store.pool)
            .await
            .unwrap();

        assert_eq!(journal_mode, "wal");
        assert_eq!(busy_timeout, 5000);
        assert_eq!(foreign_keys, 1);
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

    #[tokio::test]
    async fn claims_ready_task_and_creates_execution_transactionally() {
        let store = SqliteStore::in_memory().await.unwrap();
        store
            .insert_task(&task_record("task-exec", "Execute task"))
            .await
            .unwrap();

        let (task, execution) = store
            .claim_ready_task_for_execution(&execution_record("exec-1", "task-exec"))
            .await
            .unwrap();

        assert_eq!(task.state, "running");
        assert_eq!(execution.id, "exec-1");
        assert_eq!(execution.task_id, "task-exec");
        assert_eq!(execution.phase, "running");
        assert_eq!(execution.repair_attempt_count, 0);
        assert_eq!(
            store.get_task("task-exec").await.unwrap().unwrap().state,
            "running"
        );
    }

    #[tokio::test]
    async fn execution_claim_rolls_back_when_execution_insert_fails() {
        let store = SqliteStore::in_memory().await.unwrap();
        store
            .insert_task(&task_record("task-rollback-exec", "Rollback execution"))
            .await
            .unwrap();
        sqlx::query(
            r#"
            CREATE TRIGGER fail_execution_insert
            BEFORE INSERT ON executions
            BEGIN
                SELECT RAISE(ABORT, 'forced execution failure');
            END
            "#,
        )
        .execute(&store.pool)
        .await
        .unwrap();

        let error = store
            .claim_ready_task_for_execution(&execution_record(
                "exec-rollback",
                "task-rollback-exec",
            ))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("SQLite operation failed"));
        assert_eq!(
            store
                .get_task("task-rollback-exec")
                .await
                .unwrap()
                .unwrap()
                .state,
            "ready"
        );
        assert!(
            store
                .get_execution("exec-rollback")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn execution_phase_repair_count_and_terminal_outcomes_persist() {
        let store = SqliteStore::in_memory().await.unwrap();
        store
            .insert_task(&task_record("task-life", "Lifecycle task"))
            .await
            .unwrap();
        store
            .claim_ready_task_for_execution(&execution_record("exec-life", "task-life"))
            .await
            .unwrap();

        let execution = store
            .update_execution_phase("exec-life", "verifying")
            .await
            .unwrap();
        assert_eq!(execution.phase, "verifying");

        let (_task, execution) = store
            .transition_task_and_execution(
                "task-life",
                "running",
                "repairing",
                "exec-life",
                "repairing",
            )
            .await
            .unwrap();
        assert_eq!(execution.phase, "repairing");

        let execution = store
            .increment_execution_repair_count("exec-life")
            .await
            .unwrap();
        assert_eq!(execution.repair_attempt_count, 1);

        store
            .finish_execution(
                "task-life",
                "repairing",
                "failed",
                "exec-life",
                "implementation_failure",
                Some("implementation_failure"),
            )
            .await
            .unwrap();
        let execution = store.get_execution("exec-life").await.unwrap().unwrap();
        assert_eq!(execution.phase, "finished");
        assert_eq!(
            execution.exit_outcome.as_deref(),
            Some("implementation_failure")
        );
        assert_eq!(
            execution.failure_category.as_deref(),
            Some("implementation_failure")
        );
        assert!(execution.ended_at.is_some());
    }

    #[tokio::test]
    async fn repair_start_transition_and_count_increment_are_atomic() {
        let store = SqliteStore::in_memory().await.unwrap();
        store
            .insert_task(&task_record("task-repair-atomic", "Atomic repair"))
            .await
            .unwrap();
        store
            .claim_ready_task_for_execution(&execution_record(
                "exec-repair-atomic",
                "task-repair-atomic",
            ))
            .await
            .unwrap();

        let (task, execution) = store
            .transition_task_and_increment_repair_count(
                "task-repair-atomic",
                "running",
                "exec-repair-atomic",
            )
            .await
            .unwrap();

        assert_eq!(task.state, "repairing");
        assert_eq!(execution.phase, "repairing");
        assert_eq!(execution.repair_attempt_count, 1);
    }

    #[tokio::test]
    async fn terminal_task_states_remain_consistent_with_execution_finish() {
        for terminal in [
            "done",
            "failed",
            "cancelled",
            "needs_clarification",
            "needs_decision",
        ] {
            let store = SqliteStore::in_memory().await.unwrap();
            let task_id = format!("task-{terminal}");
            let execution_id = format!("exec-{terminal}");
            store
                .insert_task(&task_record(&task_id, "Terminal task"))
                .await
                .unwrap();
            store
                .claim_ready_task_for_execution(&execution_record(&execution_id, &task_id))
                .await
                .unwrap();

            let (task, execution) = store
                .finish_execution(
                    &task_id,
                    "running",
                    terminal,
                    &execution_id,
                    terminal,
                    (terminal != "done").then_some(terminal),
                )
                .await
                .unwrap();

            assert_eq!(task.state, terminal);
            assert_eq!(execution.phase, "finished");
            assert_eq!(execution.exit_outcome.as_deref(), Some(terminal));
            assert_eq!(
                store.get_task(&task_id).await.unwrap().unwrap().state,
                terminal
            );
        }
    }

    #[tokio::test]
    async fn stale_execution_lifecycle_updates_are_rejected() {
        let store = SqliteStore::in_memory().await.unwrap();
        store
            .insert_task(&task_record("task-stale-exec", "Stale execution"))
            .await
            .unwrap();
        store
            .update_task_state("task-stale-exec", "ready", "cancelled")
            .await
            .unwrap();

        let error = store
            .claim_ready_task_for_execution(&execution_record("exec-stale", "task-stale-exec"))
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            StoreError::StateConflict {
                expected_state,
                actual_state,
                ..
            } if expected_state == "ready" && actual_state == "cancelled"
        ));
        assert!(store.get_execution("exec-stale").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn completed_execution_metadata_survives_reopen() {
        let database_path = temp_path("relay-execution-metadata.sqlite3");

        {
            let store = SqliteStore::open(&database_path).await.unwrap();
            store
                .insert_task(&task_record("task-reopen-exec", "Reopen execution"))
                .await
                .unwrap();
            store
                .claim_ready_task_for_execution(&execution_record(
                    "exec-reopen",
                    "task-reopen-exec",
                ))
                .await
                .unwrap();
            store
                .finish_execution(
                    "task-reopen-exec",
                    "running",
                    "done",
                    "exec-reopen",
                    "success",
                    None,
                )
                .await
                .unwrap();
        }

        let reopened = SqliteStore::open(&database_path).await.unwrap();
        let execution = reopened
            .get_execution("exec-reopen")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(execution.task_id, "task-reopen-exec");
        assert_eq!(execution.phase, "finished");
        assert_eq!(execution.exit_outcome.as_deref(), Some("success"));
    }

    #[tokio::test]
    async fn commands_idempotency_and_completion() {
        let store = SqliteStore::in_memory().await.unwrap();

        let inserted = store
            .record_command("cmd-1", "submit_handoff", r#"{"title":"test"}"#)
            .await
            .unwrap();
        assert!(inserted, "first record_command should return true");

        let duplicate = store
            .record_command("cmd-1", "submit_handoff", r#"{"title":"test"}"#)
            .await
            .unwrap();
        assert!(
            !duplicate,
            "duplicate record_command should return false (idempotent)"
        );

        let cmd = store.get_command("cmd-1").await.unwrap().unwrap();
        assert_eq!(cmd.status, "received");

        store
            .complete_command("cmd-1", "applied", Some(r#"{"result":"ok"}"#))
            .await
            .unwrap();

        let completed = store.get_command("cmd-1").await.unwrap().unwrap();
        assert_eq!(completed.status, "applied");
        assert_eq!(
            completed.response_json.as_deref(),
            Some(r#"{"result":"ok"}"#)
        );
    }

    #[tokio::test]
    async fn pending_effects_lifecycle_and_idempotency_collision() {
        let store = SqliteStore::in_memory().await.unwrap();

        let effect = NewPendingEffectRecord {
            id: "eff-1".to_owned(),
            effect_type: "process_spawn".to_owned(),
            idempotency_key: "key-123".to_owned(),
            payload_json: r#"{"cmd":"cargo"}"#.to_owned(),
        };

        let created = store.insert_pending_effect(&effect).await.unwrap();
        assert_eq!(created.state, "pending");
        assert_eq!(created.attempts, 0);

        // Duplicate idempotency key must fail closed
        let duplicate = NewPendingEffectRecord {
            id: "eff-2".to_owned(),
            effect_type: "process_spawn".to_owned(),
            idempotency_key: "key-123".to_owned(),
            payload_json: r#"{"cmd":"cargo"}"#.to_owned(),
        };
        assert!(store.insert_pending_effect(&duplicate).await.is_err());

        store
            .update_effect_state("eff-1", "in_flight", None)
            .await
            .unwrap();
        let in_flight = store.get_pending_effect("eff-1").await.unwrap().unwrap();
        assert_eq!(in_flight.state, "in_flight");
        assert_eq!(in_flight.attempts, 1);

        let result = store
            .record_effect_result("res-1", "eff-1", "success", r#"{"pid":42}"#)
            .await
            .unwrap();
        assert_eq!(result.outcome, "success");

        let completed = store.get_pending_effect("eff-1").await.unwrap().unwrap();
        assert_eq!(completed.state, "completed");
    }

    #[tokio::test]
    async fn monotonic_events_retrieval_and_latest_sequence() {
        let store = SqliteStore::in_memory().await.unwrap();
        store
            .insert_task(&task_record("task-1", "Test Task"))
            .await
            .unwrap();

        store
            .append_event(Some("task-1"), None, "step_1", Some(r#"{"n":1}"#))
            .await
            .unwrap();
        store
            .append_event(Some("task-1"), None, "step_2", Some(r#"{"n":2}"#))
            .await
            .unwrap();
        store
            .append_event(Some("task-1"), None, "step_3", Some(r#"{"n":3}"#))
            .await
            .unwrap();

        let latest = store.latest_event_sequence().await.unwrap();
        assert_eq!(latest, 4);

        let events = store.get_events_after(1, 10).await.unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].sequence, 2);
        assert_eq!(events[0].event_kind, "step_1");
        assert_eq!(events[1].sequence, 3);
        assert_eq!(events[1].event_kind, "step_2");
        assert_eq!(events[2].sequence, 4);
        assert_eq!(events[2].event_kind, "step_3");
    }

    #[tokio::test]
    async fn daemon_lease_acquisition_renewal_rejection_and_stale_recovery() {
        let store = SqliteStore::in_memory().await.unwrap();

        let lease = store
            .acquire_or_renew_daemon_lease(
                "daemon-main",
                1001,
                "host-a",
                "inst-1001",
                Duration::from_secs(60),
            )
            .await
            .unwrap();
        assert_eq!(lease.holder_pid, 1001);
        assert_eq!(lease.holder_instance, "inst-1001");
        assert_eq!(lease.fencing_token, 1);

        // The same instance can renew
        let renewed = store
            .acquire_or_renew_daemon_lease(
                "daemon-main",
                1001,
                "host-a",
                "inst-1001",
                Duration::from_secs(60),
            )
            .await
            .unwrap();
        assert_eq!(renewed.fencing_token, 1);

        // Other holder is rejected while active
        let conflict = store
            .acquire_or_renew_daemon_lease(
                "daemon-main",
                1002,
                "host-b",
                "inst-1002",
                Duration::from_secs(60),
            )
            .await;
        assert!(matches!(conflict, Err(StoreError::DaemonLeaseHeld { .. })));

        // Other holder succeeds when lease is stale (ttl = 0)
        let recovered = store
            .acquire_or_renew_daemon_lease(
                "daemon-main",
                1002,
                "host-b",
                "inst-1002",
                Duration::ZERO,
            )
            .await
            .unwrap();
        assert_eq!(recovered.holder_pid, 1002);
        assert_eq!(recovered.fencing_token, 2);

        // Release lease
        store
            .release_daemon_lease("daemon-main", 1002, "inst-1002")
            .await
            .unwrap();

        // Now first holder can acquire fresh lease
        let fresh = store
            .acquire_or_renew_daemon_lease(
                "daemon-main",
                1001,
                "host-a",
                "inst-1001",
                Duration::from_secs(60),
            )
            .await
            .unwrap();
        assert_eq!(fresh.holder_pid, 1001);
        assert_eq!(fresh.fencing_token, 1);
    }

    #[tokio::test]
    async fn fenced_writes_are_refused_after_a_successor_takes_the_daemon_lease() {
        let store = SqliteStore::in_memory().await.unwrap();

        let lease = store
            .acquire_or_renew_daemon_lease(
                "daemon-main",
                1001,
                "host-a",
                "inst-1001",
                Duration::from_secs(60),
            )
            .await
            .unwrap();
        let fence = DaemonFence {
            lease_id: "daemon-main".to_owned(),
            holder_pid: lease.holder_pid,
            holder_instance: lease.holder_instance.clone(),
            fencing_token: lease.fencing_token,
        };

        store
            .insert_task(&task_record("task-fenced", "fenced"))
            .await
            .unwrap();

        // While this process holds the lease, its fenced write commits.
        let updated = store
            .update_task_state_fenced("task-fenced", "ready", "running", &fence)
            .await
            .unwrap();
        assert_eq!(updated.state, "running");

        // A successor takes the lease (stale takeover), which increments the
        // fencing token and changes the holder.
        let successor = store
            .acquire_or_renew_daemon_lease(
                "daemon-main",
                1002,
                "host-b",
                "inst-1002",
                Duration::ZERO,
            )
            .await
            .unwrap();
        assert_eq!(successor.fencing_token, lease.fencing_token + 1);

        // The stale fence can no longer commit: the write is refused instead of
        // landing after the successor took ownership.
        let refused = store
            .update_task_state_fenced("task-fenced", "running", "done", &fence)
            .await;
        assert!(matches!(
            refused,
            Err(StoreError::DaemonAuthorityLost { .. })
        ));

        let task = store.get_task("task-fenced").await.unwrap().unwrap();
        assert_eq!(
            task.state, "running",
            "a stale fenced write must not change durable state"
        );
    }

    /// R18-01: the command ledger is part of the daemon's canonical state, so it
    /// is fenced on the same lease as the semantic writes. A stale daemon must not
    /// be able to record, claim, complete, or reset command rows after a successor
    /// took over - that is what could strand a command past startup
    /// reconciliation or overwrite a newer terminal result.
    #[tokio::test]
    async fn command_ledger_writes_are_refused_after_a_successor_takes_the_daemon_lease() {
        let store = SqliteStore::in_memory().await.unwrap();

        let lease = store
            .acquire_or_renew_daemon_lease(
                "daemon-ledger",
                2001,
                "host-a",
                "inst-2001",
                Duration::from_secs(60),
            )
            .await
            .unwrap();
        let fence = DaemonFence {
            lease_id: "daemon-ledger".to_owned(),
            holder_pid: lease.holder_pid,
            holder_instance: lease.holder_instance.clone(),
            fencing_token: lease.fencing_token,
        };

        // While this process holds the lease, its ledger writes commit.
        assert!(
            store
                .record_command_fenced("cmd-ledger", "submit_handoff", "{}", &fence)
                .await
                .unwrap()
        );
        assert!(
            store
                .claim_command_fenced("cmd-ledger", &fence)
                .await
                .unwrap(),
            "the lease holder must be able to claim its own command"
        );

        // A successor takes the lease (stale takeover), changing the holder and
        // incrementing the fencing token.
        let successor = store
            .acquire_or_renew_daemon_lease(
                "daemon-ledger",
                2002,
                "host-b",
                "inst-2002",
                Duration::ZERO,
            )
            .await
            .unwrap();
        assert_eq!(successor.fencing_token, lease.fencing_token + 1);

        // Every ledger write a stale daemon could still attempt is now refused.
        let refused = [
            store
                .record_command_fenced("cmd-late", "submit_handoff", "{}", &fence)
                .await,
            store
                .claim_command_fenced("cmd-late", &fence)
                .await
                .map(|_| true),
            store
                .complete_command_fenced("cmd-ledger", "failed", None, &fence)
                .await
                .map(|_| true),
            store
                .reset_command_claims_fenced(&fence)
                .await
                .map(|_| true),
        ];
        for refusal in refused {
            assert!(
                matches!(refusal, Err(StoreError::DaemonAuthorityLost { .. })),
                "a stale command-ledger write must be refused, got {refusal:?}"
            );
        }

        // The durable ledger is unchanged: no late row was created, and the
        // claimed command is still non-terminal for the successor to resolve.
        assert!(store.get_command("cmd-late").await.unwrap().is_none());
        let row = store.get_command("cmd-ledger").await.unwrap().unwrap();
        assert_eq!(row.status, "received");
    }

    /// R18-01: a command that already reached a terminal status is never
    /// overwritten by a later completion, so a stale or duplicated completion
    /// cannot turn a recorded `applied` into `failed`.
    #[tokio::test]
    async fn a_terminal_command_is_not_overwritten_by_a_duplicate_completion() {
        let store = SqliteStore::in_memory().await.unwrap();

        let lease = store
            .acquire_or_renew_daemon_lease(
                "daemon-complete",
                3001,
                "host-a",
                "inst-3001",
                Duration::from_secs(60),
            )
            .await
            .unwrap();
        let fence = DaemonFence {
            lease_id: "daemon-complete".to_owned(),
            holder_pid: lease.holder_pid,
            holder_instance: lease.holder_instance.clone(),
            fencing_token: lease.fencing_token,
        };

        store
            .record_command_fenced("cmd-once", "submit_handoff", "{}", &fence)
            .await
            .unwrap();

        assert!(
            store
                .complete_command_fenced("cmd-once", "applied", Some(r#"{"ok":true}"#), &fence)
                .await
                .unwrap(),
            "the first completion must record the terminal result"
        );

        assert!(
            !store
                .complete_command_fenced("cmd-once", "failed", Some(r#"{"ok":false}"#), &fence)
                .await
                .unwrap(),
            "a duplicate completion must not overwrite the terminal result"
        );

        let row = store.get_command("cmd-once").await.unwrap().unwrap();
        assert_eq!(row.status, "applied");
        assert_eq!(row.response_json.as_deref(), Some(r#"{"ok":true}"#));

        // A genuinely absent command is still a typed error, not a silent no-op.
        assert!(matches!(
            store
                .complete_command_fenced("cmd-absent", "applied", None, &fence)
                .await,
            Err(StoreError::CommandNotFound { .. })
        ));
    }

    /// DB-19-02: two daemon instances inside one process share a pid and a
    /// hostname, so a process-scoped lease let the second instance renew the
    /// first's row, and a pid-scoped release let a failed second startup delete
    /// the lease the first, live instance was using. Ownership is per instance,
    /// and a release only ever removes the row of the instance that wrote it.
    #[tokio::test]
    async fn daemon_lease_ownership_is_per_instance_not_per_process() {
        let store = SqliteStore::in_memory().await.unwrap();
        let pid = 4242;

        let first = store
            .acquire_or_renew_daemon_lease(
                "daemon-instances",
                pid,
                "host-a",
                "instance-a",
                Duration::from_secs(60),
            )
            .await
            .unwrap();
        assert_eq!(first.fencing_token, 1);

        // A second instance in the same process must not inherit the first's
        // authority while the first is live.
        let second = store
            .acquire_or_renew_daemon_lease(
                "daemon-instances",
                pid,
                "host-a",
                "instance-b",
                Duration::from_secs(60),
            )
            .await;
        assert!(
            matches!(second, Err(StoreError::DaemonLeaseHeld { .. })),
            "a second daemon instance in one process must be refused, not treated as the live owner"
        );

        // ...and a failed second startup releasing under its own identity leaves
        // the live instance's lease alone.
        store
            .release_daemon_lease("daemon-instances", pid, "instance-b")
            .await
            .unwrap();

        let contested = store
            .acquire_or_renew_daemon_lease(
                "daemon-instances",
                pid + 1,
                "host-a",
                "instance-c",
                Duration::from_secs(60),
            )
            .await;
        assert!(
            matches!(contested, Err(StoreError::DaemonLeaseHeld { .. })),
            "the first instance must still own its lease after a failed second startup"
        );

        let renewed = store
            .acquire_or_renew_daemon_lease(
                "daemon-instances",
                pid,
                "host-a",
                "instance-a",
                Duration::from_secs(60),
            )
            .await
            .unwrap();
        assert_eq!(
            renewed.fencing_token, first.fencing_token,
            "renewal by the owning instance must not advance the fencing token"
        );

        // A stale row is still reclaimable by a different instance in the same
        // process - that is what makes a crashed predecessor recoverable.
        let successor = store
            .acquire_or_renew_daemon_lease(
                "daemon-instances",
                pid,
                "host-a",
                "instance-d",
                Duration::ZERO,
            )
            .await
            .unwrap();
        assert_eq!(successor.fencing_token, first.fencing_token + 1);
        assert_eq!(successor.holder_instance, "instance-d");

        // The superseded instance's release must not delete the successor's row.
        store
            .release_daemon_lease("daemon-instances", pid, "instance-a")
            .await
            .unwrap();
        let after_superseded_release = store
            .acquire_or_renew_daemon_lease(
                "daemon-instances",
                pid + 1,
                "host-a",
                "instance-e",
                Duration::from_secs(60),
            )
            .await;
        assert!(
            matches!(
                after_superseded_release,
                Err(StoreError::DaemonLeaseHeld { .. })
            ),
            "a superseded instance's release must not delete a successor's lease"
        );

        // The owning instance's release does remove its row, so the next holder
        // acquires a fresh lease instead of waiting out the TTL.
        store
            .release_daemon_lease("daemon-instances", pid, "instance-d")
            .await
            .unwrap();
        let after_release = store
            .acquire_or_renew_daemon_lease(
                "daemon-instances",
                pid + 1,
                "host-a",
                "instance-e",
                Duration::from_secs(60),
            )
            .await
            .unwrap();
        assert_eq!(
            after_release.fencing_token, 1,
            "a released lease is re-acquired as a fresh generation"
        );
    }

    /// FLOW-19-01: the daemon's effect worker mutates the same canonical
    /// `pending_effects` / `effect_results` ledger the daemon owns, so its writes
    /// are fenced on the lease *and* conditional on the state it claimed. A
    /// worker whose claim was overtaken must neither record a result nor move an
    /// effect back out of a settled state.
    #[tokio::test]
    async fn effect_worker_writes_are_fenced_and_cannot_regress_a_settled_effect() {
        use sqlx::Row;

        let store = SqliteStore::in_memory().await.unwrap();

        let lease = store
            .acquire_or_renew_daemon_lease(
                "daemon-effects",
                5001,
                "host-a",
                "inst-5001",
                Duration::from_secs(60),
            )
            .await
            .unwrap();
        let fence = DaemonFence {
            lease_id: "daemon-effects".to_owned(),
            holder_pid: lease.holder_pid,
            holder_instance: lease.holder_instance.clone(),
            fencing_token: lease.fencing_token,
        };

        store
            .insert_pending_effect(&NewPendingEffectRecord {
                id: "eff-worker".to_owned(),
                effect_type: "test_effect".to_owned(),
                idempotency_key: "idem-eff-worker".to_owned(),
                payload_json: "{}".to_owned(),
            })
            .await
            .unwrap();

        // The worker claims the effect while it holds the lease.
        assert!(
            store
                .update_effect_state_fenced("eff-worker", "in_flight", None, "pending", &fence)
                .await
                .unwrap(),
            "the lease holder must be able to claim a pending effect"
        );

        // A successor takes over and settles the effect successfully.
        let successor = store
            .acquire_or_renew_daemon_lease(
                "daemon-effects",
                5002,
                "host-b",
                "inst-5002",
                Duration::ZERO,
            )
            .await
            .unwrap();
        let successor_fence = DaemonFence {
            lease_id: "daemon-effects".to_owned(),
            holder_pid: successor.holder_pid,
            holder_instance: successor.holder_instance.clone(),
            fencing_token: successor.fencing_token,
        };
        store
            .record_effect_result_fenced(
                "success-result",
                "eff-worker",
                "success",
                r#"{"ok":true}"#,
                &successor_fence,
            )
            .await
            .unwrap();

        // The stale worker's late result is refused outright: the lease is gone.
        assert!(
            matches!(
                store
                    .settle_effect_from_worker_fenced(
                        "eff-worker-worker-result",
                        "eff-worker",
                        "failure",
                        "handler failed",
                        &fence
                    )
                    .await,
                Err(StoreError::DaemonAuthorityLost { .. })
            ),
            "a worker that lost the lease must not settle an effect"
        );
        // ...and so is its state transition, on the claim path as well.
        assert!(
            matches!(
                store
                    .update_effect_state_fenced("eff-worker", "pending", None, "in_flight", &fence)
                    .await,
                Err(StoreError::DaemonAuthorityLost { .. })
            ),
            "a worker that lost the lease must not move effect state"
        );

        let settled = store
            .get_pending_effect("eff-worker")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            settled.state, "completed",
            "a stale worker must not regress a settled effect"
        );

        // The lease holder that no longer owns the claim is refused too: the
        // effect is no longer in flight, so the second settle records nothing.
        assert!(
            !store
                .settle_effect_from_worker_fenced(
                    "successor-late-result",
                    "eff-worker",
                    "failure",
                    "late failure",
                    &successor_fence
                )
                .await
                .unwrap(),
            "an effect that is no longer in flight must not be settled again"
        );

        let rows =
            sqlx::query("SELECT id, outcome FROM effect_results WHERE effect_id = ?1 ORDER BY id")
                .bind("eff-worker")
                .fetch_all(store.pool())
                .await
                .unwrap();
        let recorded: Vec<(String, String)> = rows
            .into_iter()
            .map(|row| {
                (
                    row.try_get::<String, _>("id").unwrap(),
                    row.try_get::<String, _>("outcome").unwrap(),
                )
            })
            .collect();
        assert_eq!(
            recorded,
            vec![("success-result".to_owned(), "success".to_owned())],
            "a stale or superseded worker must not leave a contradictory result row"
        );
    }

    /// A released lease is re-acquired at the same pid with the token restarting
    /// at 1, so pid plus token cannot identify a generation: the instance is part
    /// of the fence, otherwise the previous instance's stale writes would satisfy
    /// the assertion against the new instance's row.
    #[tokio::test]
    async fn a_reacquired_lease_does_not_accept_the_previous_instances_fence() {
        let store = SqliteStore::in_memory().await.unwrap();
        let pid = 6001;

        let first = store
            .acquire_or_renew_daemon_lease(
                "daemon-reacquire",
                pid,
                "host-a",
                "instance-first",
                Duration::from_secs(60),
            )
            .await
            .unwrap();
        let stale_fence = DaemonFence {
            lease_id: "daemon-reacquire".to_owned(),
            holder_pid: first.holder_pid,
            holder_instance: first.holder_instance.clone(),
            fencing_token: first.fencing_token,
        };

        store
            .release_daemon_lease("daemon-reacquire", pid, "instance-first")
            .await
            .unwrap();

        let second = store
            .acquire_or_renew_daemon_lease(
                "daemon-reacquire",
                pid,
                "host-a",
                "instance-second",
                Duration::from_secs(60),
            )
            .await
            .unwrap();
        assert_eq!(
            second.fencing_token, stale_fence.fencing_token,
            "the token alone cannot distinguish the two generations"
        );

        store
            .record_command_fenced("cmd-reacquire", "submit_handoff", "{}", &stale_fence)
            .await
            .expect_err("the previous instance's fence must not satisfy the new instance's row");

        let current_fence = DaemonFence {
            lease_id: "daemon-reacquire".to_owned(),
            holder_pid: second.holder_pid,
            holder_instance: second.holder_instance.clone(),
            fencing_token: second.fencing_token,
        };
        assert!(
            store
                .record_command_fenced("cmd-reacquire", "submit_handoff", "{}", &current_fence)
                .await
                .unwrap(),
            "the current instance must still be able to write"
        );
    }

    #[tokio::test]
    async fn durable_resource_lease_fencing_and_lifecycle() {
        let store = SqliteStore::in_memory().await.unwrap();

        // `expires_at` carries Unix epoch seconds — the canonical representation
        // written by relay-core's durable lease wrapper. These fixtures previously
        // used SQLite date text, which is why the epoch-seconds expiry defect passed
        // unnoticed here (PR #16 review finding R16-01).
        let expires_future = "4102444800".to_string(); // 2100-01-01T00:00:00Z
        let expires_past = "946684800".to_string(); // 2000-01-01T00:00:00Z

        let lease_req1 = NewResourceLeaseRecord {
            id: "lease-1".to_string(),
            resource_type: "checkout".to_string(),
            resource_id: "chk-1".to_string(),
            lease_kind: "exclusive_write".to_string(),
            holder_id: "holder-1".to_string(),
            holder_pid: 101,
            holder_host: "localhost".to_string(),
            expires_at: expires_future.clone(),
        };

        // 1. Acquire initial lease -> fencing_token = 1
        let l1 = store.acquire_resource_lease(&lease_req1).await.unwrap();
        assert_eq!(l1.fencing_token, 1);
        assert_eq!(l1.state, "active");

        // 2. Different holder attempting concurrent active lease -> rejected
        let lease_req2 = NewResourceLeaseRecord {
            id: "lease-2".to_string(),
            resource_type: "checkout".to_string(),
            resource_id: "chk-1".to_string(),
            lease_kind: "exclusive_write".to_string(),
            holder_id: "holder-2".to_string(),
            holder_pid: 102,
            holder_host: "localhost".to_string(),
            expires_at: expires_future.clone(),
        };
        let err = store.acquire_resource_lease(&lease_req2).await.unwrap_err();
        assert!(matches!(err, StoreError::ResourceLeaseHeld { .. }));

        // 3. Same holder can renew/extend
        let l1_renewed = store.acquire_resource_lease(&lease_req1).await.unwrap();
        assert_eq!(l1_renewed.fencing_token, 1);
        assert_eq!(l1_renewed.id, "lease-1");

        // 4. Release lease
        store
            .release_resource_lease("lease-1", "holder-1")
            .await
            .unwrap();
        let released_record = store.get_resource_lease("lease-1").await.unwrap().unwrap();
        assert_eq!(released_record.state, "released");

        // 5. Now holder-2 can acquire -> fencing_token = 2 (monotonically incremented)
        let l2 = store.acquire_resource_lease(&lease_req2).await.unwrap();
        assert_eq!(l2.fencing_token, 2);
        assert_eq!(l2.state, "active");

        // 6. Expired lease allows takeover with incremented token
        // Force expiry of lease-2
        sqlx::query("UPDATE resource_leases SET expires_at = ?1 WHERE id = 'lease-2'")
            .bind(&expires_past)
            .execute(&store.pool)
            .await
            .unwrap();

        let lease_req3 = NewResourceLeaseRecord {
            id: "lease-3".to_string(),
            resource_type: "checkout".to_string(),
            resource_id: "chk-1".to_string(),
            lease_kind: "exclusive_write".to_string(),
            holder_id: "holder-3".to_string(),
            holder_pid: 103,
            holder_host: "localhost".to_string(),
            expires_at: expires_future,
        };
        let l3 = store.acquire_resource_lease(&lease_req3).await.unwrap();
        assert_eq!(l3.fencing_token, 3);
        assert_eq!(l3.state, "active");

        let prev_l2 = store.get_resource_lease("lease-2").await.unwrap().unwrap();
        assert_eq!(prev_l2.state, "expired");
    }

    #[tokio::test]
    async fn durable_resource_lease_reclaims_expired_epoch_seconds_lease() {
        // Regression for PR #16 review finding R16-01. The production writer
        // (relay-core's durable lease wrapper) stores `expires_at` as Unix epoch
        // seconds; this pins that representation at the store boundary. Evaluating
        // expiry with strftime('%s', expires_at) returned NULL for epoch seconds, so
        // expired leases were never detected and never reclaimed.
        let store = SqliteStore::in_memory().await.unwrap();

        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let epoch = |offset: i64| (now_secs + offset).to_string();
        let lease_req = |id: &str, holder: &str, expires_at: String| NewResourceLeaseRecord {
            id: id.to_string(),
            resource_type: "workflow_run".to_string(),
            resource_id: "wfr-epoch-1".to_string(),
            lease_kind: "exclusive_write".to_string(),
            holder_id: holder.to_string(),
            holder_pid: 301,
            holder_host: "localhost".to_string(),
            expires_at,
        };

        let live = store
            .acquire_resource_lease(&lease_req("lease-epoch-1", "holder-1", epoch(3600)))
            .await
            .unwrap();
        assert_eq!(live.fencing_token, 1);
        assert_eq!(live.state, "active");

        // A live epoch-seconds lease still blocks a competing holder.
        let blocked = store
            .acquire_resource_lease(&lease_req("lease-epoch-2", "holder-2", epoch(3600)))
            .await
            .unwrap_err();
        assert!(matches!(blocked, StoreError::ResourceLeaseHeld { .. }));

        // Move the active lease's expiry into the past and confirm reclamation.
        sqlx::query("UPDATE resource_leases SET expires_at = ?1 WHERE id = 'lease-epoch-1'")
            .bind(epoch(-60))
            .execute(&store.pool)
            .await
            .unwrap();

        let takeover = store
            .acquire_resource_lease(&lease_req("lease-epoch-3", "holder-3", epoch(3600)))
            .await
            .unwrap();
        assert_eq!(
            takeover.fencing_token, 2,
            "fencing token must stay monotonic"
        );
        assert_eq!(takeover.state, "active");

        let superseded = store
            .get_resource_lease("lease-epoch-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(superseded.state, "expired");
    }

    /// R12-02 part 1: the v2 orchestration tables (migration 0010) and the role
    /// attempt runtime-handle columns (migration 0011) must apply on a fresh
    /// database whose schema already stops at 0009.
    #[tokio::test]
    async fn v2_workflow_migrations_apply_after_0009() {
        use sqlx::Row;

        let store = SqliteStore::in_memory().await.unwrap();

        for table in ["workflow_runs", "role_instances", "role_attempts"] {
            let count = sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            )
            .bind(table)
            .fetch_one(&store.pool)
            .await
            .unwrap();
            assert_eq!(count, 1, "expected table {table} to exist after migrations");
        }

        let rows = sqlx::query("PRAGMA table_info(role_attempts)")
            .fetch_all(&store.pool)
            .await
            .unwrap();
        let columns = rows
            .iter()
            .map(|row| row.try_get::<String, _>("name").unwrap())
            .collect::<Vec<_>>();
        assert!(
            columns.iter().any(|name| name == "runtime_handle_id"),
            "role_attempts must carry runtime_handle_id after migration 0011"
        );
        assert!(
            columns.iter().any(|name| name == "runtime_backplane_id"),
            "role_attempts must carry runtime_backplane_id after migration 0011"
        );

        let run_rows = sqlx::query("PRAGMA table_info(workflow_runs)")
            .fetch_all(&store.pool)
            .await
            .unwrap();
        let run_columns = run_rows
            .iter()
            .map(|row| row.try_get::<String, _>("name").unwrap())
            .collect::<Vec<_>>();
        for column in [
            "approved_intent_json",
            "execution_grant_json",
            "config_snapshot_json",
        ] {
            assert!(
                run_columns.iter().any(|name| name == column),
                "workflow_runs must carry {column} after migration 0012"
            );
        }
    }

    /// R12-02 part 1: insert-if-absent is idempotent for workflow runs, role
    /// instances and role attempts, returning the existing row unchanged.
    #[tokio::test]
    async fn workflow_records_insert_if_absent_is_idempotent() {
        let store = SqliteStore::in_memory().await.unwrap();
        store
            .insert_task(&task_record("task-wf", "workflow"))
            .await
            .unwrap();

        let run = workflow_run_record("run-1", "task-wf");
        let first = store.insert_workflow_run_if_absent(&run).await.unwrap();
        let second = store.insert_workflow_run_if_absent(&run).await.unwrap();
        assert_eq!(
            first, second,
            "a replayed run insert must return the existing row"
        );
        assert_eq!(store.list_active_workflow_runs().await.unwrap().len(), 1);

        let instance = role_instance_record("inst-1", "run-1");
        let first_instance = store
            .insert_role_instance_if_absent(&instance)
            .await
            .unwrap();
        let second_instance = store
            .insert_role_instance_if_absent(&instance)
            .await
            .unwrap();
        assert_eq!(
            first_instance, second_instance,
            "a replayed role instance insert must return the existing row"
        );
        assert_eq!(
            store
                .list_role_instances_for_run("run-1")
                .await
                .unwrap()
                .len(),
            1
        );

        let attempt = role_attempt_record("attempt-1", "inst-1");
        let first_attempt = store.insert_role_attempt_if_absent(&attempt).await.unwrap();
        let second_attempt = store.insert_role_attempt_if_absent(&attempt).await.unwrap();
        assert_eq!(
            first_attempt, second_attempt,
            "a replayed role attempt insert must return the existing row"
        );
        assert_eq!(
            store
                .list_role_attempts_for_instance("inst-1")
                .await
                .unwrap()
                .len(),
            1
        );
    }

    /// R12-02 part 1: every v2 write is fenced on the daemon lease, so a stale
    /// daemon cannot land a workflow/role write after a successor took over.
    #[tokio::test]
    async fn v2_workflow_writes_are_refused_after_a_successor_takes_the_daemon_lease() {
        let store = SqliteStore::in_memory().await.unwrap();

        let lease = store
            .acquire_or_renew_daemon_lease(
                "daemon-r12",
                4001,
                "host-a",
                "inst-4001",
                Duration::from_secs(60),
            )
            .await
            .unwrap();
        let fence = DaemonFence {
            lease_id: "daemon-r12".to_owned(),
            holder_pid: lease.holder_pid,
            holder_instance: lease.holder_instance.clone(),
            fencing_token: lease.fencing_token,
        };

        // Seed the referenced rows while this process still holds the lease.
        store
            .insert_task(&task_record("task-r12", "r12"))
            .await
            .unwrap();
        store
            .insert_workflow_run_if_absent_fenced(
                &workflow_run_record("run-r12", "task-r12"),
                &fence,
            )
            .await
            .unwrap();
        store
            .insert_role_instance_if_absent_fenced(
                &role_instance_record("inst-r12", "run-r12"),
                &fence,
            )
            .await
            .unwrap();
        store
            .insert_role_attempt_if_absent_fenced(
                &role_attempt_record("attempt-r12", "inst-r12"),
                &fence,
            )
            .await
            .unwrap();

        // A successor takes the lease (stale takeover), changing the holder and
        // incrementing the fencing token.
        let successor = store
            .acquire_or_renew_daemon_lease(
                "daemon-r12",
                4002,
                "host-b",
                "inst-4002",
                Duration::ZERO,
            )
            .await
            .unwrap();
        assert_eq!(successor.fencing_token, lease.fencing_token + 1);

        // Every v2 write a stale daemon could still attempt is now refused.
        let new_run = workflow_run_record("run-late", "task-r12");
        let new_instance = role_instance_record("inst-late", "run-r12");
        let new_attempt = role_attempt_record("attempt-late", "inst-r12");
        let refusals = [
            store
                .insert_workflow_run_if_absent_fenced(&new_run, &fence)
                .await
                .map(|_| ()),
            store
                .update_workflow_run_state_fenced("run-r12", "running", &fence)
                .await
                .map(|_| ()),
            store
                .insert_role_instance_if_absent_fenced(&new_instance, &fence)
                .await
                .map(|_| ()),
            store
                .update_role_instance_fenced("inst-r12", "active", None, 1, &fence)
                .await
                .map(|_| ()),
            store
                .insert_role_attempt_if_absent_fenced(&new_attempt, &fence)
                .await
                .map(|_| ()),
            store
                .set_role_attempt_lease_fenced("attempt-r12", "lease-late", 1, &fence)
                .await,
            store
                .set_role_attempt_runtime_handle_fenced(
                    "attempt-r12",
                    "handle-late",
                    "backplane-late",
                    &fence,
                )
                .await,
            store
                .update_role_attempt_fenced("attempt-r12", "running", None, None, &fence)
                .await
                .map(|_| ()),
        ];
        for refusal in refusals {
            assert!(
                matches!(refusal, Err(StoreError::DaemonAuthorityLost { .. })),
                "a stale v2 write must be refused, got {refusal:?}"
            );
        }

        // The durable state is unchanged: no late rows were created and nothing
        // regressed.
        assert!(store.get_workflow_run("run-late").await.unwrap().is_none());
        assert!(
            store
                .get_role_instance("inst-late")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .get_role_attempt("attempt-late")
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            store
                .get_workflow_run("run-r12")
                .await
                .unwrap()
                .unwrap()
                .state,
            "pending"
        );
        assert_eq!(
            store
                .get_role_instance("inst-r12")
                .await
                .unwrap()
                .unwrap()
                .state,
            "created"
        );
        assert_eq!(
            store
                .get_role_attempt("attempt-r12")
                .await
                .unwrap()
                .unwrap()
                .state,
            "queued"
        );
    }

    /// R12-02 part 2: approving a Run is one durable fact. The run (with the
    /// approved intent, grant and snapshot it executes under), the task's
    /// transition and the single start event commit together, and a replay
    /// converges instead of duplicating any of them.
    #[tokio::test]
    async fn start_workflow_run_is_atomic_and_its_replay_converges() {
        let store = SqliteStore::in_memory().await.unwrap();
        let lease = store
            .acquire_or_renew_daemon_lease(
                "daemon-start",
                5001,
                "host-a",
                "inst-5001",
                Duration::from_secs(60),
            )
            .await
            .unwrap();
        let fence = DaemonFence {
            lease_id: "daemon-start".to_owned(),
            holder_pid: lease.holder_pid,
            holder_instance: lease.holder_instance.clone(),
            fencing_token: lease.fencing_token,
        };

        store
            .insert_task(&task_record("task-start", "start"))
            .await
            .unwrap();

        let intent_json = r#"{"id":"intent-1","generation":1}"#;
        let grant_json = r#"{"id":"grant-1"}"#;
        let snapshot_json = r#"{"id":"snapshot-1","workflow_version":1}"#;
        let event_json = r#"{"task_id":"task-start","workflow_run_id":"run-start"}"#;

        let mut run_record = workflow_run_record("run-start", "task-start");
        run_record.state = "running".to_owned();
        let start = WorkflowRunStart {
            run: &run_record,
            approved_intent_json: intent_json,
            execution_grant_json: grant_json,
            config_snapshot_json: snapshot_json,
            task_expected_state: "ready",
            task_new_state: "running",
            event_kind: "workflow_run_started",
            event_json: Some(event_json),
        };

        let run = store
            .start_workflow_run_fenced(&start, &fence)
            .await
            .unwrap();
        assert_eq!(run.state, "running");
        assert_eq!(run.approved_intent_json, intent_json);
        assert_eq!(run.execution_grant_json, grant_json);
        assert_eq!(run.config_snapshot_json, snapshot_json);
        assert_eq!(
            store.get_workflow_run("run-start").await.unwrap().unwrap(),
            run,
            "the stored authority must round-trip through the read path"
        );
        assert_eq!(
            store.get_task("task-start").await.unwrap().unwrap().state,
            "running"
        );

        // Replay of the same approval converges: same run, task already running,
        // and no second start event.
        let replay = store
            .start_workflow_run_fenced(&start, &fence)
            .await
            .unwrap();
        assert_eq!(replay, run);

        let events = store.get_events_after(0, 100).await.unwrap();
        let starts: Vec<_> = events
            .iter()
            .filter(|event| event.event_kind == "workflow_run_started")
            .collect();
        assert_eq!(
            starts.len(),
            1,
            "a replayed approval must not append a second start event, got {events:?}"
        );
        assert_eq!(starts[0].task_id.as_deref(), Some("task-start"));
        assert_eq!(starts[0].execution_id.as_deref(), Some("run-start"));

        // A task in any other state is a genuine conflict, and the run row that
        // would have accompanied it is rolled back with the refusal.
        store
            .insert_task(&task_record("task-done", "done"))
            .await
            .unwrap();
        store
            .update_task_state("task-done", "ready", "done")
            .await
            .unwrap();
        let mut late_record = workflow_run_record("run-late-start", "task-done");
        late_record.state = "running".to_owned();
        let late = WorkflowRunStart {
            run: &late_record,
            approved_intent_json: intent_json,
            execution_grant_json: grant_json,
            config_snapshot_json: snapshot_json,
            task_expected_state: "ready",
            task_new_state: "running",
            event_kind: "workflow_run_started",
            event_json: None,
        };
        let refused = store.start_workflow_run_fenced(&late, &fence).await;
        assert!(
            matches!(refused, Err(StoreError::StateConflict { .. })),
            "a Task that is not ready must refuse the approval, got {refused:?}"
        );
        assert!(
            store
                .get_workflow_run("run-late-start")
                .await
                .unwrap()
                .is_none(),
            "the run insert must roll back with the refused transition"
        );
    }

    /// R12-02 part 1: a role attempt round-trips the lease generation and the
    /// backplane runtime handle a restarted daemon must reconcile against.
    #[tokio::test]
    async fn a_role_attempt_round_trips_its_runtime_handle_and_lease() {
        let store = SqliteStore::in_memory().await.unwrap();
        store
            .insert_task(&task_record("task-rt", "rt"))
            .await
            .unwrap();
        store
            .insert_workflow_run_if_absent(&workflow_run_record("run-rt", "task-rt"))
            .await
            .unwrap();
        store
            .insert_role_instance_if_absent(&role_instance_record("inst-rt", "run-rt"))
            .await
            .unwrap();
        store
            .insert_role_attempt_if_absent(&role_attempt_record("attempt-rt", "inst-rt"))
            .await
            .unwrap();

        store
            .set_role_attempt_lease("attempt-rt", "lease-rt", 7)
            .await
            .unwrap();
        store
            .set_role_attempt_runtime_handle("attempt-rt", "handle-42", "herdr-backplane")
            .await
            .unwrap();

        let stored = store.get_role_attempt("attempt-rt").await.unwrap().unwrap();
        assert_eq!(stored.lease_id.as_deref(), Some("lease-rt"));
        assert_eq!(stored.fencing_token, Some(7));
        assert_eq!(stored.runtime_handle_id.as_deref(), Some("handle-42"));
        assert_eq!(
            stored.runtime_backplane_id.as_deref(),
            Some("herdr-backplane")
        );
        assert!(
            stored.started_at.is_some(),
            "an inserted role attempt records its start time"
        );
    }

    fn workflow_run_record(id: &str, task_id: &str) -> NewWorkflowRunRecord {
        NewWorkflowRunRecord {
            id: id.to_owned(),
            task_id: task_id.to_owned(),
            execution_grant_id: "grant-1".to_owned(),
            workflow_definition_id: "wf-def-1".to_owned(),
            state: "pending".to_owned(),
            config_snapshot_id: "snapshot-1".to_owned(),
        }
    }

    fn role_instance_record(id: &str, workflow_run_id: &str) -> NewRoleInstanceRecord {
        NewRoleInstanceRecord {
            id: id.to_owned(),
            workflow_run_id: workflow_run_id.to_owned(),
            role_definition_id: "implementer".to_owned(),
            generation: 1,
            state: "created".to_owned(),
            parent_role_instance_id: None,
        }
    }

    fn role_attempt_record(id: &str, role_instance_id: &str) -> NewRoleAttemptRecord {
        NewRoleAttemptRecord {
            id: id.to_owned(),
            role_instance_id: role_instance_id.to_owned(),
            attempt_no: 1,
            execution_target_json:
                r#"{"provider_id":"fake","runtime_profile_id":"fake/default","provider_adapter_version":"1"}"#
                    .to_owned(),
            state: "queued".to_owned(),
            lease_id: None,
            fencing_token: None,
        }
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

    fn execution_record(id: &str, task_id: &str) -> NewExecutionRecord {
        NewExecutionRecord {
            id: id.to_owned(),
            task_id: task_id.to_owned(),
            runtime_profile: "fake/default".to_owned(),
            phase: "running".to_owned(),
            repository_snapshot_json: Some(r#"{"eligibility":"eligible"}"#.to_owned()),
            runtime_metadata_json: None,
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
