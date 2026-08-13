//! Canonical Relay wire protocol types and parsers.
//!
//! This crate validates task protocol bytes. It does not know about local
//! projects, repositories, runtimes, or execution policy.

use std::collections::{BTreeMap, BTreeSet};

use serde::{
    Deserialize, Deserializer,
    de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor},
};
use serde_json::Value;
use thiserror::Error;

const SUPPORTED_PROTOCOL_VERSION: u8 = 1;
const MARKER_START: &str = "<!-- relay:task:start -->";
const MARKER_END: &str = "<!-- relay:task:end -->";
const DUPLICATE_KEY_PREFIX: &str = "duplicate JSON object key: ";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HandoffTask {
    pub protocol_version: u8,
    pub project: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceMetadata>,
    pub task: TaskSpec,
    pub ambiguity_policy: AmbiguityPolicy,
    #[serde(default, deserialize_with = "deserialize_extensions")]
    pub extensions: BTreeMap<String, Value>,
}

impl HandoffTask {
    fn validate(self) -> Result<Self, ProtocolError> {
        if self.protocol_version != SUPPORTED_PROTOCOL_VERSION {
            return Err(ProtocolError::UnsupportedProtocolVersion {
                version: self.protocol_version,
            });
        }

        validate_non_empty("project", &self.project)?;
        validate_non_empty("title", &self.title)?;
        if let Some(stack) = &self.stack {
            validate_logical_label("stack", stack)?;
        }
        if let Some(source) = &self.source {
            source.validate()?;
        }
        self.task.validate()?;

        for namespace in self.extensions.keys() {
            validate_non_empty("extensions namespace", namespace)?;
        }

        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceMetadata {
    pub kind: String,
    pub provider: String,
}

impl SourceMetadata {
    fn validate(&self) -> Result<(), ProtocolError> {
        validate_non_empty("source.kind", &self.kind)?;
        validate_non_empty("source.provider", &self.provider)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSpec {
    pub goal: String,
    #[serde(default)]
    pub requirements: Vec<String>,
    #[serde(default)]
    pub constraints: Vec<String>,
    #[serde(default)]
    pub verification: Vec<String>,
}

impl TaskSpec {
    fn validate(&self) -> Result<(), ProtocolError> {
        validate_non_empty("task.goal", &self.goal)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AmbiguityPolicy {
    NeedsClarification,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProtocolError {
    #[error("malformed JSON: {0}")]
    MalformedJson(String),
    #[error("duplicate JSON object key '{0}'")]
    DuplicateJsonKey(String),
    #[error("unsupported protocol version {version}")]
    UnsupportedProtocolVersion { version: u8 },
    #[error("invalid HandoffTask: {0}")]
    InvalidTask(String),
    #[error("missing Relay task block")]
    MissingMarkdownTaskBlock,
    #[error("incomplete Relay task block")]
    IncompleteMarkdownTaskBlock,
    #[error("multiple Relay task blocks")]
    MultipleMarkdownTaskBlocks,
    #[error("Relay task block must be a JSON fenced code block")]
    NonJsonMarkdownTaskBlock,
}

pub fn parse_handoff_task_json(input: &str) -> Result<HandoffTask, ProtocolError> {
    reject_duplicate_json_object_keys(input)?;
    serde_json::from_str::<HandoffTask>(input)
        .map_err(|error| ProtocolError::InvalidTask(error.to_string()))?
        .validate()
}

pub fn parse_handoff_task_markdown(input: &str) -> Result<HandoffTask, ProtocolError> {
    let json = extract_markdown_json(input)?;
    parse_handoff_task_json(json)
}

pub fn parse_handoff_task_text(input: &str) -> Result<HandoffTask, ProtocolError> {
    match parse_handoff_task_json(input) {
        Ok(task) => Ok(task),
        Err(json_error) => match parse_handoff_task_markdown(input) {
            Ok(task) => Ok(task),
            Err(markdown_error) => {
                if input.contains(MARKER_START) || input.contains(MARKER_END) {
                    Err(markdown_error)
                } else {
                    Err(json_error)
                }
            }
        },
    }
}

pub fn extract_markdown_json(input: &str) -> Result<&str, ProtocolError> {
    let start_count = input.matches(MARKER_START).count();
    let end_count = input.matches(MARKER_END).count();

    match (start_count, end_count) {
        (0, 0) => return Err(ProtocolError::MissingMarkdownTaskBlock),
        (1, 1) => {}
        (0, _) | (_, 0) => return Err(ProtocolError::IncompleteMarkdownTaskBlock),
        _ => return Err(ProtocolError::MultipleMarkdownTaskBlocks),
    }

    let start = input
        .find(MARKER_START)
        .expect("checked exactly one start marker");
    let after_start = start + MARKER_START.len();
    let end = input
        .find(MARKER_END)
        .expect("checked exactly one end marker");
    if end <= after_start {
        return Err(ProtocolError::IncompleteMarkdownTaskBlock);
    }

    let block = input[after_start..end].trim();
    let mut lines = block.lines();
    let first = lines
        .next()
        .ok_or(ProtocolError::NonJsonMarkdownTaskBlock)?;
    if first.trim() != "```json" {
        return Err(ProtocolError::NonJsonMarkdownTaskBlock);
    }

    let mut content_lines = lines.collect::<Vec<_>>();
    let Some(last) = content_lines.pop() else {
        return Err(ProtocolError::NonJsonMarkdownTaskBlock);
    };
    if last.trim() != "```" {
        return Err(ProtocolError::NonJsonMarkdownTaskBlock);
    }

    let content_start = block
        .find('\n')
        .map(|index| index + 1)
        .ok_or(ProtocolError::NonJsonMarkdownTaskBlock)?;
    let content_end = block
        .rfind(last)
        .ok_or(ProtocolError::NonJsonMarkdownTaskBlock)?;

    Ok(block[content_start..content_end].trim())
}

fn deserialize_extensions<'de, D>(deserializer: D) -> Result<BTreeMap<String, Value>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    match value {
        Value::Object(map) => Ok(map.into_iter().collect()),
        _ => Err(de::Error::custom("extensions must be a JSON object")),
    }
}

fn validate_non_empty(field: &str, value: &str) -> Result<(), ProtocolError> {
    if value.trim().is_empty() {
        Err(ProtocolError::InvalidTask(format!(
            "{field} cannot be empty"
        )))
    } else {
        Ok(())
    }
}

fn validate_logical_label(field: &str, value: &str) -> Result<(), ProtocolError> {
    validate_non_empty(field, value)?;
    let trimmed = value.trim();
    if trimmed
        .chars()
        .any(|char| !(char.is_ascii_alphanumeric() || matches!(char, '-' | '_' | '.')))
    {
        return Err(ProtocolError::InvalidTask(format!(
            "{field} must be a logical label, not a path or command"
        )));
    }
    Ok(())
}

fn reject_duplicate_json_object_keys(input: &str) -> Result<(), ProtocolError> {
    let mut deserializer = serde_json::Deserializer::from_str(input);
    AnyJsonSeed
        .deserialize(&mut deserializer)
        .map_err(|error| duplicate_or_malformed(error.to_string()))?;
    deserializer
        .end()
        .map_err(|error| ProtocolError::MalformedJson(error.to_string()))
}

fn duplicate_or_malformed(message: String) -> ProtocolError {
    if let Some(key) = message.strip_prefix(DUPLICATE_KEY_PREFIX) {
        let key = key.split_once(" at line ").map_or(key, |(key, _)| key);
        ProtocolError::DuplicateJsonKey(key.to_owned())
    } else {
        ProtocolError::MalformedJson(message)
    }
}

struct AnyJsonSeed;

impl<'de> DeserializeSeed<'de> for AnyJsonSeed {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(AnyJsonVisitor)
    }
}

struct AnyJsonVisitor;

impl<'de> Visitor<'de> for AnyJsonVisitor {
    type Value = ();

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("any valid JSON value")
    }

    fn visit_bool<E>(self, _value: bool) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_i64<E>(self, _value: i64) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_u64<E>(self, _value: u64) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_f64<E>(self, _value: f64) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_str<E>(self, _value: &str) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_string<E>(self, _value: String) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        while seq.next_element_seed(AnyJsonSeed)?.is_some() {}
        Ok(())
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut keys = BTreeSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !keys.insert(key.clone()) {
                return Err(de::Error::custom(format!("{DUPLICATE_KEY_PREFIX}{key}")));
            }
            map.next_value_seed(AnyJsonSeed)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"{
        "protocol_version": 1,
        "project": "practicore",
        "title": "Implement placement reassignment",
        "task": {
            "goal": "Implement approved behavior."
        },
        "ambiguity_policy": "needs_clarification"
    }"#;

    const FULL: &str = r#"{
        "protocol_version": 1,
        "project": "practicore",
        "title": "Implement placement reassignment",
        "stack": "chatgpt-codex",
        "source": {
            "kind": "conversational_ai",
            "provider": "openai"
        },
        "task": {
            "goal": "Implement approved behavior.",
            "requirements": ["Preserve history."],
            "constraints": ["Do not push automatically."],
            "verification": ["Run tests."]
        },
        "ambiguity_policy": "needs_clarification",
        "extensions": {
            "openai": {
                "codex": {
                    "use_project_skills": true
                }
            }
        }
    }"#;

    #[test]
    fn parses_minimal_valid_handoff_task() {
        let task = parse_handoff_task_json(MINIMAL).unwrap();

        assert_eq!(task.protocol_version, 1);
        assert_eq!(task.project, "practicore");
        assert_eq!(task.stack, None);
        assert_eq!(task.source, None);
        assert!(task.task.requirements.is_empty());
        assert!(task.task.constraints.is_empty());
        assert!(task.task.verification.is_empty());
    }

    #[test]
    fn parses_full_canonical_example_and_preserves_extensions() {
        let task = parse_handoff_task_json(FULL).unwrap();

        assert_eq!(task.stack.as_deref(), Some("chatgpt-codex"));
        assert_eq!(
            task.source,
            Some(SourceMetadata {
                kind: "conversational_ai".to_owned(),
                provider: "openai".to_owned(),
            })
        );
        assert_eq!(task.task.requirements, ["Preserve history."]);
        assert_eq!(
            task.extensions["openai"]["codex"]["use_project_skills"],
            Value::Bool(true)
        );
    }

    #[test]
    fn rejects_malformed_json() {
        assert!(matches!(
            parse_handoff_task_json("{"),
            Err(ProtocolError::MalformedJson(_))
        ));
    }

    #[test]
    fn rejects_unsupported_protocol_version() {
        let error = parse_handoff_task_json(
            &MINIMAL.replace("\"protocol_version\": 1", "\"protocol_version\": 2"),
        )
        .unwrap_err();

        assert_eq!(
            error,
            ProtocolError::UnsupportedProtocolVersion { version: 2 }
        );
    }

    #[test]
    fn rejects_wrong_protocol_version_type() {
        assert!(matches!(
            parse_handoff_task_json(
                &MINIMAL.replace("\"protocol_version\": 1", "\"protocol_version\": \"1\"")
            ),
            Err(ProtocolError::InvalidTask(_))
        ));
    }

    #[test]
    fn rejects_missing_and_empty_required_fields() {
        for (json, expected) in [
            (MINIMAL.replace("\"project\": \"practicore\",", ""), "missing field `project`"),
            (MINIMAL.replace("\"title\": \"Implement placement reassignment\",", ""), "missing field `title`"),
            (MINIMAL.replace("\"task\": {\n            \"goal\": \"Implement approved behavior.\"\n        },", ""), "missing field `task`"),
            (MINIMAL.replace("\"goal\": \"Implement approved behavior.\"", ""), "missing field `goal`"),
        ] {
            let error = parse_handoff_task_json(&json).unwrap_err();
            assert!(error.to_string().contains(expected), "{error}");
        }

        for json in [
            MINIMAL.replace("\"project\": \"practicore\"", "\"project\": \" \""),
            MINIMAL.replace(
                "\"title\": \"Implement placement reassignment\"",
                "\"title\": \"\"",
            ),
            MINIMAL.replace(
                "\"goal\": \"Implement approved behavior.\"",
                "\"goal\": \"\"",
            ),
        ] {
            assert!(matches!(
                parse_handoff_task_json(&json),
                Err(ProtocolError::InvalidTask(_))
            ));
        }
    }

    #[test]
    fn rejects_unsupported_ambiguity_policy() {
        assert!(matches!(
            parse_handoff_task_json(&MINIMAL.replace("needs_clarification", "decide_for_me")),
            Err(ProtocolError::InvalidTask(_))
        ));
    }

    #[test]
    fn rejects_unknown_fields_and_dangerous_direct_semantics() {
        for field in [
            "cwd",
            "path",
            "working_directory",
            "executable",
            "executable_path",
            "shell",
            "command",
            "commands",
            "credentials",
            "credential",
            "token",
            "auth_token",
            "expected_remote",
            "environment",
            "env",
        ] {
            let json = MINIMAL.replace(
                "\"ambiguity_policy\": \"needs_clarification\"",
                &format!("\"ambiguity_policy\": \"needs_clarification\", \"{field}\": \"bad\""),
            );
            assert!(
                matches!(
                    parse_handoff_task_json(&json),
                    Err(ProtocolError::InvalidTask(_))
                ),
                "{field} should be rejected"
            );
        }
    }

    #[test]
    fn rejects_path_like_stack_values() {
        for stack in ["C:/tools/codex.exe", "../codex", "codex --danger"] {
            let json = FULL.replace(
                "\"stack\": \"chatgpt-codex\"",
                &format!("\"stack\": \"{stack}\""),
            );
            assert!(matches!(
                parse_handoff_task_json(&json),
                Err(ProtocolError::InvalidTask(_))
            ));
        }
    }

    #[test]
    fn rejects_unknown_task_and_source_fields() {
        let task_field = MINIMAL.replace(
            "\"goal\": \"Implement approved behavior.\"",
            "\"goal\": \"Implement approved behavior.\", \"command\": \"cargo test\"",
        );
        assert!(matches!(
            parse_handoff_task_json(&task_field),
            Err(ProtocolError::InvalidTask(_))
        ));

        let source_field = FULL.replace(
            "\"provider\": \"openai\"",
            "\"provider\": \"openai\", \"token\": \"secret\"",
        );
        assert!(matches!(
            parse_handoff_task_json(&source_field),
            Err(ProtocolError::InvalidTask(_))
        ));
    }

    #[test]
    fn rejects_malformed_source_and_extensions() {
        let malformed_source = FULL.replace("\"kind\": \"conversational_ai\"", "\"kind\": []");
        assert!(matches!(
            parse_handoff_task_json(&malformed_source),
            Err(ProtocolError::InvalidTask(_))
        ));

        let malformed_extensions = MINIMAL.replace(
            "\"ambiguity_policy\": \"needs_clarification\"",
            "\"ambiguity_policy\": \"needs_clarification\", \"extensions\": []",
        );
        assert!(matches!(
            parse_handoff_task_json(&malformed_extensions),
            Err(ProtocolError::InvalidTask(_))
        ));
    }

    #[test]
    fn rejects_duplicate_json_object_keys() {
        let json = MINIMAL.replace(
            "\"project\": \"practicore\"",
            "\"project\": \"practicore\", \"project\": \"other\"",
        );

        assert_eq!(
            parse_handoff_task_json(&json).unwrap_err(),
            ProtocolError::DuplicateJsonKey("project".to_owned())
        );
    }

    #[test]
    fn extracts_markdown_json_block_and_ignores_prose() {
        let markdown = format!(
            "# Wrong title\nProject: wrong\n\n{MARKER_START}\n```json\n{MINIMAL}\n```\n{MARKER_END}\n"
        );
        let task = parse_handoff_task_markdown(&markdown).unwrap();

        assert_eq!(task.project, "practicore");
        assert_eq!(task.title, "Implement placement reassignment");
    }

    #[test]
    fn rejects_bad_markdown_carriers() {
        assert_eq!(
            parse_handoff_task_markdown("no block").unwrap_err(),
            ProtocolError::MissingMarkdownTaskBlock
        );
        assert_eq!(
            parse_handoff_task_markdown(&format!("{MARKER_START}\n```json\n{{}}\n```"))
                .unwrap_err(),
            ProtocolError::IncompleteMarkdownTaskBlock
        );
        assert_eq!(
            parse_handoff_task_markdown(&format!(
                "{MARKER_START}\n```text\n{{}}\n```\n{MARKER_END}"
            ))
            .unwrap_err(),
            ProtocolError::NonJsonMarkdownTaskBlock
        );
        assert_eq!(
            parse_handoff_task_markdown(&format!(
                "{MARKER_START}\n```json\n{MINIMAL}\n```\n{MARKER_END}\n{MARKER_START}\n```json\n{MINIMAL}\n```\n{MARKER_END}"
            ))
            .unwrap_err(),
            ProtocolError::MultipleMarkdownTaskBlocks
        );
    }
}
