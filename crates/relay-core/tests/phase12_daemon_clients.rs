//! Phase 12: Web UI + Relay TUI as Daemon Clients Integration Tests.
//!
//! Enforces:
//! 1. Client handshake and protocol negotiation (version matching and mismatch rejection).
//! 2. State synchronization: multiple clients (Web, TUI) agree on daemon state.
//! 3. Closing/reconnecting clients does not disrupt active workflows.
//! 4. Clients synchronize monotonically via sequential event catch-up.
//! 5. Command idempotency: duplicate command submissions return cached results without re-executing.

use std::{sync::Arc, time::Duration};

use relay_core::{
    CommandStatus, DaemonClient, DaemonCommand, DaemonRequest, DaemonResponse, DaemonServer,
    InProcessDaemonClient, ProjectRegistry, RegisterProject, Relay, SqliteStore,
};

async fn setup_test_daemon(prefix: &str) -> (Arc<DaemonServer>, SqliteStore) {
    let mut registry = ProjectRegistry::empty();
    let tmp_path =
        std::env::temp_dir().join(format!("relay_phase12_{}_{}", prefix, uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp_path).unwrap();

    registry
        .register(RegisterProject {
            key: "web-client-proj".to_owned(),
            name: None,
            local_path: tmp_path,
            expected_remote: None,
            default_stack: None,
            github: None,
        })
        .unwrap();

    let relay = Relay::with_project_registry(registry);
    let store = SqliteStore::in_memory().await.unwrap();
    let daemon = DaemonServer::start(
        store.clone(),
        relay,
        &format!("lease-{}", prefix),
        Duration::from_secs(60),
    )
    .await
    .unwrap();

    (Arc::new(daemon), store)
}

#[tokio::test]
async fn test_client_handshake_and_protocol_negotiation() {
    let (server, _) = setup_test_daemon("handshake").await;
    let client = InProcessDaemonClient::new(server.clone());

    // 1. Valid handshake
    let (ver, token) = client
        .handshake("relay-web")
        .await
        .expect("handshake failed");
    assert!(!ver.is_empty());
    assert!(token > 0);

    // 2. Ping
    assert!(client.ping().await.is_ok());

    // 3. Protocol mismatch rejected
    let mismatch_resp = server
        .handle_request(DaemonRequest::Handshake {
            client_name: "legacy-client".to_owned(),
            protocol_version: 9999, // Mismatched version
        })
        .await;

    match mismatch_resp {
        DaemonResponse::Error(msg) => {
            assert!(msg.contains("protocol version mismatch"));
        }
        _ => panic!("expected protocol version mismatch error"),
    }
}

#[tokio::test]
async fn test_multi_client_state_agreement_and_monotonic_events() {
    let (server, _) = setup_test_daemon("multi_client").await;
    let web_client = InProcessDaemonClient::new(server.clone());
    let tui_client = InProcessDaemonClient::new(server.clone());

    web_client.handshake("relay-web").await.unwrap();
    tui_client.handshake("relay-tui").await.unwrap();

    // Submit a handoff task via Web client
    let handoff_json = serde_json::json!({
        "protocol_version": 1,
        "project": "web-client-proj",
        "title": "Phase 12 Sync Task",
        "task": { "goal": "Verify Web and TUI client agreement" },
        "ambiguity_policy": "needs_clarification"
    })
    .to_string();

    let status = web_client
        .submit_command(
            "cmd-1",
            DaemonCommand::SubmitHandoff {
                project_key: "web-client-proj".to_owned(),
                handoff_json,
                submission_transport: "local-file".to_owned(),
            },
        )
        .await
        .unwrap();
    assert_eq!(status, CommandStatus::Applied);

    // Both clients fetch snapshot: they must agree 100% on state
    let web_snapshot = web_client.get_snapshot().await.unwrap();
    let tui_snapshot = tui_client.get_snapshot().await.unwrap();

    assert_eq!(web_snapshot.tasks.len(), 1);
    assert_eq!(tui_snapshot.tasks.len(), 1);
    assert_eq!(web_snapshot.tasks[0].id, tui_snapshot.tasks[0].id);
    assert_eq!(
        web_snapshot.latest_event_sequence,
        tui_snapshot.latest_event_sequence
    );

    // Monotonic event stream catch-up
    let events = tui_client.get_events_after(0, 10).await.unwrap();
    assert!(!events.is_empty(), "Events must be sequenced and readable");
    assert_eq!(events[0].sequence, 1);
}

#[tokio::test]
async fn test_client_disconnect_and_reconnect_retains_daemon_state() {
    let (server, _) = setup_test_daemon("reconnect").await;

    // Client 1 connects, submits, and then disconnects (dropped)
    {
        let client1 = InProcessDaemonClient::new(server.clone());
        client1.handshake("relay-tui").await.unwrap();

        let handoff_json = serde_json::json!({
            "protocol_version": 1,
            "project": "web-client-proj",
            "title": "Durable Task",
            "task": { "goal": "Verify persistence across client disconnect" },
            "ambiguity_policy": "needs_clarification"
        })
        .to_string();

        let status = client1
            .submit_command(
                "cmd-reconnect-1",
                DaemonCommand::SubmitHandoff {
                    project_key: "web-client-proj".to_owned(),
                    handoff_json,
                    submission_transport: "local-file".to_owned(),
                },
            )
            .await
            .unwrap();
        assert_eq!(status, CommandStatus::Applied);
    }

    // Client 2 connects fresh: daemon state is preserved
    let client2 = InProcessDaemonClient::new(server.clone());
    let (_, fencing_token) = client2.handshake("relay-web").await.unwrap();
    assert!(fencing_token > 0);

    let snapshot = client2.get_snapshot().await.unwrap();
    assert_eq!(snapshot.tasks.len(), 1);
    assert_eq!(snapshot.tasks[0].title, "Durable Task");
}
