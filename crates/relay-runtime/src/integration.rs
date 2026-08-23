use serde::{Deserialize, Serialize};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use crate::EffectiveRuntimeProfile;

/// Discovery, health, profile, and capability management for a runtime provider.
/// Separate from the `Runtime` execution trait.
pub trait RuntimeIntegration: Send + Sync {
    /// Static descriptor of this runtime provider.
    fn descriptor(&self) -> RuntimeDescriptor;

    /// Probe installation, authentication, and compatibility.
    fn probe(&self) -> Pin<Box<dyn Future<Output = RuntimeProbe> + Send + '_>>;

    /// List available built-in profiles for this provider.
    fn profiles(&self) -> Vec<RuntimeProfileDescriptor>;

    /// Query available models (may require network/CLI).
    fn models(&self) -> Pin<Box<dyn Future<Output = RuntimeModelCatalog> + Send + '_>>;

    /// Resolve a profile selection into an effective profile.
    fn resolve_profile(
        &self,
        profile_id: &str,
    ) -> Result<EffectiveRuntimeProfile, RuntimeConfigError>;

    /// Verify that configured capabilities can actually be established.
    fn verify_capabilities(
        &self,
        profile: &EffectiveRuntimeProfile,
        context: &RuntimeCapabilityContext,
    ) -> Pin<Box<dyn Future<Output = RuntimeCapabilityReport> + Send + '_>>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeDescriptor {
    pub provider_id: String,
    pub display_name: String,
    pub description: String,
    pub is_development_only: bool,
    /// True only when the adapter consumes a documented machine-readable
    /// provider interface rather than scraping an interactive UI.
    pub machine_interface_verified: bool,
}

/// Short descriptor for a profile offered by a provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeProfileDescriptor {
    pub id: String,
    pub display_name: String,
    pub purpose: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeProbe {
    pub installed: bool,
    pub version: Option<String>,
    pub authenticated: bool,
    pub compatible: bool,
    pub account_identity: Option<String>,
    pub notes: Vec<String>,
}

impl RuntimeProbe {
    pub fn is_ready(&self) -> bool {
        self.installed && self.authenticated && self.compatible
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeModelCatalog {
    pub default_model: String,
    pub available_models: Vec<RuntimeModelInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeModelInfo {
    pub id: String,
    pub display_name: String,
    pub supported_reasoning_efforts: Vec<crate::ReasoningEffort>,
    pub default_reasoning_effort: Option<crate::ReasoningEffort>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeCapabilityContext {
    pub working_directory: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeCapabilityReport {
    pub filesystem_read: CapabilityStatus,
    pub filesystem_write: CapabilityStatus,
    pub command_execution: CapabilityStatus,
    pub network: CapabilityStatus,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityStatus {
    Configured,
    Verified,
    Unknown,
    Mismatch,
    Unavailable,
}

impl CapabilityStatus {
    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Configured | Self::Verified)
    }

    pub fn marker(&self) -> &'static str {
        match self {
            Self::Configured => "○",
            Self::Verified => "✓",
            Self::Unknown => "?",
            Self::Mismatch => "×",
            Self::Unavailable => "×",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuntimeConfigError {
    #[error("unknown profile: {profile_id}")]
    UnknownProfile { profile_id: String },
    #[error("provider unavailable: {reason}")]
    ProviderUnavailable { reason: String },
    #[error(
        "profile/provider mismatch: profile {profile_id} requires provider {expected}, got {actual}"
    )]
    ProviderMismatch {
        profile_id: String,
        expected: String,
        actual: String,
    },
    #[error("invalid configuration: {reason}")]
    InvalidConfiguration { reason: String },
}
