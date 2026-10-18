//! Typed Herdr CLI transport.
//!
//! Security rule: invoke Herdr with argv only; never use a shell or build
//! `send-text` input from task text.

use std::{
    collections::HashMap,
    fmt,
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use tokio::{process::Command, time::timeout};

use super::client::HerdrClientError;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);
const EXPECTED_PROTOCOL: u32 = 22;

/// Captured result of one Herdr CLI invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrCommandOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Runs a Herdr command with an argv vector (excluding the executable name).
pub trait HerdrCommandRunner: Send + Sync {
    async fn run(&self, argv: &[String]) -> Result<HerdrCommandOutput, HerdrClientError>;
}

/// Production runner. It starts `herdr` directly and never invokes a shell.
#[derive(Debug, Clone)]
pub struct ProcessHerdrRunner {
    executable: PathBuf,
    timeout: Duration,
}

impl Default for ProcessHerdrRunner {
    fn default() -> Self {
        Self {
            executable: PathBuf::from("herdr"),
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

impl ProcessHerdrRunner {
    pub fn new(executable: impl Into<PathBuf>, timeout: Duration) -> Self {
        Self {
            executable: executable.into(),
            timeout,
        }
    }

    pub fn executable(&self) -> &Path {
        &self.executable
    }
}

impl HerdrCommandRunner for ProcessHerdrRunner {
    async fn run(&self, argv: &[String]) -> Result<HerdrCommandOutput, HerdrClientError> {
        let mut command = Command::new(&self.executable);
        command.args(argv).kill_on_drop(true);

        let result = timeout(self.timeout, command.output()).await.map_err(|_| {
            HerdrClientError::Timeout(format!("Herdr CLI exceeded {:?}", self.timeout))
        })?;

        let output = result.map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                HerdrClientError::ExecutableNotFound(self.executable.display().to_string())
            } else {
                HerdrClientError::Transport(format!(
                    "failed to run Herdr executable {}: {error}",
                    self.executable.display()
                ))
            }
        })?;

        Ok(HerdrCommandOutput {
            exit_code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

/// A parsed semantic version printed by `herdr --version`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HerdrVersion {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre_release: Option<String>,
    pub build: Option<String>,
}

impl fmt::Display for HerdrVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(pre_release) = &self.pre_release {
            write!(formatter, "-{pre_release}")?;
        }
        if let Some(build) = &self.build {
            write!(formatter, "+{build}")?;
        }
        Ok(())
    }
}

impl FromStr for HerdrVersion {
    type Err = String;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let mut version = input.trim();
        if let Some(rest) = version
            .strip_prefix("herdr ")
            .or_else(|| version.strip_prefix("Herdr "))
        {
            version = rest.trim();
        }
        if let Some(rest) = version.strip_prefix('v') {
            version = rest;
        }

        let (without_build, build) = match version.split_once('+') {
            Some((base, metadata)) if !metadata.is_empty() && !metadata.contains('+') => {
                validate_identifiers(metadata, false)?;
                (base, Some(metadata.to_owned()))
            }
            Some(_) => return Err("invalid semantic version build metadata".into()),
            None => (version, None),
        };
        let (core, pre_release) = match without_build.split_once('-') {
            Some((base, metadata)) if !metadata.is_empty() => {
                validate_identifiers(metadata, true)?;
                (base, Some(metadata.to_owned()))
            }
            Some(_) => return Err("invalid semantic version pre-release metadata".into()),
            None => (without_build, None),
        };

        let mut components = core.split('.');
        let major = parse_numeric_component(components.next(), "major")?;
        let minor = parse_numeric_component(components.next(), "minor")?;
        let patch = parse_numeric_component(components.next(), "patch")?;
        if components.next().is_some() {
            return Err("semantic version must have exactly three numeric components".into());
        }

        Ok(Self {
            major,
            minor,
            patch,
            pre_release,
            build,
        })
    }
}

fn parse_numeric_component(value: Option<&str>, name: &str) -> Result<u64, String> {
    let value = value.ok_or_else(|| format!("semantic version is missing {name}"))?;
    if value.is_empty()
        || !value.bytes().all(|byte| byte.is_ascii_digit())
        || (value.len() > 1 && value.starts_with('0'))
    {
        return Err(format!("invalid semantic version {name}: {value}"));
    }
    value
        .parse()
        .map_err(|_| format!("semantic version {name} is too large"))
}

fn validate_identifiers(value: &str, pre_release: bool) -> Result<(), String> {
    for identifier in value.split('.') {
        if identifier.is_empty()
            || !identifier
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            || (pre_release
                && identifier.len() > 1
                && identifier.starts_with('0')
                && identifier.bytes().all(|byte| byte.is_ascii_digit()))
        {
            return Err(format!("invalid semantic version identifier: {identifier}"));
        }
    }
    Ok(())
}

/// The result of checking the installed Herdr version and CLI protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrCompatibility {
    pub version: HerdrVersion,
    pub protocol: u32,
}

/// The workspace fields used by Relay. Unknown Herdr fields are ignored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HerdrWorkspace {
    pub workspace_id: String,
    pub number: u32,
    pub label: String,
    pub focused: bool,
    pub pane_count: u32,
    pub tab_count: u32,
    pub active_tab_id: String,
    pub agent_status: String,
    #[serde(default)]
    pub tokens: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HerdrTab {
    pub tab_id: String,
    pub workspace_id: String,
    pub number: u32,
    pub label: String,
    pub focused: bool,
    pub pane_count: u32,
    pub agent_status: String,
}

/// Pane fields needed by the typed Herdr operations. Additional protocol
/// fields remain forward-compatible and are ignored during deserialization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HerdrCliPaneInfo {
    pub pane_id: String,
    pub terminal_id: String,
    pub workspace_id: String,
    pub tab_id: String,
    pub focused: bool,
    pub agent_status: String,
    pub revision: u64,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub foreground_cwd: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub terminal_title: Option<String>,
    #[serde(default)]
    pub terminal_title_stripped: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub display_agent: Option<String>,
    #[serde(default)]
    pub state_labels: HashMap<String, String>,
    #[serde(default)]
    pub tokens: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HerdrWorkspaceCreation {
    pub workspace: HerdrWorkspace,
    pub tab: HerdrTab,
    pub root_pane: HerdrCliPaneInfo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HerdrTabCreation {
    pub tab: HerdrTab,
    pub root_pane: HerdrCliPaneInfo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HerdrPaneProcess {
    pub pid: u32,
    pub name: String,
    #[serde(default)]
    pub argv: Option<Vec<String>>,
    #[serde(default)]
    pub argv0: Option<String>,
    #[serde(default)]
    pub cmdline: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HerdrProcessInfo {
    pub pane_id: String,
    #[serde(default)]
    pub foreground_process_group_id: Option<u32>,
    #[serde(default)]
    pub foreground_processes: Vec<HerdrPaneProcess>,
    #[serde(default)]
    pub shell_pid: Option<u32>,
    #[serde(default)]
    pub tty: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HerdrSplitDirection {
    Right,
    Down,
}

impl HerdrSplitDirection {
    fn as_cli_arg(self) -> &'static str {
        match self {
            Self::Right => "right",
            Self::Down => "down",
        }
    }
}

/// Typed operations supported by the Herdr CLI transport.
#[allow(async_fn_in_trait)]
pub trait HerdrOps: Send + Sync {
    async fn version(&self) -> Result<HerdrVersion, HerdrClientError>;
    async fn protocol(&self) -> Result<u32, HerdrClientError>;
    async fn workspace_create(
        &self,
        cwd: &Path,
        label: &str,
    ) -> Result<HerdrWorkspaceCreation, HerdrClientError>;
    async fn workspace_get(&self, id: &str) -> Result<HerdrWorkspace, HerdrClientError>;
    async fn workspace_list(&self) -> Result<Vec<HerdrWorkspace>, HerdrClientError>;
    async fn tab_create(
        &self,
        workspace: &str,
        cwd: &Path,
        label: &str,
    ) -> Result<HerdrTabCreation, HerdrClientError>;
    async fn tab_list(&self, workspace: &str) -> Result<Vec<HerdrTab>, HerdrClientError>;
    async fn pane_get(&self, id: &str) -> Result<Option<HerdrCliPaneInfo>, HerdrClientError>;
    async fn pane_list(&self, workspace: &str) -> Result<Vec<HerdrCliPaneInfo>, HerdrClientError>;
    async fn pane_split(
        &self,
        pane: &str,
        direction: HerdrSplitDirection,
        cwd: &Path,
    ) -> Result<HerdrCliPaneInfo, HerdrClientError>;
    async fn pane_read(&self, pane: &str, lines: usize) -> Result<String, HerdrClientError>;
    async fn pane_process_info(&self, pane: &str) -> Result<HerdrProcessInfo, HerdrClientError>;
    async fn pane_send_keys(&self, pane: &str, keys: &[String]) -> Result<(), HerdrClientError>;
    async fn pane_close(&self, pane: &str) -> Result<(), HerdrClientError>;
    async fn pane_report_metadata(
        &self,
        pane: &str,
        source: &str,
        title: &str,
        tokens: &[(String, String)],
    ) -> Result<(), HerdrClientError>;
}

/// Herdr CLI client using a command runner. The generic runner keeps tests
/// deterministic without requiring a live Herdr server.
pub struct HerdrCli<R> {
    runner: R,
}

impl<R> HerdrCli<R> {
    pub fn new(runner: R) -> Self {
        Self { runner }
    }

    pub fn runner(&self) -> &R {
        &self.runner
    }
}

impl<R: HerdrCommandRunner> HerdrCli<R> {
    async fn json_result<T: DeserializeOwned>(
        &self,
        argv: Vec<String>,
        command: &str,
        expected_type: &str,
    ) -> Result<T, HerdrClientError> {
        let output = self.runner.run(&argv).await?;
        let result = parse_json_result(output, command)?;
        decode_typed_result(result, expected_type, command)
    }

    async fn plain_text(
        &self,
        argv: Vec<String>,
        command: &str,
    ) -> Result<String, HerdrClientError> {
        let output = self.runner.run(&argv).await?;
        if output.exit_code == 0 {
            return Ok(output.stdout);
        }
        Err(error_from_failed_output(output, command))
    }
}

#[derive(Debug, Deserialize)]
struct WorkspaceResponse {
    workspace: HerdrWorkspace,
}

#[derive(Debug, Deserialize)]
struct WorkspaceListResponse {
    workspaces: Vec<HerdrWorkspace>,
}

#[derive(Debug, Deserialize)]
struct TabListResponse {
    tabs: Vec<HerdrTab>,
}

#[derive(Debug, Deserialize)]
struct PaneResponse {
    pane: HerdrCliPaneInfo,
}

#[derive(Debug, Deserialize)]
struct PaneListResponse {
    panes: Vec<HerdrCliPaneInfo>,
}

#[derive(Debug, Deserialize)]
struct ProcessInfoResponse {
    process_info: HerdrProcessInfo,
}

#[derive(Debug, Deserialize)]
struct HerdrOk {
    #[serde(rename = "type")]
    _kind: String,
}

#[derive(Debug, Deserialize)]
struct WorkspaceCreatedResponse {
    workspace: HerdrWorkspace,
    tab: HerdrTab,
    root_pane: HerdrCliPaneInfo,
}

#[derive(Debug, Deserialize)]
struct TabCreatedResponse {
    tab: HerdrTab,
    root_pane: HerdrCliPaneInfo,
}

#[derive(Debug, Deserialize)]
struct PaneCreatedResponse {
    pane: HerdrCliPaneInfo,
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn parse_json_result(output: HerdrCommandOutput, command: &str) -> Result<Value, HerdrClientError> {
    let envelope: Value = serde_json::from_str(&output.stdout).map_err(|error| {
        HerdrClientError::Transport(format!(
            "invalid Herdr JSON from {command}: {error}; output: {}",
            bounded_snippet(&output.stdout, &output.stderr)
        ))
    })?;

    if let Some(result) = envelope.get("result") {
        return Ok(result.clone());
    }
    if let Some(error) = envelope.get("error") {
        return Err(parse_error_object(error).unwrap_or_else(|| {
            HerdrClientError::Transport(format!(
                "invalid Herdr error envelope from {command}: {}",
                bounded_snippet(&output.stdout, &output.stderr)
            ))
        }));
    }

    Err(HerdrClientError::Transport(format!(
        "Herdr {command} exited with code {} without a result envelope: {}",
        output.exit_code,
        bounded_snippet(&output.stdout, &output.stderr)
    )))
}

fn decode_typed_result<T: DeserializeOwned>(
    result: Value,
    expected_type: &str,
    command: &str,
) -> Result<T, HerdrClientError> {
    let found_type = result.get("type").and_then(Value::as_str);
    if found_type != Some(expected_type) {
        return Err(HerdrClientError::Transport(format!(
            "unexpected Herdr result for {command}: expected type {expected_type}, found {}",
            found_type.unwrap_or("<missing>")
        )));
    }
    serde_json::from_value(result).map_err(|error| {
        HerdrClientError::Transport(format!("invalid Herdr {command} result: {error}"))
    })
}

fn parse_error_object(error: &Value) -> Option<HerdrClientError> {
    let code = error.get("code")?.as_str()?.to_owned();
    let message = error.get("message")?.as_str()?.to_owned();
    if code == "not_found" || code.ends_with("_not_found") {
        Some(HerdrClientError::ResourceNotFound(message))
    } else {
        Some(HerdrClientError::CommandError { code, message })
    }
}

fn error_from_failed_output(output: HerdrCommandOutput, command: &str) -> HerdrClientError {
    if let Ok(envelope) = serde_json::from_str::<Value>(&output.stdout)
        && let Some(error) = envelope.get("error")
        && let Some(error) = parse_error_object(error)
    {
        return error;
    }
    HerdrClientError::Transport(format!(
        "Herdr {command} exited with code {}: {}",
        output.exit_code,
        bounded_snippet(&output.stdout, &output.stderr)
    ))
}

fn bounded_snippet(stdout: &str, stderr: &str) -> String {
    let combined = match (stdout.trim(), stderr.trim()) {
        ("", "") => "<empty output>".to_owned(),
        ("", stderr) => format!("stderr: {stderr}"),
        (stdout, "") => stdout.to_owned(),
        (stdout, stderr) => format!("{stdout} | stderr: {stderr}"),
    };
    let mut snippet: String = combined.chars().take(240).collect();
    if combined.chars().count() > 240 {
        snippet.push_str("…");
    }
    snippet
}

fn parse_version_output(output: HerdrCommandOutput) -> Result<HerdrVersion, HerdrClientError> {
    if output.exit_code != 0 {
        return Err(error_from_failed_output(output, "--version"));
    }
    let line = output
        .stdout
        .lines()
        .find(|line| !line.trim().is_empty())
        .ok_or_else(|| HerdrClientError::Transport("Herdr --version returned no version".into()))?;
    HerdrVersion::from_str(line).map_err(|error| {
        HerdrClientError::Transport(format!(
            "could not parse Herdr semantic version: {error}; output: {}",
            bounded_snippet(line, &output.stderr)
        ))
    })
}

fn parse_protocol_output(output: HerdrCommandOutput) -> Result<u32, HerdrClientError> {
    if output.exit_code != 0 {
        return Err(error_from_failed_output(output, "api schema"));
    }
    for line in output.stdout.lines() {
        if let Some(value) = line.trim().strip_prefix("protocol:") {
            return value.trim().parse().map_err(|error| {
                HerdrClientError::Transport(format!(
                    "could not parse Herdr protocol number: {error}; output: {}",
                    bounded_snippet(&output.stdout, &output.stderr)
                ))
            });
        }
    }
    Err(HerdrClientError::Transport(format!(
        "Herdr api schema output did not contain a protocol number: {}",
        bounded_snippet(&output.stdout, &output.stderr)
    )))
}

#[allow(async_fn_in_trait)]
impl<R: HerdrCommandRunner> HerdrOps for HerdrCli<R> {
    async fn version(&self) -> Result<HerdrVersion, HerdrClientError> {
        parse_version_output(self.runner.run(&["--version".into()]).await?)
    }

    async fn protocol(&self) -> Result<u32, HerdrClientError> {
        parse_protocol_output(self.runner.run(&["api".into(), "schema".into()]).await?)
    }

    async fn workspace_create(
        &self,
        cwd: &Path,
        label: &str,
    ) -> Result<HerdrWorkspaceCreation, HerdrClientError> {
        let result: WorkspaceCreatedResponse = self
            .json_result(
                vec![
                    "workspace".into(),
                    "create".into(),
                    "--cwd".into(),
                    path_arg(cwd),
                    "--label".into(),
                    label.into(),
                    "--no-focus".into(),
                ],
                "workspace create",
                "workspace_created",
            )
            .await?;
        Ok(HerdrWorkspaceCreation {
            workspace: result.workspace,
            tab: result.tab,
            root_pane: result.root_pane,
        })
    }

    async fn workspace_get(&self, id: &str) -> Result<HerdrWorkspace, HerdrClientError> {
        let result: WorkspaceResponse = self
            .json_result(
                vec!["workspace".into(), "get".into(), id.into()],
                "workspace get",
                "workspace_info",
            )
            .await?;
        Ok(result.workspace)
    }

    async fn workspace_list(&self) -> Result<Vec<HerdrWorkspace>, HerdrClientError> {
        let result: WorkspaceListResponse = self
            .json_result(
                vec!["workspace".into(), "list".into()],
                "workspace list",
                "workspace_list",
            )
            .await?;
        Ok(result.workspaces)
    }

    async fn tab_create(
        &self,
        workspace: &str,
        cwd: &Path,
        label: &str,
    ) -> Result<HerdrTabCreation, HerdrClientError> {
        let result: TabCreatedResponse = self
            .json_result(
                vec![
                    "tab".into(),
                    "create".into(),
                    "--workspace".into(),
                    workspace.into(),
                    "--cwd".into(),
                    path_arg(cwd),
                    "--label".into(),
                    label.into(),
                    "--no-focus".into(),
                ],
                "tab create",
                "tab_created",
            )
            .await?;
        Ok(HerdrTabCreation {
            tab: result.tab,
            root_pane: result.root_pane,
        })
    }

    async fn tab_list(&self, workspace: &str) -> Result<Vec<HerdrTab>, HerdrClientError> {
        let result: TabListResponse = self
            .json_result(
                vec![
                    "tab".into(),
                    "list".into(),
                    "--workspace".into(),
                    workspace.into(),
                ],
                "tab list",
                "tab_list",
            )
            .await?;
        Ok(result.tabs)
    }

    async fn pane_get(&self, id: &str) -> Result<Option<HerdrCliPaneInfo>, HerdrClientError> {
        let output = self
            .runner
            .run(&["pane".into(), "get".into(), id.into()])
            .await?;
        let result = match parse_json_result(output, "pane get") {
            Ok(result) => result,
            Err(HerdrClientError::ResourceNotFound(_)) => return Ok(None),
            Err(error) => return Err(error),
        };
        let result: PaneResponse = decode_typed_result(result, "pane_info", "pane get")?;
        Ok(Some(result.pane))
    }

    async fn pane_list(&self, workspace: &str) -> Result<Vec<HerdrCliPaneInfo>, HerdrClientError> {
        let result: PaneListResponse = self
            .json_result(
                vec![
                    "pane".into(),
                    "list".into(),
                    "--workspace".into(),
                    workspace.into(),
                ],
                "pane list",
                "pane_list",
            )
            .await?;
        Ok(result.panes)
    }

    async fn pane_split(
        &self,
        pane: &str,
        direction: HerdrSplitDirection,
        cwd: &Path,
    ) -> Result<HerdrCliPaneInfo, HerdrClientError> {
        let result: PaneCreatedResponse = self
            .json_result(
                vec![
                    "pane".into(),
                    "split".into(),
                    pane.into(),
                    "--direction".into(),
                    direction.as_cli_arg().into(),
                    "--cwd".into(),
                    path_arg(cwd),
                    "--no-focus".into(),
                ],
                "pane split",
                "pane_info",
            )
            .await?;
        Ok(result.pane)
    }

    async fn pane_read(&self, pane: &str, lines: usize) -> Result<String, HerdrClientError> {
        self.plain_text(
            vec![
                "pane".into(),
                "read".into(),
                pane.into(),
                "--source".into(),
                "recent".into(),
                "--lines".into(),
                lines.to_string(),
                "--format".into(),
                "text".into(),
            ],
            "pane read",
        )
        .await
    }

    async fn pane_process_info(&self, pane: &str) -> Result<HerdrProcessInfo, HerdrClientError> {
        let result: ProcessInfoResponse = self
            .json_result(
                vec![
                    "pane".into(),
                    "process-info".into(),
                    "--pane".into(),
                    pane.into(),
                ],
                "pane process-info",
                "pane_process_info",
            )
            .await?;
        Ok(result.process_info)
    }

    async fn pane_send_keys(&self, pane: &str, keys: &[String]) -> Result<(), HerdrClientError> {
        let mut argv = vec!["pane".into(), "send-keys".into(), pane.into()];
        argv.extend(keys.iter().cloned());
        let _: HerdrOk = self.json_result(argv, "pane send-keys", "ok").await?;
        Ok(())
    }

    async fn pane_close(&self, pane: &str) -> Result<(), HerdrClientError> {
        let _: HerdrOk = self
            .json_result(
                vec!["pane".into(), "close".into(), pane.into()],
                "pane close",
                "ok",
            )
            .await?;
        Ok(())
    }

    async fn pane_report_metadata(
        &self,
        pane: &str,
        source: &str,
        title: &str,
        tokens: &[(String, String)],
    ) -> Result<(), HerdrClientError> {
        let mut argv = vec![
            "pane".into(),
            "report-metadata".into(),
            "--source".into(),
            source.into(),
            pane.into(),
            "--title".into(),
            title.into(),
        ];
        for (key, value) in tokens {
            argv.push("--token".into());
            argv.push(format!("{key}={value}"));
        }
        let _: HerdrOk = self.json_result(argv, "pane report-metadata", "ok").await?;
        Ok(())
    }
}

/// Verifies that the installed Herdr matches the version/protocol this client
/// understands. Every mismatch fails closed.
pub async fn check_compatibility<O: HerdrOps>(
    ops: &O,
) -> Result<HerdrCompatibility, HerdrClientError> {
    let version = ops.version().await?;
    let protocol = ops.protocol().await?;
    if version.major != 0
        || version.minor != 9
        || version.pre_release.is_some()
        || protocol != EXPECTED_PROTOCOL
    {
        return Err(HerdrClientError::IncompatibleVersion {
            server_version: format!("Herdr {version} with protocol {protocol}"),
            expected_min: "Herdr 0.9.x release with protocol 22".into(),
        });
    }
    Ok(HerdrCompatibility { version, protocol })
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, path::Path, sync::Mutex};

    use super::*;

    #[derive(Default)]
    struct ScriptedRunner {
        outputs: Mutex<VecDeque<HerdrCommandOutput>>,
        calls: Mutex<Vec<Vec<String>>>,
    }

    impl ScriptedRunner {
        fn new(outputs: impl IntoIterator<Item = HerdrCommandOutput>) -> Self {
            Self {
                outputs: Mutex::new(outputs.into_iter().collect()),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<Vec<String>> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl HerdrCommandRunner for ScriptedRunner {
        async fn run(&self, argv: &[String]) -> Result<HerdrCommandOutput, HerdrClientError> {
            self.calls.lock().unwrap().push(argv.to_vec());
            self.outputs
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| HerdrClientError::Transport("no scripted response".into()))
        }
    }

    fn output(stdout: &str) -> HerdrCommandOutput {
        HerdrCommandOutput {
            exit_code: 0,
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }

    fn result(kind: &str, fields: &str) -> HerdrCommandOutput {
        output(&format!(
            "{{\"id\":\"cli:test\",\"result\":{{\"type\":\"{kind}\"{fields}}}}}"
        ))
    }

    fn workspace_json(id: &str, tab_id: &str) -> String {
        format!(
            "{{\"workspace_id\":\"{id}\",\"number\":1,\"label\":\"workspace\",\"focused\":false,\"pane_count\":1,\"tab_count\":1,\"active_tab_id\":\"{tab_id}\",\"agent_status\":\"unknown\"}}"
        )
    }

    fn tab_json(id: &str, workspace: &str) -> String {
        format!(
            "{{\"tab_id\":\"{id}\",\"workspace_id\":\"{workspace}\",\"number\":1,\"label\":\"1\",\"focused\":false,\"pane_count\":1,\"agent_status\":\"unknown\"}}"
        )
    }

    fn pane_json(id: &str, workspace: &str, tab: &str) -> String {
        format!(
            "{{\"pane_id\":\"{id}\",\"terminal_id\":\"term-test\",\"workspace_id\":\"{workspace}\",\"tab_id\":\"{tab}\",\"focused\":false,\"agent_status\":\"unknown\",\"revision\":0,\"cwd\":\"C:\\\\work\\\\repo\",\"future_field\":true}}"
        )
    }

    fn wrapped_result(kind: &str, field: &str, payload: &str) -> HerdrCommandOutput {
        result(kind, &format!(",\"{field}\":{payload}"))
    }

    #[tokio::test]
    async fn every_operation_builds_one_exact_argv_vector() {
        let ws = workspace_json("w1", "w1:t1");
        let tab = tab_json("w1:t1", "w1");
        let pane = pane_json("w1:p1", "w1", "w1:t1");
        let outputs = vec![
            output("herdr 0.9.5\n"),
            output("Herdr API schema\nprotocol: 22\nschema_version: 1\n"),
            wrapped_result(
                "workspace_created",
                "workspace",
                &format!("{ws},\"tab\":{tab},\"root_pane\":{pane}"),
            ),
            wrapped_result("workspace_info", "workspace", &ws),
            result("workspace_list", &format!(",\"workspaces\":[{ws}]")),
            result(
                "tab_created",
                &format!(",\"tab\":{tab},\"root_pane\":{pane}"),
            ),
            result("tab_list", &format!(",\"tabs\":[{tab}]")),
            wrapped_result("pane_info", "pane", &pane),
            result("pane_list", &format!(",\"panes\":[{pane}]")),
            wrapped_result("pane_info", "pane", &pane),
            output("recent terminal text\n"),
            wrapped_result(
                "pane_process_info",
                "process_info",
                &format!("{{\"pane_id\":\"w1:p1\",\"foreground_processes\":[]}}"),
            ),
            result("ok", ""),
            result("ok", ""),
            result("ok", ""),
        ];
        let runner = ScriptedRunner::new(outputs);
        let cli = HerdrCli::new(runner);
        let hostile_cwd = Path::new(r#"C:\work\repo "quoted"; & $(literal)"#);
        let hostile_label = r#"Label "quoted"; & $(literal)"#;

        cli.version().await.unwrap();
        cli.protocol().await.unwrap();
        let created = cli
            .workspace_create(hostile_cwd, hostile_label)
            .await
            .unwrap();
        assert_eq!(created.workspace.workspace_id, "w1");
        cli.workspace_get("w1").await.unwrap();
        cli.workspace_list().await.unwrap();
        cli.tab_create("w1", Path::new(r"C:\work\repo"), "second tab")
            .await
            .unwrap();
        cli.tab_list("w1").await.unwrap();
        cli.pane_get("w1:p1").await.unwrap();
        cli.pane_list("w1").await.unwrap();
        cli.pane_split(
            "w1:p1",
            HerdrSplitDirection::Right,
            Path::new(r"C:\work\repo"),
        )
        .await
        .unwrap();
        cli.pane_read("w1:p1", 17).await.unwrap();
        cli.pane_process_info("w1:p1").await.unwrap();
        cli.pane_send_keys("w1:p1", &["ctrl+c".into(), "enter".into()])
            .await
            .unwrap();
        cli.pane_close("w1:p1").await.unwrap();
        cli.pane_report_metadata(
            "w1:p1",
            "relay",
            "A title with spaces",
            &[
                ("task".into(), "T 01".into()),
                ("attempt".into(), "A1".into()),
            ],
        )
        .await
        .unwrap();

        assert_eq!(
            cli.runner().calls(),
            vec![
                vec!["--version"],
                vec!["api", "schema"],
                vec![
                    "workspace",
                    "create",
                    "--cwd",
                    r#"C:\work\repo "quoted"; & $(literal)"#,
                    "--label",
                    r#"Label "quoted"; & $(literal)"#,
                    "--no-focus"
                ],
                vec!["workspace", "get", "w1"],
                vec!["workspace", "list"],
                vec![
                    "tab",
                    "create",
                    "--workspace",
                    "w1",
                    "--cwd",
                    r"C:\work\repo",
                    "--label",
                    "second tab",
                    "--no-focus"
                ],
                vec!["tab", "list", "--workspace", "w1"],
                vec!["pane", "get", "w1:p1"],
                vec!["pane", "list", "--workspace", "w1"],
                vec![
                    "pane",
                    "split",
                    "w1:p1",
                    "--direction",
                    "right",
                    "--cwd",
                    r"C:\work\repo",
                    "--no-focus"
                ],
                vec![
                    "pane", "read", "w1:p1", "--source", "recent", "--lines", "17", "--format",
                    "text"
                ],
                vec!["pane", "process-info", "--pane", "w1:p1"],
                vec!["pane", "send-keys", "w1:p1", "ctrl+c", "enter"],
                vec!["pane", "close", "w1:p1"],
                vec![
                    "pane",
                    "report-metadata",
                    "--source",
                    "relay",
                    "w1:p1",
                    "--title",
                    "A title with spaces",
                    "--token",
                    "task=T 01",
                    "--token",
                    "attempt=A1"
                ],
            ]
            .into_iter()
            .map(|argv| argv.into_iter().map(String::from).collect::<Vec<_>>())
            .collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn parses_sanitized_sample_envelopes_and_plain_text_read() {
        let runner = ScriptedRunner::new([
            output(include_str!("../../tests/fixtures/herdr/pane-get.json")),
            HerdrCommandOutput {
                exit_code: 1,
                stdout: include_str!("../../tests/fixtures/herdr/pane-get-missing.json").into(),
                stderr: String::new(),
            },
            output(include_str!(
                "../../tests/fixtures/herdr/workspace-get.json"
            )),
            output(include_str!("../../tests/fixtures/herdr/tab-list.json")),
            output(include_str!("../../tests/fixtures/herdr/pane-list.json")),
            output(include_str!(
                "../../tests/fixtures/herdr/pane-process-info.json"
            )),
            output(include_str!("../../tests/fixtures/herdr/pane-read.txt")),
        ]);
        let cli = HerdrCli::new(runner);

        let pane = cli.pane_get("w4C:p1").await.unwrap().unwrap();
        assert_eq!(pane.pane_id, "w4C:p1");
        assert_eq!(pane.cwd.as_deref(), Some(r"C:\work\repo"));
        assert_eq!(cli.pane_get("w99:p9").await.unwrap(), None);
        assert_eq!(cli.workspace_get("w4C").await.unwrap().workspace_id, "w4C");
        assert_eq!(cli.tab_list("w4C").await.unwrap()[0].tab_id, "w4C:t1");
        assert_eq!(cli.pane_list("w4C").await.unwrap().len(), 2);
        let info = cli.pane_process_info("w4C:p1").await.unwrap();
        assert_eq!(info.foreground_processes[0].name, "powershell.exe");
        assert_eq!(info.foreground_processes[0].argv.as_ref().unwrap().len(), 2);
        assert_eq!(
            cli.pane_read("w4C:p1", 20).await.unwrap(),
            include_str!("../../tests/fixtures/herdr/pane-read.txt")
        );
    }

    #[tokio::test]
    async fn malformed_json_is_a_transport_error_with_a_bounded_snippet() {
        let runner = ScriptedRunner::new([output(&"x".repeat(600))]);
        let cli = HerdrCli::new(runner);
        let error = cli.pane_get("w1:p1").await.unwrap_err();
        let HerdrClientError::Transport(message) = error else {
            panic!("expected transport error");
        };
        assert!(message.contains("invalid Herdr JSON"));
        assert!(message.len() < 400);
    }

    #[tokio::test]
    async fn compatibility_accepts_only_version_09_and_protocol_22() {
        let compatible = HerdrCli::new(ScriptedRunner::new([
            output(include_str!("../../tests/fixtures/herdr/version.txt")),
            output(include_str!(
                "../../tests/fixtures/herdr/schema-summary.txt"
            )),
        ]));
        let result = check_compatibility(&compatible).await.unwrap();
        assert_eq!(result.version.to_string(), "0.9.5");
        assert_eq!(result.protocol, 22);

        let wrong_version = HerdrCli::new(ScriptedRunner::new([
            output("herdr 1.0.0\n"),
            output("protocol: 22\n"),
        ]));
        assert!(matches!(
            check_compatibility(&wrong_version).await,
            Err(HerdrClientError::IncompatibleVersion { .. })
        ));

        let wrong_protocol = HerdrCli::new(ScriptedRunner::new([
            output("herdr 0.9.5\n"),
            output("protocol: 23\n"),
        ]));
        assert!(matches!(
            check_compatibility(&wrong_protocol).await,
            Err(HerdrClientError::IncompatibleVersion { .. })
        ));
    }

    #[tokio::test]
    async fn missing_executable_is_a_typed_error() {
        let runner = ProcessHerdrRunner::new(
            Path::new(r"C:\missing\herdr-does-not-exist.exe"),
            Duration::from_secs(1),
        );
        let error = runner.run(&["--version".into()]).await.unwrap_err();
        assert!(matches!(error, HerdrClientError::ExecutableNotFound(_)));
    }
}
