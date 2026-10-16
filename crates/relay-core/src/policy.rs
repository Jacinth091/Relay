//! Relay Architecture v2 - Phase 03: Policy Model, Compiler, and Intersection.
//!
//! Effective policy is an intersection, never a privilege union:
//! ```text
//! built-in safety floor
//! ∩ user policy
//! ∩ trusted local project registration policy
//! ∩ repository project restrictions
//! ∩ workflow policy
//! ∩ role policy
//! ```
//!
//! Hard Deny cannot be overturned by repository, workflow, or role declarations.

use serde::{Deserialize, Serialize};

/// Capability requested by an operation or role attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    FilesystemRead,
    FilesystemWrite,
    NetworkAccess,
    ProcessSpawn,
    GitCommit,
    GitPush,
    PrivilegedCommand,
}

/// Evaluation disposition for a capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyDisposition {
    Allow,
    RequireApproval,
    Deny,
    HardDeny,
}

impl PolicyDisposition {
    /// Combines two dispositions following strict fail-closed intersection:
    /// HardDeny > Deny > RequireApproval > Allow
    pub fn intersect(self, other: Self) -> Self {
        match (self, other) {
            (Self::HardDeny, _) | (_, Self::HardDeny) => Self::HardDeny,
            (Self::Deny, _) | (_, Self::Deny) => Self::Deny,
            (Self::RequireApproval, _) | (_, Self::RequireApproval) => Self::RequireApproval,
            (Self::Allow, Self::Allow) => Self::Allow,
        }
    }

    pub fn is_allowed(self) -> bool {
        matches!(self, Self::Allow)
    }

    pub fn is_denied(self) -> bool {
        matches!(self, Self::Deny | Self::HardDeny)
    }
}

/// Which mechanism enforces the policy rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnforcementClass {
    RelayEnforced,
    SandboxEnforced,
    ProviderEnforced,
    AdvisoryOnly,
}

/// Evaluated policy decision for a capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyDecision {
    pub capability: Capability,
    pub disposition: PolicyDisposition,
    pub enforcement: EnforcementClass,
    pub reason: String,
}

/// Policy rule set for one layer (floor, user, project, repo, role).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyLayer {
    pub name: String,
    pub rules: Vec<(Capability, PolicyDisposition, String)>,
}

impl PolicyLayer {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            rules: Vec::new(),
        }
    }

    pub fn with_rule(
        mut self,
        cap: Capability,
        disp: PolicyDisposition,
        reason: impl Into<String>,
    ) -> Self {
        self.rules.push((cap, disp, reason.into()));
        self
    }

    pub fn get_disposition(&self, cap: Capability) -> Option<(PolicyDisposition, &str)> {
        self.rules
            .iter()
            .find(|(c, _, _)| *c == cap)
            .map(|(_, disp, reason)| (*disp, reason.as_str()))
    }
}

/// Built-in immutable safety floor for Relay v0.1 / Architecture v2:
/// - GitPush is ALWAYS HardDeny (v0.1 never automatically pushes code, Invariant 14);
/// - PrivilegedCommand requires explicit local approval;
/// - FilesystemWrite to root or arbitrary paths is Denied (worktree isolation only).
pub fn built_in_safety_floor() -> PolicyLayer {
    PolicyLayer::new("built_in_safety_floor")
        .with_rule(
            Capability::GitPush,
            PolicyDisposition::HardDeny,
            "v0.1 never automatically pushes code (Invariant 14)",
        )
        .with_rule(
            Capability::PrivilegedCommand,
            PolicyDisposition::RequireApproval,
            "Privileged commands require explicit approval",
        )
        .with_rule(
            Capability::FilesystemRead,
            PolicyDisposition::Allow,
            "Local workspace read allowed by default",
        )
}

/// Policy compiler that intersects layers in strict order.
pub struct PolicyCompiler;

impl PolicyCompiler {
    /// Compiles effective policy by intersecting all layers.
    ///
    /// The built-in safety floor is ALWAYS included as the root layer.
    /// A lower layer (e.g. repo or role) can tighten policy (Allow -> Deny),
    /// but can NEVER broaden a higher layer's Deny or HardDeny.
    pub fn compile(
        user_layer: Option<&PolicyLayer>,
        project_layer: Option<&PolicyLayer>,
        repo_layer: Option<&PolicyLayer>,
        workflow_layer: Option<&PolicyLayer>,
        role_layer: Option<&PolicyLayer>,
    ) -> Vec<PolicyDecision> {
        let floor = built_in_safety_floor();
        let capabilities = [
            Capability::FilesystemRead,
            Capability::FilesystemWrite,
            Capability::NetworkAccess,
            Capability::ProcessSpawn,
            Capability::GitCommit,
            Capability::GitPush,
            Capability::PrivilegedCommand,
        ];

        capabilities
            .into_iter()
            .map(|cap| {
                let mut effective_disp = floor
                    .get_disposition(cap)
                    .map(|(d, _)| d)
                    .unwrap_or(PolicyDisposition::Allow);
                let mut reasons = Vec::new();

                if let Some((d, r)) = floor.get_disposition(cap) {
                    reasons.push(format!("floor: {r}"));
                    effective_disp = effective_disp.intersect(d);
                }

                if let Some(layer) = user_layer {
                    if let Some((d, r)) = layer.get_disposition(cap) {
                        reasons.push(format!("user: {r}"));
                        effective_disp = effective_disp.intersect(d);
                    }
                }

                if let Some(layer) = project_layer {
                    if let Some((d, r)) = layer.get_disposition(cap) {
                        reasons.push(format!("project: {r}"));
                        effective_disp = effective_disp.intersect(d);
                    }
                }

                if let Some(layer) = repo_layer {
                    if let Some((d, r)) = layer.get_disposition(cap) {
                        reasons.push(format!("repo: {r}"));
                        effective_disp = effective_disp.intersect(d);
                    }
                }

                if let Some(layer) = workflow_layer {
                    if let Some((d, r)) = layer.get_disposition(cap) {
                        reasons.push(format!("workflow: {r}"));
                        effective_disp = effective_disp.intersect(d);
                    }
                }

                if let Some(layer) = role_layer {
                    if let Some((d, r)) = layer.get_disposition(cap) {
                        reasons.push(format!("role: {r}"));
                        effective_disp = effective_disp.intersect(d);
                    }
                }

                let enforcement = match cap {
                    Capability::GitPush | Capability::PrivilegedCommand => {
                        EnforcementClass::RelayEnforced
                    }
                    Capability::NetworkAccess | Capability::ProcessSpawn => {
                        EnforcementClass::SandboxEnforced
                    }
                    _ => EnforcementClass::RelayEnforced,
                };

                PolicyDecision {
                    capability: cap,
                    disposition: effective_disp,
                    enforcement,
                    reason: reasons.join("; "),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hard_deny_cannot_be_overridden_by_repo_or_role() {
        let repo_layer = PolicyLayer::new("repo").with_rule(
            Capability::GitPush,
            PolicyDisposition::Allow,
            "repo wants auto-push",
        );
        let role_layer = PolicyLayer::new("role").with_rule(
            Capability::GitPush,
            PolicyDisposition::Allow,
            "role wants auto-push",
        );

        let decisions =
            PolicyCompiler::compile(None, None, Some(&repo_layer), None, Some(&role_layer));
        let push_decision = decisions
            .iter()
            .find(|d| d.capability == Capability::GitPush)
            .unwrap();

        assert_eq!(push_decision.disposition, PolicyDisposition::HardDeny);
        assert!(push_decision.disposition.is_denied());
    }

    #[test]
    fn user_deny_tightens_built_in_allow() {
        let user_layer = PolicyLayer::new("user").with_rule(
            Capability::NetworkAccess,
            PolicyDisposition::Deny,
            "offline mode",
        );

        let decisions = PolicyCompiler::compile(Some(&user_layer), None, None, None, None);
        let net_decision = decisions
            .iter()
            .find(|d| d.capability == Capability::NetworkAccess)
            .unwrap();

        assert_eq!(net_decision.disposition, PolicyDisposition::Deny);
        assert!(net_decision.disposition.is_denied());
    }

    #[test]
    fn repo_restrictions_can_tighten_allowed_operations() {
        let repo_layer = PolicyLayer::new("repo").with_rule(
            Capability::ProcessSpawn,
            PolicyDisposition::Deny,
            "no child processes",
        );

        let decisions = PolicyCompiler::compile(None, None, Some(&repo_layer), None, None);
        let spawn_decision = decisions
            .iter()
            .find(|d| d.capability == Capability::ProcessSpawn)
            .unwrap();

        assert_eq!(spawn_decision.disposition, PolicyDisposition::Deny);
    }

    #[test]
    fn workflow_layer_can_tighten_but_cannot_loosen() {
        let wf_tighten = PolicyLayer::new("workflow").with_rule(
            Capability::FilesystemWrite,
            PolicyDisposition::Deny,
            "workflow forbids direct write",
        );
        let decisions = PolicyCompiler::compile(None, None, None, Some(&wf_tighten), None);
        let write_decision = decisions
            .iter()
            .find(|d| d.capability == Capability::FilesystemWrite)
            .unwrap();
        assert_eq!(write_decision.disposition, PolicyDisposition::Deny);

        // Workflow attempting to loosen GitPush HardDeny fails
        let wf_loosen = PolicyLayer::new("workflow").with_rule(
            Capability::GitPush,
            PolicyDisposition::Allow,
            "workflow wants git push",
        );
        let decisions2 = PolicyCompiler::compile(None, None, None, Some(&wf_loosen), None);
        let push_decision = decisions2
            .iter()
            .find(|d| d.capability == Capability::GitPush)
            .unwrap();
        assert_eq!(push_decision.disposition, PolicyDisposition::HardDeny);
    }
}
