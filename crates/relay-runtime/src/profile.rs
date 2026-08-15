use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

/// A trusted local execution policy describing how a runtime should be configured.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeProfile {
    /// Unique profile identifier, e.g. "codex/implement", "antigravity/review"
    pub id: String,
    /// Provider runtime identifier, e.g. "codex", "antigravity"
    pub provider: String,
    /// Human-readable display name
    pub display_name: String,
    /// Model selection ("default" or specific model name)
    pub model: String,
    /// Reasoning/effort setting
    pub reasoning_effort: ReasoningEffort,
    /// Filesystem capability request
    pub filesystem: FilesystemCapability,
    /// Network access
    pub network: bool,
    /// Extra writable roots beyond workspace
    pub extra_writable_roots: Vec<PathBuf>,
    /// Maximum execution duration
    pub max_duration: Duration,
    /// Maximum repair attempts
    pub max_repair_attempts: u32,
    /// Whether Relay owns approval (vs. runtime interactive prompts)
    pub relay_approval_owner: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    Default,
    Low,
    Medium,
    High,
    XHigh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilesystemCapability {
    ReadOnly,
    WorkspaceWrite,
}

/// A resolved profile with effective values and provider-specific snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveRuntimeProfile {
    pub profile: RuntimeProfile,
    /// Provider-specific configuration snapshot (opaque to core)
    pub provider_config_snapshot: Option<String>,
}

/// Profile resolution precedence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileSelection {
    /// Explicit one-run override
    Explicit(String),
    /// Project default
    ProjectDefault(String),
    /// Global default
    GlobalDefault(String),
}
