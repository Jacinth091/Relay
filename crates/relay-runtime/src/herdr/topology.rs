//! Herdr internal topology planner and metadata management.
//!
//! IMPORTANT ARCHITECTURAL INVARIANT:
//! The mapping between Relay domain concepts and Herdr physical hierarchy
//! (Checkout -> Workspace, Task -> Tab, RoleAttempt -> Pane) is STRICTLY
//! an internal adapter concern. It is NEVER exposed outside to Relay Core
//! or used as Relay identity.

use std::{collections::HashMap, path::Path};

use crate::backplane::{EffectKey, RuntimeMetadata, RuntimeProvisionRequest};

/// Internal topology planner mapping Relay execution requests to Herdr resources.
#[derive(Debug, Clone, Default)]
pub struct HerdrTopologyPlanner;

impl HerdrTopologyPlanner {
    pub fn new() -> Self {
        Self
    }

    /// Derives the display label for a checkout workspace.
    pub fn workspace_label(&self, checkout_id: Option<&str>, trusted_cwd: &Path) -> String {
        if let Some(cid) = checkout_id {
            format!("Relay Workspace [{}]", sanitize_id(cid))
        } else {
            let folder_name = trusted_cwd
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("default");
            format!("Relay Workspace [{}]", sanitize_id(folder_name))
        }
    }

    /// Builds the internal topology plan for a provisioning request.
    pub fn plan(&self, req: &RuntimeProvisionRequest) -> HerdrTopologyPlan {
        let checkout_id = req.metadata.as_ref().and_then(|m| m.checkout_id.as_deref());
        let workspace_label = self.workspace_label(checkout_id, &req.trusted_cwd);
        let tab_label = format!("Relay Attempt [{}]", sanitize_id(&req.role_attempt_id));

        let metadata_tags = self.encode_metadata(req);

        HerdrTopologyPlan {
            workspace_label,
            tab_label,
            metadata_tags,
        }
    }

    /// Encodes Relay metadata into Herdr-compatible tags.
    /// Rejects secret keys and untrusted shell scripts.
    pub fn encode_metadata(&self, req: &RuntimeProvisionRequest) -> HashMap<String, String> {
        let mut tags = HashMap::new();
        tags.insert("relay.managed_by".to_string(), "relay".to_string());
        tags.insert(
            "relay.role_attempt_id".to_string(),
            req.role_attempt_id.clone(),
        );
        tags.insert("relay.effect_key".to_string(), req.effect_key.0.clone());

        if let Some(meta) = &req.metadata {
            if let Some(root_id) = &meta.state_root_id {
                tags.insert("relay.state_root_id".to_string(), root_id.clone());
            }
            if let Some(project_id) = &meta.project_id {
                tags.insert("relay.project_id".to_string(), project_id.clone());
            }
            if let Some(checkout_id) = &meta.checkout_id {
                tags.insert("relay.checkout_id".to_string(), checkout_id.clone());
            }
            if let Some(task_id) = &meta.task_id {
                tags.insert("relay.task_id".to_string(), task_id.clone());
            }
            if let Some(wf_id) = &meta.workflow_run_id {
                tags.insert("relay.workflow_run_id".to_string(), wf_id.clone());
            }
            if let Some(inst_id) = &meta.role_instance_id {
                tags.insert("relay.role_instance_id".to_string(), inst_id.clone());
            }
        }

        tags
    }

    /// Decodes Relay metadata from Herdr tags.
    pub fn decode_metadata(&self, tags: &HashMap<String, String>) -> Option<RuntimeMetadata> {
        let role_attempt_id = tags.get("relay.role_attempt_id")?.clone();
        let effect_key_raw = tags.get("relay.effect_key")?.clone();

        Some(RuntimeMetadata {
            state_root_id: tags.get("relay.state_root_id").cloned(),
            project_id: tags.get("relay.project_id").cloned(),
            checkout_id: tags.get("relay.checkout_id").cloned(),
            task_id: tags.get("relay.task_id").cloned(),
            workflow_run_id: tags.get("relay.workflow_run_id").cloned(),
            role_instance_id: tags.get("relay.role_instance_id").cloned(),
            role_attempt_id,
            effect_key: EffectKey::new(effect_key_raw),
        })
    }
}

/// Planned internal physical resources in Herdr.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrTopologyPlan {
    pub workspace_label: String,
    pub tab_label: String,
    pub metadata_tags: HashMap<String, String>,
}

fn sanitize_id(input: &str) -> String {
    input
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backplane::{LaunchSpecification, SanitizedEnvironment};
    use std::path::PathBuf;

    #[test]
    fn test_topology_planner_isolation_and_tagging() {
        let planner = HerdrTopologyPlanner::new();

        let mut meta = RuntimeMetadata::new("attempt-42", EffectKey::new("eff-42"));
        meta.task_id = Some("task-99".to_string());
        meta.checkout_id = Some("chk-main".to_string());
        meta.project_id = Some("proj-alpha".to_string());

        let req = RuntimeProvisionRequest {
            effect_key: EffectKey::new("eff-42"),
            role_attempt_id: "attempt-42".to_string(),
            trusted_cwd: PathBuf::from("/trusted/repo"),
            launch: LaunchSpecification::new("git", vec!["status".to_string()]),
            environment: SanitizedEnvironment::new(),
            output_byte_limit: 1024,
            metadata: Some(meta),
        };

        let plan = planner.plan(&req);
        assert_eq!(plan.workspace_label, "Relay Workspace [chk-main]");
        assert_eq!(plan.tab_label, "Relay Attempt [attempt-42]");

        // Verify tags
        assert_eq!(
            plan.metadata_tags.get("relay.managed_by"),
            Some(&"relay".to_string())
        );
        assert_eq!(
            plan.metadata_tags.get("relay.role_attempt_id"),
            Some(&"attempt-42".to_string())
        );
        assert_eq!(
            plan.metadata_tags.get("relay.task_id"),
            Some(&"task-99".to_string())
        );

        // Decode roundtrip
        let decoded = planner
            .decode_metadata(&plan.metadata_tags)
            .expect("decode failed");
        assert_eq!(decoded.role_attempt_id, "attempt-42");
        assert_eq!(decoded.task_id, Some("task-99".to_string()));
        assert_eq!(decoded.effect_key.0, "eff-42");
    }
}
