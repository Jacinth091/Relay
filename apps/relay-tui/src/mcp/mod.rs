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

use relay_core::{DaemonClientHandle, MAX_LOCAL_TASK_INPUT_BYTES, Relay, SqliteStore};
use serde_json::{Value, json};
use tokio::io::{AsyncWriteExt, BufReader};

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
    /// Read-only handle, used for test/listing views only (see
    /// `store_list_view`); durable mutations (`submit_handoff`) go through
    /// `daemon` instead.
    #[allow(dead_code)]
    store: SqliteStore,
    daemon: DaemonClientHandle,
    /// Legacy-era session state: negotiated revision, if any.
    negotiated_legacy_version: Option<&'static str>,
    /// Legacy-era session state: set once `notifications/initialized` lands.
    legacy_initialized: bool,
}

impl McpServer {
    pub(crate) fn new(relay: Relay, store: SqliteStore, daemon: DaemonClientHandle) -> Self {
        Self {
            relay,
            store,
            daemon,
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

        // Notifications (messages with NO id member at all) must never
        // receive a response, even when they are malformed JSON-RPC — the
        // specification forbids replying to notifications, and replying to a
        // stream of garbage would generate unbounded outbound traffic.
        let is_notification = match envelope.as_object() {
            Some(object) => !object.contains_key("id"),
            None => true,
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

        // Notifications carry no id and are accepted silently, whatever their
        // era; the legacy initialized notification advances lifecycle state.
        let id = match request.id {
            Some(id) => id,
            None if is_notification => {
                if request.method == "notifications/initialized"
                    && request.jsonrpc == protocol::JSON_RPC_VERSION
                {
                    self.legacy_initialized = true;
                    tracing::debug!(
                        negotiated = self.negotiated_legacy_version,
                        "legacy session initialized"
                    );
                } else if request.jsonrpc != protocol::JSON_RPC_VERSION {
                    tracing::warn!("dropping malformed notification without reply");
                }
                return None;
            }
            // An explicit `"id": null` is an invalid REQUEST id (JSON-RPC 2.0
            // reserves null for error responses), so it is answered rather
            // than silently treated as a notification.
            None => {
                return Some(
                    failure_response(
                        None,
                        protocol::INVALID_REQUEST,
                        "request 'id' must be a string or number, not null".to_owned(),
                    )
                    .into_wire(),
                );
            }
        };

        // Strict JSON-RPC 2.0: the member is mandatory on every message.
        // Requests failing this check get a structured error; malformed
        // notifications were already silenced above.
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
            return Some(failure_response(Some(id), protocol::INVALID_REQUEST, detail).into_wire());
        }

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
            // Keep-alive probe; both protocol eras require an empty result.
            "ping" => success_response(id, json!({})),
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
            // Keep-alive probe; both protocol eras require an empty result.
            "ping" => success_response(id, json!({})),
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
        match service::execute_submit_handoff(&self.relay, &self.daemon, raw_params_json).await {
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

/// Hard cap on one stdio frame (one JSON-RPC message line). Generous enough
/// for the largest legal submit_handoff payload plus envelope overhead;
/// enforced while READING so a peer streaming bytes without a newline cannot
/// grow the process's memory without bound.
pub(crate) const MAX_FRAME_BYTES: usize = MAX_LOCAL_TASK_INPUT_BYTES as usize * 2 + 64 * 1024;

/// Runs the stdio loop until stdin closes. One message per line in, one
/// response line out; stdout carries protocol messages only.
///
/// Frames are read with an explicit byte budget: over-limit frames are
/// drained (through their newline) and rejected with a structured error, so
/// framing stays intact and the process never buffers an unbounded line.
pub(crate) async fn run(
    relay: Relay,
    store: SqliteStore,
    daemon: DaemonClientHandle,
) -> std::io::Result<()> {
    let mut server = McpServer::new(relay, store, daemon);
    let mut reader = BufReader::new(tokio::io::stdin());
    let mut stdout = tokio::io::stdout();

    loop {
        let read = match read_capped_line(&mut reader, MAX_FRAME_BYTES).await? {
            CappedLine::Eof => break,
            CappedLine::Line(line) => line,
            CappedLine::TooLarge(discarded_bytes) => {
                tracing::warn!(
                    discarded_bytes,
                    MAX_FRAME_BYTES,
                    "stdio frame exceeded maximum size; drained and rejected"
                );
                let response = failure_response(
                    None,
                    protocol::INVALID_REQUEST,
                    format!("message frame exceeded the maximum of {MAX_FRAME_BYTES} bytes"),
                )
                .into_wire();
                stdout.write_all(response.as_bytes()).await?;
                stdout.write_all(b"\n").await?;
                stdout.flush().await?;
                continue;
            }
        };

        let Some(response) = server.handle_line(&read).await else {
            continue;
        };

        stdout.write_all(response.as_bytes()).await?;
        stdout.write_all(b"\n").await?;
        stdout.flush().await?;
    }

    Ok(())
}

enum CappedLine {
    Eof,
    Line(String),
    /// The frame exceeded the cap; the byte count is how much was drained
    /// before (and including) the frame's terminating newline.
    TooLarge(usize),
}

/// Reads one newline-terminated frame into a String, refusing to buffer more
/// than `cap` bytes. When the cap trips, remaining bytes of the frame are
/// consumed and discarded so the stream stays framed for the next message.
async fn read_capped_line<R>(reader: &mut BufReader<R>, cap: usize) -> std::io::Result<CappedLine>
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncBufReadExt;

    let mut frame: Vec<u8> = Vec::new();
    // Bytes already consumed from the stream while accumulating `frame`
    // (every Append consumes immediately); used for over-cap accounting.
    let mut consumed_total: usize = 0;
    loop {
        // Scope the mutable borrow of `reader` so `consume` can run after.
        enum Step {
            /// Frame complete: the copied prefix was not yet consumed, so
            /// drain exactly these bytes (prefix + newline) from the reader.
            Done(usize),
            /// Append happened; consume this many bytes and keep reading.
            Append(usize),
            /// Over cap: consume this many bytes, then drain to the frame's
            /// newline without buffering.
            Trip(usize),
        }
        let step = {
            let available = reader.fill_buf().await?;
            if available.is_empty() {
                // EOF: an unterminated final frame is still delivered if it
                // has content. It can never be over cap here: the Trip path
                // never extends `frame`, so frame.len() <= cap always holds.
                return Ok(if frame.is_empty() {
                    CappedLine::Eof
                } else {
                    CappedLine::Line(String::from_utf8_lossy(&frame).into_owned())
                });
            }
            match available.iter().position(|byte| *byte == b'\n') {
                Some(position) => {
                    if frame.len() + position > cap {
                        Step::Trip(position + 1)
                    } else {
                        frame.extend_from_slice(&available[..position]);
                        // The copied prefix was NOT yet consumed: drain it
                        // plus the newline after releasing the borrow.
                        Step::Done(position + 1)
                    }
                }
                None => {
                    if frame.len() + available.len() > cap {
                        Step::Trip(available.len())
                    } else {
                        let used = available.len();
                        frame.extend_from_slice(available);
                        Step::Append(used)
                    }
                }
            }
        };
        match step {
            Step::Done(to_consume) => {
                reader.consume(to_consume);
                return Ok(CappedLine::Line(
                    String::from_utf8_lossy(&frame).into_owned(),
                ));
            }
            Step::Append(used) => {
                reader.consume(used);
                consumed_total += used;
            }
            Step::Trip(consumed) => {
                reader.consume(consumed);
                let drained = consumed_total + consumed;
                frame.clear();
                return drain_to_newline(reader, drained).await;
            }
        }
    }
}

/// Consumes bytes up to and including the next newline without buffering
/// them; used after the size cap trips. Returns the total drained count.
async fn drain_to_newline<R>(
    reader: &mut BufReader<R>,
    mut drained: usize,
) -> std::io::Result<CappedLine>
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncBufReadExt;

    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok(CappedLine::TooLarge(drained));
        }
        match available.iter().position(|byte| *byte == b'\n') {
            Some(position) => {
                drained += position + 1;
                reader.consume(position + 1);
                return Ok(CappedLine::TooLarge(drained));
            }
            None => {
                drained += available.len();
                let filled = available.len();
                reader.consume(filled);
            }
        }
    }
}
