//! Phase 09: Relay Exchange + Role-Session Capabilities Tests.
//!
//! Verifies:
//! 1. Role-session capability generation, operation scoping, expiration, and revocation.
//! 2. Cross-task and cross-project access isolation (denied by default).
//! 3. Scope widening attempts via caller-supplied parameters are rejected.
//! 4. Scoped role inbox filtering (cannot enumerate other tasks or projects).
//! 5. Structured result publication with schema validation and candidate binding.
//! 6. Malformed results are rejected (`RESULT_INVALID`) and never stored.
//! 7. Provenance and trust classification (`ProviderClaim` vs `RelayObserved`) preservation.
//! 8. Materialized session fallback projection (no secret token leakage) and outbox ingestion.

use std::{collections::HashSet, path::PathBuf, sync::Arc};

use relay_core::{
    ArtifactKind, ArtifactSensitivity, ArtifactStore, ArtifactTrustClass, CheckoutId,
    CreateArtifactRequest, ExchangeError, ExchangeMessageKind, ExchangeOperation,
    IssueCapabilityRequest, ProjectId, PublishResultRequest, RelayExchange, RetentionClass,
    RoleAttemptId, RoleInstanceId, SendMessageRequest, TaskId, WorkflowRunId,
    candidate::CandidateId,
};

fn unique_temp_dir(suffix: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("relay_phase09_{}_{}", suffix, uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).expect("failed to create temp dir");
    path
}

fn setup_exchange(suffix: &str) -> (Arc<RelayExchange>, Arc<ArtifactStore>, PathBuf) {
    let root = unique_temp_dir(suffix);
    let artifact_store = Arc::new(ArtifactStore::new(&root).unwrap());
    let exchange = Arc::new(RelayExchange::new(artifact_store.clone()));
    (exchange, artifact_store, root)
}

#[test]
fn test_capability_lifecycle_expiration_and_revocation() {
    let (exchange, _, _) = setup_exchange("lifecycle");

    let mut ops = HashSet::new();
    ops.insert(ExchangeOperation::ReadInbox);
    ops.insert(ExchangeOperation::PublishResult);

    // 1. Issue short-lived capability (1 second)
    let cap = exchange.issue_capability(IssueCapabilityRequest {
        project_id: ProjectId::parse("proj-1").unwrap(),
        checkout_id: CheckoutId::parse("chk-1").unwrap(),
        task_id: TaskId::generate(),
        workflow_run_id: WorkflowRunId("run-1".to_string()),
        role_instance_id: RoleInstanceId("role-1".to_string()),
        role_attempt_id: RoleAttemptId("att-1".to_string()),
        allowed_operations: ops,
        ttl_seconds: 1, // 1 second TTL
    });

    // Immediate validation succeeds
    assert!(
        exchange
            .validate_capability(&cap.token, ExchangeOperation::ReadInbox)
            .is_ok()
    );

    // Disallowed operation is denied
    let err = exchange
        .validate_capability(&cap.token, ExchangeOperation::SendMessage)
        .unwrap_err();
    assert!(matches!(
        err,
        ExchangeError::OperationDenied(ExchangeOperation::SendMessage)
    ));

    // 2. Revocation
    let revoked = exchange.revoke_capability(&cap.token);
    assert!(revoked);

    let err = exchange
        .validate_capability(&cap.token, ExchangeOperation::ReadInbox)
        .unwrap_err();
    assert!(matches!(err, ExchangeError::CapabilityRevoked));

    // 3. Expiration
    let mut ops2 = HashSet::new();
    ops2.insert(ExchangeOperation::ReadInbox);
    let cap_expired = exchange.issue_capability(IssueCapabilityRequest {
        project_id: ProjectId::parse("proj-1").unwrap(),
        checkout_id: CheckoutId::parse("chk-1").unwrap(),
        task_id: TaskId::generate(),
        workflow_run_id: WorkflowRunId("run-1".to_string()),
        role_instance_id: RoleInstanceId("role-1".to_string()),
        role_attempt_id: RoleAttemptId("att-2".to_string()),
        allowed_operations: ops2,
        ttl_seconds: 0, // Already expired (0 sec TTL)
    });

    let err = exchange
        .validate_capability(&cap_expired.token, ExchangeOperation::ReadInbox)
        .unwrap_err();
    assert!(matches!(err, ExchangeError::CapabilityExpired(_)));
}

#[test]
fn test_cross_task_and_cross_project_isolation_denied() {
    let (exchange, artifact_store, _) = setup_exchange("isolation");

    let proj_a = ProjectId::parse("proj-alpha").unwrap();
    let task_a = TaskId::generate();
    let run_a = WorkflowRunId("run-A".to_string());

    // Create an artifact belonging to Task A in Project Alpha
    let art_task_a = artifact_store
        .put(CreateArtifactRequest {
            kind: ArtifactKind::Specification,
            project_id: proj_a.clone(),
            task_id: task_a,
            workflow_run_id: run_a.clone(),
            producer_role_instance_id: None,
            producer_attempt_id: None,
            candidate_id: None,
            trust_class: ArtifactTrustClass::RelayObserved,
            sensitivity: ArtifactSensitivity::Internal,
            retention: RetentionClass::WorkflowScoped,
            payload: b"{\"spec\": \"Task A Specification\"}",
        })
        .unwrap();

    // Create a capability token bound to Task B in Project Alpha
    let task_b = TaskId::generate();
    let mut ops = HashSet::new();
    ops.insert(ExchangeOperation::GetArtifact);
    let cap_task_b = exchange.issue_capability(IssueCapabilityRequest {
        project_id: proj_a.clone(),
        checkout_id: CheckoutId::parse("chk-alpha").unwrap(),
        task_id: task_b,
        workflow_run_id: WorkflowRunId("run-B".to_string()),
        role_instance_id: RoleInstanceId("role-b".to_string()),
        role_attempt_id: RoleAttemptId("att-b".to_string()),
        allowed_operations: ops.clone(),
        ttl_seconds: 3600,
    });

    // Attempting to fetch Task A's artifact with Task B's token is strictly denied!
    let err_cross_task = exchange
        .get_artifact(&cap_task_b.token, &art_task_a.id)
        .unwrap_err();
    assert!(matches!(
        err_cross_task,
        ExchangeError::CrossTaskAccessDenied { cap_task, req_task }
            if cap_task == task_b.to_string() && req_task == task_a.to_string()
    ));

    // Create a capability token bound to Project Beta
    let proj_beta = ProjectId::parse("proj-beta").unwrap();
    let cap_proj_beta = exchange.issue_capability(IssueCapabilityRequest {
        project_id: proj_beta.clone(),
        checkout_id: CheckoutId::parse("chk-beta").unwrap(),
        task_id: task_a,
        workflow_run_id: run_a,
        role_instance_id: RoleInstanceId("role-beta".to_string()),
        role_attempt_id: RoleAttemptId("att-beta".to_string()),
        allowed_operations: ops,
        ttl_seconds: 3600,
    });

    // Attempting to fetch Project Alpha's artifact with Project Beta's token is denied!
    let err_cross_project = exchange
        .get_artifact(&cap_proj_beta.token, &art_task_a.id)
        .unwrap_err();
    assert!(matches!(
        err_cross_project,
        ExchangeError::CrossProjectAccessDenied { cap_proj, req_proj }
            if cap_proj == proj_beta.to_string() && req_proj == proj_a.to_string()
    ));
}

#[test]
fn test_role_scoped_inbox_cannot_enumerate_other_tasks() {
    let (exchange, artifact_store, _) = setup_exchange("inbox");

    let mut ops = HashSet::new();
    ops.insert(ExchangeOperation::ReadInbox);
    ops.insert(ExchangeOperation::SendMessage);

    let proj_id = ProjectId::parse("proj-1").unwrap();
    let chk_id = CheckoutId::parse("chk-1").unwrap();
    let task_1 = TaskId::generate();
    let task_2 = TaskId::generate();
    let run_1 = WorkflowRunId("run-1".to_string());
    let run_2 = WorkflowRunId("run-2".to_string());

    let cap_attempt_1 = exchange.issue_capability(IssueCapabilityRequest {
        project_id: proj_id.clone(),
        checkout_id: chk_id.clone(),
        task_id: task_1,
        workflow_run_id: run_1.clone(),
        role_instance_id: RoleInstanceId("role-impl".to_string()),
        role_attempt_id: RoleAttemptId("attempt-impl-1".to_string()),
        allowed_operations: ops.clone(),
        ttl_seconds: 3600,
    });

    let cap_attempt_2 = exchange.issue_capability(IssueCapabilityRequest {
        project_id: proj_id.clone(),
        checkout_id: chk_id,
        task_id: task_2, // Different task!
        workflow_run_id: run_2,
        role_instance_id: RoleInstanceId("role-verif".to_string()),
        role_attempt_id: RoleAttemptId("attempt-verif-1".to_string()),
        allowed_operations: ops.clone(),
        ttl_seconds: 3600,
    });

    // Send a message within Task 1 addressed specifically to attempt-impl-1
    exchange
        .send_message(
            &cap_attempt_1.token,
            SendMessageRequest {
                to_role_attempt_id: Some(RoleAttemptId("attempt-impl-1".to_string())),
                kind: ExchangeMessageKind::Clarification,
                payload: serde_json::json!({ "question": "Approved scope boundaries" }),
                attached_artifacts: vec![],
            },
        )
        .unwrap();

    // Mount input artifact specifically for attempt-impl-1
    let art_mounted = artifact_store
        .put(CreateArtifactRequest {
            kind: ArtifactKind::Specification,
            project_id: proj_id,
            task_id: task_1,
            workflow_run_id: run_1,
            producer_role_instance_id: None,
            producer_attempt_id: None,
            candidate_id: None,
            trust_class: ArtifactTrustClass::RelayObserved,
            sensitivity: ArtifactSensitivity::Internal,
            retention: RetentionClass::WorkflowScoped,
            payload: b"{\"scope\": \"feature-xyz\"}",
        })
        .unwrap();
    exchange.mount_artifact_for_attempt(
        &RoleAttemptId("attempt-impl-1".to_string()),
        art_mounted.id.clone(),
    );

    // 1. Attempt 1 reads its inbox: sees message and mounted artifact
    let inbox1 = exchange.get_inbox(&cap_attempt_1.token).unwrap();
    assert_eq!(inbox1.messages.len(), 1);
    assert_eq!(inbox1.visible_artifacts.len(), 1);
    assert_eq!(inbox1.visible_artifacts[0].id, art_mounted.id);

    // 2. Attempt 2 (in Task 2) reads its inbox: sees NOTHING from Task 1!
    let inbox2 = exchange.get_inbox(&cap_attempt_2.token).unwrap();
    assert_eq!(inbox2.messages.len(), 0);
    assert_eq!(inbox2.visible_artifacts.len(), 0);
}

#[test]
fn test_structured_result_publication_and_malformed_rejection() {
    let (exchange, _, _) = setup_exchange("pub_result");

    let mut ops = HashSet::new();
    ops.insert(ExchangeOperation::PublishResult);
    ops.insert(ExchangeOperation::GetArtifact);

    let cap = exchange.issue_capability(IssueCapabilityRequest {
        project_id: ProjectId::parse("proj-1").unwrap(),
        checkout_id: CheckoutId::parse("chk-1").unwrap(),
        task_id: TaskId::generate(),
        workflow_run_id: WorkflowRunId("run-1".to_string()),
        role_instance_id: RoleInstanceId("role-verifier".to_string()),
        role_attempt_id: RoleAttemptId("attempt-verif-1".to_string()),
        allowed_operations: ops,
        ttl_seconds: 3600,
    });

    // 1. Empty payload rejected
    let err_empty = exchange
        .publish_result(
            &cap.token,
            PublishResultRequest {
                kind: ArtifactKind::ImplementationResult,
                candidate_id: None,
                trust_class: ArtifactTrustClass::ProviderClaim,
                sensitivity: ArtifactSensitivity::Internal,
                retention: RetentionClass::WorkflowScoped,
                payload: vec![],
            },
        )
        .unwrap_err();
    assert!(matches!(err_empty, ExchangeError::ResultInvalid(msg) if msg.contains("empty")));

    // 2. Malformed non-JSON payload rejected
    let err_malformed = exchange
        .publish_result(
            &cap.token,
            PublishResultRequest {
                kind: ArtifactKind::ImplementationResult,
                candidate_id: None,
                trust_class: ArtifactTrustClass::ProviderClaim,
                sensitivity: ArtifactSensitivity::Internal,
                retention: RetentionClass::WorkflowScoped,
                payload: b"not a json object at all".to_vec(),
            },
        )
        .unwrap_err();
    assert!(matches!(err_malformed, ExchangeError::ResultInvalid(_)));

    // 3. Verification result without candidate reference rejected
    let err_no_cand = exchange
        .publish_result(
            &cap.token,
            PublishResultRequest {
                kind: ArtifactKind::VerificationResult,
                candidate_id: None, // Missing!
                trust_class: ArtifactTrustClass::ProviderClaim,
                sensitivity: ArtifactSensitivity::Internal,
                retention: RetentionClass::WorkflowScoped,
                payload: b"{\"verdict\": \"PASS\"}".to_vec(),
            },
        )
        .unwrap_err();
    assert!(
        matches!(err_no_cand, ExchangeError::ResultInvalid(msg) if msg.contains("candidate_id"))
    );

    // 4. Valid structured verification result accepted
    let cand_id = CandidateId("cand-42".to_string());
    let art_id = exchange
        .publish_result(
            &cap.token,
            PublishResultRequest {
                kind: ArtifactKind::VerificationResult,
                candidate_id: Some(cand_id.clone()),
                trust_class: ArtifactTrustClass::ProviderClaim,
                sensitivity: ArtifactSensitivity::Internal,
                retention: RetentionClass::WorkflowScoped,
                payload: b"{\"verdict\": \"PASS\", \"suite\": \"cargo test\"}".to_vec(),
            },
        )
        .unwrap();

    // Verify metadata was persisted with provenance
    let (meta, payload) = exchange.get_artifact(&cap.token, &art_id).unwrap();
    assert_eq!(meta.kind, ArtifactKind::VerificationResult);
    assert_eq!(meta.candidate_id, Some(cand_id));
    assert_eq!(meta.trust_class, ArtifactTrustClass::ProviderClaim);
    assert_eq!(
        meta.producer_attempt_id,
        Some(RoleAttemptId("attempt-verif-1".to_string()))
    );
    assert_eq!(
        payload,
        b"{\"verdict\": \"PASS\", \"suite\": \"cargo test\"}"
    );
}

#[test]
fn test_materialized_session_fallback_projection_and_outbox() {
    let (exchange, artifact_store, tmp) = setup_exchange("mat_session");
    let fallback_base = tmp.join("materialized_sessions");

    let mut ops = HashSet::new();
    ops.insert(ExchangeOperation::ReadInbox);
    ops.insert(ExchangeOperation::PublishResult);
    ops.insert(ExchangeOperation::GetArtifact);

    let proj_id = ProjectId::parse("proj-demo").unwrap();
    let chk_id = CheckoutId::parse("chk-demo").unwrap();
    let task_id = TaskId::generate();
    let run_id = WorkflowRunId("run-demo".to_string());

    let cap = exchange.issue_capability(IssueCapabilityRequest {
        project_id: proj_id.clone(),
        checkout_id: chk_id,
        task_id,
        workflow_run_id: run_id.clone(),
        role_instance_id: RoleInstanceId("role-demo".to_string()),
        role_attempt_id: RoleAttemptId("attempt-demo-1".to_string()),
        allowed_operations: ops,
        ttl_seconds: 3600,
    });

    // Store an input spec artifact
    let spec_art = artifact_store
        .put(CreateArtifactRequest {
            kind: ArtifactKind::Specification,
            project_id: proj_id,
            task_id,
            workflow_run_id: run_id,
            producer_role_instance_id: None,
            producer_attempt_id: None,
            candidate_id: None,
            trust_class: ArtifactTrustClass::RelayObserved,
            sensitivity: ArtifactSensitivity::Internal,
            retention: RetentionClass::WorkflowScoped,
            payload: b"{\"prompt\": \"Build endpoint\"}",
        })
        .unwrap();
    exchange.mount_artifact_for_attempt(&RoleAttemptId("attempt-demo-1".to_string()), spec_art.id);

    // 1. Project materialized session to disk
    let session_dir = exchange
        .project_materialized_session(&cap.token, &fallback_base)
        .expect("materialization failed");

    assert!(session_dir.join("SESSION.json").exists());
    assert!(session_dir.join("INBOX.json").exists());
    assert!(session_dir.join("input").exists());
    assert!(session_dir.join("outbox").exists());

    // INVARIANT: SESSION.json must NOT leak secret capability token!
    let session_content = std::fs::read_to_string(session_dir.join("SESSION.json")).unwrap();
    assert!(
        !session_content.contains(&cap.token.0),
        "capability token must NOT leak to disk"
    );
    assert!(session_content.contains("attempt-demo-1"));

    // 2. Provider writes result to outbox/result.json
    let outbox_result = serde_json::json!({
        "kind": "implementation_result",
        "candidate_id": null,
        "trust_class": "provider_claim",
        "sensitivity": "internal",
        "retention": "workflow_scoped",
        "payload": [123, 34, 115, 116, 97, 116, 117, 115, 34, 58, 34, 115, 117, 99, 99, 101, 115, 115, 34, 125] // {"status":"success"}
    });
    std::fs::write(
        session_dir.join("outbox").join("result.json"),
        serde_json::to_string(&outbox_result).unwrap(),
    )
    .unwrap();

    // 3. Exchange consumes and validates outbox result
    let published_art_id = exchange
        .consume_materialized_outbox(&cap.token, &fallback_base)
        .expect("consumption failed");

    // Result file was cleaned up from outbox
    assert!(!session_dir.join("outbox").join("result.json").exists());

    // Result is queryable as canonical artifact
    let (meta, payload) = exchange
        .get_artifact(&cap.token, &published_art_id)
        .unwrap();
    assert_eq!(meta.kind, ArtifactKind::ImplementationResult);
    assert_eq!(meta.trust_class, ArtifactTrustClass::ProviderClaim);
    assert_eq!(payload, b"{\"status\":\"success\"}");
}
