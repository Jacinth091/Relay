//! Headless MCP stdio server backing `relay serve-mcp`.
//!
//! This module is deterministic submission plumbing (ADR-0016): it speaks
//! JSON-RPC 2.0 over newline-delimited stdio frames, validates machine-initiated
//! handoffs through the same Relay Core ingress path as local paste import,
//! persists them through the same confirm path, and answers with a Relay-owned
//! TaskId.
//!
//! Protocol eras: this server is DUAL-ERA (specification `2026-07-28`,
//! Versioning). Era selection terminates in [`protocol::classify_request`];
//! both eras converge on the single era-independent submission implementation
//! in [`service`]. Relay semantics never branch on how a request arrived.
//!
//! * **Modern** (`2026-07-28`): stateless per-request `_meta`, mandatory
//!   `server/discover`, `resultType: "complete"` results.
//! * **Legacy** (through `2025-11-25`): initialize handshake with specification
//!   version negotiation; plain results without modern-only fields.
//!
//! Structural guard: this module deliberately depends on NO execution or
//! scheduler types. It cannot construct an `ExecutionScheduler`, enqueue an
//! execution, or start a runtime; acceptance lands tasks in `Ready` and
//! nothing else. Running always requires explicit Run approval in the TUI
//! (invariants 3, 5, 13, 18).
//!
//! Framing: one JSON-RPC message per line (the MCP stdio convention). Stdout
//! carries protocol messages only; all diagnostics go to stderr via tracing.

pub(crate) mod protocol;
pub(crate) mod service;

#[cfg(test)]
mod tests;

use relay_core::{Relay, SqliteStore};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use protocol::{
    META_PROTOCOL_VERSION_KEY, META_SERVER_INFO_KEY, MODERN_PROTOCOL_VERSION, McpProtocolEra,
    RequestEra, RpcId, RpcRequest, RpcResponse, SUPPORTED_PROTOCOL_VERSIONS, classify_request,
    failure_response, failure_response_with_data, negotiate_legacy_version, success_response,
};

const SERVER_NAME: &str = "relay";

/// Minimal MCP stdio server. Owns exactly what it must share: the trusted
/// project registry (via `Relay`) and the SQLite store, plus enough legacy
/// lifecycle state to distinguish an uninitialized handshake-era process
/// from an initialized one (observability only; submissions stay lenient
/// because the specifications do not gate client requests on initialize).
pub(crate) struct McpServer {
    relay: Relay,
    store: SqliteStore,
    /// Legacy-era session state: negotiated revision, if any.
    negotiated_legacy_version: Option<&'static str>,
    /// Legacy-era session state: set once `notifications/initialized` lands.
    legacy_initialized: bool,
}

impl McpServer {
    pub(crate) fn new(relay: Relay, store: SqliteStore) -> Self {
        Self {
            relay,
            store,
            negotiated_legacy_version: None,
            legacy_initialized: false,
        }
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
                        protocol::PARSE_ERROR,
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
                        protocol::INVALID_REQUEST,
                        format!("invalid JSON-RPC request: {error}"),
                    )
                    .into_wire(),
                );
            }
        };

        // Strict JSON-RPC 2.0: the member is mandatory on every message,
        // notifications included.
        if request.jsonrpc != protocol::JSON_RPC_VERSION {
            let detail = if request.jsonrpc.is_empty() {
                "request is missing the required \"jsonrpc\": \"2.0\" member".to_owned()
            } else {
                format!(
                    "unsupported jsonrpc version '{}'; expected '{}'",
                    request.jsonrpc,
                    protocol::JSON_RPC_VERSION
                )
            };
            return Some(
                failure_response(request.id, protocol::INVALID_REQUEST, detail).into_wire(),
            );
        }

        // Notifications carry no id and are accepted silently, whatever their
        // era; the legacy initialized notification advances lifecycle state.
        let id = match request.id {
            Some(id) => id,
            None => {
                if request.method == "notifications/initialized" {
                    self.legacy_initialized = true;
                    tracing::debug!(
                        negotiated = self.negotiated_legacy_version,
                        "legacy session initialized"
                    );
                }
                return None;
            }
        };

        match classify_request(&request.method, request.params.as_ref()) {
            RequestEra::InvalidMetadata => Some(
                failure_response(
                    Some(id),
                    protocol::INVALID_REQUEST,
                    format!(
                        "params._meta must carry a string '{META_PROTOCOL_VERSION_KEY}' for \
                         stateless requests; send that member or omit _meta for the legacy \
                         initialize handshake"
                    ),
                )
                .into_wire(),
            ),
            RequestEra::Legacy => {
                let method = request.method.as_str();
                let params = &request.params;
                Some(
                    self.handle_legacy(method, params, id, trimmed)
                        .await
                        .into_wire(),
                )
            }
            RequestEra::Modern { requested_version } => Some(
                self.handle_modern(&requested_version, &request.method, id, trimmed)
                    .await
                    .into_wire(),
            ),
        }
    }

    // ------------------------------------------------------------------
    // Legacy era (initialize handshake through 2025-11-25)
    // ------------------------------------------------------------------

    async fn handle_legacy(
        &mut self,
        method: &str,
        params: &Option<Value>,
        id: RpcId,
        raw_line: &str,
    ) -> RpcResponse {
        match method {
            "initialize" => self.handle_initialize(params, id),
            // Requests (with ids) using lifecycle method names are invalid;
            // `notifications/initialized` as a request cannot be answered.
            "notifications/initialized" => failure_response(
                Some(id),
                protocol::INVALID_REQUEST,
                "notifications/initialized is a notification and carries no response".to_owned(),
            ),
            "tools/list" => {
                self.warn_if_legacy_uninitialized("tools/list");
                success_response(id, json!({ "tools": service::tool_catalog() }))
            }
            "tools/call" => {
                self.warn_if_legacy_uninitialized("tools/call");
                self.handle_tools_call(raw_line, id, McpProtocolEra::Legacy)
                    .await
            }
            method => failure_response(
                Some(id),
                protocol::METHOD_NOT_FOUND,
                format!("method not found: {method}"),
            ),
        }
    }

    fn warn_if_legacy_uninitialized(&self, method: &str) {
        if !self.legacy_initialized {
            tracing::warn!(
                method,
                "legacy-era request arrived before notifications/initialized; serving anyway"
            );
        }
    }

    /// Legacy initialization per specification `2025-06-18`: echo the
    /// requested revision when supported, otherwise respond with our latest
    /// legacy revision — never an error.
    fn handle_initialize(&mut self, params: &Option<Value>, id: RpcId) -> RpcResponse {
        let Some(Value::Object(params)) = params else {
            return failure_response(
                Some(id),
                protocol::INVALID_PARAMS,
                "initialize requires a params object with protocolVersion".to_owned(),
            );
        };

        match params.get("protocolVersion") {
            Some(Value::String(_)) => {}
            _ => {
                return failure_response(
                    Some(id),
                    protocol::INVALID_PARAMS,
                    "initialize requires a string protocolVersion".to_owned(),
                );
            }
        }

        let requested = params.get("protocolVersion").and_then(Value::as_str);
        let negotiated = negotiate_legacy_version(requested);
        self.negotiated_legacy_version = Some(negotiated);
        success_response(
            id,
            json!({
                "protocolVersion": negotiated,
                "capabilities": { "tools": {} },
                "serverInfo": server_info_value(),
            }),
        )
    }

    // ------------------------------------------------------------------
    // Modern era (2026-07-28, stateless per-request metadata)
    // ------------------------------------------------------------------

    async fn handle_modern(
        &mut self,
        requested_version: &str,
        method: &str,
        id: RpcId,
        raw_line: &str,
    ) -> RpcResponse {
        if requested_version != MODERN_PROTOCOL_VERSION {
            return unsupported_protocol_version_error(id, requested_version);
        }

        match method {
            "server/discover" => success_response(id, discover_result()),
            "tools/list" => success_response(id, modern_tools_list_result()),
            "tools/call" => {
                self.handle_tools_call(raw_line, id, McpProtocolEra::Modern)
                    .await
            }
            method => failure_response(
                Some(id),
                protocol::METHOD_NOT_FOUND,
                format!("method not found: {method}"),
            ),
        }
    }

    // ------------------------------------------------------------------
    // Shared submission pipeline (era-independent)
    // ------------------------------------------------------------------

    async fn handle_tools_call(
        &mut self,
        raw_params_json: &str,
        id: RpcId,
        era: McpProtocolEra,
    ) -> RpcResponse {
        match service::execute_submit_handoff(&mut self.relay, &self.store, raw_params_json).await {
            Ok(task_id) => success_response(id, submit_result(task_id, era)),
            Err((code, message)) => failure_response(Some(id), code, message),
        }
    }
}

// ----------------------------------------------------------------------
// Response shaping per era
// ----------------------------------------------------------------------

fn server_info_value() -> Value {
    json!({
        "name": SERVER_NAME,
        "version": env!("CARGO_PKG_VERSION"),
    })
}

fn server_info_meta() -> Value {
    json!({ META_SERVER_INFO_KEY: server_info_value() })
}

/// Modern capability discovery result (specification `2026-07-28`,
/// Discovery): supported versions across BOTH eras, truthful capabilities,
/// identity via `_meta`. Advertises exactly what exists: tools only.
fn discover_result() -> Value {
    json!({
        "supportedVersions": SUPPORTED_PROTOCOL_VERSIONS,
        "capabilities": { "tools": {} },
        "_meta": server_info_meta(),
    })
}

fn modern_tools_list_result() -> Value {
    let mut result = json!({
        "resultType": "complete",
        "tools": service::tool_catalog(),
    });
    result["_meta"] = server_info_meta();
    result
}

fn submit_result(task_id: relay_core::TaskId, era: McpProtocolEra) -> Value {
    let payload = json!({
        "task_id": task_id.to_string(),
        "state": service::READY_STATE_LABEL,
    });
    let text = serde_json::to_string(&payload).expect("payload is plain JSON data");

    match era {
        McpProtocolEra::Modern => json!({
            "resultType": "complete",
            "content": [ { "type": "text", "text": text } ],
            "structuredContent": payload,
            "isError": false,
            "_meta": server_info_meta(),
        }),
        // Legacy results never carry modern-only fields such as resultType.
        McpProtocolEra::Legacy => json!({
            "content": [ { "type": "text", "text": text } ],
            "structuredContent": payload,
            "isError": false,
        }),
    }
}

/// Modern unsupported-version rejection (specification `2026-07-28`,
/// Versioning): lists every supported revision so the client can retry.
fn unsupported_protocol_version_error(id: RpcId, requested: &str) -> RpcResponse {
    failure_response_with_data(
        Some(id),
        protocol::MCP_UNSUPPORTED_PROTOCOL_VERSION,
        "Unsupported protocol version",
        Some(json!({
            "supported": SUPPORTED_PROTOCOL_VERSIONS,
            "requested": requested,
        })),
    )
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
