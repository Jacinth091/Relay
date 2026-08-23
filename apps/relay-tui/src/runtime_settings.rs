use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeSettings {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub runtime: GlobalRuntimeConfig,
    #[serde(default)]
    pub projects: HashMap<String, String>,
    /// Optional v2 project tuning, kept separate from the strict M2 project registry.
    #[serde(default)]
    pub project_tuning: HashMap<String, ProjectRuntimeSettings>,
    #[serde(default)]
    pub providers: HashMap<String, ProviderOverrideConfig>,
    #[serde(default)]
    pub profile_overrides: HashMap<String, ProfileOverrideConfig>,
    /// Execution limits edited on the Settings screen; absent values keep core defaults.
    #[serde(default)]
    pub execution: ExecutionSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExecutionSettings {
    pub max_parallel: Option<usize>,
    pub per_project_max_parallel: Option<usize>,
    pub max_repair_attempts: Option<u32>,
    pub max_duration_secs: Option<u64>,
}

impl ExecutionSettings {
    /// Overlays any persisted limit onto `limits`, leaving unset fields untouched.
    #[cfg_attr(test, allow(dead_code))]
    pub fn apply_to_limits(&self, limits: &mut relay_core::ExecutionLimits) {
        if let Some(value) = self.max_parallel {
            limits.max_parallel = value;
        }
        if let Some(value) = self.per_project_max_parallel {
            limits.per_project_max_parallel = value;
        }
        if let Some(value) = self.max_repair_attempts {
            limits.max_repair_attempts = value;
        }
        if let Some(secs) = self.max_duration_secs {
            limits.max_duration = Duration::from_secs(secs);
        }
    }
}

fn default_version() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalRuntimeConfig {
    #[serde(default = "default_profile")]
    pub default_profile: String,
}

impl Default for GlobalRuntimeConfig {
    fn default() -> Self {
        Self {
            default_profile: default_profile(),
        }
    }
}

fn default_profile() -> String {
    "codex/implement".to_owned()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderOverrideConfig {
    pub executable: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProfileOverrideConfig {
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub max_duration_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectRuntimeSettings {
    pub profile: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
}

impl Default for RuntimeSettings {
    fn default() -> Self {
        let mut profile_overrides = HashMap::new();
        profile_overrides.insert(
            "codex/implement".to_owned(),
            ProfileOverrideConfig {
                model: Some("default".to_owned()),
                reasoning_effort: Some("high".to_owned()),
                max_duration_secs: Some(1800),
            },
        );
        profile_overrides.insert(
            "antigravity/implement".to_owned(),
            ProfileOverrideConfig {
                model: Some("default".to_owned()),
                reasoning_effort: Some("high".to_owned()),
                max_duration_secs: Some(1800),
            },
        );

        Self {
            version: 1,
            runtime: GlobalRuntimeConfig::default(),
            projects: HashMap::new(),
            project_tuning: HashMap::new(),
            providers: HashMap::new(),
            profile_overrides,
            execution: ExecutionSettings::default(),
        }
    }
}

use relay_core::{
    ReasoningEffort, RuntimeExecutionSelection, RuntimeModelSelection, RuntimeReasoningSelection,
};

impl RuntimeSettings {
    pub fn default_path() -> PathBuf {
        if let Ok(override_path) = std::env::var("RELAY_RUNTIME_CONFIG_PATH") {
            return PathBuf::from(override_path);
        }
        if let Some(config_dir) = dirs::config_dir() {
            return config_dir.join("relay").join("runtimes.toml");
        }
        PathBuf::from("runtimes.toml")
    }

    pub fn resolve_selection(
        &self,
        project_key: Option<&str>,
        override_profile: Option<&str>,
        override_model: Option<&str>,
        override_effort: Option<ReasoningEffort>,
    ) -> RuntimeExecutionSelection {
        let profile_id = override_profile
            .or_else(|| {
                project_key
                    .and_then(|k| self.project_tuning.get(k))
                    .and_then(|settings| settings.profile.as_deref())
            })
            .or_else(|| {
                project_key
                    .and_then(|k| self.projects.get(k))
                    .map(|s| s.as_str())
            })
            .unwrap_or(self.runtime.default_profile.as_str())
            .to_owned();

        let profile_override = self.profile_overrides.get(&profile_id);

        let raw_model = override_model
            .map(|s| s.to_owned())
            .or_else(|| {
                project_key
                    .and_then(|k| self.project_tuning.get(k))
                    .and_then(|settings| settings.model.clone())
            })
            .or_else(|| profile_override.and_then(|p| p.model.clone()));

        let model = match raw_model.as_deref() {
            Some("default") | None => RuntimeModelSelection::ProviderDefault,
            Some(explicit) => RuntimeModelSelection::Explicit(explicit.to_owned()),
        };

        let reasoning_effort = if let Some(effort) = override_effort {
            RuntimeReasoningSelection::Explicit(effort)
        } else if let Some(raw_effort) = project_key
            .and_then(|k| self.project_tuning.get(k))
            .and_then(|settings| settings.reasoning_effort.as_deref())
            .or_else(|| profile_override.and_then(|p| p.reasoning_effort.as_deref()))
        {
            match raw_effort {
                "low" => RuntimeReasoningSelection::Explicit(ReasoningEffort::Low),
                "medium" => RuntimeReasoningSelection::Explicit(ReasoningEffort::Medium),
                "high" => RuntimeReasoningSelection::Explicit(ReasoningEffort::High),
                "xhigh" => RuntimeReasoningSelection::Explicit(ReasoningEffort::XHigh),
                _ => RuntimeReasoningSelection::ProfileDefault,
            }
        } else {
            RuntimeReasoningSelection::ProfileDefault
        };

        RuntimeExecutionSelection {
            profile_id,
            model,
            reasoning_effort,
        }
    }

    pub fn load_from_path(path: &Path) -> Result<Self, String> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = fs::read_to_string(path)
            .map_err(|e| format!("Failed to read runtime config '{path:?}': {e}"))?;
        toml::from_str(&content)
            .map_err(|e| format!("Failed to parse runtime config '{path:?}': {e}"))
    }

    #[allow(dead_code)]
    pub fn save_to_path(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                format!("Failed to create runtime config directory '{parent:?}': {e}")
            })?;
        }
        let content = toml::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize runtime config: {e}"))?;
        fs::write(path, content)
            .map_err(|e| format!("Failed to write runtime config '{path:?}': {e}"))
    }

    #[allow(dead_code)]
    pub fn resolve_profile_for_project(
        &self,
        project_key: &str,
        legacy_stack: Option<&str>,
    ) -> String {
        if let Some(profile) = self.projects.get(project_key) {
            return profile.clone();
        }
        if let Some(stack) = legacy_stack {
            if stack == "antigravity" || stack == "agy" {
                return "antigravity/implement".to_owned();
            } else if stack == "chatgpt-codex" || stack == "codex" {
                return "codex/implement".to_owned();
            }
        }
        self.runtime.default_profile.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_resolves_correctly() {
        let settings = RuntimeSettings::default();
        assert_eq!(settings.runtime.default_profile, "codex/implement");
        assert_eq!(
            settings.resolve_profile_for_project("unknown", None),
            "codex/implement"
        );
        assert_eq!(
            settings.resolve_profile_for_project("unknown", Some("antigravity")),
            "antigravity/implement"
        );
    }

    #[test]
    fn project_tuning_overrides_profile_defaults_without_changing_registry_shape() {
        let mut settings = RuntimeSettings::default();
        settings.project_tuning.insert(
            "relay".to_owned(),
            ProjectRuntimeSettings {
                profile: Some("codex/implement".to_owned()),
                model: Some("custom-model".to_owned()),
                reasoning_effort: Some("medium".to_owned()),
            },
        );
        let selection = settings.resolve_selection(Some("relay"), None, None, None);
        assert_eq!(selection.model.as_option(), Some("custom-model"));
        assert_eq!(
            selection.reasoning_effort.as_option(),
            Some(ReasoningEffort::Medium)
        );
    }
}
