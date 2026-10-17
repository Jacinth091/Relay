//! Multi-process access hardening tests for `SqliteStore`.
//!
//! Two pools against the same database file exercise the same SQLite locking
//! paths as two separate Relay processes (TUI + serve-mcp): WAL readers,
//! cross-process writers, bounded busy-timeout retries, and fail-closed
//! schema-version checks.

use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use relay_store::{NewTaskRecord, SqliteStore, StoreError};
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};

const NO_HANG_TIMEOUT: Duration = Duration::from_secs(30);
const INTERLEAVED_TASKS_PER_STORE: usize = 8;
const READER_WRITER_TASKS: usize = 40;

#[tokio::test]
async fn two_stores_interleaved_inserts_are_readable_from_both() {
    tokio::time::timeout(NO_HANG_TIMEOUT, async {
        let database_path = temp_path("interleaved.sqlite3");
        let store_a = SqliteStore::open(&database_path).await.unwrap();
        let store_b = SqliteStore::open(&database_path).await.unwrap();

        for index in 0..INTERLEAVED_TASKS_PER_STORE {
            store_a
                .insert_task(&task_record(
                    &format!("task-a-{index}"),
                    &format!("A {index}"),
                ))
                .await
                .unwrap();
            store_b
                .insert_task(&task_record(
                    &format!("task-b-{index}"),
                    &format!("B {index}"),
                ))
                .await
                .unwrap();
        }

        let expected = INTERLEAVED_TASKS_PER_STORE * 2;

        let ids_from_a = task_ids(&store_a).await;
        let ids_from_b = task_ids(&store_b).await;

        assert_eq!(ids_from_a.len(), expected);
        assert_eq!(ids_from_b.len(), expected);

        let mut sorted_a = ids_from_a.clone();
        let mut sorted_b = ids_from_b.clone();
        sorted_a.sort();
        sorted_b.sort();
        assert_eq!(sorted_a, sorted_b);
    })
    .await
    .expect("interleaved two-store test must not hang");
}

#[tokio::test]
async fn concurrent_same_task_writes_have_deterministic_winner() {
    tokio::time::timeout(NO_HANG_TIMEOUT, async {
        let database_path = temp_path("same-row.sqlite3");
        let store_a = Arc::new(SqliteStore::open(&database_path).await.unwrap());
        let store_b = Arc::new(SqliteStore::open(&database_path).await.unwrap());

        store_a
            .insert_task(&task_record("task-contested", "Contested task"))
            .await
            .unwrap();

        let a = store_a.clone();
        let write_a =
            tokio::spawn(
                async move { a.update_task_state("task-contested", "ready", "done").await },
            );
        let b = store_b.clone();
        let write_b = tokio::spawn(async move {
            b.update_task_state("task-contested", "ready", "cancelled")
                .await
        });

        let (result_a, result_b) = tokio::join!(write_a, write_b);
        let (result_a, result_b) = (result_a.unwrap(), result_b.unwrap());

        let winners = [&result_a, &result_b]
            .into_iter()
            .filter(|result| result.is_ok())
            .count();
        assert_eq!(winners, 1, "exactly one CAS writer may win");

        let loser_error = result_a.err().or(result_b.err()).unwrap();
        assert!(
            matches!(loser_error, StoreError::StateConflict { .. }),
            "loser must get a typed conflict error, got: {loser_error}"
        );

        // The surviving state matches whichever writer won; both stores agree.
        let final_a = store_a.get_task("task-contested").await.unwrap().unwrap();
        let final_b = store_b.get_task("task-contested").await.unwrap().unwrap();
        assert_eq!(final_a.state, final_b.state);
        assert!(
            final_a.state == "done" || final_a.state == "cancelled",
            "final state must be one of the competing writes"
        );
    })
    .await
    .expect("contended same-row write test must not hang");
}

#[tokio::test]
async fn reader_listing_succeeds_while_writer_commits() {
    tokio::time::timeout(NO_HANG_TIMEOUT, async {
        let database_path = temp_path("reader-writer.sqlite3");
        let writer = Arc::new(SqliteStore::open(&database_path).await.unwrap());
        let reader = Arc::new(SqliteStore::open(&database_path).await.unwrap());

        let writer_handle = {
            let writer = writer.clone();
            tokio::spawn(async move {
                for index in 0..READER_WRITER_TASKS {
                    writer
                        .insert_task(&task_record(
                            &format!("task-rw-{index}"),
                            &format!("Reader writer {index}"),
                        ))
                        .await
                        .unwrap();
                }
            })
        };

        while !writer_handle.is_finished() {
            let listed = reader.list_tasks().await.unwrap();
            assert!(
                listed.len() <= READER_WRITER_TASKS,
                "reader must never observe a torn or over-count listing"
            );
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        writer_handle.await.unwrap();

        let final_ids = task_ids(&reader).await;
        assert_eq!(final_ids.len(), READER_WRITER_TASKS);
    })
    .await
    .expect("reader/writer overlap test must not hang");
}

#[tokio::test]
async fn busy_write_is_retried_until_lock_releases_then_succeeds() {
    tokio::time::timeout(NO_HANG_TIMEOUT, async {
        let database_path = temp_path("retry-success.sqlite3");
        let store_a = SqliteStore::open(&database_path).await.unwrap();
        store_a
            .insert_task(&task_record("task-seed", "Seed task"))
            .await
            .unwrap();

        // A second pool acts as a second process holding the write lock.
        let lock_holder = open_raw_pool(&database_path).await;
        let mut transaction = lock_holder.begin().await.unwrap();
        sqlx::query("INSERT INTO events (task_id, event_kind) VALUES ('task-seed', 'lock_probe')")
            .execute(&mut *transaction)
            .await
            .unwrap();

        let writer = tokio::spawn({
            let database_path = database_path.clone();
            async move {
                let store_b =
                    SqliteStore::open_with_busy_timeout(&database_path, Duration::from_millis(50))
                        .await
                        .unwrap();
                store_b
                    .insert_task(&task_record("task-retry-win", "Retry winner"))
                    .await
            }
        });

        // Hold the write lock long enough to force at least one failed attempt.
        tokio::time::sleep(Duration::from_millis(150)).await;
        transaction.commit().await.unwrap();

        let outcome = writer.await.unwrap();
        assert!(
            outcome.is_ok(),
            "write should succeed after the lock releases, got: {:?}",
            outcome.unwrap_err()
        );
        assert!(store_a.get_task("task-retry-win").await.unwrap().is_some());
    })
    .await
    .expect("retry-success test must not hang");
}

#[tokio::test]
async fn exhausted_lock_retries_surface_typed_busy_error_without_hanging() {
    tokio::time::timeout(NO_HANG_TIMEOUT, async {
        let database_path = temp_path("retry-exhausted.sqlite3");
        let store_a = SqliteStore::open(&database_path).await.unwrap();
        store_a
            .insert_task(&task_record("task-seed-2", "Seed task"))
            .await
            .unwrap();

        let lock_holder = open_raw_pool(&database_path).await;
        let mut transaction = lock_holder.begin().await.unwrap();
        sqlx::query(
            "INSERT INTO events (task_id, event_kind) VALUES ('task-seed-2', 'lock_probe')",
        )
        .execute(&mut *transaction)
        .await
        .unwrap();

        let writer = tokio::spawn({
            let database_path = database_path.clone();
            async move {
                let store_b =
                    SqliteStore::open_with_busy_timeout(&database_path, Duration::from_millis(20))
                        .await
                        .unwrap();
                store_b
                    .insert_task(&task_record("task-retry-lose", "Retry loser"))
                    .await
            }
        });

        let outcome = writer.await.unwrap();
        let error = outcome.expect_err("locked write must fail with a typed error");
        assert!(
            matches!(&error, StoreError::DatabaseBusy { attempts, .. } if *attempts >= 2),
            "expected DatabaseBusy after bounded retries, got: {error}"
        );

        drop(transaction);
        lock_holder.close().await;
        assert!(
            store_a.get_task("task-retry-lose").await.unwrap().is_none(),
            "failed write must not leave a partial record"
        );
    })
    .await
    .expect("retry-exhaustion test must not hang");
}

#[tokio::test]
async fn schema_version_mismatch_fails_closed_with_typed_error() {
    tokio::time::timeout(NO_HANG_TIMEOUT, async {
        let database_path = temp_path("schema-newer.sqlite3");

        {
            let store = SqliteStore::open(&database_path).await.unwrap();
            store
                .insert_task(&task_record("task-schema", "Schema task"))
                .await
                .unwrap();
        }

        // Simulate a database written by a newer Relay build.
        let raw = open_raw_pool(&database_path).await;
        sqlx::query(
            r#"
            INSERT INTO _sqlx_migrations (version, description, installed_on, success, checksum, execution_time)
            VALUES (9999, 'future_migration', CURRENT_TIMESTAMP, true, X'00', 0)
            "#,
        )
        .execute(&raw)
        .await
        .unwrap();
        raw.close().await;

        let error = SqliteStore::open(&database_path)
            .await
            .expect_err("second process must refuse an unknown schema version");

        assert!(
            matches!(
                error,
                StoreError::SchemaVersionMismatch { version: 9999 }
            ),
            "expected SchemaVersionMismatch, got: {error}"
        );
    })
    .await
    .expect("schema mismatch test must not hang");
}

#[tokio::test]
async fn dirty_migration_state_fails_closed_with_typed_error() {
    tokio::time::timeout(NO_HANG_TIMEOUT, async {
        let database_path = temp_path("schema-dirty.sqlite3");

        {
            let _store = SqliteStore::open(&database_path).await.unwrap();
        }

        let raw = open_raw_pool(&database_path).await;
        sqlx::query("UPDATE _sqlx_migrations SET success = false WHERE version = 1")
            .execute(&raw)
            .await
            .unwrap();
        raw.close().await;

        let error = SqliteStore::open(&database_path)
            .await
            .expect_err("second process must refuse a dirty migration state");

        assert!(
            matches!(error, StoreError::SchemaMigrationDirty { version: 1 }),
            "expected SchemaMigrationDirty, got: {error}"
        );
    })
    .await
    .expect("dirty migration test must not hang");
}

async fn task_ids(store: &SqliteStore) -> Vec<String> {
    store
        .list_tasks()
        .await
        .unwrap()
        .into_iter()
        .map(|task| task.id)
        .collect()
}

/// Opens a standalone single-connection pool that mimics a second process.
async fn open_raw_pool(database_path: &std::path::Path) -> SqlitePool {
    let options = SqliteConnectOptions::new()
        .filename(database_path)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_millis(5000));
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap()
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
        handoff_json: format!(
            r#"{{"protocol_version":1,"project":"relay","title":"{title}","task":{{"goal":"Multi-process access test."}},"ambiguity_policy":"needs_clarification"}}"#
        ),
    }
}

fn temp_path(name: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("relay-store-concurrency-{name}-{nanos}"))
}
