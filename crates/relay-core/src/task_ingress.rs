use std::{fmt, fs, path::Path};

use relay_protocol::{
    HandoffTask, ProtocolError, parse_handoff_task_json, parse_handoff_task_markdown,
    parse_handoff_task_text,
};
use thiserror::Error;
use uuid::Uuid;

use crate::{ProjectKey, ProjectRegistryError, Relay};

/// Relay-owned task identity, separate from all planner and transport IDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TaskId(Uuid);

impl TaskId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
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

/// How a task was submitted to Relay locally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmissionTransport {
    LocalFile,
    LocalPaste,
}

impl SubmissionTransport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LocalFile => "local-file",
            Self::LocalPaste => "local-paste",
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
    pub id: TaskId,
    pub handoff: HandoffTask,
    pub project_key: ProjectKey,
    pub project_name: Option<String>,
    pub submission_transport: SubmissionTransport,
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
        let handoff = parse_handoff_task_text(input)?;
        self.task_import_candidate(handoff, SubmissionTransport::LocalPaste)
    }

    pub fn preview_local_file(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<TaskImportCandidate, TaskIngressError> {
        let path = path.as_ref();
        ensure_supported_local_task_extension(path)?;
        let input = fs::read_to_string(path).map_err(|source| TaskIngressError::FileRead {
            path: path.display().to_string(),
            source,
        })?;
        let handoff = parse_file_content(path, &input)?;
        self.task_import_candidate(handoff, SubmissionTransport::LocalFile)
    }

    pub fn confirm_task_import(&mut self, candidate: TaskImportCandidate) -> &ImportedTask {
        self.accept_imported_task(candidate.into_imported_task())
    }

    pub fn accept_imported_task(&mut self, imported: ImportedTask) -> &ImportedTask {
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
        for project in ["../relay", "C:/Users/PCC/Relay", "/tmp/relay"] {
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

        assert_eq!(imported.project_key.as_str(), "relay");
        assert!(!imported.id.to_string().is_empty());
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
            imported.submission_transport,
            SubmissionTransport::LocalFile
        );
        assert_eq!(
            imported.handoff.source.as_ref().unwrap().provider,
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
