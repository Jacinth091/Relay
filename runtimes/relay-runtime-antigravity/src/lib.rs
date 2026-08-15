//! Antigravity (AGY) CLI adapter for Relay's provider-neutral runtime boundary.
//!
//! All executable selection is trusted local configuration. Task content is
//! supplied only on stdin and is never interpreted as shell syntax.

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
    CancellationToken, CapabilityStatus, EffectiveRuntimeProfile, FilesystemCapability,
    ReasoningEffort, Runtime, RuntimeAction, RuntimeCapabilities, RuntimeCapabilityContext,
    RuntimeCapabilityReport, RuntimeConfigError, RuntimeDescriptor, RuntimeError, RuntimeEvent,
    RuntimeIdentity, RuntimeIntegration, RuntimeModelCatalog, RuntimeModelInfo, RuntimeOutcome,
    RuntimeOutcomeKind, RuntimeOutputStream, RuntimeProbe, RuntimeProfile,
    RuntimeProfileDescriptor, RuntimeRequest, RuntimeSession, RuntimeStartFuture,
};
use serde::Deserialize;
use serde_json::Value;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, BufReader},
    process::Command,
    sync::mpsc,
    time::timeout,
};
use uuid::Uuid;

const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_OUTPUT_FRAME_BYTES: usize = 256 * 1024;
const MAX_RESULT_BYTES: u64 = 64 * 1024;
const MAX_RESULT_TEXT_BYTES: usize = 8 * 1024;
const EVENT_OUTPUT_BYTE_LIMIT: usize = 2 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AntigravityRuntimeConfig {
    pub executable: PathBuf,
    pub profile_id: String,
    pub model_override: Option<String>,
    pub effort: Option<String>,
}

impl Default for AntigravityRuntimeConfig {
    fn default() -> Self {
        let configured = env::var_os("RELAY_AGY_EXECUTABLE");
        Self {
            executable: configured
                .clone()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("agy")),
            profile_id: "antigravity/default".to_owned(),
            model_override: None,
            effort: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AntigravityRuntimeHealth {
    Checking,
    Available {
        version: String,
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

impl AntigravityRuntimeHealth {
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Available { .. })
    }
    pub fn display_status(&self) -> &'static str {
        match self {
            Self::Checking => "AGY health check pending",
            Self::Available { .. } => "AGY available",
            Self::NotInstalled { .. } => "AGY not installed",
            Self::NotAuthenticated { .. } => "AGY not authenticated",
            Self::Incompatible { .. } => "AGY CLI incompatible",
            Self::DiscoveryFailed { .. } => "AGY discovery failed",
        }
    }
}

#[derive(Debug, Clone)]
pub struct AntigravityRuntime {
    config: AntigravityRuntimeConfig,
}

impl Default for AntigravityRuntime {
    fn default() -> Self {
        Self::new(AntigravityRuntimeConfig::default())
    }
}

impl RuntimeIntegration for AntigravityRuntime {
    fn descriptor(&self) -> RuntimeDescriptor {
        RuntimeDescriptor {
            provider_id: "antigravity".to_owned(),
            display_name: "Antigravity".to_owned(),
            description: "Google Antigravity CLI execution adapter".to_owned(),
            is_development_only: false,
        }
    }

    fn probe(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = RuntimeProbe> + Send + '_>> {
        Box::pin(async move {
            let health = self.discover().await;
            match health {
                AntigravityRuntimeHealth::Available { version } => RuntimeProbe {
                    installed: true,
                    version: Some(version),
                    authenticated: true,
                    compatible: true,
                    account_identity: None,
                    notes: vec!["Antigravity available and authenticated".to_owned()],
                },
                AntigravityRuntimeHealth::NotAuthenticated { version, error } => RuntimeProbe {
                    installed: true,
                    version,
                    authenticated: false,
                    compatible: true,
                    account_identity: None,
                    notes: vec![format!("Not authenticated: {error}")],
                },
                AntigravityRuntimeHealth::Incompatible { version, reason } => RuntimeProbe {
                    installed: true,
                    version: Some(version),
                    authenticated: true,
                    compatible: false,
                    account_identity: None,
                    notes: vec![format!("Incompatible: {reason}")],
                },
                AntigravityRuntimeHealth::NotInstalled { error }
                | AntigravityRuntimeHealth::DiscoveryFailed { error } => RuntimeProbe {
                    installed: false,
                    version: None,
                    authenticated: false,
                    compatible: false,
                    account_identity: None,
                    notes: vec![error],
                },
                AntigravityRuntimeHealth::Checking => RuntimeProbe {
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
                id: "antigravity/review".to_owned(),
                display_name: "Antigravity / Review".to_owned(),
                purpose: "Read-only inspection and task analysis".to_owned(),
            },
            RuntimeProfileDescriptor {
                id: "antigravity/implement".to_owned(),
                display_name: "Antigravity / Implement".to_owned(),
                purpose: "Code generation and workspace modifications".to_owned(),
            },
            RuntimeProfileDescriptor {
                id: "antigravity/deep".to_owned(),
                display_name: "Antigravity / Deep".to_owned(),
                purpose: "High effort reasoning for complex agentic workflows".to_owned(),
            },
        ]
    }

    fn models(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = RuntimeModelCatalog> + Send + '_>> {
        Box::pin(async move {
            RuntimeModelCatalog {
                default_model: "default".to_owned(),
                available_models: vec![
                    RuntimeModelInfo {
                        id: "default".to_owned(),
                        display_name: "Default (Account/CLI Setting)".to_owned(),
                    },
                    RuntimeModelInfo {
                        id: "gemini-3.6-pro".to_owned(),
                        display_name: "Gemini 3.6 Pro".to_owned(),
                    },
                    RuntimeModelInfo {
                        id: "gemini-3.6-flash".to_owned(),
                        display_name: "Gemini 3.6 Flash".to_owned(),
                    },
                ],
            }
        })
    }

    fn resolve_profile(
        &self,
        profile_id: &str,
    ) -> Result<EffectiveRuntimeProfile, RuntimeConfigError> {
        let profile = match profile_id {
            "antigravity/review" => RuntimeProfile {
                id: "antigravity/review".to_owned(),
                provider: "antigravity".to_owned(),
                display_name: "Antigravity / Review".to_owned(),
                model: "default".to_owned(),
                reasoning_effort: ReasoningEffort::Medium,
                filesystem: FilesystemCapability::ReadOnly,
                network: false,
                extra_writable_roots: vec![],
                max_duration: Duration::from_secs(20 * 60),
                max_repair_attempts: 1,
                relay_approval_owner: true,
            },
            "antigravity/implement" | "antigravity/default" => RuntimeProfile {
                id: "antigravity/implement".to_owned(),
                provider: "antigravity".to_owned(),
                display_name: "Antigravity / Implement".to_owned(),
                model: "default".to_owned(),
                reasoning_effort: ReasoningEffort::High,
                filesystem: FilesystemCapability::WorkspaceWrite,
                network: false,
                extra_writable_roots: vec![],
                max_duration: Duration::from_secs(30 * 60),
                max_repair_attempts: 2,
                relay_approval_owner: true,
            },
            "antigravity/deep" => RuntimeProfile {
                id: "antigravity/deep".to_owned(),
                provider: "antigravity".to_owned(),
                display_name: "Antigravity / Deep".to_owned(),
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

enum DiscoveryError {
    Spawn(String),
    Timeout,
}

fn resolve_executable(configured: &Path, explicit: bool) -> Result<PathBuf, String> {
    if explicit {
        return resolve_candidate(configured).ok_or_else(|| {
            format!(
                "configured AGY executable '{}' could not be started; check RELAY_AGY_EXECUTABLE",
                configured.display()
            )
        });
    }
    let path_directories = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .collect::<Vec<_>>();
    let candidates = path_directories.into_iter().flat_map(|directory| {
        let names = if cfg!(windows) {
            vec!["agy.exe", "agy.cmd", "agy"]
        } else {
            vec!["agy"]
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
            "AGY executable not found on PATH; install AGY or set RELAY_AGY_EXECUTABLE".to_owned()
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

impl AntigravityRuntime {
    pub fn new(config: AntigravityRuntimeConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &AntigravityRuntimeConfig {
        &self.config
    }

    pub fn resolve_executable(&self) -> Result<PathBuf, String> {
        let explicit = env::var_os("RELAY_AGY_EXECUTABLE").is_some()
            || self.config.executable != Path::new("agy");
        resolve_executable(&self.config.executable, explicit)
    }

    pub async fn discover(&self) -> AntigravityRuntimeHealth {
        let executable = match self.resolve_executable() {
            Ok(executable) => executable,
            Err(error) => return AntigravityRuntimeHealth::NotInstalled { error },
        };

        let version = match discover_command(&executable, &["--version"]).await {
            Ok(output) if output.status.success() => {
                String::from_utf8_lossy(&output.stdout).trim().to_owned()
            }
            Ok(output) => {
                return AntigravityRuntimeHealth::DiscoveryFailed {
                    error: safe_process_error(&output.stderr, "agy --version returned a failure"),
                };
            }
            Err(DiscoveryError::Spawn(error)) => {
                return AntigravityRuntimeHealth::DiscoveryFailed { error };
            }
            Err(DiscoveryError::Timeout) => {
                return AntigravityRuntimeHealth::DiscoveryFailed {
                    error: "agy --version timed out".to_owned(),
                };
            }
        };

        match discover_command(&executable, &["auth", "status"]).await {
            Ok(output) if output.status.success() => {}
            Ok(output) => {
                return AntigravityRuntimeHealth::NotAuthenticated {
                    version: Some(version),
                    error: safe_process_error(
                        &output.stderr,
                        "agy auth status reported no active login",
                    ),
                };
            }
            Err(DiscoveryError::Spawn(error)) => {
                return AntigravityRuntimeHealth::DiscoveryFailed { error };
            }
            Err(DiscoveryError::Timeout) => {
                return AntigravityRuntimeHealth::DiscoveryFailed {
                    error: "agy auth status timed out".to_owned(),
                };
            }
        };

        match discover_command(&executable, &["--help"]).await {
            Ok(output) if output.status.success() => {
                AntigravityRuntimeHealth::Available { version }
            }
            Ok(output) => AntigravityRuntimeHealth::Incompatible {
                version,
                reason: safe_process_error(&output.stderr, "AGY compatibility probe failed"),
            },
            Err(DiscoveryError::Spawn(error)) => {
                AntigravityRuntimeHealth::DiscoveryFailed { error }
            }
            Err(DiscoveryError::Timeout) => AntigravityRuntimeHealth::DiscoveryFailed {
                error: "AGY compatibility probe timed out".to_owned(),
            },
        }
    }
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
    result_path: PathBuf,
}

impl RuntimeTempFiles {
    fn create() -> Result<Self, RuntimeError> {
        let id = Uuid::new_v4();
        let root = env::temp_dir();
        let result_path = root.join(format!("relay-agy-{id}-result.json"));
        Ok(Self { result_path })
    }

    async fn read_result(&self) -> Result<RelayResult, String> {
        let metadata = tokio::fs::metadata(&self.result_path)
            .await
            .map_err(|error| format!("missing AGY structured result: {error}"))?;
        if metadata.len() > MAX_RESULT_BYTES {
            return Err("AGY structured result exceeded the 64 KiB limit".to_owned());
        }
        let bytes = tokio::fs::read(&self.result_path)
            .await
            .map_err(|error| format!("could not read AGY structured result: {error}"))?;
        let mut result: RelayResult = serde_json::from_slice(&bytes)
            .map_err(|error| format!("invalid AGY structured result: {error}"))?;
        result.summary = bounded_result_text(&result.summary);
        result.verification_summary = bounded_result_text(&result.verification_summary);
        Ok(result)
    }
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

fn bounded_result_text(input: &str) -> String {
    let mut safe = input
        .chars()
        .filter(|character| *character == '\n' || *character == '\t' || !character.is_control())
        .collect::<String>();
    truncate_utf8_to_bytes(&mut safe, MAX_RESULT_TEXT_BYTES);
    safe
}

impl Drop for RuntimeTempFiles {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.result_path);
    }
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

fn render_agy_json_event(event: &Value) -> Option<Vec<u8>> {
    let kind = event.get("type")?.as_str()?;
    let text = match kind {
        "thread.started" => "› AGY session started".to_owned(),
        "turn.started" => "› AGY started working".to_owned(),
        "error" => format!(
            "× AGY: {}",
            bounded_event_text(
                event
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("runtime error")
            )
        ),
        _ => return None,
    };
    Some(format!("{text}\n").into_bytes())
}

fn bounded_event_text(input: &str) -> String {
    let mut safe: String = input
        .chars()
        .filter(|character| *character == '\n' || *character == '\t' || !character.is_control())
        .collect();
    truncate_utf8_to_bytes(&mut safe, EVENT_OUTPUT_BYTE_LIMIT);
    safe
}

#[derive(Default)]
struct StreamState {
    turn_completed: bool,
    turn_failed: bool,
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
                return Err("AGY output ended with an unterminated frame".to_owned());
            }
            break;
        }
        pending.extend_from_slice(&chunk[..read]);
        while let Some(newline) = pending.iter().position(|byte| *byte == b'\n') {
            let line = pending.drain(..=newline).collect::<Vec<_>>();
            if line.len() - 1 > MAX_OUTPUT_FRAME_BYTES {
                return Err("AGY output frame exceeded limit".to_owned());
            }
            let line = String::from_utf8(line[..line.len() - 1].to_vec())
                .map_err(|_| "AGY output frame was not valid UTF-8".to_owned())?;

            if let Ok(event) = serde_json::from_str::<Value>(&line) {
                let kind = event
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                match kind {
                    "turn.completed" => state.turn_completed = true,
                    "turn.failed" => state.turn_failed = true,
                    _ => {}
                }

                if let Some(rendered) = render_agy_json_event(&event) {
                    let _ = tx
                        .send(RuntimeEvent::Output {
                            stream: RuntimeOutputStream::Stdout,
                            bytes: rendered,
                        })
                        .await;
                }
            } else {
                let _ = tx
                    .send(RuntimeEvent::Output {
                        stream: RuntimeOutputStream::Stdout,
                        bytes: format!("{}\n", line).into_bytes(),
                    })
                    .await;
            }
        }
        if pending.len() > MAX_OUTPUT_FRAME_BYTES {
            return Err("AGY output frame exceeded limit".to_owned());
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
                let _ = tx.send(RuntimeEvent::Outcome(RuntimeOutcome::new(RuntimeOutcomeKind::Cancelled, "AGY execution cancelled"))).await;
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
            format!("AGY process wait failed: {error}"),
        )),
        (Ok(status), _, _) if !status.success() => RuntimeEvent::Outcome(RuntimeOutcome::new(
            RuntimeOutcomeKind::RuntimeFailure,
            format!("AGY exited with {status}"),
        )),
        (Ok(_), Ok(stream), Ok(_)) if stream.turn_failed => {
            RuntimeEvent::Outcome(RuntimeOutcome::new(
                RuntimeOutcomeKind::RuntimeFailure,
                "AGY reported turn.failed",
            ))
        }
        (Ok(_), Ok(stream), Ok(_)) if !stream.turn_completed => RuntimeEvent::InvalidOutput {
            reason: "AGY exited without turn.completed".to_owned(),
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

impl Runtime for AntigravityRuntime {
    fn identity(&self) -> RuntimeIdentity {
        RuntimeIdentity {
            runtime_id: "antigravity".to_owned(),
            profile_id: self.config.profile_id.clone(),
            display_name: "Antigravity CLI".to_owned(),
        }
    }

    fn capabilities(&self) -> RuntimeCapabilities {
        RuntimeCapabilities {
            streaming_output: true,
            cancellation: true,
            deterministic_outcome: true,
        }
    }

    fn start(
        &self,
        request: RuntimeRequest,
        cancellation: CancellationToken,
    ) -> RuntimeStartFuture {
        let runtime = self.clone();
        Box::pin(async move {
            if cancellation.is_cancelled() {
                return Err(RuntimeError::StartFailed(
                    "execution cancelled before Antigravity start".to_owned(),
                ));
            }
            if let Some(stack) = request.intent.requested_stack.as_deref()
                && stack != "antigravity"
            {
                return Err(RuntimeError::StartFailed(format!(
                    "unsupported runtime stack '{stack}'"
                )));
            }
            let temp = RuntimeTempFiles::create()?;
            let executable = runtime
                .resolve_executable()
                .map_err(RuntimeError::StartFailed)?;

            let mut args = vec![
                "-p".to_owned(),
                "--cwd".to_owned(),
                request.trusted_working_dir.display().to_string(),
                "--output-format".to_owned(),
                "stream-json".to_owned(),
                "--sandbox".to_owned(),
            ];

            if request.profile_id.ends_with("/review") {
                args.push("--mode=plan".to_owned());
            } else {
                args.push("--mode=accept-edits".to_owned());
            }

            if let Some(model) = &runtime.config.model_override {
                args.extend(["--model".to_owned(), model.clone()]);
            }
            if let Some(effort) = &runtime.config.effort {
                args.extend(["--effort".to_owned(), effort.clone()]);
            }

            args.push("-".to_owned());

            let mut command = launcher_command(&executable, &args);
            command
                .current_dir(&request.trusted_working_dir)
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
                    "could not start configured AGY executable '{}': {error}",
                    executable.display()
                ))
            })?;

            let mut stdin = child
                .stdin
                .take()
                .ok_or_else(|| RuntimeError::StartFailed("AGY stdin was unavailable".to_owned()))?;
            let stdout = child.stdout.take().ok_or_else(|| {
                RuntimeError::StartFailed("AGY stdout was unavailable".to_owned())
            })?;
            let stderr = child.stderr.take().ok_or_else(|| {
                RuntimeError::StartFailed("AGY stderr was unavailable".to_owned())
            })?;

            let prompt = build_prompt(&request);
            stdin.write_all(prompt.as_bytes()).await.map_err(|error| {
                RuntimeError::StartFailed(format!("could not write AGY prompt: {error}"))
            })?;
            stdin.shutdown().await.map_err(|error| {
                RuntimeError::StartFailed(format!("could not close AGY stdin: {error}"))
            })?;

            let (tx, rx) = mpsc::channel(32);
            tokio::spawn(async move {
                run_child(child, stdout, stderr, tx, cancellation, temp).await;
            });

            Ok(RuntimeSession {
                session_id: format!("agy-{}", request.execution_id),
                events: rx,
            })
        })
    }
}
