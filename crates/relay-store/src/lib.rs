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
    pub acquired_at: String,
    pub heartbeat_at: String,
    pub fencing_token: i64,
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
        retry_on_transient_lock(|| self.insert_task_if_absent_once(task)).await
    }

    async fn insert_task_if_absent_once(
        &self,
        task: &NewTaskRecord,
    ) -> Result<TaskRecord, StoreError> {
        let mut transaction = self.pool.begin().await?;

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
        retry_on_transient_lock(|| self.update_task_state_once(id, expected_state, new_state)).await
    }

    async fn update_task_state_once(
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

    pub async fn record_command(
        &self,
        id: &str,
        command_type: &str,
        payload_json: &str,
    ) -> Result<bool, StoreError> {
        retry_on_transient_lock(|| self.record_command_once(id, command_type, payload_json)).await
    }

    async fn record_command_once(
        &self,
        id: &str,
        command_type: &str,
        payload_json: &str,
    ) -> Result<bool, StoreError> {
        let res = sqlx::query(
            r#"
            INSERT OR IGNORE INTO commands (id, command_type, payload_json, status)
            VALUES (?1, ?2, ?3, 'received')
            "#,
        )
        .bind(id)
        .bind(command_type)
        .bind(payload_json)
        .execute(&self.pool)
        .await?;

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

    pub async fn complete_command(
        &self,
        id: &str,
        status: &str,
        response_json: Option<&str>,
    ) -> Result<(), StoreError> {
        retry_on_transient_lock(|| self.complete_command_once(id, status, response_json)).await
    }

    async fn complete_command_once(
        &self,
        id: &str,
        status: &str,
        response_json: Option<&str>,
    ) -> Result<(), StoreError> {
        let res = sqlx::query(
            r#"
            UPDATE commands
            SET status = ?1,
                response_json = ?2,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            WHERE id = ?3
            "#,
        )
        .bind(status)
        .bind(response_json)
        .bind(id)
        .execute(&self.pool)
        .await?;

        if res.rows_affected() == 0 {
            return Err(StoreError::CommandNotFound { id: id.to_owned() });
        }
        Ok(())
    }

    pub async fn insert_pending_effect(
        &self,
        effect: &NewPendingEffectRecord,
    ) -> Result<PendingEffectRecord, StoreError> {
        retry_on_transient_lock(|| self.insert_pending_effect_once(effect)).await
    }

    async fn insert_pending_effect_once(
        &self,
        effect: &NewPendingEffectRecord,
    ) -> Result<PendingEffectRecord, StoreError> {
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
        .execute(&self.pool)
        .await?;

        self.get_pending_effect(&effect.id).await?.ok_or_else(|| {
            StoreError::PendingEffectNotFound {
                id: effect.id.clone(),
            }
        })
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
        retry_on_transient_lock(|| self.update_effect_state_once(id, state, last_error)).await
    }

    async fn update_effect_state_once(
        &self,
        id: &str,
        state: &str,
        last_error: Option<&str>,
    ) -> Result<(), StoreError> {
        let res = sqlx::query(
            r#"
            UPDATE pending_effects
            SET state = ?1,
                attempts = attempts + 1,
                last_error = ?2,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            WHERE id = ?3
            "#,
        )
        .bind(state)
        .bind(last_error)
        .bind(id)
        .execute(&self.pool)
        .await?;

        if res.rows_affected() == 0 {
            return Err(StoreError::PendingEffectNotFound { id: id.to_owned() });
        }
        Ok(())
    }

    pub async fn record_effect_result(
        &self,
        id: &str,
        effect_id: &str,
        outcome: &str,
        result_json: &str,
    ) -> Result<EffectResultRecord, StoreError> {
        retry_on_transient_lock(|| {
            self.record_effect_result_once(id, effect_id, outcome, result_json)
        })
        .await
    }

    async fn record_effect_result_once(
        &self,
        id: &str,
        effect_id: &str,
        outcome: &str,
        result_json: &str,
    ) -> Result<EffectResultRecord, StoreError> {
        let mut transaction = self.pool.begin().await?;

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

    pub async fn acquire_or_renew_daemon_lease(
        &self,
        lease_id: &str,
        holder_pid: i64,
        holder_host: &str,
        ttl: Duration,
    ) -> Result<DaemonLeaseRecord, StoreError> {
        retry_on_transient_lock(|| {
            self.acquire_or_renew_daemon_lease_once(lease_id, holder_pid, holder_host, ttl)
        })
        .await
    }

    async fn acquire_or_renew_daemon_lease_once(
        &self,
        lease_id: &str,
        holder_pid: i64,
        holder_host: &str,
        ttl: Duration,
    ) -> Result<DaemonLeaseRecord, StoreError> {
        let mut transaction = self.pool.begin().await?;

        let existing = sqlx::query(
            r#"
            SELECT lease_id, holder_pid, holder_host, acquired_at, heartbeat_at, fencing_token,
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
            let elapsed_secs: Option<i64> = row.try_get("elapsed_secs")?;

            if current_pid == holder_pid && current_host == holder_host {
                // Heartbeat renewal
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
                    sqlx::query(
                        r#"
                        UPDATE daemon_leases
                        SET holder_pid = ?1,
                            holder_host = ?2,
                            acquired_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                            heartbeat_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                            fencing_token = fencing_token + 1
                        WHERE lease_id = ?3
                        "#,
                    )
                    .bind(holder_pid)
                    .bind(holder_host)
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
                INSERT INTO daemon_leases (lease_id, holder_pid, holder_host, fencing_token)
                VALUES (?1, ?2, ?3, 1)
                "#,
            )
            .bind(lease_id)
            .bind(holder_pid)
            .bind(holder_host)
            .execute(&mut *transaction)
            .await?;
        }

        let updated = sqlx::query(
            r#"
            SELECT lease_id, holder_pid, holder_host, acquired_at, heartbeat_at, fencing_token
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
            acquired_at: updated.try_get("acquired_at")?,
            heartbeat_at: updated.try_get("heartbeat_at")?,
            fencing_token: updated.try_get("fencing_token")?,
        };

        transaction.commit().await?;
        Ok(record)
    }

    pub async fn release_daemon_lease(
        &self,
        lease_id: &str,
        holder_pid: i64,
    ) -> Result<(), StoreError> {
        retry_on_transient_lock(|| self.release_daemon_lease_once(lease_id, holder_pid)).await
    }

    async fn release_daemon_lease_once(
        &self,
        lease_id: &str,
        holder_pid: i64,
    ) -> Result<(), StoreError> {
        sqlx::query(
            r#"
            DELETE FROM daemon_leases
            WHERE lease_id = ?1
              AND holder_pid = ?2
            "#,
        )
        .bind(lease_id)
        .bind(holder_pid)
        .execute(&self.pool)
        .await?;

        Ok(())
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

/// Runs migrations, tolerating a benign cross-process race.
///
/// Two Relay processes (TUI + serve-mcp) may open a database that has never
/// been migrated at the same moment. sqlx serializes each migration in its own
/// transaction but not the applied-version bookkeeping across processes, so
/// the loser of the race sees a UNIQUE constraint on `_sqlx_migrations.version`.
/// The winner has then finished migrating; one bounded retry observes the
/// completed migration history and succeeds. Any other failure fails closed.
async fn run_migrations(pool: &SqlitePool) -> Result<(), StoreError> {
    if let Err(error) = MIGRATOR.run(pool).await {
        let display = error.to_string();
        if !display.contains("_sqlx_migrations") || !display.contains("UNIQUE constraint") {
            return Err(map_migration_error(error));
        }

        // relay-store deliberately avoids a tracing dependency; the retry is
        // observable only as normal latency.
        MIGRATOR.run(pool).await.map_err(map_migration_error)?;
    }

    Ok(())
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
            .acquire_or_renew_daemon_lease("daemon-main", 1001, "host-a", Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(lease.holder_pid, 1001);
        assert_eq!(lease.fencing_token, 1);

        // Same holder can renew
        let renewed = store
            .acquire_or_renew_daemon_lease("daemon-main", 1001, "host-a", Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(renewed.fencing_token, 1);

        // Other holder is rejected while active
        let conflict = store
            .acquire_or_renew_daemon_lease("daemon-main", 1002, "host-b", Duration::from_secs(60))
            .await;
        assert!(matches!(conflict, Err(StoreError::DaemonLeaseHeld { .. })));

        // Other holder succeeds when lease is stale (ttl = 0)
        let recovered = store
            .acquire_or_renew_daemon_lease("daemon-main", 1002, "host-b", Duration::ZERO)
            .await
            .unwrap();
        assert_eq!(recovered.holder_pid, 1002);
        assert_eq!(recovered.fencing_token, 2);

        // Release lease
        store
            .release_daemon_lease("daemon-main", 1002)
            .await
            .unwrap();

        // Now first holder can acquire fresh lease
        let fresh = store
            .acquire_or_renew_daemon_lease("daemon-main", 1001, "host-a", Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(fresh.holder_pid, 1001);
        assert_eq!(fresh.fencing_token, 1);
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
