//! Conformance tests for the dual-era MCP server.
//!
//! Legacy-era tests exercise the initialize handshake and negotiation
//! semantics; modern-era tests exercise `server/discover`, per-request
//! `_meta`, and `resultType` results. The cross-era equivalence test proves
//! both eras converge on identical Relay submission outcomes.

use super::*;
use relay_core::{
    MAX_LOCAL_TASK_INPUT_BYTES, ProjectRegistry, ProjectRegistryError, RegisterProject,
    TaskIngressError, TaskPersistenceError, TaskState, list_stored_tasks, update_stored_task_state,
};
use relay_protocol::ProtocolError;
use serde_json::json;

const TASK_JSON: &str = r#"{"protocol_version":1,"project":"relay","title":"MCP submitted handoff","source":{"kind":"conversational_ai","provider":"openai"},"task":{"goal":"Land the task durably.","requirements":["No execution."],"constraints":["Never auto-run."],"verification":["Row is Ready in SQLite."]},"ambiguity_policy":"needs_clarification"}"#;

fn temp_dir(name: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("relay-mcp-{name}-{nanos}"));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn relay_with_project(key: &str) -> Relay {
    let mut registry = ProjectRegistry::empty();
    registry
        .register(RegisterProject {
            key: key.to_owned(),
            name: Some("Relay".to_owned()),
            local_path: temp_dir("project"),
            expected_remote: None,
            default_stack: Some("chatgpt-codex".to_owned()),
            github: None,
        })
        .unwrap();
    Relay::with_project_registry(registry)
}

async fn test_server() -> McpServer {
    McpServer::new(
        relay_with_project("relay"),
        SqliteStore::in_memory().await.unwrap(),
    )
}

async fn exchange(server: &mut McpServer, raw: &str) -> Option<Value> {
    server
        .handle_line(raw)
        .await
        .map(|line| serde_json::from_str(&line).expect("response line is valid JSON"))
}

fn error_of(response: &Value) -> (i64, String) {
    let error = response
        .get("error")
        .unwrap_or_else(|| panic!("expected structured error, got: {response}"));
    (
        error["code"].as_i64().unwrap(),
        error["message"].as_str().unwrap().to_owned(),
    )
}

fn initialize_line(version: &str) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": version,
            "capabilities": {},
            "clientInfo": { "name": "conformance-test", "version": "0" }
        }
    })
    .to_string()
}

/// A modern-era request line carrying required per-request metadata.
fn modern_line(id: u64, method: &str, params: &str) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":{id},"method":"{method}","params":{{"_meta":{{"{META_PROTOCOL_VERSION_KEY}":"{MODERN_PROTOCOL_VERSION}"}}{params}}}}}"#
    )
}

fn legacy_submit_call(id: u64) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"submit_handoff","arguments":{TASK_JSON}}}}}"#
    )
}

fn modern_submit_call(id: u64) -> String {
    modern_line(
        id,
        "tools/call",
        &format!(r#","name":"submit_handoff","arguments":{TASK_JSON}"#),
    )
}

/// Completes the full legacy handshake so lifecycle warnings stay quiet.
async fn legacy_handshake(server: &mut McpServer, version: &str) -> Value {
    let handshake = exchange(server, &initialize_line(version)).await.unwrap();
    exchange(
        server,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
    )
    .await;
    handshake
}

async fn submit_valid_legacy_task(server: &mut McpServer) -> Value {
    let response = exchange(server, &legacy_submit_call(99)).await.unwrap();
    assert!(
        response.get("error").is_none(),
        "valid submit failed: {response}"
    );
    response["result"]["structuredContent"].clone()
}

async fn submit_valid_modern_task(server: &mut McpServer) -> Value {
    let response = exchange(server, &modern_submit_call(99)).await.unwrap();
    assert!(
        response.get("error").is_none(),
        "valid submit failed: {response}"
    );
    response["result"]["structuredContent"].clone()
}

/// Every negative-conformance case must leave the server responsive:
/// after the failure, a valid submit still succeeds.
async fn assert_still_responsive(server: &mut McpServer) {
    let payload = submit_valid_legacy_task(server).await;
    assert_eq!(payload["state"], "Ready");
    assert!(relay_core::TaskId::parse(payload["task_id"].as_str().unwrap()).is_ok());
}

// ----------------------------------------------------------------------
// Wire types and error mapping
// ----------------------------------------------------------------------

#[test]
fn rpc_request_round_trips() {
    let request = RpcRequest {
        jsonrpc: protocol::JSON_RPC_VERSION.to_owned(),
        id: Some(RpcId::Number(7)),
        method: "tools/call".to_owned(),
        params: Some(json!({ "name": "submit_handoff" })),
    };

    let wire = serde_json::to_string(&request).unwrap();
    let parsed: RpcRequest = serde_json::from_str(&wire).unwrap();

    assert_eq!(parsed, request);
}

#[test]
fn rpc_request_accepts_string_ids() {
    let request: RpcRequest =
        serde_json::from_str(r#"{"jsonrpc":"2.0","id":"abc-1","method":"initialize"}"#).unwrap();

    assert_eq!(request.id, Some(RpcId::Text("abc-1".to_owned())));
}

#[test]
fn rpc_response_round_trips_success_and_error() {
    let success = success_response(RpcId::Number(3), json!({ "ok": true }));
    let parsed_success: RpcResponse =
        serde_json::from_str(&serde_json::to_string(&success).unwrap()).unwrap();
    // Result and error are mutually exclusive; a success response carries no error.
    assert_eq!(parsed_success.error, None);
    assert_eq!(parsed_success.result.as_ref().unwrap()["ok"], true);

    let failure = failure_response(
        Some(RpcId::Text("x".to_owned())),
        protocol::UNKNOWN_PROJECT,
        "nope",
    );
    assert!(
        serde_json::from_str::<RpcResponse>(&failure.into_wire())
            .unwrap()
            .error
            .is_some()
    );
}

#[test]
fn error_code_mapping_table_is_stable() {
    let cases: Vec<(TaskIngressError, i64)> = vec![
        (
            TaskIngressError::InputTooLarge {
                limit: MAX_LOCAL_TASK_INPUT_BYTES,
                actual: MAX_LOCAL_TASK_INPUT_BYTES + 1,
            },
            protocol::INPUT_TOO_LARGE,
        ),
        (
            TaskIngressError::ExtensionsNotAllowed,
            protocol::UNSUPPORTED_EXTENSIONS,
        ),
        (
            TaskIngressError::Protocol(ProtocolError::UnsupportedProtocolVersion { version: 7 }),
            protocol::UNSUPPORTED_PROTOCOL_VERSION,
        ),
        (
            TaskIngressError::Protocol(ProtocolError::DuplicateJsonKey("project".to_owned())),
            protocol::INVALID_HANDOFF,
        ),
        (
            TaskIngressError::ProjectRegistry(ProjectRegistryError::UnknownProjectKey {
                key: relay_core::ProjectKey::parse("ghost").unwrap(),
            }),
            protocol::UNKNOWN_PROJECT,
        ),
        (
            TaskIngressError::ProjectRegistry(ProjectRegistryError::InvalidProjectKey {
                input: "../etc".to_owned(),
                reason: "traversal".to_owned(),
            }),
            protocol::UNKNOWN_PROJECT,
        ),
    ];

    for (error, expected_code) in cases {
        let (code, message) = service::map_ingress_error(&error);
        assert_eq!(code, expected_code, "wrong code for {error}");
        assert!(!message.is_empty());
    }
}

#[tokio::test]
async fn persistence_store_failures_map_to_store_unavailable() {
    let store = SqliteStore::in_memory().await.unwrap();
    let error = update_stored_task_state(
        &store,
        relay_core::TaskId::new(),
        TaskState::Ready,
        TaskState::Done,
    )
    .await
    .unwrap_err();
    assert!(matches!(error, TaskPersistenceError::Store(_)));

    let (code, message) = service::map_persistence_error(&error);
    assert_eq!(code, protocol::STORE_UNAVAILABLE);
    assert!(!message.is_empty());

    let non_store = TaskPersistenceError::UnknownTaskState {
        state: "bogus".to_owned(),
    };
    assert_eq!(
        service::map_persistence_error(&non_store).0,
        protocol::INTERNAL_ERROR
    );
}

#[test]
fn local_filesystem_ingress_errors_are_scrubbed_before_reaching_the_wire() {
    // FileRead-family errors embed the operator's absolute paths in their
    // Display text; that detail must stay on stderr, never reach MCP clients.
    let leaky = TaskIngressError::FileNotUtf8 {
        path: "C:\\Users\\someone\\secret\\task.md".to_owned(),
        source: String::from_utf8(vec![0xFF, 0xFE]).unwrap_err(),
    };
    let (code, message) = service::map_ingress_error(&leaky);
    assert_eq!(code, protocol::INTERNAL_ERROR);
    assert!(
        !message.contains("C:\\Users"),
        "client-facing message must not carry local paths: {message}"
    );
}

// ----------------------------------------------------------------------
// Strict JSON-RPC 2.0 validation (handoff §7.4)
// ----------------------------------------------------------------------

#[tokio::test]
async fn missing_jsonrpc_member_is_rejected_structured_and_server_survives() {
    let mut server = test_server().await;

    for raw in [
        r#"{"id":20,"method":"tools/list"}"#,
        r#"{"jsonrpc":null,"id":21,"method":"tools/list"}"#,
        r#"{"jsonrpc":"","id":22,"method":"tools/list"}"#,
    ] {
        let response = exchange(&mut server, raw).await.unwrap();
        let (code, message) = error_of(&response);
        assert_eq!(code, protocol::INVALID_REQUEST, "{raw}");
        assert!(
            message.to_lowercase().replace('-', "").contains("jsonrpc"),
            "{message}"
        );
    }

    // A notification without jsonrpc is dropped silently — JSON-RPC forbids
    // replying to notifications, malformed or not (see the dedicated test).
    assert!(
        exchange(&mut server, r#"{"method":"notifications/initialized"}"#)
            .await
            .is_none(),
        "a malformed notification must never be answered"
    );

    assert_still_responsive(&mut server).await;
}

#[tokio::test]
async fn wrong_jsonrpc_version_is_rejected_structured_and_server_survives() {
    let mut server = test_server().await;

    let (code, _) = error_of(
        &exchange(
            &mut server,
            r#"{"jsonrpc":"1.0","id":5,"method":"initialize"}"#,
        )
        .await
        .unwrap(),
    );
    assert_eq!(code, protocol::INVALID_REQUEST);

    assert_still_responsive(&mut server).await;
}

// ----------------------------------------------------------------------
// Legacy era: initialize lifecycle and negotiation
// ----------------------------------------------------------------------

#[tokio::test]
async fn initialize_negotiates_supported_version_and_server_info() {
    let mut server = test_server().await;

    let response = exchange(&mut server, &initialize_line("2025-06-18"))
        .await
        .unwrap();
    let result = &response["result"];

    assert_eq!(result["protocolVersion"], "2025-06-18");
    assert_eq!(result["serverInfo"]["name"], "relay");
    assert_eq!(result["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));
    assert!(result["capabilities"]["tools"].is_object());
}

#[tokio::test]
async fn unsupported_requested_version_negotiates_to_latest_instead_of_erroring() {
    let mut server = test_server().await;

    // Newer-than-server requests receive our latest legacy revision as a
    // successful result (specification `2025-06-18`, Version Negotiation),
    // never a JSON-RPC error.
    for requested in ["2999-01-01", "1999-01-01", "2026-07-28"] {
        let response = exchange(&mut server, &initialize_line(requested))
            .await
            .unwrap();
        assert!(
            response.get("error").is_none(),
            "negotiation must not error for {requested}: {response}"
        );
        assert_eq!(response["result"]["protocolVersion"], "2025-11-25");
    }

    // The session remains fully usable after renegotiation.
    assert_still_responsive(&mut server).await;
}

#[tokio::test]
async fn every_retained_legacy_revision_initializes() {
    let mut server = test_server().await;

    for version in protocol::LEGACY_PROTOCOL_VERSIONS {
        let response = exchange(&mut server, &initialize_line(version))
            .await
            .unwrap();
        assert_eq!(
            response["result"]["protocolVersion"], version,
            "supported revision must echo verbatim"
        );
    }

    assert_still_responsive(&mut server).await;
}

#[tokio::test]
async fn no_mutually_supported_legacy_version_degrades_to_latest_and_client_decides() {
    // MCP legacy negotiation has no error branch for "no mutual version":
    // the server always answers with a revision it supports, and a client
    // that cannot speak any of them is responsible for disconnecting
    // (specification 2025-06-18, Version Negotiation). Assert exactly that
    // contract so a future change cannot invent an error path silently.
    let mut server = test_server().await;

    let response = exchange(&mut server, &initialize_line("1900-01-01"))
        .await
        .unwrap();
    assert!(response.get("error").is_none());
    assert_eq!(
        response["result"]["protocolVersion"],
        protocol::LATEST_LEGACY_PROTOCOL_VERSION
    );
}

#[tokio::test]
async fn malformed_initialize_params_are_rejected_structured() {
    let mut server = test_server().await;

    let (code, _) = error_of(
        &exchange(
            &mut server,
            r#"{"jsonrpc":"2.0","id":30,"method":"initialize"}"#,
        )
        .await
        .unwrap(),
    );
    assert_eq!(code, protocol::INVALID_PARAMS);

    let (code, _) = error_of(
        &exchange(
            &mut server,
            r#"{"jsonrpc":"2.0","id":31,"method":"initialize","params":{"protocolVersion":20}}"#,
        )
        .await
        .unwrap(),
    );
    assert_eq!(code, protocol::INVALID_PARAMS);

    assert_still_responsive(&mut server).await;
}

#[tokio::test]
async fn initialized_notification_advances_lifecycle_state_silently() {
    let mut server = test_server().await;

    assert!(!server.legacy_initialized);
    exchange(&mut server, &initialize_line("2025-06-18")).await;

    assert!(
        exchange(
            &mut server,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#
        )
        .await
        .is_none()
    );
    assert!(server.legacy_initialized);
    assert_eq!(server.negotiated_legacy_version, Some("2025-06-18"));

    // Unknown notifications stay silent too.
    assert!(
        exchange(
            &mut server,
            r#"{"jsonrpc":"2.0","method":"notifications/whatever"}"#
        )
        .await
        .is_none()
    );
}

#[tokio::test]
async fn ping_answers_empty_result_in_both_eras() {
    let mut server = test_server().await;

    // Legacy era.
    let response = exchange(&mut server, r#"{"jsonrpc":"2.0","id":50,"method":"ping"}"#)
        .await
        .unwrap();
    assert_eq!(response["result"], json!({}));

    // Modern era.
    let response = exchange(&mut server, &modern_line(51, "ping", ""))
        .await
        .unwrap();
    assert_eq!(response["result"], json!({}));
}

#[tokio::test]
async fn malformed_notifications_never_receive_a_reply() {
    let mut server = test_server().await;

    // A notification (no id member) with a missing jsonrpc member must be
    // dropped silently: replying to notifications is forbidden, and replying
    // to a garbage stream would generate unbounded outbound traffic.
    for raw in [
        r#"{"method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"1.0","method":"notifications/whatever"}"#,
    ] {
        assert!(
            exchange(&mut server, raw).await.is_none(),
            "notification must not be answered: {raw}"
        );
    }

    // The session remains fully usable afterwards.
    assert_still_responsive(&mut server).await;
}

#[tokio::test]
async fn explicit_null_request_id_is_answered_as_invalid_request() {
    let mut server = test_server().await;

    let response = exchange(
        &mut server,
        r#"{"jsonrpc":"2.0","id":null,"method":"tools/list"}"#,
    )
    .await
    .unwrap();
    // JSON-RPC 2.0 reserves null ids for error responses; a request carrying
    // one is invalid and answered with id null — never silently dropped as a
    // notification.
    assert!(response.get("error").is_some());
    assert_eq!(response["error"]["code"], protocol::INVALID_REQUEST);
    assert!(response["id"].is_null());

    assert_still_responsive(&mut server).await;
}

#[tokio::test]
async fn end_to_end_submit_persists_ready_task_exactly_as_tui_listing_would_show() {
    let mut server = test_server().await;

    let handshake = legacy_handshake(&mut server, "2024-11-05").await;
    assert!(handshake.get("result").is_some());

    let listing = exchange(
        &mut server,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#,
    )
    .await
    .unwrap();
    assert_eq!(listing["result"]["tools"].as_array().unwrap().len(), 1);
    // Legacy results carry no modern-only result discrimination fields.
    assert!(listing["result"].get("resultType").is_none());

    let payload = submit_valid_legacy_task(&mut server).await;
    let task_id = relay_core::TaskId::parse(payload["task_id"].as_str().unwrap()).unwrap();
    assert_eq!(payload["state"], "Ready");

    let tasks = list_stored_tasks(server.store_list_view()).await.unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].id, task_id);
    assert_eq!(tasks[0].state, TaskState::Ready);
    assert_eq!(tasks[0].handoff.title, "MCP submitted handoff");
    assert_eq!(tasks[0].submission_transport.as_str(), "relay-mcp");

    // The legacy submit result carries no modern-only fields either.
    let raw_response = exchange(&mut server, &legacy_submit_call(40))
        .await
        .unwrap();
    assert!(raw_response["result"].get("resultType").is_none());
}

// ----------------------------------------------------------------------
// Modern era: discovery, per-request metadata, stateless results
// ----------------------------------------------------------------------

#[tokio::test]
async fn modern_discover_advertises_both_eras_and_truthful_capabilities() {
    let mut server = test_server().await;

    let response = exchange(&mut server, &modern_line(1, "server/discover", ""))
        .await
        .unwrap();
    let result = &response["result"];

    let versions = result["supportedVersions"].as_array().unwrap();
    assert_eq!(versions[0], MODERN_PROTOCOL_VERSION);
    assert!(versions.iter().any(|v| v == "2025-11-25"));

    // Truthful advertising: tools only — no resources/prompts/tasks padding.
    assert!(result["capabilities"]["tools"].is_object());
    assert!(
        result["capabilities"]
            .as_object()
            .unwrap()
            .keys()
            .all(|key| key == "tools")
    );

    // Server identity lives in _meta per the final specification shape.
    assert_eq!(result["_meta"][META_SERVER_INFO_KEY]["name"], "relay");
}

#[tokio::test]
async fn modern_tools_list_needs_no_handshake_and_discriminates_results() {
    let mut server = test_server().await;

    // No initialize: modern requests are self-describing.
    let listing = exchange(&mut server, &modern_line(2, "tools/list", ""))
        .await
        .unwrap();

    let result = &listing["result"];
    assert_eq!(result["resultType"], "complete");
    let tools = result["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["name"], "submit_handoff");
    assert_eq!(result["_meta"][META_SERVER_INFO_KEY]["name"], "relay");
}

#[tokio::test]
async fn modern_unsupported_version_returns_standard_error_with_supported_list() {
    let mut server = test_server().await;

    let params = json!({
        "_meta": {
            protocol::META_PROTOCOL_VERSION_KEY: "1900-01-01"
        }
    });
    let raw = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/list",
        "params": params,
    })
    .to_string();

    let response = exchange(&mut server, &raw).await.unwrap();
    let (code, message) = error_of(&response);
    assert_eq!(
        code,
        protocol::MCP_UNSUPPORTED_PROTOCOL_VERSION,
        "{message}"
    );
    assert_eq!(response["error"]["data"]["requested"], "1900-01-01");
    let supported = response["error"]["data"]["supported"].as_array().unwrap();
    assert!(supported.contains(&json!(MODERN_PROTOCOL_VERSION)));
    assert!(supported.contains(&json!("2025-11-25")));

    assert_still_responsive(&mut server).await;
}

#[tokio::test]
async fn modern_meta_without_protocol_version_member_is_rejected() {
    let mut server = test_server().await;

    let raw = r#"{"jsonrpc":"2.0","id":4,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/clientInfo":{"name":"x"}}}}"#;
    let (code, message) = error_of(&exchange(&mut server, raw).await.unwrap());
    assert_eq!(code, protocol::INVALID_REQUEST);
    assert!(message.contains(META_PROTOCOL_VERSION_KEY), "{message}");

    assert_still_responsive(&mut server).await;
}

#[tokio::test]
async fn modern_end_to_end_submit_persists_ready_task_statelessly() {
    let mut server = test_server().await;

    // No handshake at all: discover then submit, all stateless.
    exchange(&mut server, &modern_line(1, "server/discover", ""))
        .await
        .unwrap();

    let payload = submit_valid_modern_task(&mut server).await;
    let task_id = relay_core::TaskId::parse(payload["task_id"].as_str().unwrap()).unwrap();
    assert_eq!(payload["state"], "Ready");

    let tasks = list_stored_tasks(server.store_list_view()).await.unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].id, task_id);
    assert_eq!(tasks[0].state, TaskState::Ready);
    assert_eq!(tasks[0].handoff.title, "MCP submitted handoff");
    assert_eq!(tasks[0].submission_transport.as_str(), "relay-mcp");
}

// ----------------------------------------------------------------------
// Cross-era semantic equivalence (the core compatibility guarantee)
// ----------------------------------------------------------------------

#[tokio::test]
async fn identical_payloads_through_both_eras_converge_on_identical_relay_outcomes() {
    let mut server = test_server().await;

    // Era 1: full legacy handshake + submit.
    legacy_handshake(&mut server, "2025-06-18").await;
    let legacy_payload = submit_valid_legacy_task(&mut server).await;

    // Era 2: stateless modern submit of the SAME canonical payload.
    let modern_payload = submit_valid_modern_task(&mut server).await;

    // Same semantic outcome...
    assert_eq!(legacy_payload["state"], "Ready");
    assert_eq!(modern_payload["state"], "Ready");
    assert_ne!(
        legacy_payload["task_id"].as_str().unwrap(),
        modern_payload["task_id"].as_str().unwrap(),
        "each submission gets its own Relay-owned identity"
    );

    // ...and the same durable truth: two READY rows, same provenance class,
    // same canonical title, no execution side effects.
    let tasks = list_stored_tasks(server.store_list_view()).await.unwrap();
    assert_eq!(tasks.len(), 2);
    assert!(tasks.iter().all(|task| task.state == TaskState::Ready));
    assert!(
        tasks
            .iter()
            .all(|task| task.submission_transport.as_str() == "relay-mcp")
    );
    assert!(
        tasks
            .iter()
            .all(|task| task.handoff.title == "MCP submitted handoff")
    );
}

// ----------------------------------------------------------------------
// Shared negative-conformance surface (era-independent)
// ----------------------------------------------------------------------

#[tokio::test]
async fn malformed_json_returns_parse_error_and_server_survives() {
    let mut server = test_server().await;

    let (code, _) = error_of(&exchange(&mut server, "{not json at all").await.unwrap());
    assert_eq!(code, protocol::PARSE_ERROR);

    assert_still_responsive(&mut server).await;
}

#[tokio::test]
async fn unknown_method_returns_method_not_found_and_server_survives_in_both_eras() {
    let mut server = test_server().await;

    let (code, message) = error_of(
        &exchange(
            &mut server,
            r#"{"jsonrpc":"2.0","id":6,"method":"tasks/run"}"#,
        )
        .await
        .unwrap(),
    );
    assert_eq!(code, protocol::METHOD_NOT_FOUND);
    assert!(message.contains("tasks/run"));

    let (code, message) = error_of(
        &exchange(&mut server, &modern_line(7, "tasks/run", ""))
            .await
            .unwrap(),
    );
    assert_eq!(code, protocol::METHOD_NOT_FOUND);
    assert!(message.contains("tasks/run"));

    assert_still_responsive(&mut server).await;
}

#[tokio::test]
async fn unknown_tool_returns_invalid_params_and_server_survives() {
    let mut server = test_server().await;

    let (code, message) = error_of(
        &exchange(
            &mut server,
            r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"run_everything","arguments":{}}}"#,
        )
        .await
        .unwrap(),
    );
    assert_eq!(code, protocol::INVALID_PARAMS);
    assert!(message.contains("unknown tool"), "{message}");

    assert_still_responsive(&mut server).await;
}

#[tokio::test]
async fn unknown_field_in_payload_is_rejected_and_server_survives() {
    let mut server = test_server().await;
    let bad = TASK_JSON.replace(
        r#""title":"MCP submitted handoff","#,
        r#""title":"MCP submitted handoff","sneaky_field":true,"#,
    );
    let raw = format!(
        r#"{{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{{"name":"submit_handoff","arguments":{bad}}}}}"#
    );

    let (code, message) = error_of(&exchange(&mut server, &raw).await.unwrap());
    assert_eq!(code, protocol::INVALID_HANDOFF);
    assert!(
        message.contains("unknown field") || message.to_lowercase().contains("unknown"),
        "{message}"
    );

    assert_still_responsive(&mut server).await;
}

#[tokio::test]
async fn duplicate_json_key_in_payload_is_rejected_and_server_survives() {
    let mut server = test_server().await;
    let duplicated = TASK_JSON.replace(
        r#""project":"relay","title"#,
        r#""project":"relay","project":"other","title"#,
    );
    let raw = format!(
        r#"{{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{{"name":"submit_handoff","arguments":{duplicated}}}}}"#
    );

    let (code, message) = error_of(&exchange(&mut server, &raw).await.unwrap());
    assert_eq!(code, protocol::INVALID_HANDOFF);
    assert!(message.contains("duplicate"), "{message}");

    assert_still_responsive(&mut server).await;
}

#[tokio::test]
async fn unknown_project_fails_closed_and_server_survives() {
    let mut server = test_server().await;
    let ghost = TASK_JSON.replace(r#""project":"relay""#, r#""project":"ghost-project""#);
    let raw = format!(
        r#"{{"jsonrpc":"2.0","id":11,"method":"tools/call","params":{{"name":"submit_handoff","arguments":{ghost}}}}}"#
    );

    let (code, message) = error_of(&exchange(&mut server, &raw).await.unwrap());
    assert_eq!(code, protocol::UNKNOWN_PROJECT);
    assert!(message.contains("unknown project key"), "{message}");

    // Nothing was persisted by the failed submission.
    assert!(
        list_stored_tasks(server.store_list_view())
            .await
            .unwrap()
            .is_empty()
    );

    assert_still_responsive(&mut server).await;
}

#[tokio::test]
async fn oversized_payload_is_rejected_without_processing_and_server_survives() {
    let mut server = test_server().await;
    let padding = "x".repeat((MAX_LOCAL_TASK_INPUT_BYTES + 1) as usize);
    let bloated = TASK_JSON.replace(
        r#""goal":"Land the task durably.""#,
        &format!(r#""goal":"{}""#, padding),
    );
    let raw = format!(
        r#"{{"jsonrpc":"2.0","id":12,"method":"tools/call","params":{{"name":"submit_handoff","arguments":{bloated}}}}}"#
    );

    let (code, message) = error_of(&exchange(&mut server, &raw).await.unwrap());
    assert_eq!(code, protocol::INPUT_TOO_LARGE);
    assert!(message.contains("exceeding the maximum"), "{message}");

    assert!(
        list_stored_tasks(server.store_list_view())
            .await
            .unwrap()
            .is_empty()
    );

    assert_still_responsive(&mut server).await;
}

#[tokio::test]
async fn non_empty_extensions_are_rejected_at_the_boundary_and_server_survives() {
    let mut server = test_server().await;
    let extended = TASK_JSON.replace(
        r#""ambiguity_policy":"needs_clarification"}"#,
        r#""extensions":{"vendor":{"custom":1}},"ambiguity_policy":"needs_clarification"}"#,
    );
    let raw = format!(
        r#"{{"jsonrpc":"2.0","id":13,"method":"tools/call","params":{{"name":"submit_handoff","arguments":{extended}}}}}"#
    );

    let (code, message) = error_of(&exchange(&mut server, &raw).await.unwrap());
    assert_eq!(code, protocol::UNSUPPORTED_EXTENSIONS);
    assert!(message.contains("extensions"), "{message}");

    assert_still_responsive(&mut server).await;
}
