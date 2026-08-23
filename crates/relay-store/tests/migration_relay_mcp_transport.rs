//! Migration upgrade test for the `relay-mcp` submission transport.
//!
//! Builds a database carrying ONLY the pre-0003 schema (0001 + the additive
//! 0002), seeds it with rows across every table that references `tasks`, then
//! opens it through [`SqliteStore::open`] so migration 0003 rebuilds the
//! schema. All pre-existing data must survive and new submissions must be
//! able to use the `relay-mcp` transport, which the old CHECK rejected.

use std::time::{SystemTime, UNIX_EPOCH};

use relay_store::{NewTaskRecord, SqliteStore};
use sqlx::{Connection, Row, SqliteConnection, sqlite::SqliteConnectOptions};

fn temp_database_path(name: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("relay-store-{name}-{nanos}.sqlite3"))
}

fn legacy_schema_sql() -> String {
    // The exact pre-0003 schema plus its indexes.
    format!(
        "{}\n{}",
        include_str!("../migrations/0001_initial_state.sql"),
        "-- additive column from the old 0002\nALTER TABLE executions ADD COLUMN runtime_metadata_json TEXT;"
    )
}

async fn seed_legacy_database(path: &std::path::Path) {
    let mut connection = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true),
    )
    .await
    .unwrap();

    sqlx::raw_sql(&legacy_schema_sql())
        .execute(&mut connection)
        .await
        .unwrap();

    // Mark the two pre-0003 migrations as applied so opening through
    // SqliteStore applies only migration 0003.
    // Mark the two pre-0003 migrations as applied so opening through
    // SqliteStore applies only migration 0003. The migrator normally creates
    // this table itself; recreate its exact shape here.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS _sqlx_migrations (
            version BIGINT PRIMARY KEY,
            description TEXT NOT NULL,
            installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
            success BOOLEAN NOT NULL,
            checksum BLOB NOT NULL,
            execution_time BIGINT NOT NULL
        );",
    )
    .execute(&mut connection)
    .await
    .unwrap();

    let migrator = sqlx::migrate!("./migrations");
    for migration in migrator.iter().take(2) {
        sqlx::query(
            "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) \
             VALUES (?, ?, 1, ?, 0)",
        )
        .bind(migration.version)
        .bind(&*migration.description)
        .bind(&*migration.checksum)
        .execute(&mut connection)
        .await
        .unwrap();
    }

    sqlx::query(
        r#"
        INSERT INTO tasks (
            id, project_key, title, requested_stack, submission_transport,
            source_kind, source_provider, state, protocol_version, handoff_json
        ) VALUES (
            'legacy-task', 'relay', 'Legacy task', NULL, 'local-paste',
            NULL, NULL, 'ready', 1, '{"legacy":true}'
        );
        INSERT INTO external_bindings (id, task_id, binding_kind, remote_identity, sync_status)
        VALUES ('binding-1', 'legacy-task', 'github-issue', 'org/repo#7', 'synced');
        INSERT INTO executions (id, task_id, runtime_profile, phase, repair_attempt_count)
        VALUES ('exec-1', 'legacy-task', 'chatgpt-codex', 'implement', 0);
        INSERT INTO clarification_requests (id, task_id, execution_id, request_kind, category, packet_json)
        VALUES ('clar-1', 'legacy-task', 'exec-1', 'needs_decision', 'scope', '{"packet":true}');
        INSERT INTO resolutions (id, request_id, resolution_json, work_disposition)
        VALUES ('res-1', 'clar-1', '{"answer":42}', 'resume');
        INSERT INTO events (id, task_id, execution_id, event_kind, event_json)
        VALUES (11, 'legacy-task', 'exec-1', 'output', '{"chunk":1}');
        "#,
    )
    .execute(&mut connection)
    .await
    .unwrap();

    let _ = connection.close().await;
}

fn relay_mcp_record(id: &str) -> NewTaskRecord {
    NewTaskRecord {
        id: id.to_owned(),
        project_key: "relay".to_owned(),
        title: "MCP submission after upgrade".to_owned(),
        requested_stack: Some("chatgpt-codex".to_owned()),
        submission_transport: "relay-mcp".to_owned(),
        source_kind: Some("conversational_ai".to_owned()),
        source_provider: Some("openai".to_owned()),
        state: "ready".to_owned(),
        protocol_version: 1,
        handoff_json: r#"{"protocol_version":1,"project":"relay","title":"MCP submission after upgrade","task":{"goal":"Upgrade check."},"ambiguity_policy":"needs_clarification"}"#.to_owned(),
    }
}

#[tokio::test]
async fn migration_0003_preserves_data_and_enables_relay_mcp_transport() {
    let database_path = temp_database_path("upgrade");
    seed_legacy_database(&database_path).await;

    let store = SqliteStore::open(&database_path).await.unwrap();

    // Every seeded row survived the rebuild.
    let counts = sqlx::query(
        "SELECT
            (SELECT COUNT(*) FROM tasks WHERE id = 'legacy-task') AS tasks,
            (SELECT COUNT(*) FROM external_bindings WHERE id = 'binding-1') AS bindings,
            (SELECT COUNT(*) FROM executions WHERE id = 'exec-1') AS executions,
            (SELECT COUNT(*) FROM clarification_requests WHERE id = 'clar-1') AS clarifications,
            (SELECT COUNT(*) FROM resolutions WHERE id = 'res-1' AND resume_outcome IS NULL) AS resolutions,
            (SELECT COUNT(*) FROM events WHERE id = 11 AND event_kind = 'output') AS events",
    )
    .fetch_one(store.pool())
    .await
    .unwrap();

    for column in [
        "tasks",
        "bindings",
        "executions",
        "clarifications",
        "resolutions",
        "events",
    ] {
        assert_eq!(counts.get::<i64, _>(column), 1, "lost row: {column}");
    }

    // Foreign keys still resolve across the rebuilt tables.
    let binding_task = sqlx::query(
        "SELECT b.task_id FROM external_bindings b JOIN tasks t ON t.id = b.task_id WHERE b.id = 'binding-1'",
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(binding_task.get::<String, _>("task_id"), "legacy-task");

    // The rebuilt CHECK accepts the new transport.
    let inserted = store
        .insert_task(&relay_mcp_record("mcp-after-upgrade"))
        .await
        .unwrap();
    assert_eq!(inserted.submission_transport, "relay-mcp");

    // And still rejects unknown transports (fail closed).
    let mut invalid = relay_mcp_record("bogus-transport");
    invalid.submission_transport = "carrier-pigeon".to_owned();
    assert!(store.insert_task(&invalid).await.is_err());

    let _ = std::fs::remove_file(&database_path);
}
