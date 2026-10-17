//! Minimal stable Codex app-server JSONL protocol boundary.
//! The runtime adapter owns this transport; Core never sees provider RPC.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct RpcRequest<'a> {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub method: &'a str,
    pub params: Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RpcResponse {
    pub id: Option<u64>,
    pub result: Option<Value>,
    pub error: Option<RpcError>,
}

pub fn encode_response(id: u64, result: Value) -> Result<Vec<u8>, serde_json::Error> {
    let mut line = serde_json::to_vec(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    }))?;
    line.push(b'\n');
    Ok(line)
}

#[derive(Debug, Clone, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default)]
    pub data: Option<Value>,
}

pub fn encode_request(id: u64, method: &str, params: Value) -> Result<Vec<u8>, serde_json::Error> {
    let mut line = serde_json::to_vec(&RpcRequest {
        jsonrpc: "2.0",
        id,
        method,
        params,
    })?;
    line.push(b'\n');
    Ok(line)
}

pub fn decode_message(line: &[u8]) -> Result<RpcResponse, serde_json::Error> {
    serde_json::from_slice(line)
}

pub fn initialize_params(version: &str) -> Value {
    serde_json::json!({"clientInfo":{"name":"relay","title":"Relay","version":version}})
}

pub fn thread_start_params(cwd: &Path) -> Value {
    serde_json::json!({"cwd":cwd})
}

pub fn account_read_params() -> Value {
    serde_json::json!({})
}

pub fn rate_limits_read_params() -> Value {
    serde_json::json!({})
}

pub fn config_requirements_read_params() -> Value {
    serde_json::json!({})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_jsonl_request() {
        let bytes = encode_request(1, "initialize", serde_json::json!({"name":"relay"})).unwrap();
        assert_eq!(bytes.last(), Some(&b'\n'));
        let response = decode_message(br#"{"jsonrpc":"2.0","id":1,"result":{}}"#).unwrap();
        assert_eq!(response.id, Some(1));
        assert!(response.error.is_none());
        let response = encode_response(7, serde_json::json!({"decision":"accept"})).unwrap();
        assert!(String::from_utf8(response).unwrap().contains("\"id\":7"));
        assert!(account_read_params().is_object());
        assert!(rate_limits_read_params().is_object());
        assert!(config_requirements_read_params().is_object());
    }
}
