use std::{fmt, fs, io::Read, path::Path};

use relay_protocol::{
    HandoffTask, ProtocolError, parse_handoff_task_json, parse_handoff_task_markdown,
    parse_handoff_task_text,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{ProjectKey, ProjectRegistryError, Relay};

/// Maximum accepted local task input size, in bytes, for both files and pastes.
pub const MAX_LOCAL_TASK_INPUT_BYTES: u64 = 512 * 1024;

/// Relay-owned task identity, separate from all planner and transport IDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TaskId(Uuid);

impl TaskId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn generate() -> Self {
        Self::new()
    }

    pub fn parse(input: impl AsRef<str>) -> Result<Self, uuid::Error> {
        Uuid::parse_str(input.as_ref()).map(Self)
    }
}

impl Default for TaskId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// How a task was submitted to Relay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmissionTransport {
    LocalFile,
    LocalPaste,
    RelayMcp,
}

impl SubmissionTransport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LocalFile => "local-file",
            Self::LocalPaste => "local-paste",
            Self::RelayMcp => "relay-mcp",
        }
    }
}

/// Validated task ready for user confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskImportCandidate {
    handoff: HandoffTask,
    project_key: ProjectKey,
    project_name: Option<String>,
    submission_transport: SubmissionTransport,
}

impl TaskImportCandidate {
    pub fn handoff(&self) -> &HandoffTask {
        &self.handoff
    }

    pub fn project_key(&self) -> &ProjectKey {
        &self.project_key
    }

    pub fn project_name(&self) -> Option<&str> {
        self.project_name.as_deref()
    }

    pub fn submission_transport(&self) -> SubmissionTransport {
        self.submission_transport
    }

    /// Consumes a validated candidate and assigns Relay-owned task identity.
    pub fn into_imported_task(self) -> ImportedTask {
        ImportedTask {
            id: TaskId::new(),
            handoff: self.handoff,
            project_key: self.project_key,
            project_name: self.project_name,
            submission_transport: self.submission_transport,
        }
    }
}

/// Reviewable in-memory task created after explicit import confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedTask {
    id: TaskId,
    handoff: HandoffTask,
    project_key: ProjectKey,
    project_name: Option<String>,
    submission_transport: SubmissionTransport,
}

impl ImportedTask {
    pub fn id(&self) -> TaskId {
        self.id
    }

    pub fn handoff(&self) -> &HandoffTask {
        &self.handoff
    }

    pub fn project_key(&self) -> &ProjectKey {
        &self.project_key
    }

    pub fn project_name(&self) -> Option<&str> {
        self.project_name.as_deref()
    }

    pub fn submission_transport(&self) -> SubmissionTransport {
        self.submission_transport
    }
}

#[derive(Debug, Error)]
pub enum TaskIngressError {
    #[error("unsupported local task file extension for '{path}'")]
    UnsupportedFileExtension { path: String },
    #[error("failed to read local task file '{path}': {source}")]
    FileRead {
        path: String,
        source: std::io::Error,
    },
    #[error("local task file '{path}' is not valid UTF-8")]
    FileNotUtf8 {
        path: String,
        source: std::string::FromUtf8Error,
    },
    #[error(
        "local task input is {actual} bytes, exceeding the maximum of {limit} bytes; reject rather than truncate"
    )]
    InputTooLarge { limit: u64, actual: u64 },
    #[error(
        "handoff extensions are not accepted through MCP submission; resubmit without extensions"
    )]
    ExtensionsNotAllowed,
    #[error("{0}")]
    Protocol(#[from] ProtocolError),
    #[error("{0}")]
    ProjectRegistry(#[from] ProjectRegistryError),
}

impl Relay {
    pub fn preview_local_paste(
        &self,
        input: &str,
    ) -> Result<TaskImportCandidate, TaskIngressError> {
        ensure_input_within_size_limit(input.len(), MAX_LOCAL_TASK_INPUT_BYTES)?;
        let handoff = parse_handoff_task_text(input)?;
        self.task_import_candidate(handoff, SubmissionTransport::LocalPaste)
    }

    pub fn preview_local_file(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<TaskImportCandidate, TaskIngressError> {
        let path = path.as_ref();
        ensure_supported_local_task_extension(path)?;
        let input = read_local_task_file(path)?;
        let handoff = parse_file_content(path, &input)?;
        self.task_import_candidate(handoff, SubmissionTransport::LocalFile)
    }

    /// Validates a machine-initiated MCP `submit_handoff` payload inline.
    ///
    /// Per ADR-0016 the preview/confirm collapse happens at this transport
    /// boundary: validation runs exactly as local paste ingress does (same
    /// protocol parser, size cap, and project registry resolution), but the
    /// human review moves to the TUI before Run. Non-empty extensions are
    /// rejected here so remote submitters cannot smuggle extra namespaces.
    pub fn preview_mcp_submission(
        &self,
        input: &str,
    ) -> Result<TaskImportCandidate, TaskIngressError> {
        ensure_input_within_size_limit(input.len(), MAX_LOCAL_TASK_INPUT_BYTES)?;
        let handoff = parse_handoff_task_json(input)?;
        if !handoff.extensions.is_empty() {
            return Err(TaskIngressError::ExtensionsNotAllowed);
        }
        self.task_import_candidate(handoff, SubmissionTransport::RelayMcp)
    }

    pub fn confirm_task_import(&mut self, candidate: TaskImportCandidate) -> &ImportedTask {
        self.accept_imported_task(candidate.into_imported_task())
    }

    pub(crate) fn accept_imported_task(&mut self, imported: ImportedTask) -> &ImportedTask {
        self.reviewed_task = Some(imported);

        self.reviewed_task
            .as_ref()
            .expect("reviewed task was just set")
    }

    pub fn reviewed_task(&self) -> Option<&ImportedTask> {
        self.reviewed_task.as_ref()
    }

    fn task_import_candidate(
        &self,
        handoff: HandoffTask,
        submission_transport: SubmissionTransport,
    ) -> Result<TaskImportCandidate, TaskIngressError> {
        let project = self.resolve_project(&handoff.project)?;

        Ok(TaskImportCandidate {
            handoff,
            project_key: project.key().clone(),
            project_name: project.name().map(str::to_owned),
            submission_transport,
        })
    }
}

fn ensure_input_within_size_limit(actual: usize, limit: u64) -> Result<(), TaskIngressError> {
    let actual = actual as u64;
    if actual > limit {
        Err(TaskIngressError::InputTooLarge { limit, actual })
    } else {
        Ok(())
    }
}

fn read_local_task_file(path: &Path) -> Result<String, TaskIngressError> {
    let display = path.display().to_string();
    let read_error = |source| TaskIngressError::FileRead {
        path: display.clone(),
        source,
    };
    let mut file = fs::File::open(path).map_err(read_error)?;
    let metadata = file.metadata().map_err(read_error)?;
    ensure_input_within_size_limit(metadata.len() as usize, MAX_LOCAL_TASK_INPUT_BYTES)?;

    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_LOCAL_TASK_INPUT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(read_error)?;
    ensure_input_within_size_limit(bytes.len(), MAX_LOCAL_TASK_INPUT_BYTES)?;

    String::from_utf8(bytes).map_err(|source| TaskIngressError::FileNotUtf8 {
        path: display,
        source,
    })
}

fn ensure_supported_local_task_extension(path: &Path) -> Result<(), TaskIngressError> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    if file_name.ends_with(".relay.json")
        || file_name.ends_with(".json")
        || file_name.ends_with(".relay.md")
        || file_name.ends_with(".md")
    {
        Ok(())
    } else {
        Err(TaskIngressError::UnsupportedFileExtension {
            path: path.display().to_string(),
        })
    }
}

fn parse_file_content(path: &Path, input: &str) -> Result<HandoffTask, ProtocolError> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    if file_name.ends_with(".relay.md") || file_name.ends_with(".md") {
        parse_handoff_task_markdown(input)
    } else {
        parse_handoff_task_json(input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ProjectRegistry, RegisterProject};
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    const TASK_JSON: &str = r#"{
        "protocol_version": 1,
        "project": "Relay",
        "title": "Implement local ingress",
        "source": {
            "kind": "conversational_ai",
            "provider": "openai"
        },
        "task": {
            "goal": "Implement the approved M4 behavior.",
            "requirements": ["Keep import separate from execution."],
            "constraints": ["Do not mutate Git."],
            "verification": ["Run cargo test."]
        },
        "ambiguity_policy": "needs_clarification"
    }"#;

    #[test]
    fn valid_registered_project_imports_successfully() {
        let relay = relay_with_project("relay");

        let candidate = relay.preview_local_paste(TASK_JSON).unwrap();

        assert_eq!(candidate.project_key().as_str(), "relay");
        assert_eq!(candidate.project_name(), Some("Relay"));
        assert_eq!(
            candidate.submission_transport(),
            SubmissionTransport::LocalPaste
        );
    }

    #[test]
    fn normalized_valid_project_key_resolves_through_registry() {
        let relay = relay_with_project("relay");
        let candidate = relay.preview_local_paste(TASK_JSON).unwrap();

        assert_eq!(candidate.project_key().as_str(), "relay");
    }

    #[test]
    fn unknown_project_fails_closed_without_fallback() {
        let relay = relay_with_project("other");
        let error = relay.preview_local_paste(TASK_JSON).unwrap_err();

        assert!(matches!(
            error,
            TaskIngressError::ProjectRegistry(ProjectRegistryError::UnknownProjectKey { .. })
        ));
    }

    #[test]
    fn invalid_project_key_and_filesystem_project_strings_fail() {
        let relay = relay_with_project("relay");
        for project in ["../relay", "C:/Users/example/Relay", "/tmp/relay"] {
            let json = TASK_JSON.replace(
                "\"project\": \"Relay\"",
                &format!("\"project\": \"{project}\""),
            );
            let error = relay.preview_local_paste(&json).unwrap_err();
            assert!(matches!(
                error,
                TaskIngressError::ProjectRegistry(ProjectRegistryError::InvalidProjectKey { .. })
            ));
        }
    }

    #[test]
    fn import_does_not_mutate_project_registry() {
        let relay = relay_with_project("relay");
        let before = relay.project_registry().clone();

        let _candidate = relay.preview_local_paste(TASK_JSON).unwrap();

        assert_eq!(relay.project_registry(), &before);
    }

    #[test]
    fn confirmed_import_receives_relay_owned_task_id() {
        let mut relay = relay_with_project("relay");
        let candidate = relay.preview_local_paste(TASK_JSON).unwrap();

        let imported = relay.confirm_task_import(candidate).clone();

        assert_eq!(imported.project_key().as_str(), "relay");
        assert!(!imported.id().to_string().is_empty());
        assert!(relay.reviewed_task().is_some());
    }

    #[test]
    fn local_file_and_paste_provenance_remain_distinct_from_source() {
        let mut relay = relay_with_project("relay");
        let paste = relay.preview_local_paste(TASK_JSON).unwrap();
        assert_eq!(
            paste.submission_transport(),
            SubmissionTransport::LocalPaste
        );
        assert_eq!(
            paste.handoff().source.as_ref().unwrap().provider,
            "openai".to_owned()
        );

        let file = temp_path("task.relay.json");
        fs::write(&file, TASK_JSON).unwrap();
        let file_candidate = relay.preview_local_file(&file).unwrap();
        assert_eq!(
            file_candidate.submission_transport(),
            SubmissionTransport::LocalFile
        );

        let imported = relay.confirm_task_import(file_candidate).clone();
        assert_eq!(
            imported.submission_transport(),
            SubmissionTransport::LocalFile
        );
        assert_eq!(
            imported.handoff().source.as_ref().unwrap().provider,
            "openai".to_owned()
        );
    }

    #[test]
    fn source_metadata_does_not_affect_trust_or_routing() {
        let relay = relay_with_project("relay");
        let json = TASK_JSON.replace(
            "\"provider\": \"openai\"",
            "\"provider\": \"unknown-provider\"",
        );

        let candidate = relay.preview_local_paste(&json).unwrap();

        assert_eq!(candidate.project_key().as_str(), "relay");
    }

    #[test]
    fn markdown_file_uses_only_canonical_block() {
        let relay = relay_with_project("relay");
        let file = temp_path("task.relay.md");
        fs::write(
            &file,
            format!(
                "# Wrong title\nProject: other\n\n<!-- relay:task:start -->\n```json\n{TASK_JSON}\n```\n<!-- relay:task:end -->"
            ),
        )
        .unwrap();

        let candidate = relay.preview_local_file(&file).unwrap();

        assert_eq!(candidate.handoff().title, "Implement local ingress");
        assert_eq!(candidate.project_key().as_str(), "relay");
    }

    #[test]
    fn m4_does_not_replace_m3_repository_inspection_logic() {
        let relay = relay_with_project("relay");

        let candidate = relay.preview_local_paste(TASK_JSON).unwrap();

        assert_eq!(candidate.project_key().as_str(), "relay");
        assert!(relay.inspect_project_repository("relay").is_ok());
    }

    #[test]
    fn oversized_paste_fails_closed_without_truncation() {
        let relay = relay_with_project("relay");
        let padding = "x".repeat(MAX_LOCAL_TASK_INPUT_BYTES as usize);
        let json = TASK_JSON.replace("Do not mutate Git.", &padding);
        assert!(json.len() as u64 > MAX_LOCAL_TASK_INPUT_BYTES);

        let error = relay.preview_local_paste(&json).unwrap_err();

        assert!(matches!(
            error,
            TaskIngressError::InputTooLarge { limit, actual }
                if limit == MAX_LOCAL_TASK_INPUT_BYTES && actual == json.len() as u64
        ));
    }

    #[test]
    fn paste_exactly_at_size_limit_is_accepted() {
        let relay = relay_with_project("relay");
        let padding = " ".repeat((MAX_LOCAL_TASK_INPUT_BYTES as usize) - TASK_JSON.len());
        let padded = format!("{padding}{TASK_JSON}");
        assert_eq!(padded.len() as u64, MAX_LOCAL_TASK_INPUT_BYTES);

        let candidate = relay.preview_local_paste(&padded).unwrap();

        assert_eq!(candidate.project_key().as_str(), "relay");
    }

    #[test]
    fn oversized_local_file_fails_closed() {
        let relay = relay_with_project("relay");
        let file = temp_path("oversized.relay.json");
        fs::write(&file, vec![b' '; (MAX_LOCAL_TASK_INPUT_BYTES + 1) as usize]).unwrap();

        let error = relay.preview_local_file(&file).unwrap_err();

        assert!(matches!(
            error,
            TaskIngressError::InputTooLarge { limit, .. }
                if limit == MAX_LOCAL_TASK_INPUT_BYTES
        ));
    }

    #[test]
    fn mcp_submission_accepts_canonical_payload_with_relay_mcp_provenance() {
        let relay = relay_with_project("relay");

        let candidate = relay.preview_mcp_submission(TASK_JSON).unwrap();

        assert_eq!(candidate.project_key().as_str(), "relay");
        assert_eq!(
            candidate.submission_transport(),
            SubmissionTransport::RelayMcp
        );
        assert_eq!(SubmissionTransport::RelayMcp.as_str(), "relay-mcp");
    }

    #[test]
    fn mcp_submission_rejects_non_empty_extensions_at_the_boundary() {
        let relay = relay_with_project("relay");
        let json = TASK_JSON.replace(
            "\"ambiguity_policy\": \"needs_clarification\"",
            "\"extensions\": {\"vendor\": {\"x\": 1}}, \"ambiguity_policy\": \"needs_clarification\"",
        );

        let error = relay.preview_mcp_submission(&json).unwrap_err();

        assert!(matches!(error, TaskIngressError::ExtensionsNotAllowed));
    }

    #[test]
    fn empty_extensions_stay_valid_on_both_paths() {
        let relay = relay_with_project("relay");
        let json = TASK_JSON.replace(
            "\"ambiguity_policy\": \"needs_clarification\"",
            "\"extensions\": {}, \"ambiguity_policy\": \"needs_clarification\"",
        );

        assert!(relay.preview_local_paste(&json).is_ok());
        assert!(relay.preview_mcp_submission(&json).is_ok());
    }

    #[test]
    fn mcp_submission_rejects_markdown_framing() {
        let relay = relay_with_project("relay");
        let markdown = format!(
            "<!-- relay:task:start -->\n```json\n{TASK_JSON}\n```\n<!-- relay:task:end -->"
        );

        assert!(relay.preview_mcp_submission(&markdown).is_err());
    }

    #[test]
    fn mcp_submission_rejects_oversized_payload() {
        let relay = relay_with_project("relay");
        let padding = "x".repeat(MAX_LOCAL_TASK_INPUT_BYTES as usize);
        let json = TASK_JSON.replace("Do not mutate Git.", &padding);

        let error = relay.preview_mcp_submission(&json).unwrap_err();

        assert!(matches!(error, TaskIngressError::InputTooLarge { .. }));
    }

    fn relay_with_project(key: &str) -> Relay {
        let dir = temp_dir("project");
        let mut registry = ProjectRegistry::empty();
        registry
            .register(RegisterProject {
                key: key.to_owned(),
                name: Some("Relay".to_owned()),
                local_path: dir,
                expected_remote: None,
                default_stack: Some("chatgpt-codex".to_owned()),
                github: None,
            })
            .unwrap();
        Relay::with_project_registry(registry)
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let path = temp_path(name);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn temp_path(name: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::path::Path::new(name);
        let stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or(name);
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| format!(".{extension}"))
            .unwrap_or_default();
        std::env::temp_dir().join(format!("relay-task-ingress-{stem}-{nanos}{extension}"))
    }
}
