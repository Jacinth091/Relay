//! Wave A (R12-01/R12-03/R12-04) crash-safety regression tests.
//!
//! Enforces the acceptance criteria from the governed remediation plan:
//! 1. Same command id + same payload is safe and deterministic.
//! 2. Crash before `apply_command` completes (command stuck `received`) is
//!    reconciled on restart without losing or duplicating the task.
//! 3. Crash after the semantic write commits but before `complete_command`
//!    runs is reconciled without duplicating the task.
//! 4. A command payload that cannot be safely replayed is parked
//!    `needs_reconciliation` instead of silently stuck or guessed at.
//! 5. The effect worker reconciles effects stuck `in_flight` after a crash.

use std::{sync::Arc, time::Duration};

use relay_core::{
    CommandStatus, DaemonClient, DaemonCommand, DaemonServer, EffectHandler, EffectOutcome,
    EffectWorkerRegistry, InProcessDaemonClient, PendingEffectRecord, ProjectRegistry,
    RegisterProject, Relay, SqliteStore, list_stored_tasks, run_effect_worker,
};
use relay_store::DaemonFence;

/// Always succeeds; used to prove a reconciled effect completes its
/// lifecycle end to end, not just that it is unstuck from `in_flight`.
struct AlwaysSucceedHandler;

impl EffectHandler for AlwaysSucceedHandler {
    async fn handle(&self, _effect: &PendingEffectRecord) -> EffectOutcome {
        EffectOutcome::Success("{}".to_owned())
    }
}

/// Signals when it has been entered, then blocks until released, so a test can
/// act *while* the worker holds an effect's claim.
struct GatedHandler {
    started: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

impl EffectHandler for GatedHandler {
    async fn handle(&self, _effect: &PendingEffectRecord) -> EffectOutcome {
        self.started.notify_one();
        self.release.notified().await;
        EffectOutcome::Failure("late worker result".to_owned())
    }
}

fn test_relay() -> Relay {
    let mut registry = ProjectRegistry::empty();
    let path = std::env::temp_dir().join(format!("relay-wave-a-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).unwrap();
    registry
        .register(RegisterProject {
            key: "wave-a-proj".to_owned(),
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
    "project": "wave-a-proj",
    "title": "Wave A crash-safety task",
    "task": { "goal": "Verify command idempotency" },
    "ambiguity_policy": "needs_clarification"
}"#;

fn submit_handoff_command() -> DaemonCommand {
    DaemonCommand::SubmitHandoff {
        project_key: "wave-a-proj".to_owned(),
        handoff_json: TEST_HANDOFF.to_owned(),
        submission_transport: "local-paste".to_owned(),
    }
}

#[tokio::test]
async fn concurrent_duplicate_submit_creates_exactly_one_task() {
    let store = SqliteStore::in_memory().await.unwrap();
    let server = Arc::new(
        DaemonServer::start(
            store.clone(),
            test_relay(),
            "lease-concurrent",
            Duration::from_secs(60),
        )
        .await
        .unwrap(),
    );
    let client = InProcessDaemonClient::new(server.clone());

    let command_id = "cmd-concurrent-1".to_owned();
    let (first, second) = tokio::join!(
        client.submit_command(&command_id, submit_handoff_command()),
        client.submit_command(&command_id, submit_handoff_command()),
    );

    // Both submissions race on the same command id; depending on
    // interleaving either may observe the other's result first, so both
    // legitimately resolve to `Applied` or one resolves to `Duplicate` - but
    // neither may ever fail, and only one task may ever be created.
    for status in [first.unwrap(), second.unwrap()] {
        assert_ne!(status, CommandStatus::Failed);
    }

    let tasks = list_stored_tasks(&store).await.unwrap();
    assert_eq!(
        tasks.len(),
        1,
        "concurrent duplicate submits must create exactly one task"
    );
}

#[tokio::test]
async fn reconciliation_applies_a_command_recorded_but_never_applied() {
    let store = SqliteStore::in_memory().await.unwrap();
    let command_id = "cmd-crash-a".to_owned();
    let command = submit_handoff_command();
    let payload_json = serde_json::to_string(&command).unwrap();

    // Simulate a crash between recording the command (`received`) and
    // `apply_command` ever running: only the command row exists.
    store
        .record_command(&command_id, "submit_handoff", &payload_json)
        .await
        .unwrap();

    let server = DaemonServer::start(
        store.clone(),
        test_relay(),
        "lease-crash-a",
        Duration::from_secs(60),
    )
    .await
    .unwrap();
    server.reconcile_on_startup().await.unwrap();

    let tasks = list_stored_tasks(&store).await.unwrap();
    assert_eq!(
        tasks.len(),
        1,
        "reconciliation must apply the orphaned command exactly once"
    );

    let resolved = store.get_command(&command_id).await.unwrap().unwrap();
    assert_eq!(resolved.status, "applied");
}

#[tokio::test]
async fn reconciliation_does_not_duplicate_a_task_already_applied_before_the_crash() {
    let store = SqliteStore::in_memory().await.unwrap();
    let server = Arc::new(
        DaemonServer::start(
            store.clone(),
            test_relay(),
            "lease-crash-b",
            Duration::from_secs(60),
        )
        .await
        .unwrap(),
    );
    let client = InProcessDaemonClient::new(server.clone());
    let command_id = "cmd-crash-b".to_owned();

    // Apply the command normally first: this is what "the semantic write
    // committed" looks like (the task row exists, command is `applied`).
    let status = client
        .submit_command(&command_id, submit_handoff_command())
        .await
        .unwrap();
    assert_eq!(status, CommandStatus::Applied);
    assert_eq!(list_stored_tasks(&store).await.unwrap().len(), 1);

    // Simulate the crash landing between `apply_command` succeeding (the
    // task above already exists) and `complete_command` ever being recorded:
    // roll the command row back to `received` without touching the task.
    store
        .complete_command(&command_id, "received", None)
        .await
        .unwrap();

    let fresh_server = DaemonServer::start(
        store.clone(),
        test_relay(),
        "lease-crash-b-2",
        Duration::from_secs(60),
    )
    .await
    .unwrap();
    fresh_server.reconcile_on_startup().await.unwrap();

    let tasks = list_stored_tasks(&store).await.unwrap();
    assert_eq!(
        tasks.len(),
        1,
        "reconciliation must not duplicate a task the crashed attempt already committed"
    );

    let resolved = store.get_command(&command_id).await.unwrap().unwrap();
    assert_eq!(resolved.status, "applied");
}

#[tokio::test]
async fn corrupted_command_payload_is_parked_needs_reconciliation_without_affecting_healthy_rows() {
    let store = SqliteStore::in_memory().await.unwrap();

    store
        .record_command("cmd-corrupt", "submit_handoff", "{not valid json")
        .await
        .unwrap();
    store
        .record_command(
            "cmd-healthy",
            "submit_handoff",
            &serde_json::to_string(&submit_handoff_command()).unwrap(),
        )
        .await
        .unwrap();

    let server = DaemonServer::start(
        store.clone(),
        test_relay(),
        "lease-corrupt",
        Duration::from_secs(60),
    )
    .await
    .unwrap();
    server.reconcile_on_startup().await.unwrap();

    let corrupt = store.get_command("cmd-corrupt").await.unwrap().unwrap();
    assert_eq!(corrupt.status, "needs_reconciliation");

    let healthy = store.get_command("cmd-healthy").await.unwrap().unwrap();
    assert_eq!(healthy.status, "applied");

    let snapshot_count = store
        .count_commands_with_status("needs_reconciliation")
        .await
        .unwrap();
    assert_eq!(snapshot_count, 1);
}

#[tokio::test]
async fn effect_worker_reconciles_stale_in_flight_effects_after_a_crash() {
    let store = SqliteStore::in_memory().await.unwrap();
    let server = Arc::new(
        DaemonServer::start(
            store.clone(),
            test_relay(),
            "lease-effect",
            Duration::from_secs(60),
        )
        .await
        .unwrap(),
    );
    let client = InProcessDaemonClient::new(server.clone());

    client
        .submit_command(
            "cmd-queue-effect",
            DaemonCommand::QueuePendingEffect {
                effect_id: "eff-crash-1".to_owned(),
                effect_type: "test_effect".to_owned(),
                idempotency_key: "idem-crash-1".to_owned(),
                payload_json: "{}".to_owned(),
            },
        )
        .await
        .unwrap();

    // Simulate a worker having marked the effect in-flight, then crashing
    // before ever completing it. Rather than backdating the timestamp
    // directly (which would need raw SQL access this crate doesn't otherwise
    // need), let real time pass past a short staleness threshold before the
    // worker's first poll.
    store
        .update_effect_state("eff-crash-1", "in_flight", None)
        .await
        .unwrap();

    const POLL_INTERVAL: Duration = Duration::from_millis(20);
    // The worker treats an in-flight effect as stale past 6x poll_interval.
    tokio::time::sleep(POLL_INTERVAL * 7).await;

    let mut registry = EffectWorkerRegistry::new();
    registry.register("test_effect", AlwaysSucceedHandler);

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let worker_store = store.clone();
    let worker = tokio::spawn(run_effect_worker(
        worker_store,
        server.fence(),
        registry,
        POLL_INTERVAL,
        5,
        shutdown_rx,
    ));

    // Give the worker a poll cycle to reconcile the stale effect back to
    // `pending` and then dispatch it to completion - proving the effect was
    // actually unstuck, not merely renamed.
    tokio::time::sleep(POLL_INTERVAL * 3).await;
    let _ = shutdown_tx.send(true);
    let _ = worker.await;

    let effect = store
        .get_pending_effect("eff-crash-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        effect.state, "completed",
        "a stale in-flight effect must be reconciled and re-dispatched, not left stuck"
    );
}

/// FLOW-19-01: the effect worker mutates the same canonical `pending_effects` /
/// `effect_results` ledger the daemon owns, so its writes are fenced on the lease
/// and guarded on the state it claimed. A worker whose claim is overtaken while
/// its handler runs must not settle the effect or record a contradictory result.
///
/// The handler gate makes the interleaving deterministic: the successor settles
/// the effect *while* the stale worker is provably still inside its handler,
/// which is exactly the window the unfenced worker wrote through.
#[tokio::test]
async fn a_stale_effect_worker_result_is_refused_after_a_successor_takes_the_lease() {
    const POLL_INTERVAL: Duration = Duration::from_millis(20);

    let store = SqliteStore::in_memory().await.unwrap();
    let server = Arc::new(
        DaemonServer::start(
            store.clone(),
            test_relay(),
            "lease-worker-authority",
            Duration::from_secs(30),
        )
        .await
        .unwrap(),
    );
    let client = InProcessDaemonClient::new(server.clone());

    // The effect is queued through the daemon's own command path, so it is the
    // daemon's ledger the worker will write to.
    client
        .submit_command(
            "cmd-queue-gated",
            DaemonCommand::QueuePendingEffect {
                effect_id: "eff-gated".to_owned(),
                effect_type: "gated_effect".to_owned(),
                idempotency_key: "idem-gated".to_owned(),
                payload_json: "{}".to_owned(),
            },
        )
        .await
        .unwrap();

    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let mut registry = EffectWorkerRegistry::new();
    registry.register(
        "gated_effect",
        GatedHandler {
            started: started.clone(),
            release: release.clone(),
        },
    );

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let worker = tokio::spawn(run_effect_worker(
        store.clone(),
        server.fence(),
        registry,
        POLL_INTERVAL,
        5,
        shutdown_rx,
    ));

    // The worker claimed the effect and is now inside the handler.
    tokio::time::timeout(Duration::from_secs(10), started.notified())
        .await
        .expect("the worker must dispatch the queued effect");
    let claimed = store
        .get_pending_effect("eff-gated")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        claimed.state, "in_flight",
        "the worker must claim the effect before dispatching it"
    );

    // A successor takes the lease and settles the effect successfully while the
    // stale worker is still blocked in its handler.
    let successor = store
        .acquire_or_renew_daemon_lease(
            "lease-worker-authority",
            std::process::id() as i64 + 1,
            "successor-host",
            "successor-instance",
            Duration::ZERO,
        )
        .await
        .expect("forging a successor lease must succeed");
    let successor_fence = DaemonFence {
        lease_id: "lease-worker-authority".to_owned(),
        holder_pid: successor.holder_pid,
        holder_instance: successor.holder_instance.clone(),
        fencing_token: successor.fencing_token,
    };
    store
        .record_effect_result_fenced(
            "success-result",
            "eff-gated",
            "success",
            r#"{"ok":true}"#,
            &successor_fence,
        )
        .await
        .expect("the successor must be able to settle the effect it owns");

    // Release the handler. Its late failure must be refused, and the worker must
    // stop rather than keep polling without authority.
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(10), worker)
        .await
        .expect("the worker must stop once its lease is gone")
        .expect("the worker task must not panic");

    let settled = store
        .get_pending_effect("eff-gated")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        settled.state, "completed",
        "a stale worker result must not regress the successor's completed effect"
    );
    let _ = shutdown_tx.send(true);
}
