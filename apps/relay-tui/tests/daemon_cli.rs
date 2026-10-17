//! Process-boundary tests for `relay daemon`.

use std::{
    future::Future,
    process::{Child, Command, Stdio},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use relay_core::{CommandStatus, DaemonClient, DaemonCommand, NamedPipeDaemonClient};

fn unique_temp_prefix(name: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("relay-daemon-{name}-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Named pipes are a single OS-wide namespace, so every spawned daemon needs
/// a pipe name derived from its own isolated database path to avoid
/// colliding with other daemons started by concurrently running tests. Only
/// a bare suffix crosses the environment variable boundary (never a full
/// `\\.\pipe\...` path) - see `default_pipe_name`'s doc comment.
fn pipe_suffix_for(database_path: &std::path::Path) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    database_path.hash(&mut hasher);
    format!("relay-test-{:x}", hasher.finish())
}

/// The full pipe path a daemon spawned by [`spawn_daemon`] listens on. The
/// spawned process prefixes `RELAY_DAEMON_PIPE_SUFFIX` itself, so a test that
/// connects as a client must build the complete name from the same suffix.
fn pipe_name_for(database_path: &std::path::Path) -> String {
    format!(r"\\.\pipe\{}", pipe_suffix_for(database_path))
}

fn spawn_daemon(prefix: &std::path::Path, lease_id: &str, lease_ttl: u64) -> Child {
    let suffix = pipe_suffix_for(&prefix.join("state.sqlite3"));
    spawn_daemon_with_pipe_suffix(prefix, lease_id, lease_ttl, Some(&suffix))
}

/// Spawns `relay daemon` with an explicit transport suffix override.
///
/// `None` leaves `RELAY_DAEMON_PIPE_SUFFIX` unset, so the daemon derives its pipe
/// name from the durable identity of the database it serves. That is the
/// production default, and the configuration under test must not let two
/// databases share one transport address.
fn spawn_daemon_with_pipe_suffix(
    prefix: &std::path::Path,
    lease_id: &str,
    lease_ttl: u64,
    pipe_suffix: Option<&str>,
) -> Child {
    let database_path = prefix.join("state.sqlite3");
    let mut command = Command::new(env!("CARGO_BIN_EXE_relay"));
    command
        .args([
            "daemon",
            "--lease-id",
            lease_id,
            "--lease-ttl",
            &lease_ttl.to_string(),
        ])
        .env("RELAY_CONFIG_PATH", prefix.join("config.toml"))
        // Clear any inherited override first, so `None` means "derived identity".
        .env_remove("RELAY_DAEMON_PIPE_SUFFIX")
        .env("RELAY_DATABASE_PATH", database_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(suffix) = pipe_suffix {
        command.env("RELAY_DAEMON_PIPE_SUFFIX", suffix);
    }
    command.spawn().expect("failed to spawn relay binary")
}

#[test]
fn daemon_starts_and_second_instance_conflicts_on_lease() {
    let prefix = unique_temp_prefix("conflict");
    let lease_id = "test-daemon-lease";
    let database_path = prefix.join("state.sqlite3");

    // Prepare (create and migrate) the database before spawning, so the test
    // process never migrates the same fresh file concurrently with a daemon.
    let identity = prepare_database(&database_path);
    let pipe_name = pipe_name_for(&database_path);

    // Spawn the first daemon and wait until it actually answers on its pipe.
    // The lease is taken during startup, after the store has been opened and
    // migrated, so a fixed sleep here would race the lease insert itself on a
    // slow machine.
    let mut d1 = spawn_daemon(&prefix, lease_id, 30);
    let _first_client = wait_for_daemon_ready(&pipe_name, identity, &mut d1);

    // Spawn second daemon competing for the exact same lease
    let d2 = spawn_daemon(&prefix, lease_id, 30);

    // Second daemon should exit non-zero due to lease conflict
    let output2 = d2.wait_with_output().expect("d2 failed to exit");
    assert!(
        !output2.status.success(),
        "competing daemon must fail when lease held"
    );
    let stderr2 = String::from_utf8_lossy(&output2.stderr);
    assert!(
        stderr2.contains("lease") && stderr2.contains("held"),
        "stderr should mention lease conflict: {stderr2}"
    );

    // Kill d1
    let _ = d1.kill();
    let _ = d1.wait();
}

fn block_on<T>(future: impl Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build single-threaded test runtime")
        .block_on(future)
}

/// Creates and migrates the database at `database_path` and returns its durable
/// state identity.
///
/// Called *before* any daemon is spawned so that the test process is never
/// migrating the same virgin file concurrently with a daemon process: at this
/// revision the migration scripts are not idempotent across concurrent
/// processes, so two migrators racing the same fresh database can corrupt it.
fn prepare_database(database_path: &std::path::Path) -> String {
    block_on(async {
        relay_core::SqliteStore::open(database_path)
            .await
            .expect("prepare the daemon's database")
            .store_identity()
            .await
            .expect("read durable state identity")
    })
}

/// Retries pinging the daemon over its real named pipe until it responds,
/// bounded so a genuinely broken daemon fails the test instead of hanging.
/// On failure, kills `daemon` and folds its stderr into the panic message.
///
/// The client is bound to the durable identity of the database the daemon
/// serves - read by [`prepare_database`] before the daemon started - because the
/// transport refuses to talk to a daemon serving different durable state.
fn wait_for_daemon_ready(
    pipe_name: &str,
    state_identity: String,
    daemon: &mut Child,
) -> NamedPipeDaemonClient {
    let client = NamedPipeDaemonClient::new(pipe_name, state_identity);
    wait_for_client(client, pipe_name, daemon)
}

/// Waits until the daemon serving `state_identity` answers on its
/// identity-derived (default) pipe name.
fn wait_for_default_pipe_daemon(state_identity: &str, daemon: &mut Child) -> NamedPipeDaemonClient {
    let pipe_name = relay_core::default_pipe_name(state_identity);
    let client = NamedPipeDaemonClient::new(&pipe_name, state_identity.to_owned());
    wait_for_client(client, &pipe_name, daemon)
}

fn wait_for_client(
    client: NamedPipeDaemonClient,
    pipe_name: &str,
    daemon: &mut Child,
) -> NamedPipeDaemonClient {
    for _ in 0..50 {
        if block_on(client.ping()).is_ok() {
            return client;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = daemon.kill();
    let mut stderr_buf = String::new();
    if let Some(mut stderr) = daemon.stderr.take() {
        use std::io::Read;
        let _ = stderr.read_to_string(&mut stderr_buf);
    }
    let _ = daemon.wait();
    panic!("daemon never became ready on {pipe_name}; stderr:\n{stderr_buf}");
}

#[test]
fn daemon_serves_requests_over_the_real_pipe_and_recovers_after_a_hard_kill() {
    let prefix = unique_temp_prefix("pipe-roundtrip");
    let lease_id = "test-daemon-pipe";
    let database_path = prefix.join("state.sqlite3");
    // The client connects in-process (no environment variable hand-off), so
    // it builds the full pipe path directly rather than going through the
    // spawned process's own prefixing.
    let pipe_name = pipe_name_for(&database_path);

    // Prepare the database before either daemon starts (see
    // [`prepare_database`]) so only a daemon process ever migrates it.
    let identity = prepare_database(&database_path);

    // Short TTL so the test does not have to wait out a long default lease
    // window before a fresh daemon can take over after the hard kill below.
    let mut d1 = spawn_daemon(&prefix, lease_id, 2);
    let client = wait_for_daemon_ready(&pipe_name, identity, &mut d1);

    let status = block_on(client.submit_command(
        "cmd-pipe-roundtrip-1",
        DaemonCommand::QueuePendingEffect {
            effect_id: "eff-pipe-roundtrip-1".to_owned(),
            effect_type: "test".to_owned(),
            idempotency_key: "idem-pipe-roundtrip-1".to_owned(),
            payload_json: "{}".to_owned(),
        },
    ))
    .expect("submit_command over the real pipe must succeed");
    assert_eq!(status, CommandStatus::Applied);

    let snapshot = block_on(client.get_snapshot()).expect("daemon must serve a snapshot");
    assert_eq!(snapshot.pending_effects.len(), 1);
    assert_eq!(snapshot.stuck_commands_needing_reconciliation, 0);

    // Hard kill: no clean shutdown, so the lease is never explicitly
    // released and the pipe disappears out from under any connected client.
    let _ = d1.kill();
    let _ = d1.wait();

    // Let the short lease TTL lapse so the respawned daemon can reclaim it
    // rather than failing with a lease conflict against the dead process.
    std::thread::sleep(Duration::from_secs(3));

    // Staleness is judged against *this* acquisition attempt's own requested
    // TTL (not the original holder's), so the respawned daemon must request
    // a TTL no longer than the wait above for the stale lease to be reclaimed.
    // The database is already migrated; reading its identity again keeps the
    // respawned daemon bound to the same durable state as the first client.
    let identity = prepare_database(&database_path);
    let mut d2 = spawn_daemon(&prefix, lease_id, 2);
    let client2 = wait_for_daemon_ready(&pipe_name, identity, &mut d2);

    // Restart must reconstruct exact state from the same database: the
    // effect queued before the kill is still there, exactly once, and
    // startup reconciliation left nothing stuck.
    let snapshot2 =
        block_on(client2.get_snapshot()).expect("respawned daemon must serve a snapshot");
    assert_eq!(snapshot2.pending_effects.len(), 1);
    assert_eq!(snapshot2.pending_effects[0].id, "eff-pipe-roundtrip-1");
    assert_eq!(snapshot2.stuck_commands_needing_reconciliation, 0);

    let _ = d2.kill();
    let _ = d2.wait();
}

/// A handoff for the trusted project `test-proj`, submitted over the real pipe.
const TEST_HANDOFF: &str = r#"{
    "protocol_version": 1,
    "project": "test-proj",
    "title": "Daemon Test Task",
    "task": { "goal": "Verify pipe submission against the persisted registry" },
    "ambiguity_policy": "needs_clarification"
}"#;

/// R16-02 at the process boundary: with the production default transport (no
/// `RELAY_DAEMON_PIPE_SUFFIX`), two daemons serving two databases must never
/// share a transport address, and a client bound to one database must not be
/// able to reach - or falsely report a successful intake against - the other.
#[test]
fn two_databases_never_cross_write_on_the_default_pipe_identity() {
    let prefix_a = unique_temp_prefix("identity-a");
    let prefix_b = unique_temp_prefix("identity-b");
    let database_a = prefix_a.join("state.sqlite3");
    let database_b = prefix_b.join("state.sqlite3");

    // Prepare both databases before any daemon starts: the test process must not
    // migrate a virginal file at the same time as a daemon process.
    let identity_a = prepare_database(&database_a);
    let identity_b = prepare_database(&database_b);
    assert_ne!(
        identity_a, identity_b,
        "two databases must never share a durable state identity"
    );

    let mut daemon_a = spawn_daemon_with_pipe_suffix(&prefix_a, "lease-identity-a", 30, None);
    let client_a = wait_for_default_pipe_daemon(&identity_a, &mut daemon_a);
    let mut daemon_b = spawn_daemon_with_pipe_suffix(&prefix_b, "lease-identity-b", 30, None);
    let client_b = wait_for_default_pipe_daemon(&identity_b, &mut daemon_b);

    // Distinct databases have distinct durable identities, hence distinct
    // identity-derived pipe names.
    assert_ne!(
        relay_core::default_pipe_name(&identity_a),
        relay_core::default_pipe_name(&identity_b),
        "two databases must not alias onto one transport address"
    );

    // Each client writes only its own daemon.
    let status_a = block_on(client_a.submit_command(
        "cmd-identity-a",
        DaemonCommand::QueuePendingEffect {
            effect_id: "eff-identity-a".to_owned(),
            effect_type: "test".to_owned(),
            idempotency_key: "idem-identity-a".to_owned(),
            payload_json: "{}".to_owned(),
        },
    ))
    .expect("client A must submit to its own daemon");
    assert_eq!(status_a, CommandStatus::Applied);
    let status_b = block_on(client_b.submit_command(
        "cmd-identity-b",
        DaemonCommand::QueuePendingEffect {
            effect_id: "eff-identity-b".to_owned(),
            effect_type: "test".to_owned(),
            idempotency_key: "idem-identity-b".to_owned(),
            payload_json: "{}".to_owned(),
        },
    ))
    .expect("client B must submit to its own daemon");
    assert_eq!(status_b, CommandStatus::Applied);

    // A client bound to database B, addressed to database A's transport, is
    // refused - and must not report a successful intake.
    let crossed = NamedPipeDaemonClient::new(
        relay_core::default_pipe_name(&identity_a),
        identity_b.clone(),
    );
    match block_on(crossed.submit_command(
        "cmd-crossed-identity",
        DaemonCommand::QueuePendingEffect {
            effect_id: "eff-crossed-identity".to_owned(),
            effect_type: "test".to_owned(),
            idempotency_key: "idem-identity-crossed".to_owned(),
            payload_json: "{}".to_owned(),
        },
    )) {
        Err(relay_core::DaemonError::StateIdentityMismatch { .. }) => {}
        other => panic!("expected a state identity mismatch, got {other:?}"),
    }

    // Each database holds exactly its own single effect, and the refused command
    // was never recorded anywhere.
    let snapshot_a = block_on(client_a.get_snapshot()).expect("snapshot A");
    assert_eq!(snapshot_a.pending_effects.len(), 1);
    assert_eq!(snapshot_a.pending_effects[0].id, "eff-identity-a");
    let snapshot_b = block_on(client_b.get_snapshot()).expect("snapshot B");
    assert_eq!(snapshot_b.pending_effects.len(), 1);
    assert_eq!(snapshot_b.pending_effects[0].id, "eff-identity-b");

    let store_b = block_on(relay_core::SqliteStore::open(&database_b)).expect("open database B");
    assert!(
        block_on(store_b.get_command("cmd-crossed-identity"))
            .expect("read command row")
            .is_none(),
        "a refused request must not be recorded against the wrong database"
    );

    let _ = daemon_a.kill();
    let _ = daemon_a.wait();
    let _ = daemon_b.kill();
    let _ = daemon_b.wait();
}

/// R16-03 at the process boundary: an out-of-process daemon started with an
/// empty registry must honour a project registered afterwards, so Add Project
/// takes effect without restarting `relay daemon`.
#[test]
fn out_of_process_daemon_honours_a_project_registered_after_start() {
    let prefix = unique_temp_prefix("registry-refresh");
    let database_path = prefix.join("state.sqlite3");
    let config_path = prefix.join("config.toml");
    let repo_path = prefix.join("repo");
    std::fs::create_dir_all(&repo_path).unwrap();

    // The daemon starts before the project is registered.
    let identity = prepare_database(&database_path);
    let mut daemon = spawn_daemon(&prefix, "lease-registry-refresh", 30);
    let client = wait_for_daemon_ready(&pipe_name_for(&database_path), identity, &mut daemon);

    // Add Project: persist the registration to the trusted config the daemon reads.
    let mut registry = relay_core::ProjectRegistry::load(&config_path).expect("load config");
    registry
        .register(relay_core::RegisterProject {
            key: "test-proj".to_owned(),
            name: None,
            local_path: repo_path,
            expected_remote: None,
            default_stack: None,
            github: None,
        })
        .expect("register project");
    registry.save(&config_path).expect("save config");

    // The long-running daemon picks the registration up at submit time.
    let status = block_on(client.submit_command(
        "cmd-registry-refresh",
        DaemonCommand::SubmitHandoff {
            project_key: "test-proj".to_owned(),
            handoff_json: TEST_HANDOFF.to_owned(),
            submission_transport: "local-paste".to_owned(),
        },
    ))
    .expect("submission must succeed once the project is registered");
    assert_eq!(status, CommandStatus::Applied);

    let snapshot = block_on(client.get_snapshot()).expect("snapshot");
    assert_eq!(snapshot.tasks.len(), 1);
    assert_eq!(snapshot.tasks[0].title, "Daemon Test Task");
    assert_eq!(snapshot.tasks[0].state, "ready");

    let _ = daemon.kill();
    let _ = daemon.wait();
}
