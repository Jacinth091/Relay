//! Codex CLI adapter for Relay's provider-neutral runtime boundary.
//!
//! All executable selection is trusted local configuration. Task content is
//! supplied only on stdin and is never interpreted as shell syntax.

pub mod app_server;

#[cfg(unix)]
use std::os::unix::process::CommandExt as _;
use std::{
    env,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

use relay_runtime::{
    CancellationToken, CapabilityStatus, EffectiveRuntimeProfile, FileChangeKind,
    FilesystemCapability, ReasoningEffort, Runtime, RuntimeAction, RuntimeActivity,
    RuntimeCapabilities, RuntimeCapabilityContext, RuntimeCapabilityReport, RuntimeConfigError,
    RuntimeDescriptor, RuntimeError, RuntimeEvent, RuntimeIdentity, RuntimeIntegration,
    RuntimeModelCatalog, RuntimeModelInfo, RuntimeOutcome, RuntimeOutcomeKind, RuntimeOutputStream,
    RuntimeProbe, RuntimeProfile, RuntimeProfileDescriptor, RuntimeRequest, RuntimeSession,
    RuntimeStartFuture, RuntimeText, RuntimeTextFormat,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::Command,
    sync::mpsc,
    time::timeout,
};
use uuid::Uuid;

const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_CODEX_JSONL_FRAME_BYTES: usize = 256 * 1024;
const MAX_CODEX_RESULT_BYTES: u64 = 64 * 1024;
const MAX_CODEX_RESULT_TEXT_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexRuntimeConfig {
    pub executable: PathBuf,
    pub profile_id: String,
    pub model_override: Option<String>,
}

impl Default for CodexRuntimeConfig {
    fn default() -> Self {
        let configured = env::var_os("RELAY_CODEX_EXECUTABLE");
        Self {
            executable: configured
                .clone()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("codex")),
            profile_id: "codex/default".to_owned(),
            model_override: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexRuntimeHealth {
    Checking,
    Available {
        version: String,
        account: Option<String>,
    },
    NotInstalled {
        error: String,
    },
    NotAuthenticated {
        version: Option<String>,
        error: String,
    },
    Incompatible {
        version: String,
        reason: String,
    },
    DiscoveryFailed {
        error: String,
    },
}

impl CodexRuntimeHealth {
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Available { .. })
    }

    pub fn display_status(&self) -> &'static str {
        match self {
            Self::Checking => "Codex health check pending",
            Self::Available { .. } => "Codex available",
            Self::NotInstalled { .. } => "Codex not installed",
            Self::NotAuthenticated { .. } => "Codex not authenticated",
            Self::Incompatible { .. } => "Codex CLI incompatible",
            Self::DiscoveryFailed { .. } => "Codex discovery failed",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CodexRuntime {
    config: CodexRuntimeConfig,
}

impl CodexRuntime {
    pub fn new(config: CodexRuntimeConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &CodexRuntimeConfig {
        &self.config
    }

    pub fn resolve_executable(&self) -> Result<PathBuf, String> {
        let explicit = env::var_os("RELAY_CODEX_EXECUTABLE").is_some()
            || self.config.executable != Path::new("codex");
        resolve_executable(&self.config.executable, explicit)
    }

    /// Checks CLI availability and existing CLI authentication without starting an agent turn.
    pub async fn discover(&self) -> CodexRuntimeHealth {
        let executable = match self.resolve_executable() {
            Ok(executable) => executable,
            Err(error) => return CodexRuntimeHealth::NotInstalled { error },
        };
        let version = match discover_command(&executable, &["--version"]).await {
            Ok(output) if output.status.success() => {
                String::from_utf8_lossy(&output.stdout).trim().to_owned()
            }
            Ok(output) => {
                return CodexRuntimeHealth::DiscoveryFailed {
                    error: safe_process_error(&output.stderr, "codex --version returned a failure"),
                };
            }
            Err(DiscoveryError::Spawn(error)) => {
                return CodexRuntimeHealth::DiscoveryFailed { error };
            }
            Err(DiscoveryError::Timeout) => {
                return CodexRuntimeHealth::DiscoveryFailed {
                    error: "codex --version timed out".to_owned(),
                };
            }
        };
        match discover_command(&executable, &["login", "status"]).await {
            Ok(output) if output.status.success() => {}
            Ok(output) => {
                return CodexRuntimeHealth::NotAuthenticated {
                    version: Some(version),
                    error: safe_process_error(
                        &output.stderr,
                        "codex login status reported no active login",
                    ),
                };
            }
            Err(DiscoveryError::Spawn(error)) => {
                return CodexRuntimeHealth::DiscoveryFailed { error };
            }
            Err(DiscoveryError::Timeout) => {
                return CodexRuntimeHealth::DiscoveryFailed {
                    error: "codex login status timed out".to_owned(),
                };
            }
        };
        match discover_command(
            &executable,
            &["--ask-for-approval", "never", "exec", "--help"],
        )
        .await
        {
            Ok(output) if output.status.success() && required_exec_flags_present(&output) => {
                let account = self.discover_account_identity().await;
                CodexRuntimeHealth::Available { version, account }
            }
            Ok(output) if output.status.success() => CodexRuntimeHealth::Incompatible {
                version,
                reason: "installed Codex CLI is missing required M8 exec flags".to_owned(),
            },
            Ok(output) => CodexRuntimeHealth::Incompatible {
                version,
                reason: safe_process_error(&output.stderr, "Codex exec compatibility probe failed"),
            },
            Err(DiscoveryError::Spawn(error)) => CodexRuntimeHealth::DiscoveryFailed { error },
            Err(DiscoveryError::Timeout) => CodexRuntimeHealth::DiscoveryFailed {
                error: "Codex exec compatibility probe timed out".to_owned(),
            },
        }
    }

    async fn discover_account_identity(&self) -> Option<String> {
        let executable = self.resolve_executable().ok()?;
        let output = discover_command(&executable, &["login", "status"])
            .await
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        text.lines()
            .map(str::trim)
            .find(|line| line.contains('@'))
            .map(str::to_owned)
    }

    /// Read a provider-owned app-server diagnostic without starting an agent turn.
    pub async fn app_server_read(&self, method: &str) -> Result<Value, String> {
        let executable = self.resolve_executable()?;
        query_app_server(&executable, method, serde_json::json!({})).await
    }

    pub fn command_spec(
        &self,
        request: &RuntimeRequest,
        schema_path: &Path,
        result_path: &Path,
    ) -> CodexCommandSpec {
        self.command_spec_with_executable(
            self.config.executable.clone(),
            request,
            schema_path,
            result_path,
        )
    }

    fn command_spec_with_executable(
        &self,
        executable: PathBuf,
        request: &RuntimeRequest,
        schema_path: &Path,
        result_path: &Path,
    ) -> CodexCommandSpec {
        let sandbox_mode = if request.profile_id.ends_with("/review") {
            "read-only"
        } else {
            "workspace-write"
        };

        let mut args = vec![
            "--ask-for-approval".to_owned(),
            "never".to_owned(),
            "exec".to_owned(),
            "--ignore-user-config".to_owned(),
            "--ignore-rules".to_owned(),
            "--ephemeral".to_owned(),
            "--strict-config".to_owned(),
            "--json".to_owned(),
            "--color".to_owned(),
            "never".to_owned(),
            "--sandbox".to_owned(),
            sandbox_mode.to_owned(),
            "-c".to_owned(),
            "sandbox_workspace_write.writable_roots=[]".to_owned(),
            "-c".to_owned(),
            "sandbox_workspace_write.network_access=false".to_owned(),
            "--cd".to_owned(),
            request.trusted_working_dir.display().to_string(),
            "--output-schema".to_owned(),
            schema_path.display().to_string(),
            "--output-last-message".to_owned(),
            result_path.display().to_string(),
        ];
        let selected_model = request
            .runtime_selection
            .model
            .as_option()
            .or(self.config.model_override.as_deref());

        if let Some(model) = selected_model {
            args.extend(["--model".to_owned(), model.to_owned()]);
        }

        let selected_reasoning = request
            .runtime_selection
            .reasoning_effort
            .as_option()
            .or_else(|| {
                if request.profile_id.ends_with("/deep") {
                    Some(relay_runtime::ReasoningEffort::XHigh)
                } else if request.profile_id.ends_with("/review") {
                    Some(relay_runtime::ReasoningEffort::Medium)
                } else if request.profile_id.ends_with("/implement") {
                    Some(relay_runtime::ReasoningEffort::High)
                } else {
                    None
                }
            });

        if let Some(effort) = selected_reasoning {
            let effort_str = match effort {
                relay_runtime::ReasoningEffort::Default => None,
                relay_runtime::ReasoningEffort::Low => Some("low"),
                relay_runtime::ReasoningEffort::Medium => Some("medium"),
                relay_runtime::ReasoningEffort::High => Some("high"),
                relay_runtime::ReasoningEffort::XHigh => Some("xhigh"),
            };
            if let Some(str_val) = effort_str {
                args.extend([
                    "-c".to_owned(),
                    format!("model_reasoning_effort=\"{str_val}\""),
                ]);
            }
        }

        args.push("-".to_owned());
        CodexCommandSpec {
            executable,
            args,
            current_dir: request.trusted_working_dir.clone(),
        }
    }
}

impl Default for CodexRuntime {
    fn default() -> Self {
        Self::new(CodexRuntimeConfig::default())
    }
}

impl RuntimeIntegration for CodexRuntime {
    fn descriptor(&self) -> RuntimeDescriptor {
        RuntimeDescriptor {
            provider_id: "codex".to_owned(),
            display_name: "Codex CLI".to_owned(),
            description: "Native Codex CLI execution adapter".to_owned(),
            is_development_only: false,
            machine_interface_verified: true,
        }
    }

    fn probe(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = RuntimeProbe> + Send + '_>> {
        Box::pin(async move {
            let health = self.discover().await;
            match health {
                CodexRuntimeHealth::Available { version, account } => RuntimeProbe {
                    installed: true,
                    version: Some(version),
                    authenticated: true,
                    compatible: true,
                    account_identity: account,
                    notes: vec!["Codex available and authenticated".to_owned()],
                },
                CodexRuntimeHealth::NotAuthenticated { version, error } => RuntimeProbe {
                    installed: true,
                    version,
                    authenticated: false,
                    compatible: true,
                    account_identity: None,
                    notes: vec![format!("Not authenticated: {error}")],
                },
                CodexRuntimeHealth::Incompatible { version, reason } => RuntimeProbe {
                    installed: true,
                    version: Some(version),
                    authenticated: true,
                    compatible: false,
                    account_identity: None,
                    notes: vec![format!("Incompatible: {reason}")],
                },
                CodexRuntimeHealth::NotInstalled { error }
                | CodexRuntimeHealth::DiscoveryFailed { error } => RuntimeProbe {
                    installed: false,
                    version: None,
                    authenticated: false,
                    compatible: false,
                    account_identity: None,
                    notes: vec![error],
                },
                CodexRuntimeHealth::Checking => RuntimeProbe {
                    installed: false,
                    version: None,
                    authenticated: false,
                    compatible: false,
                    account_identity: None,
                    notes: vec!["Health check pending".to_owned()],
                },
            }
        })
    }

    fn profiles(&self) -> Vec<RuntimeProfileDescriptor> {
        vec![
            RuntimeProfileDescriptor {
                id: "codex/review".to_owned(),
                display_name: "Codex / Review".to_owned(),
                purpose: "Inspection and read-only task review".to_owned(),
            },
            RuntimeProfileDescriptor {
                id: "codex/implement".to_owned(),
                display_name: "Codex / Implement".to_owned(),
                purpose: "Normal code changes inside workspace".to_owned(),
            },
            RuntimeProfileDescriptor {
                id: "codex/deep".to_owned(),
                display_name: "Codex / Deep".to_owned(),
                purpose: "Deep reasoning for complex implementation tasks".to_owned(),
            },
        ]
    }

    fn models(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = RuntimeModelCatalog> + Send + '_>> {
        let runtime = self.clone();
        Box::pin(async move {
            if env::var("RELAY_CODEX_TRANSPORT").as_deref() != Ok("exec") {
                if let Ok(executable) = runtime.resolve_executable() {
                    if let Ok(catalog) = discover_app_server_models(&executable).await {
                        return catalog;
                    }
                }
            }
            RuntimeModelCatalog {
                default_model: "default".to_owned(),
                available_models: vec![RuntimeModelInfo {
                    id: "default".to_owned(),
                    display_name: "Default (Account/CLI Setting)".to_owned(),
                    supported_reasoning_efforts: vec![],
                    default_reasoning_effort: None,
                }],
            }
        })
    }

    fn resolve_profile(
        &self,
        profile_id: &str,
    ) -> Result<EffectiveRuntimeProfile, RuntimeConfigError> {
        let profile = match profile_id {
            "codex/review" => RuntimeProfile {
                id: "codex/review".to_owned(),
                provider: "codex".to_owned(),
                display_name: "Codex / Review".to_owned(),
                model: "default".to_owned(),
                reasoning_effort: ReasoningEffort::Medium,
                filesystem: FilesystemCapability::ReadOnly,
                network: false,
                extra_writable_roots: vec![],
                max_duration: Duration::from_secs(20 * 60),
                max_repair_attempts: 1,
                relay_approval_owner: true,
            },
            "codex/implement" | "codex/default" => RuntimeProfile {
                id: "codex/implement".to_owned(),
                provider: "codex".to_owned(),
                display_name: "Codex / Implement".to_owned(),
                model: "default".to_owned(),
                reasoning_effort: ReasoningEffort::High,
                filesystem: FilesystemCapability::WorkspaceWrite,
                network: false,
                extra_writable_roots: vec![],
                max_duration: Duration::from_secs(30 * 60),
                max_repair_attempts: 2,
                relay_approval_owner: true,
            },
            "codex/deep" => RuntimeProfile {
                id: "codex/deep".to_owned(),
                provider: "codex".to_owned(),
                display_name: "Codex / Deep".to_owned(),
                model: "default".to_owned(),
                reasoning_effort: ReasoningEffort::XHigh,
                filesystem: FilesystemCapability::WorkspaceWrite,
                network: false,
                extra_writable_roots: vec![],
                max_duration: Duration::from_secs(60 * 60),
                max_repair_attempts: 3,
                relay_approval_owner: true,
            },
            _ => {
                return Err(RuntimeConfigError::UnknownProfile {
                    profile_id: profile_id.to_owned(),
                });
            }
        };
        Ok(EffectiveRuntimeProfile {
            profile,
            provider_config_snapshot: Some(format!(
                "executable={}",
                self.config.executable.display()
            )),
        })
    }

    fn verify_capabilities(
        &self,
        profile: &EffectiveRuntimeProfile,
        context: &RuntimeCapabilityContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = RuntimeCapabilityReport> + Send + '_>>
    {
        let is_dir = context.working_directory.is_dir();
        let path_str = context.working_directory.display().to_string();
        let fs_write_status = match profile.profile.filesystem {
            FilesystemCapability::ReadOnly => CapabilityStatus::Configured,
            FilesystemCapability::WorkspaceWrite => {
                if is_dir {
                    CapabilityStatus::Verified
                } else {
                    CapabilityStatus::Mismatch
                }
            }
        };
        Box::pin(async move {
            RuntimeCapabilityReport {
                filesystem_read: CapabilityStatus::Verified,
                filesystem_write: fs_write_status,
                command_execution: CapabilityStatus::Verified,
                network: CapabilityStatus::Configured,
                notes: vec![format!("Working directory: {path_str}")],
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexCommandSpec {
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub current_dir: PathBuf,
}

impl Runtime for CodexRuntime {
    fn identity(&self) -> RuntimeIdentity {
        RuntimeIdentity {
            runtime_id: "codex".to_owned(),
            profile_id: self.config.profile_id.clone(),
            display_name: "Codex CLI".to_owned(),
        }
    }

    fn capabilities(&self) -> RuntimeCapabilities {
        RuntimeCapabilities {
            streaming_output: true,
            cancellation: true,
            deterministic_outcome: true,
            permission_support: if env::var("RELAY_CODEX_TRANSPORT").as_deref() != Ok("exec") {
                relay_runtime::RuntimePermissionSupport::LiveApproval
            } else {
                relay_runtime::RuntimePermissionSupport::PreapprovedOnly
            },
        }
    }

    fn start(
        &self,
        request: RuntimeRequest,
        cancellation: CancellationToken,
    ) -> RuntimeStartFuture {
        let runtime = self.clone();
        Box::pin(async move {
            if env::var("RELAY_CODEX_TRANSPORT").as_deref() != Ok("exec") {
                return start_app_server(runtime, request, cancellation).await;
            }
            if cancellation.is_cancelled() {
                return Err(RuntimeError::StartFailed(
                    "execution cancelled before Codex start".to_owned(),
                ));
            }
            if let Some(stack) = request.intent.requested_stack.as_deref()
                && stack != "chatgpt-codex"
            {
                return Err(RuntimeError::StartFailed(format!(
                    "unsupported runtime stack '{stack}'"
                )));
            }
            let temp = RuntimeTempFiles::create()?;
            let executable = runtime
                .resolve_executable()
                .map_err(RuntimeError::StartFailed)?;
            let spec = runtime.command_spec_with_executable(
                executable,
                &request,
                &temp.schema_path,
                &temp.result_path,
            );
            let mut command = launcher_command(&spec.executable, &spec.args);
            command
                .current_dir(&spec.current_dir)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            #[cfg(unix)]
            unsafe {
                command.pre_exec(|| {
                    if libc::setpgid(0, 0) == 0 {
                        Ok(())
                    } else {
                        Err(std::io::Error::last_os_error())
                    }
                });
            }
            sanitize_git_environment(&mut command);
            let mut child = command.spawn().map_err(|error| {
                RuntimeError::StartFailed(format!(
                    "could not start configured Codex executable '{}': {error}",
                    spec.executable.display()
                ))
            })?;
            let mut stdin = child.stdin.take().ok_or_else(|| {
                RuntimeError::StartFailed("Codex stdin was unavailable".to_owned())
            })?;
            let stdout = child.stdout.take().ok_or_else(|| {
                RuntimeError::StartFailed("Codex stdout was unavailable".to_owned())
            })?;
            let stderr = child.stderr.take().ok_or_else(|| {
                RuntimeError::StartFailed("Codex stderr was unavailable".to_owned())
            })?;
            let prompt = build_prompt(&request);
            stdin.write_all(prompt.as_bytes()).await.map_err(|error| {
                RuntimeError::StartFailed(format!("could not write Codex prompt: {error}"))
            })?;
            stdin.shutdown().await.map_err(|error| {
                RuntimeError::StartFailed(format!("could not close Codex stdin: {error}"))
            })?;
            let (tx, rx) = mpsc::channel(32);
            tokio::spawn(async move {
                run_child(child, stdout, stderr, tx, cancellation, temp).await;
            });
            Ok(RuntimeSession {
                session_id: format!("codex-{}", request.execution_id),
                events: rx,
                control: None,
            })
        })
    }
}

async fn start_app_server(
    runtime: CodexRuntime,
    request: RuntimeRequest,
    cancellation: CancellationToken,
) -> Result<RuntimeSession, RuntimeError> {
    let executable = runtime
        .resolve_executable()
        .map_err(RuntimeError::StartFailed)?;
    let mut command = launcher_command(
        &executable,
        &[
            "app-server".to_owned(),
            "--listen".to_owned(),
            "stdio://".to_owned(),
        ],
    );
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    sanitize_git_environment(&mut command);
    let mut child = command
        .spawn()
        .map_err(|e| RuntimeError::StartFailed(format!("could not start Codex app-server: {e}")))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| RuntimeError::StartFailed("app-server stdin unavailable".to_owned()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| RuntimeError::StartFailed("app-server stdout unavailable".to_owned()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| RuntimeError::StartFailed("app-server stderr unavailable".to_owned()))?;
    let (tx, rx) = mpsc::channel(64);
    let (control_tx, mut control_rx) = mpsc::channel(16);
    let tx_stderr = tx.clone();
    tokio::spawn(async move {
        let _ = read_stderr(stderr, tx_stderr).await;
    });
    let cwd = request.trusted_working_dir.clone();
    let prompt = build_prompt(&request);
    tokio::spawn(async move {
        let mut reader = BufReader::new(stdout).lines();
        let mut next_id = 1_u64;
        if write_rpc(
            &mut stdin,
            next_id,
            "initialize",
            app_server::initialize_params(env!("CARGO_PKG_VERSION")),
        )
        .await
        .is_err()
        {
            return;
        }
        next_id += 1;
        if read_rpc_response(&mut reader, next_id - 1).await.is_err() {
            let _ = tx
                .send(RuntimeEvent::InvalidOutput {
                    reason: "Codex app-server initialize failed".to_owned(),
                })
                .await;
            return;
        }
        let _ = stdin
            .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"initialized\",\"params\":{}}\n")
            .await;
        next_id += 1;
        if write_rpc(
            &mut stdin,
            next_id,
            "configRequirements/read",
            serde_json::json!({}),
        )
        .await
        .is_err()
        {
            let _ = tx
                .send(RuntimeEvent::InvalidOutput {
                    reason: "Codex app-server config requirements probe failed".to_owned(),
                })
                .await;
            terminate_process_tree(&mut child).await;
            return;
        }
        let requirements = match read_rpc_response(&mut reader, next_id).await {
            Ok(value) => value,
            Err(error) => {
                let _ = tx
                    .send(RuntimeEvent::InvalidOutput {
                        reason: format!("Codex managed configuration could not be read: {error}"),
                    })
                    .await;
                terminate_process_tree(&mut child).await;
                return;
            }
        };
        let sandbox_name = if request.profile_id.ends_with("/review") {
            "readOnly"
        } else {
            "workspaceWrite"
        };
        if let Some(modes) = requirements
            .pointer("/result/requirements/allowedSandboxModes")
            .and_then(Value::as_array)
            && !modes
                .iter()
                .filter_map(Value::as_str)
                .any(|mode| mode.eq_ignore_ascii_case(sandbox_name))
        {
            let _ = tx.send(RuntimeEvent::InvalidOutput { reason: format!("Relay profile is incompatible with managed sandbox requirements ({sandbox_name})") }).await;
            terminate_process_tree(&mut child).await;
            return;
        }
        if let Some(policies) = requirements
            .pointer("/result/requirements/allowedApprovalPolicies")
            .and_then(Value::as_array)
            && !policies
                .iter()
                .filter_map(Value::as_str)
                .any(|p| p.eq_ignore_ascii_case("never") || p.eq_ignore_ascii_case("always"))
        {
            let _ = tx.send(RuntimeEvent::InvalidOutput { reason: "Relay requires 'never' or 'always' approval policy support in Codex managed requirements".to_owned() }).await;
            terminate_process_tree(&mut child).await;
            return;
        }
        if write_rpc(
            &mut stdin,
            next_id,
            "thread/start",
            app_server::thread_start_params(&cwd),
        )
        .await
        .is_err()
        {
            return;
        }
        let thread_response = match read_rpc_response(&mut reader, next_id).await {
            Ok(value) => value,
            Err(error) => {
                let _ = tx
                    .send(RuntimeEvent::InvalidOutput {
                        reason: format!("Codex app-server thread/start failed: {error}"),
                    })
                    .await;
                return;
            }
        };
        let thread_id = thread_response
            .pointer("/result/thread/id")
            .and_then(Value::as_str)
            .or_else(|| {
                thread_response
                    .pointer("/result/id")
                    .and_then(Value::as_str)
            })
            .unwrap_or("")
            .to_owned();
        if thread_id.is_empty() {
            let _ = tx
                .send(RuntimeEvent::InvalidOutput {
                    reason: "Codex app-server returned no thread id".to_owned(),
                })
                .await;
            return;
        }
        next_id += 1;
        let sandbox_policy = if request.profile_id.ends_with("/review") {
            serde_json::json!({"type":"readOnly"})
        } else {
            serde_json::json!({"type":"workspaceWrite", "writableRoots":[cwd], "networkAccess":false})
        };
        let effort_str = request
            .runtime_selection
            .reasoning_effort
            .as_option()
            .map(|e| match e {
                relay_runtime::ReasoningEffort::Default => "default",
                relay_runtime::ReasoningEffort::Low => "low",
                relay_runtime::ReasoningEffort::Medium => "medium",
                relay_runtime::ReasoningEffort::High => "high",
                relay_runtime::ReasoningEffort::XHigh => "xhigh",
            });
        if write_rpc(&mut stdin, next_id, "turn/start", serde_json::json!({"threadId":thread_id, "input": [{"type": "text", "text": prompt}], "cwd":cwd, "model":request.runtime_selection.model.as_option(), "effort":effort_str, "sandboxPolicy":sandbox_policy})).await.is_err() { return; }

        let mut turn_id = None;
        loop {
            let line =
                match tokio::time::timeout(std::time::Duration::from_secs(15), reader.next_line())
                    .await
                {
                    Ok(Ok(Some(line))) => line,
                    _ => break,
                };
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if value.get("id").and_then(Value::as_u64) == Some(next_id) {
                turn_id = value
                    .pointer("/result/turn/id")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                break;
            } else if let Some(activity) = normalize_app_server_activity(&value) {
                let _ = tx.send(RuntimeEvent::Activity(activity)).await;
            }
        }
        loop {
            tokio::select! {
                _ = cancellation.cancelled() => { let _ = write_rpc(&mut stdin, next_id, "turn/interrupt", serde_json::json!({"threadId":thread_id, "turnId":turn_id})).await; break; }
                Some(control) = control_rx.recv() => {
                    if let relay_runtime::RuntimeControl::PermissionDecision { request_id, grant } = control {
                        if let Ok(id) = request_id.parse::<u64>() {
                            let decision_str = match grant.decision {
                                relay_runtime::RuntimePermissionDecision::AllowOnce => "accept",
                                relay_runtime::RuntimePermissionDecision::AllowForRun => "acceptForSession",
                                relay_runtime::RuntimePermissionDecision::Deny => "decline",
                                relay_runtime::RuntimePermissionDecision::Cancel => "cancel",
                            };
                            let scope_str = match grant.scope {
                                relay_runtime::RuntimePermissionScope::Once => "once",
                                relay_runtime::RuntimePermissionScope::Turn => "turn",
                                relay_runtime::RuntimePermissionScope::Session => "session",
                            };
                            if let Ok(line) = app_server::encode_response(id, serde_json::json!({
                                "decision": decision_str,
                                "scope": scope_str,
                                "grantedPaths": grant.granted_paths
                            })) {
                                let _ = stdin.write_all(&line).await;
                            }
                        }
                    }
                }
                line = reader.next_line() => {
                    let Ok(Some(line)) = line else { break; };
                    let Ok(value) = serde_json::from_str::<Value>(&line) else { let _ = tx.send(RuntimeEvent::InvalidOutput { reason: "invalid app-server JSONL".to_owned() }).await; break; };
                    if let Some(activity) = normalize_app_server_activity(&value) {
                        let _ = tx.send(RuntimeEvent::Activity(activity)).await;
                    }
                    let kind = value.get("method").or_else(|| value.get("type")).and_then(Value::as_str).unwrap_or("");
                    if kind.contains("requestApproval") {
                        let request_id = value.get("id").and_then(Value::as_u64).map(|id| id.to_string()).unwrap_or_else(|| "unknown".to_owned());
                        let permission_kind = if kind.contains("commandExecution") { relay_runtime::RuntimePermissionKind::CommandExecution } else if kind.contains("fileChange") { relay_runtime::RuntimePermissionKind::FileChange } else { relay_runtime::RuntimePermissionKind::Permissions };
                        let params = value.get("params");

                        let item_id = params.and_then(|p| p.get("itemId")).and_then(Value::as_str).map(str::to_owned);
                        let thread_id_val = params.and_then(|p| p.get("threadId")).and_then(Value::as_str).map(str::to_owned);
                        let turn_id_val = params.and_then(|p| p.get("turnId")).and_then(Value::as_str).map(str::to_owned);
                        let reason = params.and_then(|p| p.get("reason")).and_then(Value::as_str).map(str::to_owned);

                        let command = params.and_then(|p| p.get("command")).and_then(Value::as_str).map(str::to_owned);
                        let req_cwd = params.and_then(|p| p.get("cwd")).and_then(Value::as_str).map(PathBuf::from).or_else(|| Some(cwd.clone()));
                        let command_actions = params.and_then(|p| p.get("commandActions")).and_then(Value::as_array).map(|arr| arr.iter().filter_map(Value::as_str).map(str::to_owned).collect());
                        let network_approval_context = params.and_then(|p| p.get("networkApprovalContext")).and_then(Value::as_str).map(str::to_owned);

                        let grant_root = params.and_then(|p| p.get("grantRoot")).and_then(Value::as_str).map(PathBuf::from);
                        let environment_id = params.and_then(|p| p.get("environmentId")).and_then(Value::as_str).map(str::to_owned);

                        let paths = if permission_kind == relay_runtime::RuntimePermissionKind::FileChange {
                            params.and_then(|p| p.get("fileChange")).and_then(|p| p.get("paths")).and_then(Value::as_array).map(|paths| paths.iter().filter_map(Value::as_str).map(PathBuf::from).collect()).unwrap_or_default()
                        } else {
                            vec![]
                        };

                        let filesystem_perm = params.and_then(|p| p.get("filesystem")).and_then(Value::as_bool).unwrap_or(permission_kind == relay_runtime::RuntimePermissionKind::FileChange);
                        let network_perm = params.and_then(|p| p.get("network")).and_then(Value::as_bool).unwrap_or(network_approval_context.is_some());

                        let requested = relay_runtime::RuntimeRequestedPermissions {
                            workspace_write: filesystem_perm,
                            network: network_perm,
                            paths,
                        };

                        let available_decisions = params.and_then(|p| p.get("availableDecisions")).and_then(Value::as_array).map(|arr| {
                            arr.iter().filter_map(Value::as_str).filter_map(|s| {
                                match s {
                                    "accept" => Some(relay_runtime::RuntimePermissionDecisionKind::AllowOnce),
                                    "acceptForSession" => Some(relay_runtime::RuntimePermissionDecisionKind::AllowForRun),
                                    "decline" => Some(relay_runtime::RuntimePermissionDecisionKind::Deny),
                                    "cancel" => Some(relay_runtime::RuntimePermissionDecisionKind::Cancel),
                                    _ => None,
                                }
                            }).collect()
                        }).unwrap_or_else(|| vec![
                            relay_runtime::RuntimePermissionDecisionKind::AllowOnce,
                            relay_runtime::RuntimePermissionDecisionKind::AllowForRun,
                            relay_runtime::RuntimePermissionDecisionKind::Deny,
                            relay_runtime::RuntimePermissionDecisionKind::Cancel,
                        ]);

                        let _ = tx.send(RuntimeEvent::PermissionRequested(relay_runtime::RuntimePermissionRequest {
                            request_id,
                            kind: permission_kind,
                            reason,
                            command,
                            cwd: req_cwd,
                            requested,
                            available_decisions,
                            item_id,
                            thread_id: thread_id_val,
                            turn_id: turn_id_val,
                            environment_id,
                            grant_root,
                            command_actions,
                            network_approval_context,
                        })).await;
                    } else if kind == "turn/completed" || kind == "result" {
                        let summary = value.pointer("/params/turn/summary").or_else(|| value.pointer("/params/summary")).and_then(Value::as_str).unwrap_or("Codex app-server turn completed");
                        let status = value.pointer("/params/turn/status").and_then(Value::as_str).unwrap_or("completed");

                        let outcome_kind = match status {
                            "interrupted" => RuntimeOutcomeKind::Cancelled,
                            "failed" => RuntimeOutcomeKind::RuntimeFailure,
                            _ => RuntimeOutcomeKind::Success,
                        };

                        let error_msg = value.pointer("/params/turn/error")
                            .or_else(|| value.pointer("/params/codexErrorInfo"))
                            .map(|v| v.to_string())
                            .unwrap_or_default();

                        let final_summary = if !error_msg.is_empty() {
                            format!("{summary}\nError details: {error_msg}")
                        } else {
                            summary.to_owned()
                        };

                        let _ = tx.send(RuntimeEvent::Outcome(RuntimeOutcome::new(outcome_kind, final_summary))).await; break;
                    } else if let Some(text) = value.get("text").and_then(Value::as_str).or_else(|| value.get("delta").and_then(Value::as_str)) {
                        let _ = tx.send(RuntimeEvent::Output { stream: RuntimeOutputStream::Stdout, bytes: format!("{text}\n").into_bytes() }).await;
                    }
                }
            }
        }
        terminate_process_tree(&mut child).await;
    });
    Ok(RuntimeSession {
        session_id: format!("codex-app-server-{}", request.execution_id),
        events: rx,
        control: Some(control_tx),
    })
}

async fn write_rpc(
    stdin: &mut tokio::process::ChildStdin,
    id: u64,
    method: &str,
    params: Value,
) -> Result<(), String> {
    let line = app_server::encode_request(id, method, params).map_err(|e| e.to_string())?;
    stdin.write_all(&line).await.map_err(|e| e.to_string())
}

async fn read_rpc_response<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut tokio::io::Lines<R>,
    expected_id: u64,
) -> Result<Value, String> {
    loop {
        let line = tokio::time::timeout(Duration::from_secs(15), reader.next_line())
            .await
            .map_err(|_| "app-server response timed out".to_owned())?
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "app-server closed before response".to_owned())?;
        let value: Value = serde_json::from_str(&line).map_err(|error| error.to_string())?;
        if value.get("id").and_then(Value::as_u64) == Some(expected_id) {
            if let Some(error) = value.get("error") {
                return Err(error.to_string());
            }
            return Ok(value);
        }
    }
}

async fn query_app_server(executable: &Path, method: &str, params: Value) -> Result<Value, String> {
    let mut command = launcher_command(
        executable,
        &[
            "app-server".to_owned(),
            "--listen".to_owned(),
            "stdio://".to_owned(),
        ],
    );
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "app-server stdin unavailable".to_owned())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "app-server stdout unavailable".to_owned())?;
    let mut reader = BufReader::new(stdout).lines();
    write_rpc(
        &mut stdin,
        1,
        "initialize",
        app_server::initialize_params(env!("CARGO_PKG_VERSION")),
    )
    .await?;
    let _ = read_rpc_response(&mut reader, 1).await?;
    stdin
        .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"initialized\",\"params\":{}}\n")
        .await
        .map_err(|error| error.to_string())?;
    write_rpc(&mut stdin, 2, method, params).await?;
    let result = read_rpc_response(&mut reader, 2).await?;
    terminate_process_tree(&mut child).await;
    Ok(result.get("result").cloned().unwrap_or(Value::Null))
}

async fn discover_app_server_models(executable: &Path) -> Result<RuntimeModelCatalog, String> {
    let mut command = launcher_command(
        executable,
        &[
            "app-server".to_owned(),
            "--listen".to_owned(),
            "stdio://".to_owned(),
        ],
    );
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "model probe stdin unavailable".to_owned())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "model probe stdout unavailable".to_owned())?;
    let mut reader = BufReader::new(stdout).lines();
    write_rpc(
        &mut stdin,
        1,
        "initialize",
        app_server::initialize_params(env!("CARGO_PKG_VERSION")),
    )
    .await?;
    let _ = read_rpc_response(&mut reader, 1).await?;
    stdin
        .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"initialized\",\"params\":{}}\n")
        .await
        .map_err(|e| e.to_string())?;
    let mut next_id = 2;
    let mut available_models = Vec::new();
    let mut default_model = "default".to_owned();
    let mut cursor: Option<String> = None;

    loop {
        let params = if let Some(c) = &cursor {
            serde_json::json!({"cursor": c})
        } else {
            serde_json::json!({})
        };

        write_rpc(&mut stdin, next_id, "model/list", params).await?;
        let response = read_rpc_response(&mut reader, next_id).await?;
        next_id += 1;

        let models = response
            .pointer("/result/data")
            .and_then(Value::as_array)
            .ok_or_else(|| "Codex model catalog incompatible".to_owned())?;

        for model in models {
            if model
                .get("hidden")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                continue;
            }
            let Some(id) = model.get("id").and_then(Value::as_str) else {
                continue;
            };
            let display_name = model
                .get("displayName")
                .and_then(Value::as_str)
                .unwrap_or(id);
            if model
                .get("isDefault")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                default_model = id.to_owned();
            }
            let supported_reasoning_efforts = model
                .get("supportedReasoningEfforts")
                .and_then(Value::as_array)
                .map(|efforts| {
                    efforts
                        .iter()
                        .filter_map(|effort| {
                            let text = if let Some(obj) = effort.as_object() {
                                obj.get("reasoningEffort").and_then(Value::as_str)
                            } else {
                                effort.as_str()
                            };
                            match text {
                                Some("low") => Some(ReasoningEffort::Low),
                                Some("medium") => Some(ReasoningEffort::Medium),
                                Some("high") => Some(ReasoningEffort::High),
                                Some("xhigh") => Some(ReasoningEffort::XHigh),
                                _ => None,
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();

            let default_reasoning_effort = model.get("defaultReasoningEffort").and_then(|v| {
                let text = if let Some(obj) = v.as_object() {
                    obj.get("reasoningEffort").and_then(Value::as_str)
                } else {
                    v.as_str()
                };
                match text {
                    Some("low") => Some(ReasoningEffort::Low),
                    Some("medium") => Some(ReasoningEffort::Medium),
                    Some("high") => Some(ReasoningEffort::High),
                    Some("xhigh") => Some(ReasoningEffort::XHigh),
                    _ => None,
                }
            });

            available_models.push(RuntimeModelInfo {
                id: id.to_owned(),
                display_name: display_name.to_owned(),
                supported_reasoning_efforts,
                default_reasoning_effort,
            });
        }

        if let Some(next_cursor) = response
            .pointer("/result/nextCursor")
            .and_then(Value::as_str)
        {
            cursor = Some(next_cursor.to_owned());
        } else {
            break;
        }
    }
    let _ = child.kill().await;
    if available_models.is_empty() {
        return Err("model/list returned no visible models".to_owned());
    }
    Ok(RuntimeModelCatalog {
        default_model,
        available_models,
    })
}

async fn terminate_process_tree(child: &mut tokio::process::Child) {
    #[cfg(windows)]
    if let Some(pid) = child.id() {
        let pid = pid.to_string();
        let mut taskkill = Command::new("taskkill");
        taskkill
            .args(["/T", "/F", "/PID", pid.as_str()])
            .kill_on_drop(true);
        let _ = taskkill.status().await;
    }
    #[cfg(not(windows))]
    {
        if let Some(pid) = child.id() {
            unsafe {
                libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
            }
        } else {
            let _ = child.kill().await;
        }
    }
    let _ = child.wait().await;
}

async fn run_child(
    mut child: tokio::process::Child,
    stdout: tokio::process::ChildStdout,
    stderr: tokio::process::ChildStderr,
    tx: mpsc::Sender<RuntimeEvent>,
    cancellation: CancellationToken,
    temp: RuntimeTempFiles,
) {
    let mut stdout_reader = tokio::spawn(read_stdout(stdout, tx.clone()));
    let mut stderr_reader = tokio::spawn(read_stderr(stderr, tx.clone()));
    let mut stdout_result = None;
    let mut stderr_result = None;
    let exit = loop {
        tokio::select! {
            status = child.wait() => break status.map_err(|error| error.to_string()),
            _ = cancellation.cancelled() => {
                terminate_process_tree(&mut child).await;
                stdout_reader.abort();
                stderr_reader.abort();
                let _ = tx.send(RuntimeEvent::Outcome(RuntimeOutcome::new(RuntimeOutcomeKind::Cancelled, "Codex execution cancelled"))).await;
                return;
            }
            result = &mut stdout_reader, if stdout_result.is_none() => {
                match result {
                    Ok(Ok(state)) => stdout_result = Some(Ok(state)),
                    Ok(Err(error)) => {
                        terminate_process_tree(&mut child).await;
                        stderr_reader.abort();
                        let _ = tx.send(RuntimeEvent::InvalidOutput { reason: error }).await;
                        return;
                    }
                    Err(error) => {
                        terminate_process_tree(&mut child).await;
                        stderr_reader.abort();
                        let _ = tx.send(RuntimeEvent::InvalidOutput { reason: format!("stdout reader failed: {error}") }).await;
                        return;
                    }
                }
            }
            result = &mut stderr_reader, if stderr_result.is_none() => {
                match result {
                    Ok(Ok(())) => stderr_result = Some(Ok(())),
                    Ok(Err(error)) => {
                        terminate_process_tree(&mut child).await;
                        stdout_reader.abort();
                        let _ = tx.send(RuntimeEvent::InvalidOutput { reason: error }).await;
                        return;
                    }
                    Err(error) => {
                        terminate_process_tree(&mut child).await;
                        stdout_reader.abort();
                        let _ = tx.send(RuntimeEvent::InvalidOutput { reason: format!("stderr reader failed: {error}") }).await;
                        return;
                    }
                }
            }
        }
    };
    let stdout_result = if let Some(result) = stdout_result {
        result
    } else {
        match stdout_reader.await {
            Ok(result) => result,
            Err(error) => Err(format!("stdout reader failed: {error}")),
        }
    };
    let stderr_result = if let Some(result) = stderr_result {
        result
    } else {
        match stderr_reader.await {
            Ok(result) => result,
            Err(error) => Err(format!("stderr reader failed: {error}")),
        }
    };
    let outcome = match (exit, stdout_result, stderr_result) {
        (_, Err(error), _) | (_, _, Err(error)) => RuntimeEvent::InvalidOutput { reason: error },
        (Err(error), _, _) => RuntimeEvent::Outcome(RuntimeOutcome::new(
            RuntimeOutcomeKind::RuntimeFailure,
            format!("Codex process wait failed: {error}"),
        )),
        (Ok(status), _, _) if !status.success() => RuntimeEvent::Outcome(RuntimeOutcome::new(
            RuntimeOutcomeKind::RuntimeFailure,
            format!("Codex exited with {status}"),
        )),
        (Ok(_), Ok(stream), Ok(_)) if stream.turn_failed => {
            RuntimeEvent::Outcome(RuntimeOutcome::new(
                RuntimeOutcomeKind::RuntimeFailure,
                "Codex reported turn.failed",
            ))
        }
        (Ok(_), Ok(stream), Ok(_)) if !stream.turn_completed => RuntimeEvent::InvalidOutput {
            reason: "Codex exited without turn.completed".to_owned(),
        },
        (Ok(_), Ok(_), Ok(_)) => match temp.read_result().await {
            Ok(result) => RuntimeEvent::Outcome(RuntimeOutcome::new(
                result.outcome.into(),
                format!(
                    "{}\nVerification: {}",
                    result.summary, result.verification_summary
                ),
            )),
            Err(error) => RuntimeEvent::InvalidOutput { reason: error },
        },
    };
    let _ = tx.send(outcome).await;
}

#[derive(Default)]
struct StreamState {
    turn_completed: bool,
    turn_failed: bool,
}

const EVENT_OUTPUT_BYTE_LIMIT: usize = 2 * 1024;

#[allow(dead_code)]
fn render_codex_json_event(event: &Value) -> Option<Vec<u8>> {
    let kind = event.get("type")?.as_str()?;
    let text = match kind {
        "thread.started" => "› Codex session started".to_owned(),
        "turn.started" => "› Codex started working".to_owned(),
        "error" => format!(
            "× Codex: {}",
            bounded_event_text(
                event
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("runtime error")
            )
        ),
        "item.started" | "item.updated" | "item.completed" => render_item_event(event)?,
        _ => return None,
    };
    Some(format!("{text}\n").into_bytes())
}

fn normalize_app_server_activity(event: &Value) -> Option<RuntimeActivity> {
    let kind = event
        .get("type")
        .or_else(|| event.get("method"))
        .and_then(Value::as_str)?;
    let item = event.get("item").or_else(|| event.pointer("/params/item"));
    match kind {
        "thread.started" | "thread/started" => Some(RuntimeActivity::SessionStarted),
        "turn.started" | "turn/started" => Some(RuntimeActivity::Analyzing {
            description: "Codex is analyzing the task".to_owned(),
        }),
        "item.started" | "item/started" => {
            let item = item?;
            match item.get("type")?.as_str()? {
                "commandExecution" => Some(RuntimeActivity::CommandStarted {
                    logical_command: item
                        .get("command")
                        .and_then(Value::as_str)
                        .unwrap_or("command")
                        .to_owned(),
                    raw_command: None,
                    working_directory: item.get("cwd").and_then(Value::as_str).map(PathBuf::from),
                }),
                "agentMessage" => Some(RuntimeActivity::AgentMessage {
                    message: RuntimeText {
                        content: bounded_event_text(
                            item.get("text").and_then(Value::as_str).unwrap_or_default(),
                        ),
                        format: RuntimeTextFormat::Markdown,
                    },
                }),
                _ => None,
            }
        }
        "item.completed" | "item/completed" => {
            let item = item?;
            match item.get("type")?.as_str()? {
                "commandExecution" => Some(RuntimeActivity::CommandCompleted {
                    logical_command: item
                        .get("command")
                        .and_then(Value::as_str)
                        .unwrap_or("command")
                        .to_owned(),
                    success: item.get("exitCode").and_then(Value::as_i64) == Some(0),
                    exit_code: item
                        .get("exitCode")
                        .and_then(Value::as_i64)
                        .and_then(|code| i32::try_from(code).ok()),
                    working_directory: item.get("cwd").and_then(Value::as_str).map(PathBuf::from),
                }),
                "fileChange" => {
                    let change = item.get("changes").and_then(Value::as_array)?.first()?;
                    let path = PathBuf::from(change.get("path").and_then(Value::as_str)?);
                    let change_kind = match change
                        .get("kind")
                        .and_then(Value::as_str)
                        .unwrap_or("modified")
                    {
                        "add" | "added" => FileChangeKind::Added,
                        "delete" | "deleted" => FileChangeKind::Deleted,
                        _ => FileChangeKind::Modified,
                    };
                    Some(RuntimeActivity::FileChanged { path, change_kind })
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn normalize_codex_activity(event: &Value) -> Option<RuntimeActivity> {
    let kind = event
        .get("type")
        .or_else(|| event.get("method"))
        .and_then(Value::as_str)?;
    let item = event.get("item").or_else(|| event.pointer("/params/item"));
    match kind {
        "thread.started" | "thread/started" => Some(RuntimeActivity::SessionStarted),
        "turn.started" | "turn/started" => Some(RuntimeActivity::Analyzing {
            description: "Codex is analyzing the task".to_owned(),
        }),
        "item.started" | "item/started" => {
            let item = item?;
            match item.get("type")?.as_str()? {
                "command_execution" => Some(RuntimeActivity::CommandStarted {
                    logical_command: item
                        .get("command")
                        .and_then(Value::as_str)
                        .unwrap_or("command")
                        .to_owned(),
                    raw_command: None,
                    working_directory: item.get("cwd").and_then(Value::as_str).map(PathBuf::from),
                }),
                "agent_message" => Some(RuntimeActivity::AgentMessage {
                    message: RuntimeText {
                        content: bounded_event_text(
                            item.get("text").and_then(Value::as_str).unwrap_or_default(),
                        ),
                        format: RuntimeTextFormat::Markdown,
                    },
                }),
                _ => None,
            }
        }
        "item.completed" | "item/completed" => {
            let item = item?;
            match item.get("type")?.as_str()? {
                "command_execution" => Some(RuntimeActivity::CommandCompleted {
                    logical_command: item
                        .get("command")
                        .and_then(Value::as_str)
                        .unwrap_or("command")
                        .to_owned(),
                    success: item.get("exit_code").and_then(Value::as_i64) == Some(0),
                    exit_code: item
                        .get("exit_code")
                        .and_then(Value::as_i64)
                        .and_then(|code| i32::try_from(code).ok()),
                    working_directory: item.get("cwd").and_then(Value::as_str).map(PathBuf::from),
                }),
                "file_change" => {
                    let change = item.get("changes").and_then(Value::as_array)?.first()?;
                    let path = PathBuf::from(change.get("path").and_then(Value::as_str)?);
                    let change_kind = match change
                        .get("kind")
                        .and_then(Value::as_str)
                        .unwrap_or("modified")
                    {
                        "add" | "added" => FileChangeKind::Added,
                        "delete" | "deleted" => FileChangeKind::Deleted,
                        _ => FileChangeKind::Modified,
                    };
                    Some(RuntimeActivity::FileChanged { path, change_kind })
                }
                _ => None,
            }
        }
        _ => None,
    }
}

#[allow(dead_code)]
fn render_item_event(event: &Value) -> Option<String> {
    let item = event.get("item")?;
    match item.get("type")?.as_str()? {
        "agent_message" => Some(format!(
            "› {}",
            bounded_event_text(item.get("text").and_then(Value::as_str)?)
        )),
        "command_execution" => {
            let command = bounded_event_text(
                item.get("command")
                    .and_then(Value::as_str)
                    .unwrap_or("command"),
            );
            match event.get("type").and_then(Value::as_str) {
                Some("item.started") => Some(format!("› Running {command}")),
                Some("item.completed") => {
                    let output = item
                        .get("aggregated_output")
                        .or_else(|| item.get("output"))
                        .and_then(Value::as_str)
                        .map(bounded_event_text);
                    let status = if item.get("exit_code").and_then(Value::as_i64) == Some(0) {
                        format!("✓ {command}")
                    } else {
                        format!("× {command} failed")
                    };
                    Some(output.map_or(status.clone(), |output| format!("{status}\n  {output}")))
                }
                _ => None,
            }
        }
        "file_change" => {
            if item.get("status").and_then(Value::as_str) == Some("failed") {
                return Some(format!(
                    "× Codex file change failed: {}",
                    bounded_event_text(
                        item.get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("file change failed")
                    )
                ));
            }
            let changes = item.get("changes").and_then(Value::as_array)?;
            let rendered = changes
                .iter()
                .filter_map(|change| {
                    let path = change.get("path").and_then(Value::as_str)?;
                    let label = match change
                        .get("kind")
                        .and_then(Value::as_str)
                        .unwrap_or("modified")
                    {
                        "add" | "added" | "create" | "created" => "Added",
                        "delete" | "deleted" | "remove" | "removed" => "Deleted",
                        _ => "Modified",
                    };
                    Some(format!("› {label} {}", bounded_event_text(path)))
                })
                .collect::<Vec<_>>();
            (!rendered.is_empty()).then(|| rendered.join("\n"))
        }
        "error" => Some(format!(
            "× Codex: {}",
            bounded_event_text(
                item.get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("runtime error")
            )
        )),
        "reasoning" => Some("› Codex is analyzing the task".to_owned()),
        _ => None,
    }
}

fn bounded_event_text(input: &str) -> String {
    let mut safe: String = input
        .chars()
        .filter(|character| *character == '\n' || *character == '\t' || !character.is_control())
        .collect();
    truncate_utf8_to_bytes(&mut safe, EVENT_OUTPUT_BYTE_LIMIT);
    safe
}

fn truncate_utf8_to_bytes(value: &mut String, max_bytes: usize) {
    if value.len() <= max_bytes {
        return;
    }
    let boundary = value
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= max_bytes)
        .last()
        .unwrap_or(0);
    value.truncate(boundary);
    value.push('…');
}

async fn read_stdout(
    stdout: tokio::process::ChildStdout,
    tx: mpsc::Sender<RuntimeEvent>,
) -> Result<StreamState, String> {
    let mut reader = BufReader::new(stdout);
    let mut pending = Vec::new();
    let mut chunk = [0_u8; 8 * 1024];
    let mut state = StreamState::default();
    loop {
        let read = reader
            .read(&mut chunk)
            .await
            .map_err(|error| error.to_string())?;
        if read == 0 {
            if !pending.is_empty() {
                return Err("Codex JSONL ended with an unterminated frame".to_owned());
            }
            break;
        }
        pending.extend_from_slice(&chunk[..read]);
        while let Some(newline) = pending.iter().position(|byte| *byte == b'\n') {
            let line = pending.drain(..=newline).collect::<Vec<_>>();
            if line.len() - 1 > MAX_CODEX_JSONL_FRAME_BYTES {
                return Err("Codex JSONL frame exceeded the 256 KiB limit".to_owned());
            }
            let line = String::from_utf8(line[..line.len() - 1].to_vec())
                .map_err(|_| "Codex JSONL frame was not valid UTF-8".to_owned())?;
            let event: Value = serde_json::from_str(&line)
                .map_err(|error| format!("malformed Codex JSONL: {error}"))?;
            let kind = event
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            match kind {
                "turn.completed" => state.turn_completed = true,
                "turn.failed" => state.turn_failed = true,
                _ => {}
            }
            if let Some(activity) = normalize_codex_activity(&event) {
                let _ = tx.send(RuntimeEvent::Activity(activity)).await;
            } else if matches!(kind, "error" | "turn.failed") {
                // Preserve provider diagnostics for `/logs raw` without
                // making provider-formatted UI text part of normal activity.
                let _ = tx
                    .send(RuntimeEvent::Output {
                        stream: RuntimeOutputStream::Stderr,
                        bytes: format!("{line}\n").into_bytes(),
                    })
                    .await;
            }
        }
        if pending.len() > MAX_CODEX_JSONL_FRAME_BYTES {
            return Err("Codex JSONL frame exceeded the 256 KiB limit".to_owned());
        }
    }
    Ok(state)
}

async fn read_stderr(
    stderr: tokio::process::ChildStderr,
    tx: mpsc::Sender<RuntimeEvent>,
) -> Result<(), String> {
    let mut reader = BufReader::new(stderr);
    let mut chunk = [0_u8; 8 * 1024];
    loop {
        let read = reader
            .read(&mut chunk)
            .await
            .map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        let _ = tx
            .send(RuntimeEvent::Output {
                stream: RuntimeOutputStream::Stderr,
                bytes: chunk[..read].to_vec(),
            })
            .await;
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RelayResult {
    outcome: RelayResultKind,
    summary: String,
    verification_summary: String,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RelayResultKind {
    Success,
    ImplementationFailure,
    VerificationFailure,
    NeedsClarification,
    NeedsDecision,
}
impl From<RelayResultKind> for RuntimeOutcomeKind {
    fn from(value: RelayResultKind) -> Self {
        match value {
            RelayResultKind::Success => Self::Success,
            RelayResultKind::ImplementationFailure => Self::ImplementationFailure,
            RelayResultKind::VerificationFailure => Self::VerificationFailure,
            RelayResultKind::NeedsClarification => Self::NeedsClarification,
            RelayResultKind::NeedsDecision => Self::NeedsDecision,
        }
    }
}

struct RuntimeTempFiles {
    schema_path: PathBuf,
    result_path: PathBuf,
}
impl RuntimeTempFiles {
    fn create() -> Result<Self, RuntimeError> {
        let id = Uuid::new_v4();
        let root = env::temp_dir();
        let schema_path = root.join(format!("relay-codex-{id}-schema.json"));
        let result_path = root.join(format!("relay-codex-{id}-result.json"));
        std::fs::write(
            &schema_path,
            serde_json::to_vec(&outcome_schema()).expect("static schema serializes"),
        )
        .map_err(|error| {
            RuntimeError::StartFailed(format!("could not create Relay outcome schema: {error}"))
        })?;
        Ok(Self {
            schema_path,
            result_path,
        })
    }
    async fn read_result(&self) -> Result<RelayResult, String> {
        let metadata = tokio::fs::metadata(&self.result_path)
            .await
            .map_err(|error| format!("missing Codex structured result: {error}"))?;
        if metadata.len() > MAX_CODEX_RESULT_BYTES {
            return Err("Codex structured result exceeded the 64 KiB limit".to_owned());
        }
        let bytes = tokio::fs::read(&self.result_path)
            .await
            .map_err(|error| format!("could not read Codex structured result: {error}"))?;
        let mut result: RelayResult = serde_json::from_slice(&bytes)
            .map_err(|error| format!("invalid Codex structured result: {error}"))?;
        result.summary = bounded_result_text(&result.summary);
        result.verification_summary = bounded_result_text(&result.verification_summary);
        Ok(result)
    }
}

fn bounded_result_text(input: &str) -> String {
    let mut safe = input
        .chars()
        .filter(|character| *character == '\n' || *character == '\t' || !character.is_control())
        .collect::<String>();
    truncate_utf8_to_bytes(&mut safe, MAX_CODEX_RESULT_TEXT_BYTES);
    safe
}
impl Drop for RuntimeTempFiles {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.schema_path);
        let _ = std::fs::remove_file(&self.result_path);
    }
}

fn outcome_schema() -> Value {
    json!({ "type": "object", "properties": { "outcome": { "enum": ["success", "implementation_failure", "verification_failure", "needs_clarification", "needs_decision"] }, "summary": { "type": "string" }, "verification_summary": { "type": "string" } }, "required": ["outcome", "summary", "verification_summary"], "additionalProperties": false })
}

pub fn build_prompt(request: &RuntimeRequest) -> String {
    let intent = &request.intent;
    let list = |items: &[String]| {
        if items.is_empty() {
            "- None\n".to_owned()
        } else {
            items.iter().map(|item| format!("- {item}\n")).collect()
        }
    };
    let repair = match &request.action {
        RuntimeAction::Implement => String::new(),
        RuntimeAction::Repair {
            attempt,
            previous_outcome,
        } => format!(
            "\nRepair attempt: {attempt}\nPrevious outcome: {}\nRepair only the existing approved intent.\n",
            previous_outcome.as_str()
        ),
    };
    format!(
        "Relay execution context\n\nTitle:\n{}\n\nGoal:\n{}\n\nApproved requirements:\n{}\nConstraints:\n{}\nVerification:\n{}{}\nExecution policy:\n- Work only within the approved task intent.\n- Work only inside the provided repository.\n- Do not commit.\n- Do not push.\n- Do not change repository remotes.\n- Do not broaden scope.\n- Run the approved verification where possible.\n- If approved intent is ambiguous, return needs_clarification.\n- If continuing requires architecture, scope, destructive, or security-policy change, return needs_decision.\n- Return the required structured Relay outcome.\n",
        intent.title,
        intent.goal,
        list(&intent.requirements),
        list(&intent.constraints),
        list(&intent.verification),
        repair
    )
}

enum DiscoveryError {
    Spawn(String),
    Timeout,
}

fn resolve_executable(configured: &Path, explicit: bool) -> Result<PathBuf, String> {
    if explicit {
        return resolve_candidate(configured).ok_or_else(|| format!("configured Codex executable '{}' could not be started; check RELAY_CODEX_EXECUTABLE", configured.display()));
    }
    let path_directories = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .collect::<Vec<_>>();
    let candidates = path_directories.into_iter().flat_map(|directory| {
        let names = if cfg!(windows) {
            vec!["codex.exe", "codex.cmd", "codex"]
        } else {
            vec!["codex"]
        };
        names
            .into_iter()
            .map(move |name| directory.join(name))
            .collect::<Vec<_>>()
    });
    candidates
        .into_iter()
        .find(|candidate| resolve_candidate(candidate).is_some())
        .ok_or_else(|| {
            "Codex executable not found on PATH; install Codex or set RELAY_CODEX_EXECUTABLE"
                .to_owned()
        })
}

fn resolve_candidate(candidate: &Path) -> Option<PathBuf> {
    if candidate.components().count() > 1 || candidate.is_absolute() {
        return candidate.is_file().then(|| candidate.to_owned());
    }
    let path_directories = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .collect::<Vec<_>>();
    path_directories.into_iter().find_map(|directory| {
        let direct = directory.join(candidate);
        if direct.is_file() {
            return Some(direct);
        }
        if cfg!(windows) && candidate.extension().is_none() {
            for extension in ["exe", "cmd"] {
                let with_extension =
                    directory.join(format!("{}.{}", candidate.display(), extension));
                if with_extension.is_file() {
                    return Some(with_extension);
                }
            }
        }
        None
    })
}

fn launcher_command(executable: &Path, args: &[String]) -> Command {
    #[cfg(windows)]
    let mut command = if executable
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("cmd"))
    {
        let mut cmd = Command::new("cmd.exe");
        cmd.args(["/D", "/C", "call"]);
        cmd.arg(executable);
        cmd.args(args);
        cmd
    } else {
        let mut cmd = Command::new(executable);
        cmd.args(args);
        cmd
    };
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);

    #[cfg(not(windows))]
    let mut command = Command::new(executable);
    #[cfg(not(windows))]
    command.args(args);

    command
}

fn sanitize_git_environment(command: &mut Command) {
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_PARAMETERS",
    ] {
        command.env_remove(key);
    }
    for (key, _) in env::vars_os() {
        let upper = key.to_string_lossy().to_ascii_uppercase();
        if upper.starts_with("GIT_CONFIG_KEY_") || upper.starts_with("GIT_CONFIG_VALUE_") {
            command.env_remove(key);
        }
    }
}

async fn discover_command(
    executable: &Path,
    args: &[&str],
) -> Result<std::process::Output, DiscoveryError> {
    let args = args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
    match timeout(DISCOVERY_TIMEOUT, {
        let mut command = launcher_command(executable, &args);
        command.kill_on_drop(true);
        sanitize_git_environment(&mut command);
        command.output()
    })
    .await
    {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(error)) => Err(DiscoveryError::Spawn(error.to_string())),
        Err(_) => Err(DiscoveryError::Timeout),
    }
}
fn safe_process_error(bytes: &[u8], fallback: &str) -> String {
    let text = String::from_utf8_lossy(bytes)
        .trim()
        .replace(['\r', '\n'], " ");
    if text.is_empty() {
        fallback.to_owned()
    } else {
        text.chars().take(512).collect()
    }
}

fn required_exec_flags_present(output: &std::process::Output) -> bool {
    let help = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let required = [
        "--json",
        "--ignore-user-config",
        "--ignore-rules",
        "--ephemeral",
        "--strict-config",
        "--color",
        "--sandbox",
        "--cd",
        "--output-schema",
        "--output-last-message",
        "-c",
    ];
    required.iter().all(|flag| {
        help.lines().any(|line| {
            line.split_whitespace()
                .map(|token| token.trim_matches(|character| matches!(character, ',' | ':')))
                .any(|token| token == *flag)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use relay_protocol::AmbiguityPolicy;
    use relay_runtime::{ExecutionId, RuntimeTaskIntent};
    fn request(goal: &str) -> RuntimeRequest {
        RuntimeRequest {
            execution_id: ExecutionId::new(),
            task_id: "task".to_owned(),
            intent: RuntimeTaskIntent {
                title: "Title".to_owned(),
                goal: goal.to_owned(),
                requirements: vec!["Requirement".to_owned()],
                constraints: vec!["Constraint".to_owned()],
                verification: vec!["cargo test".to_owned()],
                requested_stack: None,
                ambiguity_policy: AmbiguityPolicy::NeedsClarification,
            },
            trusted_working_dir: PathBuf::from("C:/trusted/repo"),
            action: RuntimeAction::Implement,
            profile_id: "codex/default".to_owned(),
            runtime_selection: relay_runtime::RuntimeExecutionSelection::default(),
        }
    }
    #[test]
    fn command_keeps_prompt_off_arguments() {
        let runtime = CodexRuntime::default();
        let request = request("; git push\n--full-auto $(rm -rf .)");
        let spec = runtime.command_spec(&request, Path::new("schema"), Path::new("result"));
        assert_eq!(spec.args[..3], ["--ask-for-approval", "never", "exec"]);
        assert!(spec.args.contains(&"--json".to_owned()));
        assert!(spec.args.contains(&"workspace-write".to_owned()));
        assert!(spec.args.contains(&"--ignore-user-config".to_owned()));
        assert!(spec.args.contains(&"--ignore-rules".to_owned()));
        assert!(spec.args.contains(&"--ephemeral".to_owned()));
        assert!(spec.args.contains(&"--strict-config".to_owned()));
        assert!(
            spec.args
                .windows(2)
                .any(|args| args == ["-c", "sandbox_workspace_write.writable_roots=[]"])
        );
        assert!(
            spec.args
                .windows(2)
                .any(|args| { args == ["-c", "sandbox_workspace_write.network_access=false"] })
        );
        assert!(!spec.args.iter().any(|arg| arg.contains("git push")
            || arg.contains("full-auto")
            || arg.contains("skip-git-repo-check")
            || arg.contains("--add-dir")
            || arg.contains("danger-full-access")
            || arg == "--yolo"
            || arg.contains("dangerously-bypass")));
    }
    #[test]
    fn prompt_preserves_policy_and_repair_details() {
        let mut request = request("Goal");
        request.action = RuntimeAction::Repair {
            attempt: 2,
            previous_outcome: RuntimeOutcomeKind::VerificationFailure,
        };
        let prompt = build_prompt(&request);
        for expected in [
            "Title",
            "Goal",
            "Requirement",
            "Constraint",
            "cargo test",
            "Do not commit.",
            "Do not push.",
            "Repair attempt: 2",
            "structured Relay outcome",
        ] {
            assert!(prompt.contains(expected), "{expected}");
        }
    }
    #[test]
    fn result_rejects_unknown_and_extra_fields() {
        assert!(
            serde_json::from_str::<RelayResult>(
                r#"{\"outcome\":\"unknown\",\"summary\":\"x\",\"verification_summary\":\"y\"}"#
            )
            .is_err()
        );
        assert!(serde_json::from_str::<RelayResult>(r#"{\"outcome\":\"success\",\"summary\":\"x\",\"verification_summary\":\"y\",\"extra\":true}"#).is_err());
    }

    #[test]
    fn formatter_surfaces_safe_progress_and_hides_reasoning_text() {
        let command = serde_json::json!({"type":"item.started","item":{"type":"command_execution","command":"cargo check --workspace"}});
        assert_eq!(
            String::from_utf8(render_codex_json_event(&command).unwrap()).unwrap(),
            "› Running cargo check --workspace\n"
        );
        let reasoning = serde_json::json!({"type":"item.completed","item":{"type":"reasoning","text":"private chain of thought"}});
        assert_eq!(
            String::from_utf8(render_codex_json_event(&reasoning).unwrap()).unwrap(),
            "› Codex is analyzing the task\n"
        );
        assert!(render_codex_json_event(&serde_json::json!({"type":"future.event"})).is_none());
    }

    #[test]
    fn normalizes_structured_activity() {
        assert!(matches!(
            normalize_codex_activity(&serde_json::json!({"type":"thread.started"})),
            Some(RuntimeActivity::SessionStarted)
        ));
        assert!(matches!(
            normalize_codex_activity(
                &serde_json::json!({"type":"item.started","item":{"type":"command_execution","command":"cargo test"}})
            ),
            Some(RuntimeActivity::CommandStarted { .. })
        ));
    }

    #[test]
    fn formatter_maps_file_changes_and_command_output() {
        let files = serde_json::json!({"type":"item.completed","item":{"type":"file_change","changes":[{"kind":"add","path":"src/new.rs"},{"kind":"modify","path":"src/lib.rs"},{"kind":"delete","path":"old.rs"}]}});
        let rendered = String::from_utf8(render_codex_json_event(&files).unwrap()).unwrap();
        assert!(rendered.contains("Added src/new.rs"));
        assert!(rendered.contains("Modified src/lib.rs"));
        assert!(rendered.contains("Deleted old.rs"));
        let command = serde_json::json!({"type":"item.completed","item":{"type":"command_execution","command":"cargo test","exit_code":0,"aggregated_output":"running 3 tests"}});
        let rendered = String::from_utf8(render_codex_json_event(&command).unwrap()).unwrap();
        assert!(rendered.contains("✓ cargo test"));
        assert!(rendered.contains("running 3 tests"));
        let failed_change = serde_json::json!({"type":"item.completed","item":{"type":"file_change","status":"failed","message":"patch rejected","changes":[]}});
        let rendered = String::from_utf8(render_codex_json_event(&failed_change).unwrap()).unwrap();
        assert!(rendered.contains("file change failed"));
        let item_error = serde_json::json!({"type":"item.completed","item":{"type":"error","message":"tool failed"}});
        let rendered = String::from_utf8(render_codex_json_event(&item_error).unwrap()).unwrap();
        assert!(rendered.contains("tool failed"));
    }

    #[test]
    fn event_text_truncation_is_utf8_safe() {
        for text in [
            "a".repeat(3000),
            "é".repeat(3000),
            "€".repeat(3000),
            "😀".repeat(3000),
        ] {
            let result = bounded_event_text(&text);
            assert!(result.is_char_boundary(result.len()));
            assert!(result.len() <= EVENT_OUTPUT_BYTE_LIMIT + "…".len());
            assert!(result.ends_with('…'));
        }
        assert_eq!(bounded_event_text("abc"), "abc");
        let exact = "a".repeat(EVENT_OUTPUT_BYTE_LIMIT);
        assert_eq!(bounded_event_text(&exact), exact);
    }

    #[test]
    fn explicit_codex_path_resolution_is_fail_closed() {
        let missing = PathBuf::from("C:/definitely-missing-relay-codex");
        let error = resolve_executable(&missing, true).unwrap_err();
        assert!(error.contains("RELAY_CODEX_EXECUTABLE"));
        let file =
            std::env::temp_dir().join(format!("relay-codex-candidate-{}", std::process::id()));
        std::fs::write(&file, b"fixture").unwrap();
        assert_eq!(resolve_executable(&file, true).unwrap(), file);
        let _ = std::fs::remove_file(file);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_cmd_launcher_preserves_argument_boundaries() {
        let root = std::env::temp_dir().join(format!("relay-codex cmd {}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let script = root.join("codex.cmd");
        std::fs::write(&script, "@echo off\r\necho %1\r\n").unwrap();
        let args = vec!["value with spaces & symbols".to_owned()];
        let output = launcher_command(&script, &args).output().await.unwrap();
        assert!(
            output.status.success(),
            "status={:?} stderr={:?} stdout={:?}",
            output.status,
            output.stderr,
            output.stdout
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("value with spaces & symbols"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_cmd_tree_termination_reaps_wrapper() {
        let root = std::env::temp_dir().join(format!("relay-codex tree {}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let script = root.join("tree.cmd");
        std::fs::write(
            &script,
            "@echo off\r\nstart \"\" /b ping 127.0.0.1 -n 30 > nul\r\nping 127.0.0.1 -n 30 > nul\r\n",
        )
        .unwrap();
        let mut command = launcher_command(&script, &[]);
        command.kill_on_drop(true);
        let mut child = command.spawn().unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        terminate_process_tree(&mut child).await;
        assert!(child.try_wait().unwrap().is_some());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn command_spec_translates_model_and_reasoning_selection() {
        let runtime = CodexRuntime::default();
        let mut req = request("test goal");
        req.runtime_selection = relay_runtime::RuntimeExecutionSelection {
            profile_id: "codex/implement".to_owned(),
            model: relay_runtime::RuntimeModelSelection::Explicit("gpt-5.3-codex".to_owned()),
            reasoning_effort: relay_runtime::RuntimeReasoningSelection::Explicit(
                relay_runtime::ReasoningEffort::High,
            ),
        };
        let spec = runtime.command_spec(&req, Path::new("schema"), Path::new("result"));
        assert!(spec.args.contains(&"--model".to_owned()));
        assert!(spec.args.contains(&"gpt-5.3-codex".to_owned()));
        assert!(
            spec.args
                .contains(&"model_reasoning_effort=\"high\"".to_owned())
        );
    }
}
