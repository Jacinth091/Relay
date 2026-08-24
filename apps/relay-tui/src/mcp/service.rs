//! Era-independent Relay submission semantics for the MCP server.
//!
//! This layer is deliberately blind to the protocol era: both modern
//! stateless requests and legacy handshake-era requests converge here, so
//! there is exactly ONE Relay submission implementation (handoff Â§5.1).
//! It owns the tool catalog, the `submit_handoff` pipeline (size cap â†’
//! ingress preview â†’ durable persistence), and the stable error-code
//! mapping tables.
//!
//! Structural guard: this module depends on NO execution or scheduler
//! types. Submission lands tasks in `Ready`; nothing executes without
//! explicit local Run approval in the TUI.

use relay_core::{
    MAX_LOCAL_TASK_INPUT_BYTES, Relay, SqliteStore, StoredTask, TaskIngressError,
    TaskPersistenceError,
};
use relay_protocol::ProtocolError;
use serde::Deserialize;
use serde_json::Value;

use crate::mcp::protocol as p;

pub(crate) const SUBMIT_HANDOFF_TOOL_NAME: &str = "submit_handoff";
/// User-facing state label mandated by the M9 pack output contract.
pub(crate) const READY_STATE_LABEL: &str = "Ready";

pub(crate) const SUBMIT_HANDOFF_DESCRIPTION: &str = "Submit ONE canonical HandoffTask v1 JSON object \
(the same schema as local paste import; no wrapper keys, no templates). The task is validated \
against the trusted local project registry and durably persisted, landing it in Ready state. \
Submission NEVER executes anything: running the task requires explicit Run approval in the \
Relay TUI.";

/// The complete, deterministically ordered tool catalog. M9 exposes exactly
/// one public tool; ordering stays explicit so future additions cannot drift.
pub(crate) fn tool_catalog() -> Value {
    serde_json::json!([
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
    ])
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

/// Runs one `submit_handoff` call end to end against the given raw
/// `tools/call` params JSON. Returns the Relay-owned task id on durable
/// success, or a final `(error code, message)` pair â€” every failure branch
/// leaves nothing persisted.
pub(crate) async fn execute_submit_handoff(
    relay: &mut Relay,
    store: &SqliteStore,
    raw_params_json: &str,
) -> Result<relay_core::TaskId, (i64, String)> {
    let envelope: ToolCallEnvelope<'_> = match serde_json::from_str(raw_params_json) {
        Ok(envelope) => envelope,
        Err(error) => {
            return Err((
                p::INVALID_PARAMS,
                format!("invalid tools/call params: {error}"),
            ));
        }
    };

    let Some(call) = envelope.params else {
        return Err((
            p::INVALID_PARAMS,
            "tools/call requires a params object with a tool name".to_owned(),
        ));
    };

    if call.name != SUBMIT_HANDOFF_TOOL_NAME {
        return Err((
            p::INVALID_PARAMS,
            format!(
                "unknown tool '{}'; available tools: {SUBMIT_HANDOFF_TOOL_NAME}",
                call.name
            ),
        ));
    }

    let Some(arguments) = call.arguments else {
        return Err((
            p::INVALID_PARAMS,
            format!(
                "tool '{SUBMIT_HANDOFF_TOOL_NAME}' requires 'arguments' containing one \
                 canonical HandoffTask v1 JSON object"
            ),
        ));
    };

    let raw_payload = arguments.get();
    ensure_payload_within_size_cap(raw_payload)?;

    let candidate = match relay.preview_mcp_submission(raw_payload) {
        Ok(candidate) => candidate,
        Err(error) => return Err(map_ingress_error(&error)),
    };

    // Never claim success unless the store confirmed durability.
    let stored: StoredTask = match relay.confirm_task_import_with_store(store, candidate).await {
        Ok(stored) => stored,
        Err(error) => return Err(map_persistence_error(&error)),
    };

    Ok(stored.id)
}

fn ensure_payload_within_size_cap(raw_payload: &str) -> Result<(), (i64, String)> {
    let actual = raw_payload.len() as u64;
    if actual > MAX_LOCAL_TASK_INPUT_BYTES {
        Err((
            p::INPUT_TOO_LARGE,
            format!(
                "submit_handoff arguments are {actual} bytes, exceeding the maximum of \
                 {MAX_LOCAL_TASK_INPUT_BYTES} bytes"
            ),
        ))
    } else {
        Ok(())
    }
}

/// Maps every validation failure branch to its stable JSON-RPC error code.
///
/// Local-filesystem error variants are scrubbed before reaching the wire:
/// their `Display` text can embed absolute paths from the operator's machine,
/// so the full detail goes to stderr diagnostics and the client receives a
/// generic message (fail-closed information disclosure posture).
pub(crate) fn map_ingress_error(error: &TaskIngressError) -> (i64, String) {
    use crate::mcp::protocol as p;
    match error {
        TaskIngressError::InputTooLarge { .. } => (p::INPUT_TOO_LARGE, error.to_string()),
        TaskIngressError::ExtensionsNotAllowed => (p::UNSUPPORTED_EXTENSIONS, error.to_string()),
        TaskIngressError::Protocol(ProtocolError::UnsupportedProtocolVersion { .. }) => {
            (p::UNSUPPORTED_PROTOCOL_VERSION, error.to_string())
        }
        TaskIngressError::Protocol(_) => (p::INVALID_HANDOFF, error.to_string()),
        TaskIngressError::ProjectRegistry(_) => (p::UNKNOWN_PROJECT, error.to_string()),
        TaskIngressError::UnsupportedFileExtension { .. }
        | TaskIngressError::FileRead { .. }
        | TaskIngressError::FileNotUtf8 { .. } => {
            tracing::warn!(%error, "local file ingress error surfaced over MCP");
            (
                p::INTERNAL_ERROR,
                "task could not be read at the local ingress boundary".to_owned(),
            )
        }
    }
}

/// Maps persistence failures; any store problem means durability was NOT
/// confirmed and must surface as STORE_UNAVAILABLE rather than success.
pub(crate) fn map_persistence_error(error: &TaskPersistenceError) -> (i64, String) {
    use crate::mcp::protocol as p;
    match error {
        TaskPersistenceError::Store(_) => (p::STORE_UNAVAILABLE, error.to_string()),
        _ => (p::INTERNAL_ERROR, error.to_string()),
    }
}
