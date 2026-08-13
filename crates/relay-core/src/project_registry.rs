use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

const PROJECT_KEY_MAX_LEN: usize = 64;

/// Validated logical project key supplied by remote tasks and resolved locally.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProjectKey(String);

impl ProjectKey {
    /// Normalizes and validates a logical project key.
    ///
    /// Normalization trims surrounding whitespace and lowercases ASCII letters.
    pub fn parse(input: impl AsRef<str>) -> Result<Self, ProjectRegistryError> {
        let original = input.as_ref();
        let normalized = original.trim().to_ascii_lowercase();

        if normalized.is_empty() {
            return Err(ProjectRegistryError::InvalidProjectKey {
                input: original.to_owned(),
                reason: "project key cannot be empty".to_owned(),
            });
        }

        if normalized.len() > PROJECT_KEY_MAX_LEN {
            return Err(ProjectRegistryError::InvalidProjectKey {
                input: original.to_owned(),
                reason: format!("project key cannot exceed {PROJECT_KEY_MAX_LEN} bytes"),
            });
        }

        if !normalized.is_ascii() {
            return Err(ProjectRegistryError::InvalidProjectKey {
                input: original.to_owned(),
                reason: "project key must be ASCII".to_owned(),
            });
        }

        let mut chars = normalized.chars();
        let first = chars.next().expect("checked non-empty key");
        if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
            return Err(ProjectRegistryError::InvalidProjectKey {
                input: original.to_owned(),
                reason: "project key must start with an ASCII letter or digit".to_owned(),
            });
        }

        let last = normalized.chars().last().expect("checked non-empty key");
        if !last.is_ascii_lowercase() && !last.is_ascii_digit() {
            return Err(ProjectRegistryError::InvalidProjectKey {
                input: original.to_owned(),
                reason: "project key must end with an ASCII letter or digit".to_owned(),
            });
        }

        if normalized.chars().any(|char| {
            !char.is_ascii_lowercase() && !char.is_ascii_digit() && char != '-' && char != '_'
        }) {
            return Err(ProjectRegistryError::InvalidProjectKey {
                input: original.to_owned(),
                reason: "project key may contain only ASCII letters, digits, '-' or '_'".to_owned(),
            });
        }

        Ok(Self(normalized))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ProjectKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Trusted expected remote value owned by local configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedRemote(String);

impl ExpectedRemote {
    pub fn parse(input: impl Into<String>) -> Result<Self, ProjectRegistryError> {
        let remote = input.into();
        let remote = remote.trim();
        if remote.is_empty() {
            return Err(ProjectRegistryError::InvalidExpectedRemote {
                reason: "expected remote cannot be empty when configured".to_owned(),
            });
        }

        Ok(Self(remote.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Optional trusted GitHub repository metadata for future external bindings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitHubRepositoryMetadata {
    pub owner: String,
    pub repo: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub default_labels: Vec<String>,
}

impl GitHubRepositoryMetadata {
    fn validate(&self) -> Result<(), ProjectRegistryError> {
        if self.owner.trim().is_empty() {
            return Err(ProjectRegistryError::InvalidGitHubMetadata {
                reason: "GitHub owner cannot be empty".to_owned(),
            });
        }

        if self.repo.trim().is_empty() {
            return Err(ProjectRegistryError::InvalidGitHubMetadata {
                reason: "GitHub repository cannot be empty".to_owned(),
            });
        }

        if self
            .default_labels
            .iter()
            .any(|label| label.trim().is_empty())
        {
            return Err(ProjectRegistryError::InvalidGitHubMetadata {
                reason: "GitHub labels cannot be empty".to_owned(),
            });
        }

        Ok(())
    }
}

/// Locally trusted project metadata resolved from a logical key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedProject {
    key: ProjectKey,
    name: Option<String>,
    local_path: PathBuf,
    expected_remote: Option<ExpectedRemote>,
    default_stack: Option<String>,
    github: Option<GitHubRepositoryMetadata>,
}

impl TrustedProject {
    pub fn key(&self) -> &ProjectKey {
        &self.key
    }

    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    pub fn local_path(&self) -> &Path {
        &self.local_path
    }

    pub fn expected_remote(&self) -> Option<&ExpectedRemote> {
        self.expected_remote.as_ref()
    }

    pub fn default_stack(&self) -> Option<&str> {
        self.default_stack.as_deref()
    }

    pub fn github(&self) -> Option<&GitHubRepositoryMetadata> {
        self.github.as_ref()
    }
}

/// Request to register a trusted local project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisterProject {
    pub key: String,
    pub name: Option<String>,
    pub local_path: PathBuf,
    pub expected_remote: Option<String>,
    pub default_stack: Option<String>,
    pub github: Option<GitHubRepositoryMetadata>,
}

/// Trusted local registry mapping logical project keys to local metadata.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ProjectRegistry {
    projects: BTreeMap<ProjectKey, TrustedProject>,
}

impl ProjectRegistry {
    pub fn empty() -> Self {
        Self::default()
    }

    /// Loads a registry from TOML. A missing file is an empty registry.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ProjectRegistryError> {
        let path = path.as_ref();
        let toml = match fs::read_to_string(path) {
            Ok(toml) => toml,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Self::empty()),
            Err(source) => {
                return Err(ProjectRegistryError::ConfigRead {
                    path: path.to_path_buf(),
                    source,
                });
            }
        };

        Self::from_toml_str(&toml).map_err(|error| error.with_config_path(path))
    }

    pub fn from_toml_str(toml: &str) -> Result<Self, ProjectRegistryError> {
        let config: RegistryConfigToml =
            toml::from_str(toml).map_err(|source| ProjectRegistryError::ConfigParse {
                source: Box::new(source),
            })?;
        Self::from_config(config)
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), ProjectRegistryError> {
        let path = path.as_ref();
        let toml = self.to_toml_string()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| ProjectRegistryError::ConfigWrite {
                path: path.to_path_buf(),
                source,
            })?;
        }

        fs::write(path, toml).map_err(|source| ProjectRegistryError::ConfigWrite {
            path: path.to_path_buf(),
            source,
        })
    }

    pub fn to_toml_string(&self) -> Result<String, ProjectRegistryError> {
        let config = RegistryConfigToml {
            projects: self
                .projects
                .iter()
                .map(|(key, project)| (key.as_str().to_owned(), ProjectToml::from(project)))
                .collect(),
        };

        toml::to_string_pretty(&config).map_err(|source| ProjectRegistryError::ConfigSerialize {
            source: Box::new(source),
        })
    }

    pub fn register(
        &mut self,
        request: RegisterProject,
    ) -> Result<&TrustedProject, ProjectRegistryError> {
        let project = validate_project(request)?;
        let key = project.key.clone();

        if self.projects.contains_key(&key) {
            return Err(ProjectRegistryError::DuplicateProjectKey { key });
        }

        self.projects.insert(key.clone(), project);
        Ok(self
            .projects
            .get(&key)
            .expect("registered project should be retrievable"))
    }

    pub fn get(&self, key: impl AsRef<str>) -> Result<&TrustedProject, ProjectRegistryError> {
        let key = ProjectKey::parse(key)?;
        self.projects
            .get(&key)
            .ok_or(ProjectRegistryError::UnknownProjectKey { key })
    }

    pub fn list(&self) -> impl ExactSizeIterator<Item = &TrustedProject> {
        self.projects.values()
    }

    pub fn len(&self) -> usize {
        self.projects.len()
    }

    pub fn is_empty(&self) -> bool {
        self.projects.is_empty()
    }

    fn from_config(config: RegistryConfigToml) -> Result<Self, ProjectRegistryError> {
        let mut registry = Self::empty();

        for (raw_key, project) in config.projects {
            let request = RegisterProject {
                key: raw_key,
                name: project.name,
                local_path: project.path,
                expected_remote: project.expected_remote,
                default_stack: project.default_stack,
                github: project.github,
            };
            registry.register(request)?;
        }

        Ok(registry)
    }
}

/// Returns Relay's default local configuration path.
///
/// `RELAY_CONFIG_PATH` overrides platform defaults and is useful for tests.
pub fn default_config_path() -> PathBuf {
    if let Some(path) = env::var_os("RELAY_CONFIG_PATH") {
        return PathBuf::from(path);
    }

    if cfg!(windows) {
        if let Some(app_data) = env::var_os("APPDATA") {
            return PathBuf::from(app_data).join("Relay").join("config.toml");
        }
    }

    if let Some(config_home) = env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(config_home).join("relay").join("config.toml");
    }

    if let Some(home) = env::var_os("HOME") {
        return PathBuf::from(home)
            .join(".config")
            .join("relay")
            .join("config.toml");
    }

    PathBuf::from("relay.toml")
}

fn validate_project(request: RegisterProject) -> Result<TrustedProject, ProjectRegistryError> {
    let key = ProjectKey::parse(&request.key)?;
    let local_path = validate_project_path(Some(&key), &request.local_path)?;
    let expected_remote = request
        .expected_remote
        .map(ExpectedRemote::parse)
        .transpose()?;

    if let Some(default_stack) = request.default_stack.as_deref() {
        if default_stack.trim().is_empty() {
            return Err(ProjectRegistryError::InvalidDefaultStack {
                reason: "default stack cannot be empty when configured".to_owned(),
            });
        }
    }

    if let Some(github) = &request.github {
        github.validate()?;
    }

    Ok(TrustedProject {
        key,
        name: normalize_optional_text(request.name),
        local_path,
        expected_remote,
        default_stack: normalize_optional_text(request.default_stack),
        github: request.github,
    })
}

fn normalize_optional_text(text: Option<String>) -> Option<String> {
    text.and_then(|text| {
        let trimmed = text.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_owned())
    })
}

fn validate_project_path(
    key: Option<&ProjectKey>,
    path: &Path,
) -> Result<PathBuf, ProjectRegistryError> {
    if path.as_os_str().is_empty() {
        return Err(ProjectRegistryError::InvalidProjectPath {
            key: key.cloned(),
            path: path.to_path_buf(),
            reason: "project path cannot be empty".to_owned(),
        });
    }

    if !path.is_absolute() {
        return Err(ProjectRegistryError::InvalidProjectPath {
            key: key.cloned(),
            path: path.to_path_buf(),
            reason: "project path must be absolute".to_owned(),
        });
    }

    let metadata = fs::metadata(path).map_err(|source| ProjectRegistryError::ProjectPathIo {
        key: key.cloned(),
        path: path.to_path_buf(),
        source,
    })?;

    if !metadata.is_dir() {
        return Err(ProjectRegistryError::InvalidProjectPath {
            key: key.cloned(),
            path: path.to_path_buf(),
            reason: "project path must be an existing directory".to_owned(),
        });
    }

    Ok(path.to_path_buf())
}

#[derive(Debug, Error)]
pub enum ProjectRegistryError {
    #[error("failed to read Relay config at {path}: {source}")]
    ConfigRead {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("malformed Relay TOML config: {source}")]
    ConfigParse { source: Box<toml::de::Error> },
    #[error("malformed Relay TOML config at {path}: {source}")]
    ConfigParseAtPath {
        path: PathBuf,
        source: Box<toml::de::Error>,
    },
    #[error("failed to serialize Relay config: {source}")]
    ConfigSerialize { source: Box<toml::ser::Error> },
    #[error("failed to write Relay config at {path}: {source}")]
    ConfigWrite {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid project key '{input}': {reason}")]
    InvalidProjectKey { input: String, reason: String },
    #[error("duplicate project key '{key}'")]
    DuplicateProjectKey { key: ProjectKey },
    #[error("unknown project key '{key}'")]
    UnknownProjectKey { key: ProjectKey },
    #[error("invalid project path{key_text} '{path}': {reason}", key_text = format_key(key))]
    InvalidProjectPath {
        key: Option<ProjectKey>,
        path: PathBuf,
        reason: String,
    },
    #[error("failed to inspect project path{key_text} '{path}': {source}", key_text = format_key(key))]
    ProjectPathIo {
        key: Option<ProjectKey>,
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid expected remote: {reason}")]
    InvalidExpectedRemote { reason: String },
    #[error("invalid default stack: {reason}")]
    InvalidDefaultStack { reason: String },
    #[error("invalid GitHub metadata: {reason}")]
    InvalidGitHubMetadata { reason: String },
}

impl ProjectRegistryError {
    fn with_config_path(self, path: &Path) -> Self {
        match self {
            Self::ConfigParse { source } => Self::ConfigParseAtPath {
                path: path.to_path_buf(),
                source,
            },
            error => error,
        }
    }
}

fn format_key(key: &Option<ProjectKey>) -> String {
    key.as_ref()
        .map(|key| format!(" for key '{}'", key.as_str()))
        .unwrap_or_default()
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RegistryConfigToml {
    #[serde(default)]
    projects: BTreeMap<String, ProjectToml>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProjectToml {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_remote: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    default_stack: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    github: Option<GitHubRepositoryMetadata>,
}

impl From<&TrustedProject> for ProjectToml {
    fn from(project: &TrustedProject) -> Self {
        Self {
            name: project.name.clone(),
            path: project.local_path.clone(),
            expected_remote: project
                .expected_remote
                .as_ref()
                .map(|remote| remote.as_str().to_owned()),
            default_stack: project.default_stack.clone(),
            github: project.github.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn parses_valid_config() {
        let dir = temp_project_dir("valid-config");
        let toml = format!(
            r#"
                [projects.Relay]
                name = "Relay"
                path = "{}"
                expected_remote = "https://github.com/Jacinth091/Relay.git"
                default_stack = "chatgpt-codex"

                [projects.Relay.github]
                owner = "Jacinth091"
                repo = "Relay"
                default_labels = ["relay", "handoff"]
            "#,
            toml_path(&dir)
        );

        let registry = ProjectRegistry::from_toml_str(&toml).expect("valid config should parse");
        let project = registry.get("relay").expect("project should resolve");

        assert_eq!(registry.len(), 1);
        assert_eq!(project.key().as_str(), "relay");
        assert_eq!(project.name(), Some("Relay"));
        assert_eq!(project.local_path(), dir.as_path());
        assert_eq!(
            project.expected_remote().map(ExpectedRemote::as_str),
            Some("https://github.com/Jacinth091/Relay.git")
        );
        assert_eq!(project.default_stack(), Some("chatgpt-codex"));
        assert_eq!(
            project.github(),
            Some(&GitHubRepositoryMetadata {
                owner: "Jacinth091".to_owned(),
                repo: "Relay".to_owned(),
                default_labels: vec!["relay".to_owned(), "handoff".to_owned()],
            })
        );
    }

    #[test]
    fn missing_config_loads_empty_registry() {
        let path = unique_temp_path("missing-config").join("relay.toml");

        let registry = ProjectRegistry::load(&path).expect("missing config should be empty");

        assert!(registry.is_empty());
    }

    #[test]
    fn reports_malformed_toml() {
        let error = ProjectRegistry::from_toml_str("[projects.relay").unwrap_err();

        assert!(matches!(error, ProjectRegistryError::ConfigParse { .. }));
        assert!(error.to_string().contains("malformed Relay TOML config"));
    }

    #[test]
    fn rejects_duplicate_normalized_keys() {
        let dir = temp_project_dir("duplicate-key");
        let toml = format!(
            r#"
                [projects.Relay]
                path = "{}"

                [projects.relay]
                path = "{}"
            "#,
            toml_path(&dir),
            toml_path(&dir)
        );

        let error = ProjectRegistry::from_toml_str(&toml).unwrap_err();

        assert!(matches!(
            error,
            ProjectRegistryError::DuplicateProjectKey { .. }
        ));
        assert_eq!(error.to_string(), "duplicate project key 'relay'");
    }

    #[test]
    fn unknown_project_lookup_fails_clearly() {
        let registry = ProjectRegistry::empty();

        let error = registry.get("missing").unwrap_err();

        assert_eq!(error.to_string(), "unknown project key 'missing'");
    }

    #[test]
    fn rejects_nonexistent_project_path() {
        let missing = unique_temp_path("nonexistent-path").join("missing");
        let toml = format!(
            r#"
                [projects.relay]
                path = "{}"
            "#,
            toml_path(&missing)
        );

        let error = ProjectRegistry::from_toml_str(&toml).unwrap_err();

        assert!(matches!(error, ProjectRegistryError::ProjectPathIo { .. }));
        assert!(error.to_string().contains("failed to inspect project path"));
    }

    #[test]
    fn rejects_relative_project_path() {
        let toml = r#"
            [projects.relay]
            path = "relative/path"
        "#;

        let error = ProjectRegistry::from_toml_str(toml).unwrap_err();

        assert!(matches!(
            error,
            ProjectRegistryError::InvalidProjectPath { .. }
        ));
        assert!(error.to_string().contains("project path must be absolute"));
    }

    #[test]
    fn normalizes_and_validates_project_keys() {
        assert_eq!(
            ProjectKey::parse(" Relay_Core ").unwrap().as_str(),
            "relay_core"
        );

        for invalid in ["", "-relay", "relay-", "relay core", "relay.core", "rélay"] {
            assert!(
                matches!(
                    ProjectKey::parse(invalid),
                    Err(ProjectRegistryError::InvalidProjectKey { .. })
                ),
                "{invalid} should be invalid"
            );
        }
    }

    #[test]
    fn parses_optional_github_metadata_without_network_access() {
        let dir = temp_project_dir("github-metadata");
        let toml = format!(
            r#"
                [projects.relay]
                path = "{}"

                [projects.relay.github]
                owner = "Jacinth091"
                repo = "Relay"
            "#,
            toml_path(&dir)
        );

        let project = ProjectRegistry::from_toml_str(&toml)
            .unwrap()
            .get("relay")
            .unwrap()
            .clone();

        assert_eq!(
            project.github(),
            Some(&GitHubRepositoryMetadata {
                owner: "Jacinth091".to_owned(),
                repo: "Relay".to_owned(),
                default_labels: Vec::new(),
            })
        );
    }

    #[test]
    fn registry_order_is_deterministic() {
        let beta = temp_project_dir("deterministic-beta");
        let alpha = temp_project_dir("deterministic-alpha");
        let toml = format!(
            r#"
                [projects.beta]
                path = "{}"

                [projects.alpha]
                path = "{}"
            "#,
            toml_path(&beta),
            toml_path(&alpha)
        );

        let registry = ProjectRegistry::from_toml_str(&toml).unwrap();
        let keys = registry
            .list()
            .map(|project| project.key().as_str().to_owned())
            .collect::<Vec<_>>();

        assert_eq!(keys, ["alpha", "beta"]);
    }

    #[test]
    fn register_and_save_round_trip() {
        let project_dir = temp_project_dir("register-round-trip-project");
        let config_path = unique_temp_path("register-round-trip-config").join("relay.toml");
        let mut registry = ProjectRegistry::empty();

        registry
            .register(RegisterProject {
                key: "Relay".to_owned(),
                name: Some("Relay".to_owned()),
                local_path: project_dir,
                expected_remote: Some("git@github.com:Jacinth091/Relay.git".to_owned()),
                default_stack: Some("chatgpt-codex".to_owned()),
                github: None,
            })
            .unwrap();
        registry.save(&config_path).unwrap();

        let loaded = ProjectRegistry::load(&config_path).unwrap();

        assert_eq!(loaded.len(), 1);
        assert_eq!(
            loaded
                .get("RELAY")
                .unwrap()
                .expected_remote()
                .map(ExpectedRemote::as_str),
            Some("git@github.com:Jacinth091/Relay.git")
        );
    }

    fn temp_project_dir(name: &str) -> PathBuf {
        let path = unique_temp_path(name);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn unique_temp_path(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        env::temp_dir().join(format!("relay-{name}-{nanos}"))
    }

    fn toml_path(path: &Path) -> String {
        path.to_string_lossy().replace('\\', "\\\\")
    }
}
