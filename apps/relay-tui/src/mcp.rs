//! Headless MCP stdio server backing `relay serve-mcp`.
//!
//! This module is deterministic submission plumbing (ADR-0016): it speaks a
//! minimal JSON-RPC 2.0 subset over newline-delimited stdio frames, validates
//! machine-initiated handoffs through the same Relay Core ingress path as
//! local paste import, persists them through the same confirm path, and
//! answers with a Relay-owned [`TaskId`].
//!
//! Structural guard: this module deliberately depends on NO execution or
//! scheduler types. It cannot construct an `ExecutionScheduler`, enqueue an
//! execution, or start a runtime; acceptance lands tasks in `Ready` and
//! nothing else. Running always requires explicit Run approval in the TUI
//! (invariants 3, 5, 13, 18).
//!
//! Framing: one JSON-RPC message per line (the MCP stdio convention). Stdout
//! carries protocol messages only; all diagnostics go to stderr via tracing.

use std::fmt;

use relay_core::{
    MAX_LOCAL_TASK_INPUT_BYTES, Relay, SqliteStore, StoredTask, TaskId, TaskIngressError,
    TaskPersistenceError,
};
use relay_protocol::ProtocolError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const JSON_RPC_VERSION: &str = "2.0";

/// JSON-RPC 2.0 reserved error codes.
pub(crate) const PARSE_ERROR: i64 = -32700;
pub(crate) const INVALID_REQUEST: i64 = -32600;
pub(crate) const METHOD_NOT_FOUND: i64 = -32601;
pub(crate) const INVALID_PARAMS: i64 = -32602;
pub(crate) const INTERNAL_ERROR: i64 = -32603;

/// Relay-stable application error codes (JSON-RPC server range -32000..-32099).
pub(crate) const STORE_UNAVAILABLE: i64 = -32000;
pub(crate) const UNKNOWN_PROJECT: i64 = -32001;
pub(crate) const UNSUPPORTED_EXTENSIONS: i64 = -32002;
pub(crate) const INPUT_TOO_LARGE: i64 = -32003;
pub(crate) const UNSUPPORTED_PROTOCOL_VERSION: i64 = -32004;
pub(crate) const INVALID_HANDOFF: i64 = -32005;

/// MCP protocol dates this server understands.
const SUPPORTED_MCP_PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

const SERVER_NAME: &str = "relay";
const SUBMIT_HANDOFF_TOOL_NAME: &str = "submit_handoff";
/// User-facing state label mandated by the M9 pack output contract.
const READY_STATE_LABEL: &str = "Ready";

const SUBMIT_HANDOFF_DESCRIPTION: &str = "Submit ONE canonical HandoffTask v1 JSON object \
(the same schema as local paste import; no wrapper keys, no templates). The task is validated \
against the trusted local project registry and durably persisted, landing it in Ready state. \
Submission NEVER executes anything: running the task requires explicit Run approval in the \
Relay TUI.";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum RpcId {
    Number(i64),
    Text(String),
}

impl fmt::Display for RpcId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Number(value) => write!(formatter, "{value}"),
            Self::Text(value) => write!(formatter, "{value}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct RpcRequest {
    #[serde(default)]
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Option<RpcId>,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct RpcErrorBody {
    pub code: i64,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct RpcResponse {
    pub jsonrpc: String,
    pub id: Option<RpcId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcErrorBody>,
}

fn success_response(id: RpcId, result: Value) -> RpcResponse {
    RpcResponse {
        jsonrpc: JSON_RPC_VERSION.to_owned(),
        id: Some(id),
        result: Some(result),
        error: None,
    }
}

fn failure_response(id: Option<RpcId>, code: i64, message: impl Into<String>) -> RpcResponse {
    RpcResponse {
        jsonrpc: JSON_RPC_VERSION.to_owned(),
        id,
        result: None,
        error: Some(RpcErrorBody {
            code,
            message: message.into(),
        }),
    }
}

/// Minimal MCP stdio server. Owns exactly what it must share: the trusted
/// project registry (via `Relay`) and the SQLite store.
pub(crate) struct McpServer {
    relay: Relay,
    store: SqliteStore,
}

/// Typed view of a `tools/call` envelope that borrows the RAW argument text so
/// duplicate JSON keys inside the handoff payload survive envelope parsing and
/// reach the protocol validator untouched.
#[derive(Deserialize)]
struct ToolCallEnvelope<'a> {
    #[serde(borrow)]
    params: Option<ToolCallParams<'a>>,
}

#[derive(Deserialize)]
struct ToolCallParams<'a> {
    name: String,
    #[serde(default, borrow)]
    arguments: Option<&'a serde_json::value::RawValue>,
}

impl McpServer {
    pub(crate) fn new(relay: Relay, store: SqliteStore) -> Self {
        Self { relay, store }
    }

    /// Handles one framed input line. Returns the serialized response line, or
    /// `None` for notifications and blank lines. The server stays alive across
    /// every failure branch; only stdin EOF ends the session.
    pub(crate) async fn handle_line(&mut self, line: &str) -> Option<String> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return None;
        }

        let envelope: Value = match serde_json::from_str(trimmed) {
            Ok(envelope) => envelope,
            Err(error) => {
                return Some(
                    failure_response(
                        None,
                        PARSE_ERROR,
                        format!("request is not valid JSON: {error}"),
                    )
                    .into_wire(),
                );
            }
        };

        let request: RpcRequest = match serde_json::from_value(envelope) {
            Ok(request) => request,
            Err(error) => {
                return Some(
                    failure_response(
                        None,
                        INVALID_REQUEST,
                        format!("invalid JSON-RPC request: {error}"),
                    )
                    .into_wire(),
                );
            }
        };

        if !request.jsonrpc.is_empty() && request.jsonrpc != JSON_RPC_VERSION {
            return Some(
                failure_response(
                    request.id,
                    INVALID_REQUEST,
                    format!(
                        "unsupported jsonrpc version '{}'; expected '{JSON_RPC_VERSION}'",
                        request.jsonrpc
                    ),
                )
                .into_wire(),
            );
        }

        // Notifications (no id) are accepted silently, whatever their method.
        let id = request.id?;

        match request.method.as_str() {
            "initialize" => Some(self.handle_initialize(&request.params, id).into_wire()),
            "notifications/initialized" => None,
            "tools/list" => Some(success_response(id, tools_list_result()).into_wire()),
            "tools/call" => Some(self.handle_tools_call(trimmed, id).await.into_wire()),
            method => Some(
                failure_response(
                    Some(id),
                    METHOD_NOT_FOUND,
                    format!("method not found: {method}"),
                )
                .into_wire(),
            ),
        }
    }

    fn handle_initialize(&self, params: &Option<Value>, id: RpcId) -> RpcResponse {
        let Some(Value::Object(params)) = params else {
            return failure_response(
                Some(id),
                INVALID_PARAMS,
                "initialize requires a params object with protocolVersion".to_owned(),
            );
        };

        let requested = params.get("protocolVersion").and_then(Value::as_str);
        match requested {
            Some(version) if SUPPORTED_MCP_PROTOCOL_VERSIONS.contains(&version) => {
                success_response(
                    id,
                    serde_json::json!({
                        "protocolVersion": version,
                        "capabilities": { "tools": {} },
                        "serverInfo": {
                            "name": SERVER_NAME,
                            "version": env!("CARGO_PKG_VERSION"),
                        },
                    }),
                )
            }
            other => failure_response(
                Some(id),
                UNSUPPORTED_PROTOCOL_VERSION,
                format!(
                    "unsupported MCP protocol version {}; supported versions: {}",
                    other.map_or_else(|| "missing".to_owned(), |version| format!("'{version}'")),
                    SUPPORTED_MCP_PROTOCOL_VERSIONS.join(", ")
                ),
            ),
        }
    }

    async fn handle_tools_call(&mut self, raw_line: &str, id: RpcId) -> RpcResponse {
        let envelope: ToolCallEnvelope<'_> = match serde_json::from_str(raw_line) {
            Ok(envelope) => envelope,
            Err(error) => {
                return failure_response(
                    Some(id),
                    INVALID_PARAMS,
                    format!("invalid tools/call params: {error}"),
                );
            }
        };

        let Some(call) = envelope.params else {
            return failure_response(
                Some(id),
                INVALID_PARAMS,
                "tools/call requires a params object with a tool name".to_owned(),
            );
        };

        if call.name != SUBMIT_HANDOFF_TOOL_NAME {
            return failure_response(
                Some(id),
                INVALID_PARAMS,
                format!(
                    "unknown tool '{}'; available tools: {SUBMIT_HANDOFF_TOOL_NAME}",
                    call.name
                ),
            );
        }

        let Some(arguments) = call.arguments else {
            return failure_response(
                Some(id),
                INVALID_PARAMS,
                format!(
                    "tool '{SUBMIT_HANDOFF_TOOL_NAME}' requires 'arguments' containing one \
                     canonical HandoffTask v1 JSON object"
                ),
            );
        };

        let raw_payload = arguments.get();
        if let Err((code, message)) = ensure_payload_within_size_cap(raw_payload) {
            return failure_response(Some(id), code, message);
        }

        let candidate = match self.relay.preview_mcp_submission(raw_payload) {
            Ok(candidate) => candidate,
            Err(error) => {
                let (code, message) = map_ingress_error(&error);
                return failure_response(Some(id), code, message);
            }
        };

        // Never claim success unless the store confirmed durability.
        let stored: StoredTask = match self
            .relay
            .confirm_task_import_with_store(&self.store, candidate)
            .await
        {
            Ok(stored) => stored,
            Err(error) => {
                let (code, message) = map_persistence_error(&error);
                return failure_response(Some(id), code, message);
            }
        };

        success_response(id, submit_handoff_result(&stored.id))
    }
}

impl RpcResponse {
    fn into_wire(self) -> String {
        serde_json::to_string(&self).unwrap_or_else(|_| fallback_error_line())
    }
}

fn fallback_error_line() -> String {
    r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32603,"message":"internal error: failed to serialize response"}}"#.to_owned()
}

fn ensure_payload_within_size_cap(raw_payload: &str) -> Result<(), (i64, String)> {
    let actual = raw_payload.len() as u64;
    if actual > MAX_LOCAL_TASK_INPUT_BYTES {
        Err((
            INPUT_TOO_LARGE,
            format!(
                "submit_handoff arguments are {actual} bytes, exceeding the maximum of \
                 {MAX_LOCAL_TASK_INPUT_BYTES} bytes"
            ),
        ))
    } else {
        Ok(())
    }
}

fn tools_list_result() -> Value {
    serde_json::json!({
        "tools": [
            {
                "name": SUBMIT_HANDOFF_TOOL_NAME,
                "description": SUBMIT_HANDOFF_DESCRIPTION,
                "inputSchema": {
                    "type": "object",
                    "title": "HandoffTask v1",
                    "additionalProperties": false,
                    "required": [
                        "protocol_version",
                        "project",
                        "title",
                        "task",
                        "ambiguity_policy"
                    ],
                    "properties": {
                        "protocol_version": { "type": "integer", "const": 1 },
                        "project": { "type": "string", "description": "Trusted local project registry key" },
                        "title": { "type": "string" },
                        "stack": { "type": "string" },
                        "source": {
                            "type": "object",
                            "additionalProperties": false,
                            "required": ["kind", "provider"],
                            "properties": {
                                "kind": { "type": "string" },
                                "provider": { "type": "string" }
                            }
                        },
                        "task": {
                            "type": "object",
                            "additionalProperties": false,
                            "required": ["goal"],
                            "properties": {
                                "goal": { "type": "string" },
                                "requirements": { "type": "array", "items": { "type": "string" } },
                                "constraints": { "type": "array", "items": { "type": "string" } },
                                "verification": { "type": "array", "items": { "type": "string" } }
                            }
                        },
                        "ambiguity_policy": { "type": "string", "const": "needs_clarification" },
                        "extensions": {
                            "type": "object",
                            "maxProperties": 0,
                            "description": "Rejected at this transport boundary; resubmit without extensions"
                        }
                    }
                }
            }
        ]
    })
}

fn submit_handoff_result(task_id: &TaskId) -> Value {
    let payload = serde_json::json!({
        "task_id": task_id.to_string(),
        "state": READY_STATE_LABEL,
    });
    let text = serde_json::to_string(&payload).expect("payload is plain JSON data");

    serde_json::json!({
        "content": [ { "type": "text", "text": text } ],
        "structuredContent": payload,
        "isError": false,
    })
}

/// Maps every validation failure branch to its stable JSON-RPC error code.
pub(crate) fn map_ingress_error(error: &TaskIngressError) -> (i64, String) {
    match error {
        TaskIngressError::InputTooLarge { .. } => (INPUT_TOO_LARGE, error.to_string()),
        TaskIngressError::ExtensionsNotAllowed => (UNSUPPORTED_EXTENSIONS, error.to_string()),
        TaskIngressError::Protocol(ProtocolError::UnsupportedProtocolVersion { .. }) => {
            (UNSUPPORTED_PROTOCOL_VERSION, error.to_string())
        }
        TaskIngressError::Protocol(_) => (INVALID_HANDOFF, error.to_string()),
        TaskIngressError::ProjectRegistry(_) => (UNKNOWN_PROJECT, error.to_string()),
        TaskIngressError::UnsupportedFileExtension { .. }
        | TaskIngressError::FileRead { .. }
        | TaskIngressError::FileNotUtf8 { .. } => (INTERNAL_ERROR, error.to_string()),
    }
}

/// Maps persistence failures; any store problem means durability was NOT
/// confirmed and must surface as STORE_UNAVAILABLE rather than success.
pub(crate) fn map_persistence_error(error: &TaskPersistenceError) -> (i64, String) {
    match error {
        TaskPersistenceError::Store(_) => (STORE_UNAVAILABLE, error.to_string()),
        _ => (INTERNAL_ERROR, error.to_string()),
    }
}

#[cfg(test)]
impl McpServer {
    /// Test-only read access to the store for durable-listing assertions.
    fn store_list_view(&self) -> &SqliteStore {
        &self.store
    }
}

/// Runs the stdio loop until stdin closes. One message per line in, one
/// response line out; stdout carries protocol messages only.
pub(crate) async fn run(relay: Relay, store: SqliteStore) -> std::io::Result<()> {
    let mut server = McpServer::new(relay, store);
    let mut reader = BufReader::new(tokio::io::stdin());
    let mut stdout = tokio::io::stdout();
    let mut line = String::new();

    loop {
        line.clear();
        let bytes_read = reader.read_line(&mut line).await?;
        if bytes_read == 0 {
            break;
        }

        let Some(response) = server.handle_line(&line).await else {
            continue;
        };

        stdout.write_all(response.as_bytes()).await?;
        stdout.write_all(b"\n").await?;
        stdout.flush().await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use relay_core::{
        ProjectRegistry, ProjectRegistryError, RegisterProject, TaskState, list_stored_tasks,
        update_stored_task_state,
    };
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

    async fn submit_valid_task(server: &mut McpServer) -> Value {
        let raw = format!(
            r#"{{"jsonrpc":"2.0","id":99,"method":"tools/call","params":{{"name":"submit_handoff","arguments":{TASK_JSON}}}}}"#
        );
        let response = exchange(server, &raw).await.unwrap();
        assert!(
            response.get("error").is_none(),
            "valid submit failed: {response}"
        );
        response["result"]["structuredContent"].clone()
    }

    /// Every negative-conformance case must leave the server responsive:
    /// after the failure, a valid submit still succeeds.
    async fn assert_still_responsive(server: &mut McpServer) {
        let payload = submit_valid_task(server).await;
        assert_eq!(payload["state"], "Ready");
        assert!(TaskId::parse(payload["task_id"].as_str().unwrap()).is_ok());
    }

    #[test]
    fn rpc_request_round_trips() {
        let request = RpcRequest {
            jsonrpc: JSON_RPC_VERSION.to_owned(),
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
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":"abc-1","method":"initialize"}"#)
                .unwrap();

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

        let failure = failure_response(Some(RpcId::Text("x".to_owned())), UNKNOWN_PROJECT, "nope");
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
                INPUT_TOO_LARGE,
            ),
            (
                TaskIngressError::ExtensionsNotAllowed,
                UNSUPPORTED_EXTENSIONS,
            ),
            (
                TaskIngressError::Protocol(ProtocolError::UnsupportedProtocolVersion {
                    version: 7,
                }),
                UNSUPPORTED_PROTOCOL_VERSION,
            ),
            (
                TaskIngressError::Protocol(ProtocolError::DuplicateJsonKey("project".to_owned())),
                INVALID_HANDOFF,
            ),
            (
                TaskIngressError::ProjectRegistry(ProjectRegistryError::UnknownProjectKey {
                    key: relay_core::ProjectKey::parse("ghost").unwrap(),
                }),
                UNKNOWN_PROJECT,
            ),
            (
                TaskIngressError::ProjectRegistry(ProjectRegistryError::InvalidProjectKey {
                    input: "../etc".to_owned(),
                    reason: "traversal".to_owned(),
                }),
                UNKNOWN_PROJECT,
            ),
        ];

        for (error, expected_code) in cases {
            let (code, message) = map_ingress_error(&error);
            assert_eq!(code, expected_code, "wrong code for {error}");
            assert!(!message.is_empty());
        }
    }

    #[tokio::test]
    async fn persistence_store_failures_map_to_store_unavailable() {
        let store = SqliteStore::in_memory().await.unwrap();
        let error =
            update_stored_task_state(&store, TaskId::new(), TaskState::Ready, TaskState::Done)
                .await
                .unwrap_err();
        assert!(matches!(error, TaskPersistenceError::Store(_)));

        let (code, message) = map_persistence_error(&error);
        assert_eq!(code, STORE_UNAVAILABLE);
        assert!(!message.is_empty());

        let non_store = TaskPersistenceError::UnknownTaskState {
            state: "bogus".to_owned(),
        };
        assert_eq!(map_persistence_error(&non_store).0, INTERNAL_ERROR);
    }

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
    async fn tools_list_advertises_exactly_submit_handoff_with_ready_and_approval_language() {
        let mut server = test_server().await;

        let response = exchange(
            &mut server,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        )
        .await
        .unwrap();
        let tools = response["result"]["tools"].as_array().unwrap();

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "submit_handoff");

        let description = tools[0]["description"].as_str().unwrap();
        assert!(description.contains("Ready state"), "{description}");
        assert!(
            description.contains("explicit Run approval in the Relay TUI"),
            "{description}"
        );
        assert_eq!(tools[0]["inputSchema"]["type"], "object");
        assert_eq!(tools[0]["inputSchema"]["additionalProperties"], false);
    }

    #[tokio::test]
    async fn notifications_are_accepted_silently() {
        let mut server = test_server().await;

        assert!(
            exchange(
                &mut server,
                r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#
            )
            .await
            .is_none()
        );
        // Unknown notifications stay silent too.
        assert!(
            exchange(
                &mut server,
                r#"{"jsonrpc":"2.0","method":"notifications/whatever"}"#
            )
            .await
            .is_none()
        );

        // And the session remains usable afterwards.
        assert_still_responsive(&mut server).await;
    }

    #[tokio::test]
    async fn end_to_end_submit_persists_ready_task_exactly_as_tui_listing_would_show() {
        let mut server = test_server().await;

        let handshake = exchange(&mut server, &initialize_line("2024-11-05"))
            .await
            .unwrap();
        assert!(handshake.get("result").is_some());

        exchange(
            &mut server,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        )
        .await;

        let listing = exchange(
            &mut server,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#,
        )
        .await
        .unwrap();
        assert_eq!(listing["result"]["tools"].as_array().unwrap().len(), 1);

        let payload = submit_valid_task(&mut server).await;
        let task_id = TaskId::parse(payload["task_id"].as_str().unwrap()).unwrap();
        assert_eq!(payload["state"], "Ready");

        let tasks = list_stored_tasks(server.store_list_view()).await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, task_id);
        assert_eq!(tasks[0].state, TaskState::Ready);
        assert_eq!(tasks[0].handoff.title, "MCP submitted handoff");
        assert_eq!(tasks[0].submission_transport.as_str(), "relay-mcp");
    }

    #[tokio::test]
    async fn bad_mcp_protocol_version_is_rejected_structured_and_server_survives() {
        let mut server = test_server().await;

        let response = exchange(&mut server, &initialize_line("1999-01-01"))
            .await
            .unwrap();
        let (code, message) = error_of(&response);
        assert_eq!(code, UNSUPPORTED_PROTOCOL_VERSION);
        assert!(
            message.contains("unsupported MCP protocol version"),
            "{message}"
        );

        let recovery = exchange(&mut server, &initialize_line("2025-03-26"))
            .await
            .unwrap();
        assert!(recovery.get("result").is_some());

        assert_still_responsive(&mut server).await;
    }

    #[tokio::test]
    async fn malformed_json_returns_parse_error_and_server_survives() {
        let mut server = test_server().await;

        let (code, _) = error_of(&exchange(&mut server, "{not json at all").await.unwrap());
        assert_eq!(code, PARSE_ERROR);

        assert_still_responsive(&mut server).await;
    }

    #[tokio::test]
    async fn invalid_envelope_returns_invalid_request_and_server_survives() {
        let mut server = test_server().await;

        let (code, _) = error_of(
            &exchange(
                &mut server,
                r#"{"jsonrpc":"1.0","id":5,"method":"initialize"}"#,
            )
            .await
            .unwrap(),
        );
        assert_eq!(code, INVALID_REQUEST);

        assert_still_responsive(&mut server).await;
    }

    #[tokio::test]
    async fn unknown_method_returns_method_not_found_and_server_survives() {
        let mut server = test_server().await;

        let (code, message) = error_of(
            &exchange(
                &mut server,
                r#"{"jsonrpc":"2.0","id":6,"method":"tasks/run"}"#,
            )
            .await
            .unwrap(),
        );
        assert_eq!(code, METHOD_NOT_FOUND);
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
        assert_eq!(code, INVALID_PARAMS);
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
        assert_eq!(code, INVALID_HANDOFF);
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
        assert_eq!(code, INVALID_HANDOFF);
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
        assert_eq!(code, UNKNOWN_PROJECT);
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
        assert_eq!(code, INPUT_TOO_LARGE);
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
        assert_eq!(code, UNSUPPORTED_EXTENSIONS);
        assert!(message.contains("extensions"), "{message}");

        assert_still_responsive(&mut server).await;
    }
}
