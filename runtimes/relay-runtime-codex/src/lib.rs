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
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
    sync::mpsc,
    time::timeout,
};
use uuid::Uuid;

const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexRuntimeConfig {
    pub executable: PathBuf,
    pub profile_id: String,
    pub model_override: Option<String>,
}

impl Default for CodexRuntimeConfig {
    fn default() -> Self {
        Self {
            executable: env::var_os("RELAY_CODEX_EXECUTABLE")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("codex")),
            profile_id: "codex/default".to_owned(),
            model_override: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexRuntimeHealth {
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

    /// Checks CLI availability and existing CLI authentication without starting an agent turn.
    pub async fn discover(&self) -> CodexRuntimeHealth {
        let version = match discover_command(&self.config.executable, &["--version"]).await {
            Ok(output) if output.status.success() => {
                String::from_utf8_lossy(&output.stdout).trim().to_owned()
            }
            Ok(output) => {
                return CodexRuntimeHealth::NotInstalled {
                    error: safe_process_error(&output.stderr, "codex --version returned a failure"),
                };
            }
            Err(DiscoveryError::Spawn(error)) => return CodexRuntimeHealth::NotInstalled { error },
            Err(DiscoveryError::Timeout) => {
                return CodexRuntimeHealth::DiscoveryFailed {
                    error: "codex --version timed out".to_owned(),
                };
            }
        };
        match discover_command(&self.config.executable, &["login", "status"]).await {
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
            &self.config.executable,
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
        let mut args = vec![
            "--ask-for-approval".to_owned(),
            "never".to_owned(),
            "exec".to_owned(),
            "--ignore-user-config".to_owned(),
            "--strict-config".to_owned(),
            "--json".to_owned(),
            "--color".to_owned(),
            "never".to_owned(),
            "--sandbox".to_owned(),
            "workspace-write".to_owned(),
            "-c".to_owned(),
            "sandbox_workspace_write.writable_roots=[]".to_owned(),
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
            executable: self.config.executable.clone(),
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
            let temp = RuntimeTempFiles::create()?;
            let spec = runtime.command_spec(&request, &temp.schema_path, &temp.result_path);
            let mut command = Command::new(&spec.executable);
            command
                .args(&spec.args)
                .current_dir(&spec.current_dir)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
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
                Some("item.completed")
                    if item.get("exit_code").and_then(Value::as_i64) == Some(0) =>
                {
                    Some(format!("✓ {command}"))
                }
                Some("item.completed") => Some(format!("× {command} failed")),
                _ => None,
            }
        }
        "file_change" => item
            .get("path")
            .and_then(Value::as_str)
            .map(|path| format!("› Modified {}", bounded_event_text(path))),
        "reasoning" => Some("› Codex is analyzing the task".to_owned()),
        _ => None,
    }
}

fn bounded_event_text(input: &str) -> String {
    let mut safe: String = input
        .chars()
        .filter(|character| *character == '\n' || *character == '\t' || !character.is_control())
        .collect();
    if safe.len() > EVENT_OUTPUT_BYTE_LIMIT {
        safe.truncate(EVENT_OUTPUT_BYTE_LIMIT);
        safe.push('…');
    }
    safe
}

async fn read_stdout(
    stdout: tokio::process::ChildStdout,
    tx: mpsc::Sender<RuntimeEvent>,
) -> Result<StreamState, String> {
    let mut lines = BufReader::new(stdout).lines();
    let mut state = StreamState::default();
    while let Some(line) = lines.next_line().await.map_err(|error| error.to_string())? {
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
    Ok(state)
}

async fn read_stderr(
    stderr: tokio::process::ChildStderr,
    tx: mpsc::Sender<RuntimeEvent>,
) -> Result<(), String> {
    let mut lines = BufReader::new(stderr).lines();
    while let Some(line) = lines.next_line().await.map_err(|error| error.to_string())? {
        let _ = tx
            .send(RuntimeEvent::Output {
                stream: RuntimeOutputStream::Stderr,
                bytes: format!("{line}\n").into_bytes(),
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
        let bytes = tokio::fs::read(&self.result_path)
            .await
            .map_err(|error| format!("missing Codex structured result: {error}"))?;
        serde_json::from_slice(&bytes)
            .map_err(|error| format!("invalid Codex structured result: {error}"))
    }
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
async fn discover_command(
    executable: &Path,
    args: &[&str],
) -> Result<std::process::Output, DiscoveryError> {
    match timeout(
        DISCOVERY_TIMEOUT,
        Command::new(executable).args(args).output(),
    )
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
    [
        "--json",
        "--ignore-user-config",
        "--strict-config",
        "--sandbox",
        "--cd",
        "--output-schema",
        "--output-last-message",
        "-c",
    ]
    .iter()
    .all(|flag| help.contains(flag))
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
        assert!(spec.args.contains(&"--strict-config".to_owned()));
        assert!(
            spec.args
                .windows(2)
                .any(|args| args == ["-c", "sandbox_workspace_write.writable_roots=[]"])
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
}
