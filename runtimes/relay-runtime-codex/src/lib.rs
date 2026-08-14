//! Codex CLI adapter for Relay's provider-neutral runtime boundary.
//!
//! All executable selection is trusted local configuration. Task content is
//! supplied only on stdin and is never interpreted as shell syntax.

use std::{
    env,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use relay_runtime::{
    CancellationToken, Runtime, RuntimeAction, RuntimeCapabilities, RuntimeError, RuntimeEvent,
    RuntimeIdentity, RuntimeOutcome, RuntimeOutcomeKind, RuntimeOutputStream, RuntimeRequest,
    RuntimeSession, RuntimeStartFuture,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, BufReader},
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
                CodexRuntimeHealth::Available { version }
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
            "workspace-write".to_owned(),
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
        if let Some(model) = &self.config.model_override {
            args.extend(["--model".to_owned(), model.clone()]);
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
            })
        })
    }
}

async fn run_child(
    mut child: tokio::process::Child,
    stdout: tokio::process::ChildStdout,
    stderr: tokio::process::ChildStderr,
    tx: mpsc::Sender<RuntimeEvent>,
    cancellation: CancellationToken,
    temp: RuntimeTempFiles,
) {
    let stdout_reader = tokio::spawn(read_stdout(stdout, tx.clone()));
    let stderr_reader = tokio::spawn(read_stderr(stderr, tx.clone()));
    let exit = tokio::select! {
        status = child.wait() => status.map_err(|error| error.to_string()),
        _ = cancellation.cancelled() => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            let _ = tx.send(RuntimeEvent::Outcome(RuntimeOutcome::new(RuntimeOutcomeKind::Cancelled, "Codex execution cancelled"))).await;
            return;
        }
    };
    let stdout_result = stdout_reader
        .await
        .unwrap_or_else(|error| Err(format!("stdout reader failed: {error}")));
    let stderr_result = stderr_reader
        .await
        .unwrap_or_else(|error| Err(format!("stderr reader failed: {error}")));
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
        if pending.len() > MAX_CODEX_JSONL_FRAME_BYTES {
            return Err("Codex JSONL frame exceeded the 256 KiB limit".to_owned());
        }
        while let Some(newline) = pending.iter().position(|byte| *byte == b'\n') {
            let line = pending.drain(..=newline).collect::<Vec<_>>();
            let line = String::from_utf8_lossy(&line[..line.len() - 1]);
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
            if let Some(rendered) = render_codex_json_event(&event) {
                let _ = tx
                    .send(RuntimeEvent::Output {
                        stream: RuntimeOutputStream::Stdout,
                        bytes: rendered,
                    })
                    .await;
            }
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
    if executable
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("cmd"))
    {
        let mut command = Command::new("cmd.exe");
        command.arg("/C").arg(executable).args(args);
        return command;
    }
    let mut command = Command::new(executable);
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
        if key.to_string_lossy().starts_with("GIT_CONFIG_KEY_")
            || key.to_string_lossy().starts_with("GIT_CONFIG_VALUE_")
        {
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
}
