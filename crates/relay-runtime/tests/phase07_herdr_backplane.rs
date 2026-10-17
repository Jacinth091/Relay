//! Phase 07: HerdrBackplane One-Role Vertical Slice Tests.
//!
//! Verifies:
//! 1. Herdr protocol capability negotiation and version checks.
//! 2. Duplicate effect reconciliation idempotency (no duplicate panes spawned).
//! 3. Relay restart while Herdr session survives (reattached and reconciled as Alive).
//! 4. Herdr restart/crash while Relay survives (reconciled as ResourceNotFound, never fabricating completion).
//! 5. Missing and mismatched resource reconciliation outcomes.
//! 6. Manual user intervention / termination detection (never fabricating semantic role completion).
//! 7. Internal topology planner isolation (adapter policy, never exposed as Relay domain identity).
//! 8. Cleanup ownership preservation (never deleting user-created unrelated Herdr resources).
//! 9. Bounded output capture and terminal input streaming.

use std::{collections::HashMap, path::PathBuf, sync::Arc};

use relay_runtime::{
    BackplaneId, EffectKey, HerdrBackplane, HerdrCapabilities, HerdrClient, HerdrClientError,
    HerdrPaneStatus, LaunchSpecification, MockHerdrTransport, ProcessStatus, ReconcileOutcome,
    RuntimeBackplane, RuntimeBindingState, RuntimeHandle, RuntimeMetadata, RuntimeProvisionRequest,
    SanitizedEnvironment,
};

fn unique_temp_dir(suffix: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("relay_phase07_{}_{}", suffix, uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).expect("failed to create temp dir");
    path
}

#[tokio::test]
async fn test_fake_herdr_protocol_and_capabilities() {
    let mock = Arc::new(MockHerdrTransport::new());
    let client = HerdrClient::new(mock.clone());

    // 1. Valid default capabilities
    let caps = client
        .negotiate_capabilities()
        .await
        .expect("negotiation failed");
    assert_eq!(caps.protocol_version, "1.0.0");
    assert!(caps.supports_metadata);
    assert!(caps.supports_interrupt);

    // 2. Incompatible major version
    let bad_mock = Arc::new(MockHerdrTransport::with_capabilities(HerdrCapabilities {
        protocol_version: "2.0.0".to_string(),
        ..Default::default()
    }));
    let bad_client = HerdrClient::new(bad_mock);
    let err = bad_client.negotiate_capabilities().await.unwrap_err();
    assert!(matches!(err, HerdrClientError::IncompatibleVersion { .. }));

    // 3. Missing critical capability (metadata)
    let missing_meta_mock = Arc::new(MockHerdrTransport::with_capabilities(HerdrCapabilities {
        protocol_version: "1.0.0".to_string(),
        supports_metadata: false,
        ..Default::default()
    }));
    let missing_client = HerdrClient::new(missing_meta_mock);
    let err = missing_client.negotiate_capabilities().await.unwrap_err();
    assert!(matches!(err, HerdrClientError::MissingCapability(cap) if cap == "supports_metadata"));
}

#[tokio::test]
async fn test_herdr_duplicate_effect_reconciliation_idempotency() {
    let mock = Arc::new(MockHerdrTransport::new());
    let backplane = HerdrBackplane::new("herdr-test", mock.clone(), 8);
    let tmp = unique_temp_dir("idempotency");

    let req = RuntimeProvisionRequest {
        effect_key: EffectKey::new("eff-dup-1"),
        role_attempt_id: "attempt-dup-1".to_string(),
        trusted_cwd: tmp.clone(),
        launch: LaunchSpecification::new("dummy-exec", vec!["--arg".to_string()]),
        environment: SanitizedEnvironment::new(),
        output_byte_limit: 1024,
        metadata: Some(RuntimeMetadata::new(
            "attempt-dup-1",
            EffectKey::new("eff-dup-1"),
        )),
    };

    // First provision: creates physical pane
    let handle1 = backplane
        .provision(req.clone())
        .await
        .expect("first provision failed");
    assert_eq!(mock.panes.lock().unwrap().len(), 1);

    // Second provision with the identical effect_key (e.g. scheduler retry or lost RPC)
    let handle2 = backplane
        .provision(req)
        .await
        .expect("second provision failed");
    assert_eq!(handle1, handle2);

    // INVARIANT: Exactly 1 pane spawned in Herdr, duplicate was deduplicated idempotently
    assert_eq!(mock.panes.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn test_relay_restart_herdr_session_survives() {
    let mock = Arc::new(MockHerdrTransport::new());
    let backplane1 = HerdrBackplane::new("herdr-test", mock.clone(), 8);
    let tmp = unique_temp_dir("restart_relay");

    let mut meta = RuntimeMetadata::new("att-survive-1", EffectKey::new("eff-survive-1"));
    meta.task_id = Some("task-survive".to_string());
    meta.checkout_id = Some("chk-main".to_string());

    let req = RuntimeProvisionRequest {
        effect_key: EffectKey::new("eff-survive-1"),
        role_attempt_id: "att-survive-1".to_string(),
        trusted_cwd: tmp.clone(),
        launch: LaunchSpecification::new("agent-cli", vec![]),
        environment: SanitizedEnvironment::new(),
        output_byte_limit: 2048,
        metadata: Some(meta),
    };

    let handle1 = backplane1.provision(req).await.expect("provision failed");
    assert_eq!(mock.panes.lock().unwrap().len(), 1);

    // Relay snapshots active bindings to durable storage before shutdown
    let snapshot = backplane1.bindings().snapshot();
    assert_eq!(snapshot.len(), 1);

    // Simulate Relay restart: fresh backplane instance initialized
    let backplane2 = HerdrBackplane::new("herdr-test", mock.clone(), 8);
    // Reload bindings from durable storage
    backplane2.bindings().restore_from_snapshot(snapshot);

    // Reconcile surviving Herdr session
    let outcome = backplane2
        .reconcile(&handle1)
        .await
        .expect("reconcile failed");
    match outcome {
        ReconcileOutcome::Alive { pid } => {
            assert!(pid > 0);
        }
        other => panic!("expected Alive outcome, got {:?}", other),
    }

    // Live observation continues across Relay restart
    mock.simulate_pane_output(&handle1.handle_id, b"resumed output\n", b"");
    let obs = backplane2.observe(&handle1).await.expect("observe failed");
    assert_eq!(obs.status, ProcessStatus::Running);
    assert_eq!(obs.stdout_chunk, b"resumed output\n");
}

#[tokio::test]
async fn test_herdr_restart_relay_survives() {
    let mock = Arc::new(MockHerdrTransport::new());
    let backplane = HerdrBackplane::new("herdr-test", mock.clone(), 8);
    let tmp = unique_temp_dir("restart_herdr");

    let req = RuntimeProvisionRequest {
        effect_key: EffectKey::new("eff-herdr-crash"),
        role_attempt_id: "att-crash-1".to_string(),
        trusted_cwd: tmp.clone(),
        launch: LaunchSpecification::new("agent-cli", vec![]),
        environment: SanitizedEnvironment::new(),
        output_byte_limit: 1024,
        metadata: Some(RuntimeMetadata::new(
            "att-crash-1",
            EffectKey::new("eff-herdr-crash"),
        )),
    };

    let handle = backplane.provision(req).await.expect("provision failed");

    // Simulate Herdr daemon crashing / restarting: all physical panes are lost
    mock.simulate_server_restart();

    // Relay reconciles active binding
    let outcome = backplane
        .reconcile(&handle)
        .await
        .expect("reconcile failed");
    assert_eq!(outcome, ReconcileOutcome::ResourceNotFound);

    // Check binding updated to Missing
    let binding = backplane
        .bindings()
        .get_by_handle(&handle.handle_id)
        .unwrap();
    assert_eq!(binding.state, RuntimeBindingState::Missing);

    // Observe reports Lost
    let obs = backplane.observe(&handle).await.expect("observe failed");
    assert_eq!(obs.status, ProcessStatus::Lost);

    // INVARIANT: Herdr loss NEVER fabricates semantic role completion!
    assert!(!matches!(obs.status, ProcessStatus::ExitedSuccess { .. }));
}

#[tokio::test]
async fn test_missing_and_mismatched_resource_reconciliation() {
    let mock = Arc::new(MockHerdrTransport::new());
    let backplane = HerdrBackplane::new("herdr-test", mock.clone(), 8);
    let tmp = unique_temp_dir("mismatch");

    let req = RuntimeProvisionRequest {
        effect_key: EffectKey::new("eff-mismatch"),
        role_attempt_id: "att-orig".to_string(),
        trusted_cwd: tmp.clone(),
        launch: LaunchSpecification::new("agent-cli", vec![]),
        environment: SanitizedEnvironment::new(),
        output_byte_limit: 1024,
        metadata: Some(RuntimeMetadata::new(
            "att-orig",
            EffectKey::new("eff-mismatch"),
        )),
    };

    let handle = backplane.provision(req).await.expect("provision failed");

    // 1. Non-existent handle reconciliation
    let bogus_handle = RuntimeHandle {
        handle_id: "non-existent-pane".to_string(),
        backplane_id: BackplaneId::new("herdr-test"),
        pid: None,
        created_at: "now".to_string(),
    };
    let outcome_missing = backplane
        .reconcile(&bogus_handle)
        .await
        .expect("reconcile failed");
    assert_eq!(outcome_missing, ReconcileOutcome::ResourceNotFound);

    // 2. Corrupted / repurposed pane metadata
    let mut hijacked_tags = HashMap::new();
    hijacked_tags.insert(
        "relay.role_attempt_id".to_string(),
        "att-attacker-hijack".to_string(),
    );
    mock.corrupt_metadata(&handle.handle_id, hijacked_tags);

    let outcome_mismatch = backplane
        .reconcile(&handle)
        .await
        .expect("reconcile failed");
    assert!(matches!(
        outcome_mismatch,
        ReconcileOutcome::Mismatched { .. }
    ));

    // Binding is marked Mismatched
    let binding = backplane
        .bindings()
        .get_by_handle(&handle.handle_id)
        .unwrap();
    assert!(matches!(
        binding.state,
        RuntimeBindingState::Mismatched { .. }
    ));
}

#[tokio::test]
async fn test_manual_user_termination_detection() {
    let mock = Arc::new(MockHerdrTransport::new());
    let backplane = HerdrBackplane::new("herdr-test", mock.clone(), 8);
    let tmp = unique_temp_dir("user_term");

    let req = RuntimeProvisionRequest {
        effect_key: EffectKey::new("eff-term-1"),
        role_attempt_id: "att-term-1".to_string(),
        trusted_cwd: tmp.clone(),
        launch: LaunchSpecification::new("agent-cli", vec![]),
        environment: SanitizedEnvironment::new(),
        output_byte_limit: 1024,
        metadata: Some(RuntimeMetadata::new(
            "att-term-1",
            EffectKey::new("eff-term-1"),
        )),
    };

    let handle = backplane.provision(req).await.expect("provision failed");

    // User opens Herdr terminal and manually closes/terminates the pane
    mock.simulate_external_termination(&handle.handle_id);

    // Observation reflects external termination
    let obs = backplane.observe(&handle).await.expect("observe failed");
    assert_eq!(obs.status, ProcessStatus::Interrupted);

    // Reconcile reflects non-successful exit
    let outcome = backplane
        .reconcile(&handle)
        .await
        .expect("reconcile failed");
    assert_eq!(outcome, ReconcileOutcome::FailedExit { exit_code: None });

    // Binding state reflects interrupted
    let binding = backplane
        .bindings()
        .get_by_handle(&handle.handle_id)
        .unwrap();
    assert_eq!(binding.state, RuntimeBindingState::Interrupted);

    // INVARIANT: Manual user termination NEVER fabricates semantic completion!
    assert_ne!(obs.status, ProcessStatus::ExitedSuccess { exit_code: 0 });
}

#[tokio::test]
async fn test_internal_topology_planner_isolation_and_ownership() {
    let mock = Arc::new(MockHerdrTransport::new());
    let backplane = HerdrBackplane::new("herdr-test", mock.clone(), 8);
    let tmp = unique_temp_dir("topology");

    let mut meta = RuntimeMetadata::new("role-att-88", EffectKey::new("eff-topo-88"));
    meta.task_id = Some("task-omega".to_string());
    meta.checkout_id = Some("chk-gamma".to_string());
    meta.project_id = Some("proj-alpha".to_string());

    let req = RuntimeProvisionRequest {
        effect_key: EffectKey::new("eff-topo-88"),
        role_attempt_id: "role-att-88".to_string(),
        trusted_cwd: tmp.clone(),
        launch: LaunchSpecification::new("agent-cli", vec![]),
        environment: SanitizedEnvironment::new(),
        output_byte_limit: 1024,
        metadata: Some(meta),
    };

    let handle = backplane.provision(req).await.expect("provision failed");

    // Relay Core domain ID is independent of physical IDs:
    // Core only sees RuntimeHandle and RuntimeBinding, not Herdr tab/workspace structs
    assert_eq!(handle.handle_id, "pane-relay-role-att-88");

    let pane_info = mock
        .panes
        .lock()
        .unwrap()
        .get(&handle.handle_id)
        .cloned()
        .unwrap();
    assert_eq!(pane_info.tab_id, "tab-relay-task-omega");
    assert_eq!(pane_info.workspace_id, "ws-relay-chk-gamma");
    assert_eq!(
        pane_info.metadata.get("relay.managed_by"),
        Some(&"relay".to_string())
    );
    assert_eq!(
        pane_info.metadata.get("relay.task_id"),
        Some(&"task-omega".to_string())
    );
    assert_eq!(
        pane_info.metadata.get("relay.project_id"),
        Some(&"proj-alpha".to_string())
    );
}

#[tokio::test]
async fn test_cleanup_ownership_preservation() {
    let mock = Arc::new(MockHerdrTransport::new());
    let backplane = HerdrBackplane::new("herdr-test", mock.clone(), 8);
    let tmp = unique_temp_dir("cleanup");

    // 1. Relay provisions an attempt
    let req = RuntimeProvisionRequest {
        effect_key: EffectKey::new("eff-clean-1"),
        role_attempt_id: "att-clean-1".to_string(),
        trusted_cwd: tmp.clone(),
        launch: LaunchSpecification::new("agent-cli", vec![]),
        environment: SanitizedEnvironment::new(),
        output_byte_limit: 1024,
        metadata: Some(RuntimeMetadata::new(
            "att-clean-1",
            EffectKey::new("eff-clean-1"),
        )),
    };
    let relay_handle = backplane.provision(req).await.expect("provision failed");

    // 2. An external user creates an unrelated pane in Herdr
    let user_pane_id = "user-interactive-pane".to_string();
    mock.panes.lock().unwrap().insert(
        user_pane_id.clone(),
        relay_runtime::HerdrPaneInfo {
            pane_id: user_pane_id.clone(),
            tab_id: "user-tab".to_string(),
            workspace_id: "user-ws".to_string(),
            pid: 9999,
            status: HerdrPaneStatus::Running,
            metadata: HashMap::new(),
            user_attached: true,
        },
    );

    // 3. Attempting to dispose the user-owned pane must NOT delete it!
    let user_handle = RuntimeHandle {
        handle_id: user_pane_id.clone(),
        backplane_id: BackplaneId::new("herdr-test"),
        pid: Some(9999),
        created_at: "now".to_string(),
    };
    let receipt_user = backplane
        .dispose(&user_handle)
        .await
        .expect("dispose user handle");
    assert!(
        !receipt_user.reaped,
        "Relay must NOT reap user-created Herdr resources"
    );
    assert!(mock.panes.lock().unwrap().contains_key(&user_pane_id));

    // 4. Disposing the Relay-owned pane cleanly reaps it
    let receipt_relay = backplane
        .dispose(&relay_handle)
        .await
        .expect("dispose relay handle");
    assert!(
        receipt_relay.reaped,
        "Relay must reap its own created resources"
    );
    assert!(
        !mock
            .panes
            .lock()
            .unwrap()
            .contains_key(&relay_handle.handle_id)
    );

    // Binding is marked Disposed
    let binding = backplane
        .bindings()
        .get_by_handle(&relay_handle.handle_id)
        .unwrap();
    assert_eq!(binding.state, RuntimeBindingState::Disposed);
}

#[tokio::test]
async fn test_output_bounding_and_input_transmission() {
    let mock = Arc::new(MockHerdrTransport::new());
    let backplane = HerdrBackplane::new("herdr-test", mock.clone(), 8);
    let tmp = unique_temp_dir("bounds");

    // Output limit clamped to 15 bytes
    let req = RuntimeProvisionRequest {
        effect_key: EffectKey::new("eff-bound-1"),
        role_attempt_id: "att-bound-1".to_string(),
        trusted_cwd: tmp.clone(),
        launch: LaunchSpecification::new("agent-cli", vec![]),
        environment: SanitizedEnvironment::new(),
        output_byte_limit: 15,
        metadata: Some(RuntimeMetadata::new(
            "att-bound-1",
            EffectKey::new("eff-bound-1"),
        )),
    };
    let handle = backplane.provision(req).await.expect("provision failed");

    // Send input bytes
    let send_receipt = backplane
        .send(&handle, b"hello stdin\n")
        .await
        .expect("send failed");
    assert_eq!(send_receipt.bytes_written, 12);

    // Simulate stdout producing 50 bytes
    let large_output = b"01234567890123456789012345678901234567890123456789";
    mock.simulate_pane_output(&handle.handle_id, large_output, b"");

    // Observe captures only up to 15 bytes!
    let obs = backplane.observe(&handle).await.expect("observe failed");
    assert_eq!(obs.stdout_chunk.len(), 15);
    assert_eq!(obs.total_bytes_captured, 15);
    assert_eq!(&obs.stdout_chunk, &large_output[..15]);
}
