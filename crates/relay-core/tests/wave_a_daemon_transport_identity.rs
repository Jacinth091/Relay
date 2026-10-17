//! Real named-pipe coverage for the PR #16 governed review corrections.
//!
//! These corrections can only be *proved* across a genuine transport round trip,
//! so they are tested against a daemon serving a real pipe rather than the
//! in-process client:
//!
//! - R16-02: a client bound to one database's durable identity cannot reach - let
//!   alone mutate - a daemon serving a different database, and a refused request
//!   is never reported as a successful intake.
//! - R16-03: a project registered *after* the daemon started is honoured, because
//!   submissions are resolved against the persisted trusted registry.
//! - R16-04: losing the daemon lease stops the daemon serving and releases its
//!   transport, so a successor can bind it.
//! - R17-02 (re-review of R16-04): `run_full_daemon` does not report a running
//!   daemon until its listener has actually bound the transport, retries binding
//!   while a stale instance still holds the pipe, and fails startup truthfully if
//!   it cannot bind at all.

#![cfg(windows)]

use std::{
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use relay_core::{
    CommandStatus, DaemonClient, DaemonCommand, DaemonError, DaemonRequest, DaemonResponse,
    NamedPipeDaemonClient, ProjectRegistry, RegisterProject, Relay, SqliteStore, default_pipe_name,
    list_stored_tasks, run_full_daemon,
};
use tokio::net::windows::named_pipe::ServerOptions;

const HANDOFF: &str = r#"{
    "protocol_version": 1,
    "project": "test-proj",
    "title": "Daemon Test Task",
    "task": { "goal": "Verify transport identity binding" },
    "ambiguity_policy": "needs_clarification"
}"#;

fn temp_prefix(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("relay-transport-{name}-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn client_for(server: &relay_core::DaemonServer) -> NamedPipeDaemonClient {
    NamedPipeDaemonClient::new(
        default_pipe_name(server.state_identity()),
        server.state_identity().to_owned(),
    )
}

fn queue_effect(effect_id: &str, idempotency_key: &str) -> DaemonCommand {
    DaemonCommand::QueuePendingEffect {
        effect_id: effect_id.to_owned(),
        effect_type: "test".to_owned(),
        idempotency_key: idempotency_key.to_owned(),
        payload_json: "{}".to_owned(),
    }
}

/// Retries until the daemon's transport accepts requests.
///
/// `run_full_daemon` binds the pipe from a spawned task, so a freshly started
/// daemon is not necessarily reachable the instant it is returned.
async fn wait_until_serving(client: &NamedPipeDaemonClient) {
    for _ in 0..100 {
        if client.ping().await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("daemon never became reachable on its pipe");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pipe_client_cannot_reach_a_daemon_serving_a_different_database() {
    let dir = temp_prefix("identity");
    let store = SqliteStore::open(dir.join("state.sqlite3")).await.unwrap();
    let other_store = SqliteStore::open(dir.join("other.sqlite3")).await.unwrap();

    let identity = store.store_identity().await.unwrap();
    let other_identity = other_store.store_identity().await.unwrap();
    assert_ne!(
        identity, other_identity,
        "two databases must never share a durable state identity"
    );

    let (server, embedded) = run_full_daemon(
        store,
        Relay::new(),
        "lease-identity",
        Duration::from_secs(30),
    )
    .await
    .unwrap();
    assert_eq!(server.state_identity(), identity);

    // A client bound to the served database reaches it normally.
    let own_client = client_for(&server);
    wait_until_serving(&own_client).await;

    // The same transport address, reached by a client bound to a *different*
    // database, must be refused: a reachable pipe is not proof of matching identity.
    let crossed = NamedPipeDaemonClient::new(default_pipe_name(&identity), other_identity);
    match crossed.ping().await {
        Err(DaemonError::StateIdentityMismatch { .. }) => {}
        other => panic!("expected a state identity mismatch, got {other:?}"),
    }

    // A refused request must never be reported as a successful intake.
    match crossed
        .submit_command(
            "cmd-crossed-identity",
            queue_effect("eff-crossed", "idem-crossed"),
        )
        .await
    {
        Err(DaemonError::StateIdentityMismatch { .. }) => {}
        other => panic!("expected a state identity mismatch on submit, got {other:?}"),
    }

    // Nothing was written to either database.
    assert!(list_stored_tasks(server.store()).await.unwrap().is_empty());
    assert!(
        other_store
            .get_command("cmd-crossed-identity")
            .await
            .unwrap()
            .is_none(),
        "a refused request must not be recorded anywhere"
    );
    assert!(list_stored_tasks(&other_store).await.unwrap().is_empty());

    embedded.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_handoff_for_a_project_registered_after_daemon_start_is_accepted() {
    let dir = temp_prefix("registry-refresh");
    let config_path = dir.join("config.toml");
    let repo_path = dir.join("repo");
    std::fs::create_dir_all(&repo_path).unwrap();

    let store = SqliteStore::open(dir.join("state.sqlite3")).await.unwrap();

    // The daemon starts from the persisted trusted config, which does not know the
    // project yet - the state after Relay has started but before Add Project.
    let relay = Relay::load_from_config_path(&config_path).unwrap();
    let (server, embedded) = run_full_daemon(
        store.clone(),
        relay,
        "lease-registry-refresh",
        Duration::from_secs(30),
    )
    .await
    .unwrap();
    let client = client_for(&server);
    wait_until_serving(&client).await;

    // The approved Add Project action persists the registration to the same trusted
    // config the daemon reads; the daemon must honour it without a restart.
    let mut registry = ProjectRegistry::load(&config_path).unwrap();
    registry
        .register(RegisterProject {
            key: "test-proj".to_owned(),
            name: None,
            local_path: repo_path,
            expected_remote: None,
            default_stack: None,
            github: None,
        })
        .unwrap();
    registry.save(&config_path).unwrap();

    let status = client
        .submit_command(
            "cmd-registry-refresh",
            DaemonCommand::SubmitHandoff {
                project_key: "test-proj".to_owned(),
                handoff_json: HANDOFF.to_owned(),
                submission_transport: "local-paste".to_owned(),
            },
        )
        .await
        .expect("submission over the pipe must succeed once the project is registered");
    assert_eq!(status, CommandStatus::Applied);

    // The handoff is durably READY in the daemon's own database.
    let tasks = list_stored_tasks(&store).await.unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].handoff.title, "Daemon Test Task");
    assert_eq!(tasks[0].state.as_str(), "ready");

    embedded.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn losing_the_lease_stops_the_daemon_serving_and_releases_its_pipe() {
    let dir = temp_prefix("lease-loss");
    let store = SqliteStore::open(dir.join("state.sqlite3")).await.unwrap();

    // A short TTL keeps the test quick: the heartbeat ticks every second.
    let (server, mut embedded) = run_full_daemon(
        store.clone(),
        Relay::new(),
        "lease-loss",
        Duration::from_secs(2),
    )
    .await
    .unwrap();
    let client = client_for(&server);
    wait_until_serving(&client).await;
    let held_token = server.fencing_token();

    // A successor takes the lease: a different holder on an already-elapsed TTL.
    let forged = store
        .acquire_or_renew_daemon_lease(
            "lease-loss",
            std::process::id() as i64 + 1,
            "forged-successor",
            Duration::ZERO,
        )
        .await
        .expect("forging a successor lease must succeed");
    assert!(
        forged.fencing_token > held_token,
        "successor fencing token must be monotonic"
    );

    // The next heartbeat fails, so the daemon stops claiming authority...
    tokio::time::timeout(Duration::from_secs(20), embedded.wait_for_shutdown())
        .await
        .expect("the daemon must stop once it loses the lease");

    match server.handle_request(DaemonRequest::Ping).await {
        DaemonResponse::Error(message) => assert!(
            message.contains("no longer authoritative"),
            "unexpected message: {message}"
        ),
        other => panic!("expected a fail-closed error, got {other:?}"),
    }

    // ...and releases its transport so a successor can bind it.
    let mut released = false;
    for _ in 0..60 {
        if matches!(client.ping().await, Err(DaemonError::NotRunning)) {
            released = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        released,
        "the daemon must release its pipe once it loses the lease"
    );

    embedded.shutdown().await;

    // A successor can take over the database and serve it: authority genuinely
    // moved, and the stale daemon's exit did not leave the lease unrecoverable.
    // The forged successor row is aged past the TTL the new daemon will request,
    // so it is reclaimable with a strictly higher fencing token.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let (successor, successor_embedded) =
        run_full_daemon(store, Relay::new(), "lease-loss", Duration::from_secs(1))
            .await
            .expect("a successor must be able to take over the released lease");
    assert!(
        successor.fencing_token() > forged.fencing_token,
        "the successor must hold a strictly higher fencing token"
    );
    let successor_client = client_for(&successor);
    wait_until_serving(&successor_client).await;
    successor_embedded.shutdown().await;
}

/// R17-02: a successor that cannot bind the transport must not be reported as a
/// running daemon. Previously `run_full_daemon` returned as soon as it spawned
/// the listener, so a bind failure left a "running" daemon whose pipe belonged
/// to no one and whose error had only been logged.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_daemon_that_cannot_bind_its_pipe_reports_startup_failure() {
    let dir = temp_prefix("bind-failure");
    let store = SqliteStore::open(dir.join("state.sqlite3")).await.unwrap();
    let identity = store.store_identity().await.unwrap();
    let pipe_name = default_pipe_name(&identity);

    // Hold the first pipe instance for the whole test, so the daemon can never
    // bind its transport.
    let _blocker = ServerOptions::new()
        .first_pipe_instance(true)
        .create(&pipe_name)
        .expect("the test must be able to hold the pipe first");

    let started = run_full_daemon(
        store,
        Relay::new(),
        "lease-bind-failure",
        Duration::from_secs(30),
    )
    .await;
    assert!(
        started.is_err(),
        "a daemon that cannot bind its transport must not report itself running"
    );
}

/// R17-02: binding is retried while a stale instance still holds the pipe, so a
/// takeover that overlaps a live predecessor converges on a serving daemon
/// instead of a silently dead one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_daemon_waits_for_a_held_pipe_and_then_serves() {
    let dir = temp_prefix("bind-retry");
    let store = SqliteStore::open(dir.join("state.sqlite3")).await.unwrap();
    let identity = store.store_identity().await.unwrap();
    let pipe_name = default_pipe_name(&identity);

    let blocker = ServerOptions::new()
        .first_pipe_instance(true)
        .create(&pipe_name)
        .expect("the test must be able to hold the pipe first");
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        drop(blocker);
    });

    let (server, embedded) = run_full_daemon(
        store,
        Relay::new(),
        "lease-bind-retry",
        Duration::from_secs(30),
    )
    .await
    .expect("the daemon must bind once the held pipe is released");

    let client = client_for(&server);
    wait_until_serving(&client).await;
    let status = client
        .submit_command("cmd-after-retry", queue_effect("eff-retry", "idem-retry"))
        .await
        .expect("the retried listener must serve commands");
    assert_eq!(status, CommandStatus::Applied);

    embedded.shutdown().await;
}
