//! Phase 07: HerdrBackplane role-host integration coverage.

use std::{path::PathBuf, sync::Arc};

use relay_runtime::{
    BackplaneError, BackplaneId, EffectKey, FakeHerdr, HerdrBackplane, HerdrClientError,
    HerdrVersion, LaunchSpecification, ProcessStatus, ReconcileOutcome, RoleHostResult,
    RoleHostSpec, RuntimeBackplane, RuntimeBindingState, RuntimeHandle, RuntimeMetadata,
    RuntimeProvisionRequest, SanitizedEnvironment, TrustedToken, check_compatibility,
    observed_role_result, role_host_result_path, role_host_spec_path,
};

fn unique_temp_dir(suffix: &str) -> PathBuf {
    let path =
        std::env::temp_dir().join(format!("relay_phase07_{suffix}_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).expect("failed to create temp dir");
    path
}

fn new_backplane(
    ops: Arc<FakeHerdr>,
    role_host_dir: PathBuf,
    relay_executable: PathBuf,
) -> HerdrBackplane<FakeHerdr> {
    HerdrBackplane::new("herdr-test", ops, 8, relay_executable, role_host_dir)
}

fn request(cwd: PathBuf, attempt: &str, effect: &str) -> RuntimeProvisionRequest {
    let mut environment = SanitizedEnvironment::new();
    environment.insert("RELAY_TEST_ALLOWED", "visible-value");
    RuntimeProvisionRequest {
        effect_key: EffectKey::new(effect),
        role_attempt_id: attempt.to_owned(),
        trusted_cwd: cwd,
        launch: LaunchSpecification::new(
            "powershell.exe",
            vec![
                "-NoProfile".into(),
                "-Command".into(),
                "Write-Output task; & 'quoted'".into(),
            ],
        ),
        environment,
        output_byte_limit: 1024,
        metadata: Some(RuntimeMetadata::new(attempt, EffectKey::new(effect))),
    }
}

#[tokio::test]
async fn compatibility_rejects_an_unsupported_herdr_version() {
    let ops = FakeHerdr::new();
    assert_eq!(check_compatibility(&ops).await.unwrap().protocol, 22);

    ops.set_version(HerdrVersion {
        major: 1,
        minor: 0,
        patch: 0,
        pre_release: None,
        build: None,
    });
    assert!(matches!(
        check_compatibility(&ops).await,
        Err(HerdrClientError::IncompatibleVersion { .. })
    ));
    ops.set_version(HerdrVersion {
        major: 0,
        minor: 9,
        patch: 5,
        pre_release: None,
        build: None,
    });
    ops.set_protocol(23);
    assert!(matches!(
        check_compatibility(&ops).await,
        Err(HerdrClientError::IncompatibleVersion { .. })
    ));
}

#[tokio::test]
async fn provisions_real_herdr_ids_and_only_types_the_trusted_launcher() {
    let ops = Arc::new(FakeHerdr::new());
    let root = unique_temp_dir("provision");
    let role_host_dir = root.join("state/role-host");
    let backplane = new_backplane(
        ops.clone(),
        role_host_dir.clone(),
        PathBuf::from(r"C:\Relay\relay.exe"),
    );
    let req = request(root.clone(), "attempt-host-1", "effect-host-1");
    let handle = backplane.provision(req.clone()).await.unwrap();

    let workspaces = ops.workspace_list().await.unwrap();
    assert_eq!(workspaces.len(), 1);
    let workspace = &workspaces[0];
    let tabs = ops.tab_list(&workspace.workspace_id).await.unwrap();
    let attempt_tab = tabs
        .iter()
        .find(|tab| tab.label == "Relay Attempt [attempt-host-1]")
        .expect("attempt tab should use the topology planner label");
    let pane = ops.pane_get(&handle.handle_id).await.unwrap().unwrap();
    assert_eq!(pane.workspace_id, workspace.workspace_id);
    assert_eq!(pane.tab_id, attempt_tab.tab_id);
    assert_eq!(pane.title.as_deref(), Some("attempt-host-1"));

    let spec_path = role_host_spec_path(&role_host_dir, "attempt-host-1").unwrap();
    let spec: RoleHostSpec = serde_json::from_slice(&std::fs::read(spec_path).unwrap()).unwrap();
    assert_eq!(spec.attempt_id, "attempt-host-1");
    assert_eq!(
        spec.argv,
        req.launch.args.iter().cloned().fold(
            vec![req.launch.executable.to_string_lossy().into_owned()],
            |mut argv, arg| {
                argv.push(arg);
                argv
            }
        )
    );
    assert!(
        spec.env
            .iter()
            .any(|(key, value)| { key == "RELAY_TEST_ALLOWED" && value == "visible-value" })
    );

    let calls = ops.pane_run_trusted_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, handle.handle_id);
    assert_eq!(
        calls[0]
            .1
            .iter()
            .map(TrustedToken::as_str)
            .collect::<Vec<_>>(),
        vec![
            r"C:\Relay\relay.exe",
            "role-host",
            "--attempt",
            "attempt-host-1"
        ]
    );
    assert!(
        !calls[0]
            .1
            .iter()
            .any(|token| token.as_str().contains("quoted"))
    );
}

#[tokio::test]
async fn unsafe_relay_path_fails_before_any_herdr_operation() {
    let ops = Arc::new(FakeHerdr::new());
    let root = unique_temp_dir("unsafe-executable");
    let backplane = new_backplane(
        ops.clone(),
        root.join("state/role-host"),
        PathBuf::from(r"C:\Program Files\Relay\relay.exe"),
    );
    let error = backplane
        .provision(request(root, "attempt-unsafe-1", "effect-unsafe-1"))
        .await
        .unwrap_err();
    assert!(
        matches!(error, BackplaneError::LaunchFailed(message) if message.contains("can't be typed safely"))
    );
    assert_eq!(ops.operation_count(), 0);
    assert!(ops.workspace_list().await.unwrap().is_empty());
}

#[tokio::test]
async fn duplicate_effect_key_reuses_the_same_herdr_pane_and_tab() {
    let ops = Arc::new(FakeHerdr::new());
    let root = unique_temp_dir("idempotency");
    let role_host_dir = root.join("state/role-host");
    let backplane = new_backplane(
        ops.clone(),
        role_host_dir,
        PathBuf::from(r"C:\Relay\relay.exe"),
    );
    let req = request(root.clone(), "attempt-dup-1", "effect-dup-1");
    let first = backplane.provision(req.clone()).await.unwrap();
    let second = backplane.provision(req).await.unwrap();
    assert_eq!(first, second);
    assert_eq!(ops.pane_run_trusted_calls().len(), 1);
    let workspace = ops.workspace_list().await.unwrap().pop().unwrap();
    assert_eq!(
        ops.tab_list(&workspace.workspace_id)
            .await
            .unwrap()
            .iter()
            .filter(|tab| tab.label == "Relay Attempt [attempt-dup-1]")
            .count(),
        1
    );
}

#[tokio::test]
async fn binding_snapshot_can_reconcile_a_surviving_herdr_pane() {
    let ops = Arc::new(FakeHerdr::new());
    let root = unique_temp_dir("reattach");
    let role_host_dir = root.join("state/role-host");
    let first = new_backplane(
        ops.clone(),
        role_host_dir.clone(),
        PathBuf::from(r"C:\Relay\relay.exe"),
    );
    let handle = first
        .provision(request(
            root.clone(),
            "attempt-reattach-1",
            "effect-reattach-1",
        ))
        .await
        .unwrap();
    let binding = first.bindings().snapshot().pop().unwrap();
    let second = new_backplane(
        ops.clone(),
        role_host_dir,
        PathBuf::from(r"C:\Relay\relay.exe"),
    );
    let outcome = second.reconcile_binding(&binding).await.unwrap();
    assert_eq!(outcome, ReconcileOutcome::Alive { pid: 1 });
    assert_eq!(handle.handle_id, binding.runtime_handle.handle_id);
}

#[tokio::test]
async fn server_unavailable_is_lost_and_never_fabricates_success() {
    let ops = Arc::new(FakeHerdr::new());
    let root = unique_temp_dir("server-down");
    let backplane = new_backplane(
        ops.clone(),
        root.join("state/role-host"),
        PathBuf::from(r"C:\Relay\relay.exe"),
    );
    let handle = backplane
        .provision(request(
            root,
            "attempt-server-down-1",
            "effect-server-down-1",
        ))
        .await
        .unwrap();
    ops.set_server_down(true);
    let observation = backplane.observe(&handle).await.unwrap();
    assert_eq!(observation.status, ProcessStatus::Lost);
    assert!(!matches!(
        observation.status,
        ProcessStatus::ExitedSuccess { .. }
    ));
}

#[tokio::test]
async fn missing_handle_is_not_reconciled_as_a_live_attempt() {
    let ops = Arc::new(FakeHerdr::new());
    let root = unique_temp_dir("missing");
    let backplane = new_backplane(
        ops,
        root.join("state/role-host"),
        PathBuf::from(r"C:\Relay\relay.exe"),
    );
    let missing = RuntimeHandle {
        handle_id: "w99:p9".into(),
        backplane_id: BackplaneId::new("herdr-test"),
        pid: None,
        created_at: "0".into(),
    };
    assert!(matches!(
        backplane.reconcile(&missing).await,
        Err(BackplaneError::HandleNotFound(_))
    ));
}

#[tokio::test]
async fn user_closed_pane_is_interrupted_and_never_success() {
    let ops = Arc::new(FakeHerdr::new());
    let root = unique_temp_dir("user-close");
    let backplane = new_backplane(
        ops.clone(),
        root.join("state/role-host"),
        PathBuf::from(r"C:\Relay\relay.exe"),
    );
    let handle = backplane
        .provision(request(root, "attempt-close-1", "effect-close-1"))
        .await
        .unwrap();
    ops.pane_close(&handle.handle_id).await.unwrap();
    let observation = backplane.observe(&handle).await.unwrap();
    assert_eq!(observation.status, ProcessStatus::Interrupted);
    assert_ne!(
        observation.status,
        ProcessStatus::ExitedSuccess { exit_code: 0 }
    );
    assert_eq!(
        backplane.reconcile(&handle).await.unwrap(),
        ReconcileOutcome::FailedExit { exit_code: None }
    );
    assert_eq!(
        backplane
            .bindings()
            .get_by_handle(&handle.handle_id)
            .unwrap()
            .state,
        RuntimeBindingState::Interrupted
    );
}

#[tokio::test]
async fn dispose_closes_only_the_owned_tab_keeps_result_and_rejects_input() {
    let ops = Arc::new(FakeHerdr::new());
    let root = unique_temp_dir("dispose");
    let role_host_dir = root.join("state/role-host");
    let backplane = new_backplane(
        ops.clone(),
        role_host_dir.clone(),
        PathBuf::from(r"C:\Relay\relay.exe"),
    );
    let handle = backplane
        .provision(request(
            root.clone(),
            "attempt-dispose-1",
            "effect-dispose-1",
        ))
        .await
        .unwrap();
    assert!(backplane.interrupt(&handle).await.unwrap().initiated);
    assert_eq!(
        ops.pane_send_keys_calls(),
        vec![(handle.handle_id.clone(), vec!["ctrl+c".into()])]
    );
    assert!(matches!(
        backplane.send(&handle, b"text").await,
        Err(BackplaneError::Unsupported(_))
    ));

    let result_path = role_host_result_path(&role_host_dir, "attempt-dispose-1").unwrap();
    std::fs::write(
        &result_path,
        serde_json::to_vec(&RoleHostResult {
            exit_code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
            interrupted: false,
        })
        .unwrap(),
    )
    .unwrap();
    let spec_path = role_host_spec_path(&role_host_dir, "attempt-dispose-1").unwrap();
    let receipt = backplane.dispose(&handle).await.unwrap();
    assert!(receipt.reaped);
    assert!(ops.pane_get(&handle.handle_id).await.unwrap().is_none());
    assert!(!spec_path.exists());
    assert!(result_path.exists());
}

#[tokio::test]
async fn disposing_an_unowned_user_pane_does_not_close_it() {
    let ops = Arc::new(FakeHerdr::new());
    let root = unique_temp_dir("user-pane");
    let backplane = new_backplane(
        ops.clone(),
        root.join("state/role-host"),
        PathBuf::from(r"C:\Relay\relay.exe"),
    );
    let user = ops.workspace_create(&root, "user workspace").await.unwrap();
    let handle = RuntimeHandle {
        handle_id: user.root_pane.pane_id.clone(),
        backplane_id: BackplaneId::new("herdr-test"),
        pid: Some(1),
        created_at: "now".into(),
    };
    assert!(matches!(
        backplane.dispose(&handle).await,
        Err(BackplaneError::HandleNotFound(_))
    ));
    assert!(ops.pane_get(&handle.handle_id).await.unwrap().is_some());
}

#[tokio::test]
async fn result_file_supplies_the_existing_typed_role_result_parser() {
    let ops = Arc::new(FakeHerdr::new());
    let root = unique_temp_dir("result");
    let role_host_dir = root.join("state/role-host");
    let backplane = new_backplane(
        ops,
        role_host_dir.clone(),
        PathBuf::from(r"C:\Relay\relay.exe"),
    );
    let handle = backplane
        .provision(request(root, "attempt-result-1", "effect-result-1"))
        .await
        .unwrap();
    let output = "{\"role_result\":\"implemented\",\"detail\":\"done\"}\n";
    std::fs::write(
        role_host_result_path(&role_host_dir, "attempt-result-1").unwrap(),
        serde_json::to_vec(&RoleHostResult {
            exit_code: Some(0),
            stdout: output.into(),
            stderr: "diagnostic".into(),
            interrupted: false,
        })
        .unwrap(),
    )
    .unwrap();
    let observation = backplane.observe(&handle).await.unwrap();
    assert_eq!(
        observation.status,
        ProcessStatus::ExitedSuccess { exit_code: 0 }
    );
    assert_eq!(
        String::from_utf8_lossy(&observation.stderr_chunk),
        "diagnostic"
    );
    let published = observed_role_result(&observation).unwrap();
    assert_eq!(published.payload.kind(), "implemented");
}
