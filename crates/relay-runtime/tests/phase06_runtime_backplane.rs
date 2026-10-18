//! Phase 06: Runtime Backplane Seam, LocalProcessBackplane & FakeBackplane Integration Tests.
//!
//! Validates:
//! 1. LocalProcessBackplane starts an executable with explicit arguments, trusted CWD, and streams output.
//! 2. Streaming output captures within configured byte limits.
//! 3. Interrupt/cancellation terminates the child process.
//! 4. Duplicate effect key is idempotent (returns existing handle).
//! 5. Reconcile accurately detects clean exit vs alive process.
//! 6. Rejection of unauthorized/non-existent CWD.
//! 7. FakeBackplane behaves deterministically for unit/mock scenarios.

use std::{fs, path::PathBuf};

#[cfg(windows)]
use relay_runtime::process_is_alive;
use relay_runtime::{
    BackplaneError, EffectKey, LaunchSpecification, LocalProcessBackplane, ProcessStatus,
    ReconcileOutcome, RuntimeBackplane, RuntimeHandle, RuntimeProvisionRequest,
    SanitizedEnvironment,
};
use relay_runtime_fake::FakeBackplane;

fn unique_temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("relay-p06-{name}-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[tokio::test]
async fn test_local_process_backplane_basic_execution_and_observe() {
    let backplane = LocalProcessBackplane::new("test-local-bp", 4);
    let tmp = unique_temp_dir("basic");

    let req = RuntimeProvisionRequest {
        effect_key: EffectKey::new("eff-basic-1"),
        role_attempt_id: "attempt-1".to_owned(),
        trusted_cwd: tmp.clone(),
        launch: LaunchSpecification::new("git", vec!["--version".to_owned()]),
        environment: SanitizedEnvironment::new(),
        output_byte_limit: 1024,
        metadata: None,
    };

    let handle = backplane.provision(req).await.expect("provisioning failed");
    assert_eq!(handle.backplane_id.0, "test-local-bp");
    assert!(!handle.handle_id.is_empty());

    // Allow process to run and complete
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let obs = backplane.observe(&handle).await.expect("observe failed");
    assert_eq!(obs.handle_id, handle.handle_id);
    match obs.status {
        ProcessStatus::ExitedSuccess { exit_code } => assert_eq!(exit_code, 0),
        ProcessStatus::Running => {
            // Give another moment if under heavy CI
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let obs2 = backplane.observe(&handle).await.unwrap();
            assert!(matches!(
                obs2.status,
                ProcessStatus::ExitedSuccess { exit_code: 0 }
            ));
        }
        other => panic!("expected ExitedSuccess, got {:?}", other),
    }

    let stdout_str = String::from_utf8_lossy(&obs.stdout_chunk);
    assert!(
        stdout_str.contains("git version"),
        "expected git version in stdout, got: {stdout_str}"
    );

    // Reconcile confirms cleanly exited
    let outcome = backplane
        .reconcile(&handle)
        .await
        .expect("reconcile failed");
    assert!(matches!(
        outcome,
        ReconcileOutcome::CleanlyExited { exit_code: Some(0) }
    ));
}

#[tokio::test]
async fn test_local_process_backplane_output_limit_clamping() {
    let backplane = LocalProcessBackplane::new("test-limit-bp", 4);
    let tmp = unique_temp_dir("limit");

    // Output limit of 10 bytes
    let req = RuntimeProvisionRequest {
        effect_key: EffectKey::new("eff-limit-1"),
        role_attempt_id: "attempt-limit".to_owned(),
        trusted_cwd: tmp.clone(),
        launch: LaunchSpecification::new("git", vec!["--version".to_owned()]),
        environment: SanitizedEnvironment::new(),
        output_byte_limit: 10,
        metadata: None,
    };

    let handle = backplane.provision(req).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let obs = backplane.observe(&handle).await.unwrap();
    assert!(
        obs.stdout_chunk.len() <= 10,
        "stdout exceeded clamped limit: {}",
        obs.stdout_chunk.len()
    );
}

#[tokio::test]
async fn test_local_process_backplane_idempotent_effect_key() {
    let backplane = LocalProcessBackplane::new("test-idempotent-bp", 4);
    let tmp = unique_temp_dir("idempotent");

    let req = RuntimeProvisionRequest {
        effect_key: EffectKey::new("eff-shared-key"),
        role_attempt_id: "attempt-first".to_owned(),
        trusted_cwd: tmp.clone(),
        launch: LaunchSpecification::new("git", vec!["--version".to_owned()]),
        environment: SanitizedEnvironment::new(),
        output_byte_limit: 1024,
        metadata: None,
    };

    let handle1 = backplane.provision(req.clone()).await.unwrap();

    // Provision again with exact same effect_key
    let mut req2 = req.clone();
    req2.role_attempt_id = "attempt-duplicate".to_owned();
    let handle2 = backplane.provision(req2).await.unwrap();

    assert_eq!(
        handle1.handle_id, handle2.handle_id,
        "duplicate effect_key must return same handle"
    );
}

#[tokio::test]
async fn test_local_process_backplane_rejects_nonexistent_cwd() {
    let backplane = LocalProcessBackplane::new("test-reject-bp", 4);
    let bogus = PathBuf::from("C:\\relay_nonexistent_directory_bogus_12345");

    let req = RuntimeProvisionRequest {
        effect_key: EffectKey::new("eff-reject"),
        role_attempt_id: "attempt-bogus".to_owned(),
        trusted_cwd: bogus.clone(),
        launch: LaunchSpecification::new("git", vec!["--version".to_owned()]),
        environment: SanitizedEnvironment::new(),
        output_byte_limit: 1024,
        metadata: None,
    };

    let err = backplane.provision(req).await.unwrap_err();
    assert!(
        matches!(err, BackplaneError::UnauthorizedCwd(ref p) if *p == bogus),
        "expected UnauthorizedCwd, got: {:?}",
        err
    );
}

#[tokio::test]
async fn test_local_process_backplane_dispose() {
    let backplane = LocalProcessBackplane::new("test-dispose-bp", 4);
    let tmp = unique_temp_dir("dispose");

    let req = RuntimeProvisionRequest {
        effect_key: EffectKey::new("eff-dispose"),
        role_attempt_id: "attempt-disp".to_owned(),
        trusted_cwd: tmp.clone(),
        launch: LaunchSpecification::new("git", vec!["--version".to_owned()]),
        environment: SanitizedEnvironment::new(),
        output_byte_limit: 1024,
        metadata: None,
    };

    let handle = backplane.provision(req).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let receipt = backplane.dispose(&handle).await.unwrap();
    assert!(receipt.reaped, "process slot should be reaped");

    // Second dispose returns false (already reaped)
    let receipt2 = backplane.dispose(&handle).await.unwrap();
    assert!(
        !receipt2.reaped,
        "second dispose should return reaped: false"
    );

    // Observe after dispose fails with HandleNotFound
    let obs_err = backplane.observe(&handle).await.unwrap_err();
    assert!(matches!(obs_err, BackplaneError::HandleNotFound(_)));
}

#[tokio::test]
async fn test_fake_backplane_deterministic_flow() {
    let backplane = FakeBackplane::new();

    let req = RuntimeProvisionRequest {
        effect_key: EffectKey::new("eff-fake-1"),
        role_attempt_id: "role-att-99".to_owned(),
        trusted_cwd: PathBuf::from("."),
        launch: LaunchSpecification::new("test-proc", vec![]),
        environment: SanitizedEnvironment::new(),
        output_byte_limit: 512,
        metadata: None,
    };

    let handle = backplane.provision(req).await.unwrap();
    assert_eq!(handle.backplane_id.0, "fake-backplane");
    assert_eq!(handle.handle_id, "fake-h-role-att-99");

    let obs = backplane.observe(&handle).await.unwrap();
    assert!(matches!(
        obs.status,
        ProcessStatus::ExitedSuccess { exit_code: 0 }
    ));
    assert_eq!(obs.stdout_chunk, b"fake backplane execution output\n");

    let send_rcpt = backplane.send(&handle, b"hello backplane").await.unwrap();
    assert_eq!(send_rcpt.bytes_written, 15);
    assert_eq!(
        backplane.get_sent_input(&handle.handle_id).unwrap(),
        b"hello backplane"
    );

    let snap = backplane.snapshot().await.unwrap();
    assert_eq!(snap.active_processes, 1);

    let disp = backplane.dispose(&handle).await.unwrap();
    assert!(disp.reaped);

    let reconcile = backplane.reconcile(&handle).await.unwrap();
    assert!(matches!(reconcile, ReconcileOutcome::ResourceNotFound));
}

#[cfg(windows)]
fn spawn_long_running_probe() -> std::process::Child {
    std::process::Command::new("ping")
        .args(["-n", "60", "127.0.0.1"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("failed to spawn long-running probe process")
}

#[cfg(not(windows))]
fn spawn_long_running_probe() -> std::process::Child {
    std::process::Command::new("sleep")
        .arg("60")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("failed to spawn long-running probe process")
}

fn handle_for_pid(backplane: &LocalProcessBackplane, handle_id: &str, pid: u32) -> RuntimeHandle {
    RuntimeHandle {
        handle_id: handle_id.to_owned(),
        backplane_id: backplane.id(),
        pid: Some(pid),
        created_at: "test".to_owned(),
    }
}

#[tokio::test]
async fn test_local_process_backplane_child_sees_only_sanitized_environment() {
    // A secret in this test process must not reach the launched child.
    unsafe {
        std::env::set_var("RELAY_TEST_SECRET", "leaked-secret-value");
    }

    let backplane = LocalProcessBackplane::new("test-env-bp", 4);
    let tmp = unique_temp_dir("env");

    #[cfg(windows)]
    let (program, args) = ("cmd", vec!["/C".to_owned(), "set".to_owned()]);
    #[cfg(not(windows))]
    let (program, args) = ("env", Vec::new());

    let mut environment = SanitizedEnvironment::new();
    environment.insert("RELAY_TEST_ALLOWED", "visible-value");

    let req = RuntimeProvisionRequest {
        effect_key: EffectKey::new("eff-env-1"),
        role_attempt_id: "attempt-env".to_owned(),
        trusted_cwd: tmp.clone(),
        launch: LaunchSpecification::new(program, args),
        environment,
        output_byte_limit: 64 * 1024,
        metadata: None,
    };

    let handle = backplane.provision(req).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let obs = backplane.observe(&handle).await.unwrap();
    let stdout = String::from_utf8_lossy(&obs.stdout_chunk);

    assert!(
        !stdout.contains("RELAY_TEST_SECRET"),
        "child inherited an unsanitized parent variable: {stdout}"
    );
    assert!(
        stdout.contains("RELAY_TEST_ALLOWED=visible-value"),
        "sanitized variable missing from child environment: {stdout}"
    );
    assert!(
        stdout.contains("PATH="),
        "allow-listed PATH should be inherited: {stdout}"
    );

    let _ = backplane.dispose(&handle).await;
}

#[cfg(windows)]
#[tokio::test]
async fn test_local_process_backplane_dispose_kills_grandchild() {
    let backplane = LocalProcessBackplane::new("test-tree-bp", 4);
    let tmp = unique_temp_dir("tree");
    let pid_file = tmp.join("grandchild.pid");

    let script = format!(
        "$g = Start-Process -FilePath 'ping.exe' -ArgumentList '-n','60','127.0.0.1' \
         -PassThru -WindowStyle Hidden; Set-Content -LiteralPath '{}' -Value $g.Id; \
         Start-Sleep -Seconds 60",
        pid_file.display()
    );

    let req = RuntimeProvisionRequest {
        effect_key: EffectKey::new("eff-tree-1"),
        role_attempt_id: "attempt-tree".to_owned(),
        trusted_cwd: tmp.clone(),
        launch: LaunchSpecification::new(
            "powershell",
            vec!["-NoProfile".to_owned(), "-Command".to_owned(), script],
        ),
        environment: SanitizedEnvironment::new(),
        output_byte_limit: 4096,
        metadata: None,
    };

    let handle = backplane.provision(req).await.unwrap();

    // Wait for the child to publish its grandchild's PID.
    let mut grandchild_pid = None;
    for _ in 0..100 {
        if let Ok(text) = fs::read_to_string(&pid_file) {
            if let Ok(pid) = text.trim().parse::<u32>() {
                grandchild_pid = Some(pid);
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let grandchild_pid = grandchild_pid.expect("grandchild PID was never published");

    assert!(
        process_is_alive(grandchild_pid).await,
        "grandchild {grandchild_pid} should be running before dispose"
    );

    let receipt = backplane.dispose(&handle).await.unwrap();
    assert!(receipt.reaped);

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(
        !process_is_alive(grandchild_pid).await,
        "dispose must kill the whole process tree; grandchild {grandchild_pid} survived"
    );
}

#[tokio::test]
async fn test_local_process_backplane_reconcile_reports_live_orphan() {
    let mut probe = spawn_long_running_probe();
    let pid = probe.id();

    // A fresh backplane instance (as after a daemon restart) has no active slot
    // for this handle, but the recorded PID is still running.
    let backplane = LocalProcessBackplane::new("test-orphan-live-bp", 4);
    let handle = handle_for_pid(&backplane, "orphan-live-handle", pid);

    let outcome = backplane.reconcile(&handle).await.unwrap();
    assert_eq!(outcome, ReconcileOutcome::Orphaned { pid });

    let _ = probe.kill();
    let _ = probe.wait();
}

#[tokio::test]
async fn test_local_process_backplane_reconcile_dead_pid_is_not_found() {
    let mut probe = spawn_long_running_probe();
    let pid = probe.id();
    probe.kill().expect("failed to kill probe process");
    probe.wait().expect("failed to reap probe process");

    let backplane = LocalProcessBackplane::new("test-orphan-dead-bp", 4);
    let handle = handle_for_pid(&backplane, "orphan-dead-handle", pid);

    let outcome = backplane.reconcile(&handle).await.unwrap();
    assert_eq!(outcome, ReconcileOutcome::ResourceNotFound);
}
