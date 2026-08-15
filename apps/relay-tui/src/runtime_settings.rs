use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeSettings {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub runtime: GlobalRuntimeConfig,
    #[serde(default)]
    pub projects: HashMap<String, String>,
    #[serde(default)]
    pub providers: HashMap<String, ProviderOverrideConfig>,
    #[serde(default)]
    pub profile_overrides: HashMap<String, ProfileOverrideConfig>,
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
            providers: HashMap::new(),
            profile_overrides,
        }
    }
}

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

    pub fn load_from_path(path: &Path) -> Result<Self, String> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = fs::read_to_string(path)
            .map_err(|e| format!("Failed to read runtime config '{path:?}': {e}"))?;
        toml::from_str(&content)
            .map_err(|e| format!("Failed to parse runtime config '{path:?}': {e}"))
    }

    pub fn save_to_path(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create runtime config directory '{parent:?}': {e}"))?;
        }
        let content = toml::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize runtime config: {e}"))?;
        fs::write(path, content)
            .map_err(|e| format!("Failed to write runtime config '{path:?}': {e}"))
    }

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
}
