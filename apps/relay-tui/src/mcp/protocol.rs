//! MCP wire-protocol layer for `relay serve-mcp`.
//!
//! Owns JSON-RPC 2.0 framing, the protocol-era seam (modern stateless
//! `2026-07-28` vs legacy handshake-era revisions), version tables, and
//! negotiation rules. Everything here is pure wire mechanics: Relay
//! submission semantics live in [`super::service`] and are era-independent.
//!
//! Era model — verified against the authoritative specification at
//! `https://modelcontextprotocol.io/specification/2026-07-28/`:
//!
//! * **Modern** — per-request `_meta` carries
//!   `io.modelcontextprotocol/protocolVersion`; there is no initialize
//!   handshake; capability discovery is the `server/discover` RPC; version
//!   mismatch answers [`MCP_UNSUPPORTED_PROTOCOL_VERSION`] with a `supported`
//!   list (`basic/versioning`). A dual-era stdio server selects behavior by
//!   how the client opens: lifecycle methods select legacy; requests carrying
//!   modern `_meta` select modern; requests carrying neither are served under
//!   legacy semantics (`basic/transports/stdio#backward-compatibility`,
//!   `basic/versioning#backward-compatibility-with-initialization-based-versions`).
//! * **Legacy** — revisions through `2025-11-25`; session established by
//!   `initialize` / `notifications/initialized`; an unsupported requested
//!   version is answered by re-serving one of our supported versions, never
//!   an error (`2025-06-18 basic/lifecycle`, Version Negotiation).

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(crate) const JSON_RPC_VERSION: &str = "2.0";

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
/// HandoffTask protocol (task payload `protocol_version`) mismatches only.
pub(crate) const UNSUPPORTED_PROTOCOL_VERSION: i64 = -32004;
pub(crate) const INVALID_HANDOFF: i64 = -32005;

/// MCP-standard modern-era error: a request whose per-request
/// `_meta` protocol version is not supported. Wire shape per the
/// authoritative `2026-07-28` specification, "Versioning and Compatibility"
/// (modelcontextprotocol.io/specification/2026-07-28/basic/versioning):
/// `{ code: -32022, message, data: { supported: [...], requested } }`.
/// Distinct from Relay's application codes above.
pub(crate) const MCP_UNSUPPORTED_PROTOCOL_VERSION: i64 = -32022;

/// Current modern MCP revision served statelessly via per-request `_meta`.
pub(crate) const MODERN_PROTOCOL_VERSION: &str = "2026-07-28";

/// Latest legacy (handshake-era) revision; the baseline a legacy client is
/// negotiated down to when its requested version is not supported.
pub(crate) const LATEST_LEGACY_PROTOCOL_VERSION: &str = "2025-11-25";

/// Legacy (handshake-era) revisions this server understands, newest first.
pub(crate) const LEGACY_PROTOCOL_VERSIONS: [&str; 4] =
    ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// Every protocol revision this server supports across both eras, newest
/// first. Advertised verbatim in discover results and unsupported-version
/// errors so clients can pick a mutually supported revision.
pub(crate) const SUPPORTED_PROTOCOL_VERSIONS: [&str; 5] = [
    MODERN_PROTOCOL_VERSION,
    LATEST_LEGACY_PROTOCOL_VERSION,
    "2025-06-18",
    "2025-03-26",
    "2024-11-05",
];

/// `_meta` key carrying the per-request protocol version (modern era).
pub(crate) const META_PROTOCOL_VERSION_KEY: &str = "io.modelcontextprotocol/protocolVersion";
/// `_meta` key carrying server identity in responses (both eras MAY; modern SHOULD).
pub(crate) const META_SERVER_INFO_KEY: &str = "io.modelcontextprotocol/serverInfo";

/// Which protocol era a request belongs to. The era terminates at this
/// adapter: downstream submission semantics never branch on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum McpProtocolEra {
    /// Stateless request carrying `io.modelcontextprotocol/protocolVersion`.
    Modern,
    /// Handshake-era client (`initialize` lifecycle or no per-request meta).
    Legacy,
}

/// Era classification outcome for one parsed request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RequestEra {
    /// Serve statelessly under the requested (supported) protocol version.
    Modern { requested_version: String },
    /// Serve under legacy handshake semantics.
    Legacy,
    /// A `_meta` object was present but lacked the required
    /// `io.modelcontextprotocol/protocolVersion` member: malformed modern
    /// metadata that must not silently downgrade to legacy handling.
    InvalidMetadata,
}

/// Classifies one request into its protocol era.
///
/// Lifecycle methods select the legacy era unconditionally (a dual-era
/// stdio server selects behavior by how the client opens — specification
/// `2026-07-28`, Versioning). Any other request is modern when its params
/// carry `_meta.io.modelcontextprotocol/protocolVersion`, and legacy
/// otherwise (handshake-era clients send no such metadata).
pub(crate) fn classify_request(method: &str, params: Option<&Value>) -> RequestEra {
    if matches!(method, "initialize" | "notifications/initialized") {
        return RequestEra::Legacy;
    }

    let meta = params.and_then(|params| params.get("_meta"));
    match meta {
        None => RequestEra::Legacy,
        Some(Value::Object(meta)) => match meta.get(META_PROTOCOL_VERSION_KEY) {
            Some(Value::String(version)) => RequestEra::Modern {
                requested_version: version.clone(),
            },
            _ => RequestEra::InvalidMetadata,
        },
        Some(_) => RequestEra::InvalidMetadata,
    }
}

/// Legacy version negotiation (verified against the authoritative
/// specification, modelcontextprotocol.io/specification/2025-06-18/basic/
/// lifecycle, "Version Negotiation"): echo the requested version when we
/// support it; otherwise respond with another version we support — SHOULD be
/// our latest legacy revision. Never an error.
pub(crate) fn negotiate_legacy_version(requested: Option<&str>) -> &'static str {
    let matched = requested.and_then(|version| {
        LEGACY_PROTOCOL_VERSIONS
            .iter()
            .find(|candidate| **candidate == version)
            .copied()
    });
    matched.unwrap_or(LATEST_LEGACY_PROTOCOL_VERSION)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum RpcId {
    Number(i64),
    Text(String),
}

impl std::fmt::Display for RpcId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
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

pub(crate) fn success_response(id: RpcId, result: Value) -> RpcResponse {
    RpcResponse {
        jsonrpc: JSON_RPC_VERSION.to_owned(),
        id: Some(id),
        result: Some(result),
        error: None,
    }
}

pub(crate) fn failure_response(
    id: Option<RpcId>,
    code: i64,
    message: impl Into<String>,
) -> RpcResponse {
    failure_response_with_data(id, code, message, None)
}

pub(crate) fn failure_response_with_data(
    id: Option<RpcId>,
    code: i64,
    message: impl Into<String>,
    data: Option<Value>,
) -> RpcResponse {
    RpcResponse {
        jsonrpc: JSON_RPC_VERSION.to_owned(),
        id,
        result: None,
        error: Some(RpcErrorBody {
            code,
            message: message.into(),
            data,
        }),
    }
}

impl RpcResponse {
    pub(crate) fn into_wire(self) -> String {
        serde_json::to_string(&self).unwrap_or_else(|_| fallback_error_line())
    }
}

fn fallback_error_line() -> String {
    r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32603,"message":"internal error: failed to serialize response"}}"#.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lifecycle_methods_classify_as_legacy_regardless_of_meta() {
        let with_meta = json!({
            "_meta": { META_PROTOCOL_VERSION_KEY: MODERN_PROTOCOL_VERSION }
        });
        assert_eq!(
            classify_request("initialize", Some(&with_meta)),
            RequestEra::Legacy
        );
        assert_eq!(
            classify_request("notifications/initialized", Some(&with_meta)),
            RequestEra::Legacy
        );
    }

    #[test]
    fn per_request_meta_selects_modern_era() {
        let params = json!({
            "_meta": { META_PROTOCOL_VERSION_KEY: MODERN_PROTOCOL_VERSION }
        });
        assert_eq!(
            classify_request("tools/call", Some(&params)),
            RequestEra::Modern {
                requested_version: MODERN_PROTOCOL_VERSION.to_owned()
            }
        );
    }

    #[test]
    fn missing_params_or_meta_is_legacy_while_empty_meta_is_invalid() {
        assert_eq!(classify_request("tools/list", None), RequestEra::Legacy);
        assert_eq!(
            classify_request("tools/list", Some(&json!({}))),
            RequestEra::Legacy
        );
        assert_eq!(
            classify_request("tools/list", Some(&json!({ "_meta": {} }))),
            RequestEra::InvalidMetadata
        );
        assert_eq!(
            classify_request("tools/list", Some(&json!({ "_meta": "garbage" }))),
            RequestEra::InvalidMetadata
        );
        assert_eq!(
            classify_request(
                "tools/list",
                Some(&json!({ "_meta": { META_PROTOCOL_VERSION_KEY: 20 } }))
            ),
            RequestEra::InvalidMetadata
        );
    }

    #[test]
    fn legacy_negotiation_echoes_supported_and_downgrades_to_latest() {
        assert_eq!(negotiate_legacy_version(Some("2024-11-05")), "2024-11-05");
        assert_eq!(negotiate_legacy_version(Some("2025-06-18")), "2025-06-18");
        assert_eq!(negotiate_legacy_version(Some("2025-11-25")), "2025-11-25");

        // Unsupported, newer-than-server, and missing requests all receive
        // our latest legacy revision instead of an error.
        assert_eq!(
            negotiate_legacy_version(Some("2999-01-01")),
            LATEST_LEGACY_PROTOCOL_VERSION
        );
        assert_eq!(
            negotiate_legacy_version(Some("1999-01-01")),
            LATEST_LEGACY_PROTOCOL_VERSION
        );
        assert_eq!(
            negotiate_legacy_version(None),
            LATEST_LEGACY_PROTOCOL_VERSION
        );
    }

    #[test]
    fn error_responses_carry_optional_data() {
        let response = failure_response_with_data(
            Some(RpcId::Number(1)),
            MCP_UNSUPPORTED_PROTOCOL_VERSION,
            "Unsupported protocol version",
            Some(json!({
                "supported": SUPPORTED_PROTOCOL_VERSIONS,
                "requested": "1900-01-01"
            })),
        );
        let wire: Value = serde_json::from_str(&response.into_wire()).unwrap();
        assert_eq!(wire["error"]["code"], -32022);
        assert_eq!(wire["error"]["data"]["requested"], "1900-01-01");
        assert_eq!(
            wire["error"]["data"]["supported"].as_array().unwrap().len(),
            5
        );

        // Plain failures omit the data member entirely.
        let plain: Value =
            serde_json::from_str(&failure_response(None, PARSE_ERROR, "bad").into_wire()).unwrap();
        assert!(plain["error"].get("data").is_none());
    }
}
