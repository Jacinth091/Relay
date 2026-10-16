//! Herdr transport client and protocol types.
//!
//! Provides JSON-RPC communication, capability negotiation,
//! and mock/IPC transport implementations.

use std::{
    collections::HashMap,
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum HerdrClientError {
    #[error("transport error: {0}")]
    Transport(String),
    #[error(
        "incompatible protocol version: server reported {server_version}, expected minimum {expected_min}"
    )]
    IncompatibleVersion {
        server_version: String,
        expected_min: String,
    },
    #[error("missing required capability: {0}")]
    MissingCapability(String),
    #[error("RPC error ({code}): {message}")]
    RpcError { code: i64, message: String },
    #[error("resource not found: {0}")]
    ResourceNotFound(String),
    #[error("external user termination detected on pane: {0}")]
    ExternalUserTermination(String),
    #[error("RPC call timed out: {0}")]
    Timeout(String),
    #[error("mismatched resource metadata: {0}")]
    MismatchedMetadata(String),
}

/// Capability report returned during startup handshake.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HerdrCapabilities {
    pub protocol_version: String,
    pub supported_methods: Vec<String>,
    pub supports_metadata: bool,
    pub supports_interrupt: bool,
    pub supports_pty_streaming: bool,
    pub supports_activity_detection: bool,
}

impl Default for HerdrCapabilities {
    fn default() -> Self {
        Self {
            protocol_version: "1.0.0".to_string(),
            supported_methods: vec![
                "system.capabilities".into(),
                "workspace.ensure".into(),
                "tab.ensure".into(),
                "pane.spawn".into(),
                "pane.get".into(),
                "pane.list".into(),
                "pane.send_input".into(),
                "pane.interrupt".into(),
                "pane.dispose".into(),
                "pane.read_output".into(),
            ],
            supports_metadata: true,
            supports_interrupt: true,
            supports_pty_streaming: true,
            supports_activity_detection: true,
        }
    }
}

impl HerdrCapabilities {
    pub fn is_compatible(&self) -> Result<(), HerdrClientError> {
        // Must be version 1.x
        if !self.protocol_version.starts_with("1.") {
            return Err(HerdrClientError::IncompatibleVersion {
                server_version: self.protocol_version.clone(),
                expected_min: "1.0.0".to_string(),
            });
        }
        if !self.supports_metadata {
            return Err(HerdrClientError::MissingCapability(
                "supports_metadata".into(),
            ));
        }
        if !self.supports_interrupt {
            return Err(HerdrClientError::MissingCapability(
                "supports_interrupt".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HerdrPaneStatus {
    Starting,
    Running,
    Exited { code: i32 },
    TerminatedByUser,
    Lost,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HerdrPaneInfo {
    pub pane_id: String,
    pub tab_id: String,
    pub workspace_id: String,
    pub pid: u32,
    pub status: HerdrPaneStatus,
    pub metadata: HashMap<String, String>,
    pub user_attached: bool,
}

#[derive(Debug, Clone, Default)]
pub struct PaneOutputBuffer {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct HerdrOutputChunk {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub type HerdrFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, HerdrClientError>> + Send + 'a>>;

/// Transport abstraction for Herdr communication.
pub trait HerdrTransport: Send + Sync + 'static {
    fn call<'a>(
        &'a self,
        method: &'a str,
        params: serde_json::Value,
    ) -> HerdrFuture<'a, serde_json::Value>;
}

/// Scriptable mock transport representing a local or remote Herdr daemon.
pub struct MockHerdrTransport {
    pub capabilities: HerdrCapabilities,
    pub workspaces: Arc<Mutex<HashMap<String, String>>>,
    pub tabs: Arc<Mutex<HashMap<String, String>>>,
    pub panes: Arc<Mutex<HashMap<String, HerdrPaneInfo>>>,
    pub buffers: Arc<Mutex<HashMap<String, PaneOutputBuffer>>>,
    pub next_pid: Arc<Mutex<u32>>,
    pub is_alive: Arc<AtomicBool>,
    pub fail_next_calls: Arc<Mutex<usize>>,
}

impl Default for MockHerdrTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl MockHerdrTransport {
    pub fn new() -> Self {
        Self {
            capabilities: HerdrCapabilities::default(),
            workspaces: Arc::new(Mutex::new(HashMap::new())),
            tabs: Arc::new(Mutex::new(HashMap::new())),
            panes: Arc::new(Mutex::new(HashMap::new())),
            buffers: Arc::new(Mutex::new(HashMap::new())),
            next_pid: Arc::new(Mutex::new(5000)),
            is_alive: Arc::new(AtomicBool::new(true)),
            fail_next_calls: Arc::new(Mutex::new(0)),
        }
    }

    pub fn with_capabilities(capabilities: HerdrCapabilities) -> Self {
        let mut s = Self::new();
        s.capabilities = capabilities;
        s
    }

    pub fn set_call_failures(&self, count: usize) {
        *self.fail_next_calls.lock().unwrap() = count;
    }

    pub fn simulate_external_termination(&self, pane_id: &str) {
        let mut panes = self.panes.lock().unwrap();
        if let Some(pane) = panes.get_mut(pane_id) {
            pane.status = HerdrPaneStatus::TerminatedByUser;
        }
    }

    pub fn simulate_user_attach(&self, pane_id: &str, attached: bool) {
        let mut panes = self.panes.lock().unwrap();
        if let Some(pane) = panes.get_mut(pane_id) {
            pane.user_attached = attached;
        }
    }

    pub fn simulate_pane_output(&self, pane_id: &str, stdout: &[u8], stderr: &[u8]) {
        let mut buffers = self.buffers.lock().unwrap();
        let entry = buffers.entry(pane_id.to_string()).or_default();
        entry.stdout.extend_from_slice(stdout);
        entry.stderr.extend_from_slice(stderr);
    }

    pub fn simulate_pane_exit(&self, pane_id: &str, exit_code: i32) {
        let mut panes = self.panes.lock().unwrap();
        if let Some(pane) = panes.get_mut(pane_id) {
            pane.status = HerdrPaneStatus::Exited { code: exit_code };
        }
    }

    pub fn simulate_server_restart(&self) {
        // Simulates Herdr crashing and restarting: all in-memory processes lost!
        self.panes.lock().unwrap().clear();
        self.buffers.lock().unwrap().clear();
    }

    pub fn corrupt_metadata(&self, pane_id: &str, new_tags: HashMap<String, String>) {
        let mut panes = self.panes.lock().unwrap();
        if let Some(pane) = panes.get_mut(pane_id) {
            pane.metadata = new_tags;
        }
    }
}

impl HerdrTransport for MockHerdrTransport {
    fn call<'a>(
        &'a self,
        method: &'a str,
        params: serde_json::Value,
    ) -> HerdrFuture<'a, serde_json::Value> {
        Box::pin(async move {
            if !self.is_alive.load(Ordering::SeqCst) {
                return Err(HerdrClientError::Transport(
                    "connection refused: Herdr server down".into(),
                ));
            }

            let mut fails = self.fail_next_calls.lock().unwrap();
            if *fails > 0 {
                *fails -= 1;
                return Err(HerdrClientError::Transport(
                    "simulated transient transport error".into(),
                ));
            }
            drop(fails);

            match method {
                "system.capabilities" => Ok(serde_json::to_value(&self.capabilities).unwrap()),
                "workspace.ensure" => {
                    let id = params["id"]
                        .as_str()
                        .ok_or_else(|| HerdrClientError::RpcError {
                            code: -32602,
                            message: "missing workspace id".into(),
                        })?;
                    let name = params["name"].as_str().unwrap_or(id);
                    self.workspaces
                        .lock()
                        .unwrap()
                        .insert(id.to_string(), name.to_string());
                    Ok(serde_json::json!({ "workspace_id": id }))
                }
                "tab.ensure" => {
                    let id = params["id"]
                        .as_str()
                        .ok_or_else(|| HerdrClientError::RpcError {
                            code: -32602,
                            message: "missing tab id".into(),
                        })?;
                    let title = params["title"].as_str().unwrap_or(id);
                    self.tabs
                        .lock()
                        .unwrap()
                        .insert(id.to_string(), title.to_string());
                    Ok(serde_json::json!({ "tab_id": id }))
                }
                "pane.spawn" => {
                    let pane_id =
                        params["pane_id"]
                            .as_str()
                            .ok_or_else(|| HerdrClientError::RpcError {
                                code: -32602,
                                message: "missing pane_id".into(),
                            })?;
                    let tab_id = params["tab_id"].as_str().unwrap_or("default");
                    let workspace_id = params["workspace_id"].as_str().unwrap_or("default");

                    let metadata: HashMap<String, String> = params["metadata"]
                        .as_object()
                        .map(|m| {
                            m.iter()
                                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                                .collect()
                        })
                        .unwrap_or_default();

                    let mut next_pid = self.next_pid.lock().unwrap();
                    let pid = *next_pid;
                    *next_pid += 1;
                    drop(next_pid);

                    let info = HerdrPaneInfo {
                        pane_id: pane_id.to_string(),
                        tab_id: tab_id.to_string(),
                        workspace_id: workspace_id.to_string(),
                        pid,
                        status: HerdrPaneStatus::Running,
                        metadata,
                        user_attached: false,
                    };

                    self.panes
                        .lock()
                        .unwrap()
                        .insert(pane_id.to_string(), info.clone());
                    Ok(serde_json::to_value(&info).unwrap())
                }
                "pane.get" => {
                    let pane_id =
                        params["pane_id"]
                            .as_str()
                            .ok_or_else(|| HerdrClientError::RpcError {
                                code: -32602,
                                message: "missing pane_id".into(),
                            })?;
                    let panes = self.panes.lock().unwrap();
                    if let Some(pane) = panes.get(pane_id) {
                        Ok(serde_json::to_value(pane).unwrap())
                    } else {
                        Err(HerdrClientError::ResourceNotFound(format!(
                            "pane not found: {}",
                            pane_id
                        )))
                    }
                }
                "pane.list" => {
                    let panes = self.panes.lock().unwrap();
                    let list: Vec<HerdrPaneInfo> = panes.values().cloned().collect();
                    Ok(serde_json::to_value(list).unwrap())
                }
                "pane.send_input" => {
                    let pane_id =
                        params["pane_id"]
                            .as_str()
                            .ok_or_else(|| HerdrClientError::RpcError {
                                code: -32602,
                                message: "missing pane_id".into(),
                            })?;
                    let panes = self.panes.lock().unwrap();
                    if !panes.contains_key(pane_id) {
                        return Err(HerdrClientError::ResourceNotFound(pane_id.to_string()));
                    }
                    Ok(serde_json::json!({ "sent": true }))
                }
                "pane.interrupt" => {
                    let pane_id =
                        params["pane_id"]
                            .as_str()
                            .ok_or_else(|| HerdrClientError::RpcError {
                                code: -32602,
                                message: "missing pane_id".into(),
                            })?;
                    let mut panes = self.panes.lock().unwrap();
                    if let Some(pane) = panes.get_mut(pane_id) {
                        pane.status = HerdrPaneStatus::Exited { code: 130 }; // SIGINT standard code
                        Ok(serde_json::json!({ "interrupted": true }))
                    } else {
                        Err(HerdrClientError::ResourceNotFound(pane_id.to_string()))
                    }
                }
                "pane.dispose" => {
                    let pane_id =
                        params["pane_id"]
                            .as_str()
                            .ok_or_else(|| HerdrClientError::RpcError {
                                code: -32602,
                                message: "missing pane_id".into(),
                            })?;
                    let mut panes = self.panes.lock().unwrap();
                    let reaped = panes.remove(pane_id).is_some();
                    self.buffers.lock().unwrap().remove(pane_id);
                    Ok(serde_json::json!({ "reaped": reaped }))
                }
                "pane.read_output" => {
                    let pane_id =
                        params["pane_id"]
                            .as_str()
                            .ok_or_else(|| HerdrClientError::RpcError {
                                code: -32602,
                                message: "missing pane_id".into(),
                            })?;
                    let mut buffers = self.buffers.lock().unwrap();
                    let buf = buffers.remove(pane_id).unwrap_or_default();
                    Ok(serde_json::json!({
                        "stdout": buf.stdout,
                        "stderr": buf.stderr,
                    }))
                }
                other => Err(HerdrClientError::RpcError {
                    code: -32601,
                    message: format!("method not found: {}", other),
                }),
            }
        })
    }
}

/// Client facade for interacting with Herdr daemon.
#[derive(Clone)]
pub struct HerdrClient {
    transport: Arc<dyn HerdrTransport>,
}

impl HerdrClient {
    pub fn new(transport: Arc<dyn HerdrTransport>) -> Self {
        Self { transport }
    }

    /// Negotiates capabilities and ensures version compatibility.
    pub async fn negotiate_capabilities(&self) -> Result<HerdrCapabilities, HerdrClientError> {
        let resp = self
            .transport
            .call("system.capabilities", serde_json::json!({}))
            .await?;
        let caps: HerdrCapabilities = serde_json::from_value(resp).map_err(|e| {
            HerdrClientError::Transport(format!("invalid capabilities response: {}", e))
        })?;
        caps.is_compatible()?;
        Ok(caps)
    }

    pub async fn ensure_workspace(
        &self,
        id: &str,
        name: &str,
        path: Option<&PathBuf>,
    ) -> Result<(), HerdrClientError> {
        self.transport
            .call(
                "workspace.ensure",
                serde_json::json!({
                    "id": id,
                    "name": name,
                    "path": path,
                }),
            )
            .await?;
        Ok(())
    }

    pub async fn ensure_tab(
        &self,
        id: &str,
        workspace_id: &str,
        title: &str,
    ) -> Result<(), HerdrClientError> {
        self.transport
            .call(
                "tab.ensure",
                serde_json::json!({
                    "id": id,
                    "workspace_id": workspace_id,
                    "title": title,
                }),
            )
            .await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn spawn_pane(
        &self,
        pane_id: &str,
        tab_id: &str,
        workspace_id: &str,
        executable: &PathBuf,
        args: &[String],
        cwd: &PathBuf,
        env: &HashMap<String, String>,
        metadata: &HashMap<String, String>,
    ) -> Result<HerdrPaneInfo, HerdrClientError> {
        let resp = self
            .transport
            .call(
                "pane.spawn",
                serde_json::json!({
                    "pane_id": pane_id,
                    "tab_id": tab_id,
                    "workspace_id": workspace_id,
                    "executable": executable,
                    "args": args,
                    "cwd": cwd,
                    "env": env,
                    "metadata": metadata,
                }),
            )
            .await?;

        serde_json::from_value(resp)
            .map_err(|e| HerdrClientError::Transport(format!("invalid pane spawn response: {}", e)))
    }

    pub async fn get_pane(&self, pane_id: &str) -> Result<HerdrPaneInfo, HerdrClientError> {
        let resp = self
            .transport
            .call("pane.get", serde_json::json!({ "pane_id": pane_id }))
            .await?;
        serde_json::from_value(resp)
            .map_err(|e| HerdrClientError::Transport(format!("invalid pane get response: {}", e)))
    }

    pub async fn list_panes(&self) -> Result<Vec<HerdrPaneInfo>, HerdrClientError> {
        let resp = self
            .transport
            .call("pane.list", serde_json::json!({}))
            .await?;
        serde_json::from_value(resp)
            .map_err(|e| HerdrClientError::Transport(format!("invalid pane list response: {}", e)))
    }

    pub async fn send_input(&self, pane_id: &str, input: &[u8]) -> Result<(), HerdrClientError> {
        self.transport
            .call(
                "pane.send_input",
                serde_json::json!({
                    "pane_id": pane_id,
                    "input": input,
                }),
            )
            .await?;
        Ok(())
    }

    pub async fn interrupt_pane(&self, pane_id: &str) -> Result<(), HerdrClientError> {
        self.transport
            .call("pane.interrupt", serde_json::json!({ "pane_id": pane_id }))
            .await?;
        Ok(())
    }

    pub async fn dispose_pane(&self, pane_id: &str) -> Result<bool, HerdrClientError> {
        let resp = self
            .transport
            .call("pane.dispose", serde_json::json!({ "pane_id": pane_id }))
            .await?;
        Ok(resp["reaped"].as_bool().unwrap_or(false))
    }

    pub async fn read_output(&self, pane_id: &str) -> Result<HerdrOutputChunk, HerdrClientError> {
        let resp = self
            .transport
            .call(
                "pane.read_output",
                serde_json::json!({ "pane_id": pane_id }),
            )
            .await?;
        let stdout: Vec<u8> = serde_json::from_value(resp["stdout"].clone()).unwrap_or_default();
        let stderr: Vec<u8> = serde_json::from_value(resp["stderr"].clone()).unwrap_or_default();
        Ok(HerdrOutputChunk { stdout, stderr })
    }
}
