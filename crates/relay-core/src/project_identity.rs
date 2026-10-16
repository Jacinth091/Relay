//! Relay Architecture v2 - Phase 02: Project, Checkout, and Trust Foundation.
//!
//! Separates:
//! - `DeclaredProjectIdentity`: untrusted repository claim (e.g., from `.relay/project.toml`);
//! - `TrustedProjectRegistration`: locally approved durable project identity;
//! - `CheckoutId` / `Checkout`: specific trusted local repository working root.
//!
//! A repository cannot grant itself trusted filesystem paths, credentials,
//! provider access, or privileged policy authority.

use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{GitHubRepositoryMetadata, ProjectKey, ProjectRegistryError};

#[derive(Debug, Error)]
pub enum ProjectIdentityError {
    #[error("invalid project id: {reason}")]
    InvalidProjectId { reason: String },
    #[error("invalid checkout id: {reason}")]
    InvalidCheckoutId { reason: String },
    #[error("path is not absolute: {0}")]
    PathNotAbsolute(PathBuf),
    #[error("checkout not found: {0}")]
    CheckoutNotFound(String),
    #[error(
        "identity mismatch: declared project '{declared}' does not match registered project '{registered}'"
    )]
    IdentityMismatch {
        declared: String,
        registered: String,
    },
    #[error(
        "remote fingerprint mismatch: checkout remote '{actual}' does not match expected '{expected}'"
    )]
    RemoteMismatch { expected: String, actual: String },
    #[error("checkout is untrusted or revoked")]
    CheckoutNotTrusted,
    #[error("project registry error: {0}")]
    Registry(#[from] ProjectRegistryError),
}

/// Logical Project ID identifying a software project in Relay.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ProjectId(String);

impl ProjectId {
    pub fn parse(input: impl AsRef<str>) -> Result<Self, ProjectIdentityError> {
        let key = ProjectKey::parse(input.as_ref()).map_err(|err| {
            ProjectIdentityError::InvalidProjectId {
                reason: err.to_string(),
            }
        })?;
        Ok(Self(key.as_str().to_owned()))
    }

    pub fn from_project_key(key: &ProjectKey) -> Self {
        Self(key.as_str().to_owned())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn to_project_key(&self) -> ProjectKey {
        ProjectKey::parse(&self.0).expect("ProjectId is guaranteed a valid ProjectKey")
    }
}

impl std::fmt::Display for ProjectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Unique identifier for one trusted local clone/root of a project.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CheckoutId(String);

impl CheckoutId {
    pub fn parse(input: impl AsRef<str>) -> Result<Self, ProjectIdentityError> {
        let trimmed = input.as_ref().trim();
        if trimmed.is_empty() {
            return Err(ProjectIdentityError::InvalidCheckoutId {
                reason: "checkout id cannot be empty".to_owned(),
            });
        }
        if !trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(ProjectIdentityError::InvalidCheckoutId {
                reason: "checkout id may only contain ASCII alphanumeric characters, '-' or '_'"
                    .to_owned(),
            });
        }
        Ok(Self(trimmed.to_owned()))
    }

    pub fn generate() -> Self {
        Self(format!("co-{}", uuid::Uuid::new_v4().simple()))
    }

    pub fn for_project_key(key: &ProjectKey) -> Self {
        Self(format!("co-{}", key.as_str()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for CheckoutId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Trust state for a specific local checkout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckoutTrustState {
    Trusted,
    Untrusted,
    Revoked,
}

/// Remote URL or fingerprint owned by trusted local configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteFingerprint(String);

impl RemoteFingerprint {
    pub fn new(remote: impl Into<String>) -> Self {
        Self(remote.into().trim().to_owned())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Checks if an observed remote matches this expected fingerprint.
    pub fn matches(&self, observed: &str) -> bool {
        let clean_expected = self.0.trim().to_ascii_lowercase();
        let clean_expected = clean_expected.trim_end_matches(".git");
        let clean_observed = observed.trim().to_ascii_lowercase();
        let clean_observed = clean_observed.trim_end_matches(".git");
        clean_expected == clean_observed
    }
}

impl std::fmt::Display for RemoteFingerprint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Untrusted project identity claimed inside repository portable configuration.
///
/// This declaration is purely advisory until matched against a local
/// `TrustedProjectRegistration`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclaredProjectIdentity {
    pub schema_version: u32,
    pub project_id: ProjectId,
    pub name: String,
}

impl DeclaredProjectIdentity {
    pub fn new(schema_version: u32, project_id: ProjectId, name: impl Into<String>) -> Self {
        Self {
            schema_version,
            project_id,
            name: name.into(),
        }
    }

    /// Verifies that this declared identity matches a trusted registration.
    ///
    /// Fails closed if the declared `project_id` attempts to claim or hijack
    /// another registered project.
    pub fn verify_matches(
        &self,
        registration: &TrustedProjectRegistration,
    ) -> Result<(), ProjectIdentityError> {
        if self.project_id != registration.project_id {
            return Err(ProjectIdentityError::IdentityMismatch {
                declared: self.project_id.to_string(),
                registered: registration.project_id.to_string(),
            });
        }
        Ok(())
    }
}

/// One trusted local checkout/worktree root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkout {
    pub id: CheckoutId,
    pub project_id: ProjectId,
    pub canonical_root: PathBuf,
    pub expected_remote: Option<RemoteFingerprint>,
    pub allowed_worktree_root: PathBuf,
    pub trust_state: CheckoutTrustState,
    pub registered_at: String,
}

impl Checkout {
    pub fn new(
        id: CheckoutId,
        project_id: ProjectId,
        canonical_root: impl Into<PathBuf>,
        expected_remote: Option<RemoteFingerprint>,
        allowed_worktree_root: Option<PathBuf>,
        trust_state: CheckoutTrustState,
    ) -> Result<Self, ProjectIdentityError> {
        let root = canonical_root.into();
        if !root.is_absolute() {
            return Err(ProjectIdentityError::PathNotAbsolute(root));
        }

        let worktree_root = match allowed_worktree_root {
            Some(wt) => {
                if !wt.is_absolute() {
                    return Err(ProjectIdentityError::PathNotAbsolute(wt));
                }
                wt
            }
            None => root.join(".relay").join("worktrees"),
        };

        let registered_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs().to_string())
            .unwrap_or_else(|_| "0".to_owned());

        Ok(Self {
            id,
            project_id,
            canonical_root: root,
            expected_remote,
            allowed_worktree_root: worktree_root,
            trust_state,
            registered_at,
        })
    }

    /// Verifies that an observed remote matches this checkout's expected remote (if configured).
    pub fn verify_remote(&self, observed_remote: &str) -> Result<(), ProjectIdentityError> {
        if let Some(expected) = &self.expected_remote {
            if !expected.matches(observed_remote) {
                return Err(ProjectIdentityError::RemoteMismatch {
                    expected: expected.to_string(),
                    actual: observed_remote.to_owned(),
                });
            }
        }
        Ok(())
    }

    /// Verifies that this checkout is in a trusted state.
    pub fn ensure_trusted(&self) -> Result<(), ProjectIdentityError> {
        if self.trust_state != CheckoutTrustState::Trusted {
            return Err(ProjectIdentityError::CheckoutNotTrusted);
        }
        Ok(())
    }
}

/// Locally approved project registration binding logical project identity to machine evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedProjectRegistration {
    pub project_id: ProjectId,
    pub name: Option<String>,
    pub default_stack: Option<String>,
    pub github: Option<GitHubRepositoryMetadata>,
    pub checkouts: Vec<Checkout>,
}

impl TrustedProjectRegistration {
    pub fn new(
        project_id: ProjectId,
        name: Option<String>,
        default_stack: Option<String>,
        github: Option<GitHubRepositoryMetadata>,
    ) -> Self {
        Self {
            project_id,
            name,
            default_stack,
            github,
            checkouts: Vec::new(),
        }
    }

    pub fn primary_checkout(&self) -> Option<&Checkout> {
        self.checkouts
            .iter()
            .find(|c| c.trust_state == CheckoutTrustState::Trusted)
            .or_else(|| self.checkouts.first())
    }

    pub fn get_checkout(&self, id: &CheckoutId) -> Option<&Checkout> {
        self.checkouts.iter().find(|c| &c.id == id)
    }

    pub fn add_checkout(&mut self, checkout: Checkout) {
        self.checkouts.push(checkout);
    }
}

/// Adapts existing `crate::TrustedProject` to `TrustedProjectRegistration`.
impl From<&crate::TrustedProject> for TrustedProjectRegistration {
    fn from(tp: &crate::TrustedProject) -> Self {
        let project_id = ProjectId::from_project_key(tp.key());
        let checkout_id = CheckoutId::for_project_key(tp.key());
        let expected_remote = tp
            .expected_remote()
            .map(|r| RemoteFingerprint::new(r.as_str()));

        let checkout = Checkout::new(
            checkout_id,
            project_id.clone(),
            tp.local_path().to_path_buf(),
            expected_remote,
            None,
            CheckoutTrustState::Trusted,
        )
        .expect("TrustedProject local_path is validated absolute");

        Self {
            project_id,
            name: tp.name().map(ToOwned::to_owned),
            default_stack: tp.default_stack().map(ToOwned::to_owned),
            github: tp.github().cloned(),
            checkouts: vec![checkout],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn project_id_validates_and_roundtrips() {
        let pid = ProjectId::parse("relay-core").unwrap();
        assert_eq!(pid.as_str(), "relay-core");
        assert_eq!(pid.to_string(), "relay-core");
        assert_eq!(pid.to_project_key().as_str(), "relay-core");

        // Invalid key
        assert!(ProjectId::parse("INVALID KEY WITH SPACES").is_err());
        assert!(ProjectId::parse("").is_err());
    }

    #[test]
    fn checkout_id_validation_and_generation() {
        let co1 = CheckoutId::parse("main-repo").unwrap();
        assert_eq!(co1.as_str(), "main-repo");

        let generated = CheckoutId::generate();
        assert!(generated.as_str().starts_with("co-"));

        assert!(CheckoutId::parse("bad id with spaces").is_err());
        assert!(CheckoutId::parse("").is_err());
    }

    #[test]
    fn checkout_requires_absolute_paths() {
        let pid = ProjectId::parse("proj-1").unwrap();
        let co_id = CheckoutId::parse("co-1").unwrap();

        // Relative path rejected
        let rel_err = Checkout::new(
            co_id.clone(),
            pid.clone(),
            Path::new("relative/path"),
            None,
            None,
            CheckoutTrustState::Trusted,
        );
        assert!(matches!(
            rel_err,
            Err(ProjectIdentityError::PathNotAbsolute(_))
        ));

        // Absolute path accepted
        #[cfg(windows)]
        let abs_path = PathBuf::from("C:\\projects\\my-repo");
        #[cfg(not(windows))]
        let abs_path = PathBuf::from("/projects/my-repo");

        let checkout = Checkout::new(
            co_id,
            pid,
            abs_path.clone(),
            Some(RemoteFingerprint::new("https://github.com/org/repo.git")),
            None,
            CheckoutTrustState::Trusted,
        )
        .unwrap();

        assert_eq!(checkout.canonical_root, abs_path);
        assert_eq!(
            checkout.allowed_worktree_root,
            abs_path.join(".relay").join("worktrees")
        );
        assert!(checkout.ensure_trusted().is_ok());
    }

    #[test]
    fn declared_identity_matching_and_hijack_prevention() {
        let reg_pid = ProjectId::parse("valid-project").unwrap();
        let registration = TrustedProjectRegistration::new(reg_pid.clone(), None, None, None);

        // Matching declared identity
        let valid_declared = DeclaredProjectIdentity::new(1, reg_pid, "Valid");
        assert!(valid_declared.verify_matches(&registration).is_ok());

        // Hijack attempt: claiming a different project_id
        let impostor_pid = ProjectId::parse("other-project").unwrap();
        let impostor_declared = DeclaredProjectIdentity::new(1, impostor_pid, "Impostor");
        let err = impostor_declared.verify_matches(&registration).unwrap_err();

        assert!(matches!(err, ProjectIdentityError::IdentityMismatch { .. }));
    }

    #[test]
    fn remote_fingerprint_matching_and_git_suffix_leniency() {
        let fp = RemoteFingerprint::new("https://github.com/org/relay.git");
        assert!(fp.matches("https://github.com/org/relay"));
        assert!(fp.matches("https://github.com/org/relay.git"));
        assert!(fp.matches("HTTPS://GITHUB.COM/ORG/RELAY.GIT"));
        assert!(!fp.matches("https://github.com/org/other-repo"));
    }
}
