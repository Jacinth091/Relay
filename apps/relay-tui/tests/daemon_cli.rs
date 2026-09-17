//! Process-boundary tests for `relay daemon`.

use std::{
    process::{Child, Command, Stdio},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

fn unique_temp_prefix(name: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("relay-daemon-{name}-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn spawn_daemon(prefix: &std::path::Path, lease_id: &str, lease_ttl: u64) -> Child {
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
        .env("RELAY_DATABASE_PATH", prefix.join("state.sqlite3"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command.spawn().expect("failed to spawn relay binary")
}

#[test]
fn daemon_starts_and_second_instance_conflicts_on_lease() {
    let prefix = unique_temp_prefix("conflict");
    let lease_id = "test-daemon-lease";

    // Spawn first daemon
    let mut d1 = spawn_daemon(&prefix, lease_id, 30);

    // Give it a brief moment to acquire the lease
    std::thread::sleep(Duration::from_millis(500));

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
