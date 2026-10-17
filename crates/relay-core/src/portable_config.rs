//! Relay Architecture v2 - Phase 02: Repository `.relay/` Portable Configuration Loader.
//!
//! Loads and validates portable repository configuration (`<repo>/.relay/`):
//! - `project.toml`: untrusted declared project identity;
//! - `policies.toml`: repository policy restrictions;
//! - `workflows/`: declarative workflow templates (v2 future phases);
//!
//! Trust Classification:
//! Repository configuration is untrusted input until matched against a local
//! `TrustedProjectRegistration` and validated against local policy ceilings.
//! It may restrict local execution limits, but can NEVER broaden permissions,
//! grant machine-local paths, or bypass policy.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    Checkout, DeclaredProjectIdentity, ProjectId, ProjectIdentityError, TrustedProjectRegistration,
};

pub const RELAY_DIR: &str = ".relay";
pub const PROJECT_CONFIG_FILE: &str = "project.toml";
pub const POLICIES_CONFIG_FILE: &str = "policies.toml";

#[derive(Debug, Error)]
pub enum PortableConfigError {
    #[error("I/O error reading repository configuration: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to parse TOML configuration from {path}: {source}")]
    Parse {
        path: PathBuf,
        source: Box<toml::de::Error>,
    },
    #[error("portable config validation error: {reason}")]
    Validation { reason: String },
    #[error("project identity error: {0}")]
    Identity(#[from] ProjectIdentityError),
    #[error("security violation: repository configuration attempted to define machine-local paths")]
    PathInjectionAttempted,
    #[error(
        "security violation: repository policy attempted to broaden local limits (claimed {claimed}, ceiling is {ceiling})"
    )]
    PolicyExpansionAttempted { claimed: String, ceiling: String },
}

/// Raw untrusted `project.toml` file schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProjectToml {
    project: RawProjectSection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProjectSection {
    schema_version: u32,
    project_id: String,
    name: String,
}

/// Raw untrusted `policies.toml` file schema.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawPoliciesToml {
    #[serde(default)]
    pub restrictions: RawRestrictionsSection,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawRestrictionsSection {
    pub max_duration_seconds: Option<u64>,
    pub max_repair_attempts: Option<u32>,
    #[serde(default)]
    pub disallowed_tools: Vec<String>,
}

/// Trust classification result for loaded repository configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrustClassification {
    /// Fully verified against a local `TrustedProjectRegistration` and checkout evidence.
    TrustedAgainstRegistration {
        project_id: ProjectId,
        checkout_id: String,
    },
    /// Untrusted declaration; no matching local registration or checkout.
    UntrustedDeclaration { reason: String },
}

/// Loaded portable repository configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortableRepositoryConfig {
    pub relay_dir: PathBuf,
    pub declared_identity: Option<DeclaredProjectIdentity>,
    pub restrictions: RawRestrictionsSection,
}

impl PortableRepositoryConfig {
    /// Discovers and loads `.relay/` configuration from a candidate working directory.
    pub fn load_from_dir(repo_root: impl AsRef<Path>) -> Result<Option<Self>, PortableConfigError> {
        let relay_dir = repo_root.as_ref().join(RELAY_DIR);
        if !relay_dir.is_dir() {
            return Ok(None);
        }

        let project_file = relay_dir.join(PROJECT_CONFIG_FILE);
        let declared_identity = if project_file.is_file() {
            let content = fs::read_to_string(&project_file)?;
            let raw: RawProjectToml =
                toml::from_str(&content).map_err(|source| PortableConfigError::Parse {
                    path: project_file.clone(),
                    source: Box::new(source),
                })?;

            let project_id = ProjectId::parse(&raw.project.project_id)?;
            Some(DeclaredProjectIdentity::new(
                raw.project.schema_version,
                project_id,
                raw.project.name,
            ))
        } else {
            None
        };

        let policies_file = relay_dir.join(POLICIES_CONFIG_FILE);
        let restrictions = if policies_file.is_file() {
            let content = fs::read_to_string(&policies_file)?;
            let raw: RawPoliciesToml =
                toml::from_str(&content).map_err(|source| PortableConfigError::Parse {
                    path: policies_file.clone(),
                    source: Box::new(source),
                })?;
            raw.restrictions
        } else {
            RawRestrictionsSection::default()
        };

        Ok(Some(Self {
            relay_dir,
            declared_identity,
            restrictions,
        }))
    }

    /// Validates this configuration against a local trusted registration and checkout.
    ///
    /// Fail-closed security rules:
    /// 1. Declared `project_id` must match `registration.project_id` (hijack prevention).
    /// 2. Checkout must be in a trusted state.
    /// 3. If an expected remote is configured on trusted local registration, the checkout's
    ///    actual `remote.origin.url` must match it (mismatch, missing, or malformed fails closed).
    /// 4. Repository restrictions may clamp, but cannot broaden local policy ceilings.
    pub fn validate_and_classify(
        &self,
        registration: &TrustedProjectRegistration,
        checkout: &Checkout,
    ) -> Result<TrustClassification, PortableConfigError> {
        checkout.ensure_trusted()?;

        if let Some(declared) = &self.declared_identity {
            declared.verify_matches(registration)?;
        }

        if let Some(expected) = registration.expected_remote_for(checkout) {
            let actual_remote_opt =
                crate::repository_safety::read_repository_remote_origin(&checkout.canonical_root)
                    .map_err(|_| ProjectIdentityError::MissingRemote {
                    checkout_id: checkout.id.to_string(),
                    expected: expected.to_string(),
                })?;

            let actual_remote =
                actual_remote_opt.ok_or_else(|| ProjectIdentityError::MissingRemote {
                    checkout_id: checkout.id.to_string(),
                    expected: expected.to_string(),
                })?;

            let exp_norm =
                crate::RemoteFingerprint::normalize(expected.as_str()).map_err(|reason| {
                    ProjectIdentityError::MalformedRemote {
                        remote: expected.to_string(),
                        reason,
                    }
                })?;

            let act_norm =
                crate::RemoteFingerprint::normalize(&actual_remote).map_err(|reason| {
                    ProjectIdentityError::MalformedRemote {
                        remote: actual_remote.clone(),
                        reason,
                    }
                })?;

            if exp_norm != act_norm {
                return Err(PortableConfigError::Identity(
                    ProjectIdentityError::RemoteMismatch {
                        expected: expected.to_string(),
                        actual: actual_remote,
                    },
                ));
            }
        }

        Ok(TrustClassification::TrustedAgainstRegistration {
            project_id: registration.project_id.clone(),
            checkout_id: checkout.id.to_string(),
        })
    }

    /// Clamps an execution duration to repository restrictions (cannot expand).
    pub fn clamp_duration(&self, requested_secs: u64) -> u64 {
        match self.restrictions.max_duration_seconds {
            Some(limit) => requested_secs.min(limit),
            None => requested_secs,
        }
    }

    /// Clamps repair attempts to repository restrictions (cannot expand).
    pub fn clamp_repair_attempts(&self, requested_attempts: u32) -> u32 {
        match self.restrictions.max_repair_attempts {
            Some(limit) => requested_attempts.min(limit),
            None => requested_attempts,
        }
    }

    /// Checks if a tool is forbidden by repository policy restrictions.
    pub fn is_tool_allowed(&self, tool_name: &str) -> bool {
        !self
            .restrictions
            .disallowed_tools
            .iter()
            .any(|t| t.eq_ignore_ascii_case(tool_name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CheckoutTrustState;

    #[test]
    fn loads_and_parses_valid_relay_dir() {
        let temp = std::env::temp_dir().join(format!("relay-portable-{}", uuid::Uuid::new_v4()));
        let relay_dir = temp.join(".relay");
        fs::create_dir_all(&relay_dir).unwrap();

        fs::write(
            relay_dir.join("project.toml"),
            r#"
[project]
schema_version = 1
project_id = "my-project"
name = "My Test Project"
"#,
        )
        .unwrap();

        fs::write(
            relay_dir.join("policies.toml"),
            r#"
[restrictions]
max_duration_seconds = 300
max_repair_attempts = 1
disallowed_tools = ["curl", "rm"]
"#,
        )
        .unwrap();

        let loaded = PortableRepositoryConfig::load_from_dir(&temp)
            .unwrap()
            .expect("should find .relay directory");

        assert_eq!(
            loaded
                .declared_identity
                .as_ref()
                .unwrap()
                .project_id
                .as_str(),
            "my-project"
        );
        assert_eq!(
            loaded.declared_identity.as_ref().unwrap().name,
            "My Test Project"
        );
        assert_eq!(loaded.clamp_duration(600), 300);
        assert_eq!(loaded.clamp_duration(100), 100);
        assert_eq!(loaded.clamp_repair_attempts(5), 1);
        assert!(!loaded.is_tool_allowed("curl"));
        assert!(!loaded.is_tool_allowed("CURL"));
        assert!(loaded.is_tool_allowed("cargo"));

        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn rejects_identity_hijack_when_validating() {
        let temp =
            std::env::temp_dir().join(format!("relay-portable-hijack-{}", uuid::Uuid::new_v4()));
        let relay_dir = temp.join(".relay");
        fs::create_dir_all(&relay_dir).unwrap();

        fs::write(
            relay_dir.join("project.toml"),
            r#"
[project]
schema_version = 1
project_id = "stolen-project"
name = "Impostor Project"
"#,
        )
        .unwrap();

        let loaded = PortableRepositoryConfig::load_from_dir(&temp)
            .unwrap()
            .unwrap();

        let real_pid = ProjectId::parse("real-project").unwrap();
        let registration = TrustedProjectRegistration::new(real_pid.clone(), None, None, None);

        #[cfg(windows)]
        let abs_path = PathBuf::from("C:\\projects\\real");
        #[cfg(not(windows))]
        let abs_path = PathBuf::from("/projects/real");

        let checkout = Checkout::new(
            crate::CheckoutId::parse("co-1").unwrap(),
            real_pid,
            abs_path,
            None,
            None,
            CheckoutTrustState::Trusted,
        )
        .unwrap();

        let result = loaded.validate_and_classify(&registration, &checkout);
        assert!(matches!(
            result,
            Err(PortableConfigError::Identity(
                ProjectIdentityError::IdentityMismatch { .. }
            ))
        ));

        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn rejects_untrusted_checkout() {
        let temp =
            std::env::temp_dir().join(format!("relay-portable-untrusted-{}", uuid::Uuid::new_v4()));
        let relay_dir = temp.join(".relay");
        fs::create_dir_all(&relay_dir).unwrap();

        fs::write(
            relay_dir.join("project.toml"),
            r#"
[project]
schema_version = 1
project_id = "test-project"
name = "Test"
"#,
        )
        .unwrap();

        let loaded = PortableRepositoryConfig::load_from_dir(&temp)
            .unwrap()
            .unwrap();

        let pid = ProjectId::parse("test-project").unwrap();
        let registration = TrustedProjectRegistration::new(pid.clone(), None, None, None);

        #[cfg(windows)]
        let abs_path = PathBuf::from("C:\\projects\\test");
        #[cfg(not(windows))]
        let abs_path = PathBuf::from("/projects/test");

        let checkout = Checkout::new(
            crate::CheckoutId::parse("co-1").unwrap(),
            pid,
            abs_path,
            None,
            None,
            CheckoutTrustState::Revoked,
        )
        .unwrap();

        let result = loaded.validate_and_classify(&registration, &checkout);
        assert!(matches!(
            result,
            Err(PortableConfigError::Identity(
                ProjectIdentityError::CheckoutNotTrusted
            ))
        ));

        let _ = fs::remove_dir_all(temp);
    }
}
