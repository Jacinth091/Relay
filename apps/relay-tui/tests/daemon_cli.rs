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
        .env("RELAY_DAEMON_PIPE_SUFFIX", pipe_suffix_for(&database_path))
        .env("RELAY_DATABASE_PATH", database_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command.spawn().expect("failed to spawn relay binary")
}

#[test]
fn daemon_starts_and_second_instance_conflicts_on_lease() {
    let prefix = unique_temp_prefix("conflict");
    let lease_id = "test-daemon-lease";
    let database_path = prefix.join("state.sqlite3");

    // Spawn the first daemon and wait until it actually answers on its pipe.
    // The lease is taken during startup, after the store has been opened and
    // migrated, so a fixed sleep here would race both the first migration of a
    // fresh database and the lease insert itself on a slow machine.
    let mut d1 = spawn_daemon(&prefix, lease_id, 30);
    let _first_client = wait_for_daemon_ready(&pipe_name_for(&database_path), &mut d1);

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

/// Retries pinging the daemon over its real named pipe until it responds,
/// bounded so a genuinely broken daemon fails the test instead of hanging.
/// On failure, kills `daemon` and folds its stderr into the panic message.
fn wait_for_daemon_ready(pipe_name: &str, daemon: &mut Child) -> NamedPipeDaemonClient {
    let client = NamedPipeDaemonClient::new(pipe_name);
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

    // Short TTL so the test does not have to wait out a long default lease
    // window before a fresh daemon can take over after the hard kill below.
    let mut d1 = spawn_daemon(&prefix, lease_id, 2);
    let client = wait_for_daemon_ready(&pipe_name, &mut d1);

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
    let mut d2 = spawn_daemon(&prefix, lease_id, 2);
    let client2 = wait_for_daemon_ready(&pipe_name, &mut d2);

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
