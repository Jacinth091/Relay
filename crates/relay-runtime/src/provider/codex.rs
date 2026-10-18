//! Codex one-shot role launch adapter. The v1 `codex app-server` transport
//! lives in `runtimes/relay-runtime-codex`. Deterministic and synchronous:
//! live installation/authentication probing is owned by `RuntimeIntegration`,
//! not this trait -- `health_check` here reports the last known snapshot,
//! updated externally via `set_health`.

use std::sync::{Arc, Mutex};

use super::{
    ExecutionTarget, LaunchContext, ProviderAdapter, ProviderCapabilities, ProviderError,
    ProviderHealthSnapshot, ProviderId, ProviderReadiness, RoleAttempt,
    interpret_process_observation,
};
use crate::{LaunchSpecification, RuntimeObservation};

pub struct CodexProviderAdapter {
    executable: String,
    health: Mutex<ProviderHealthSnapshot>,
}

impl CodexProviderAdapter {
    pub fn new() -> Self {
        Self {
            executable: "codex".to_owned(),
            health: Mutex::new(ProviderHealthSnapshot::Healthy),
        }
    }

    pub fn with_executable(executable: impl Into<String>) -> Self {
        Self {
            executable: executable.into(),
            health: Mutex::new(ProviderHealthSnapshot::Healthy),
        }
    }

    pub fn set_health(&self, health: ProviderHealthSnapshot) {
        *self.health.lock().expect("codex health lock poisoned") = health;
    }

    pub fn arc(self) -> Arc<Self> {
        Arc::new(self)
    }
}

impl Default for CodexProviderAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderAdapter for CodexProviderAdapter {
    fn provider_id(&self) -> ProviderId {
        ProviderId::new("codex")
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supported_roles: ["implementer", "verifier", "remediator"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            filesystem_write: true,
            network: false,
            streaming_output: true,
            cancellation: true,
            supported_models: vec!["default".to_owned()],
        }
    }

    fn health_check(&self) -> ProviderHealthSnapshot {
        self.health
            .lock()
            .expect("codex health lock poisoned")
            .clone()
    }

    fn build_launch(
        &self,
        target: &ExecutionTarget,
        _attempt: &RoleAttempt,
        context: &LaunchContext,
    ) -> Result<LaunchSpecification, ProviderError> {
        if target.provider_id != self.provider_id() {
            return Err(ProviderError::LaunchBuildFailed(format!(
                "execution target pinned to provider '{}', not codex",
                target.provider_id
            )));
        }
        if context.role_prompt.len() > 24_000 {
            return Err(ProviderError::LaunchBuildFailed(
                "role prompt exceeds the 24000-byte Windows command-line limit".to_owned(),
            ));
        }

        Ok(LaunchSpecification::new(
            self.executable.clone(),
            vec![
                "exec".to_owned(),
                "--skip-git-repo-check".to_owned(),
                "--sandbox".to_owned(),
                "workspace-write".to_owned(),
                context.role_prompt.clone(),
            ],
        ))
    }

    fn interpret_observation(&self, obs: &RuntimeObservation) -> ProviderReadiness {
        interpret_process_observation(obs, |obs| {
            // Preserve a conservative signal if a Codex exec output reports
            // that it is waiting for an approval request.
            String::from_utf8_lossy(&obs.stdout_chunk).contains("requestApproval")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        EffectKey, ProviderCapabilityRequirements, RuntimeModelSelection, RuntimeReasoningSelection,
    };

    fn launch_inputs(
        prompt: String,
    ) -> (
        CodexProviderAdapter,
        ExecutionTarget,
        RoleAttempt,
        LaunchContext,
    ) {
        let adapter = CodexProviderAdapter::new();
        let target = ExecutionTarget {
            provider_id: ProviderId::new("codex"),
            runtime_profile_id: "codex/default".to_owned(),
            model: RuntimeModelSelection::ProviderDefault,
            reasoning_effort: RuntimeReasoningSelection::ProfileDefault,
            capability_snapshot: adapter.capabilities(),
            provider_adapter_version: "1".to_owned(),
        };
        let attempt = RoleAttempt::new(
            "role-instance-codex-launch",
            1,
            ProviderCapabilityRequirements::new("implementer"),
        );
        let context = LaunchContext::new(
            ".",
            "implementer",
            EffectKey::new("effect-codex-launch"),
            prompt,
        );
        (adapter, target, attempt, context)
    }

    #[test]
    fn builds_one_shot_exec_with_prompt_as_one_unmodified_argument() {
        let prompt = "Implement the approved change; treat `\" && del *` as text.".to_owned();
        let (adapter, target, attempt, context) = launch_inputs(prompt.clone());

        let spec = adapter
            .build_launch(&target, &attempt, &context)
            .expect("Codex launch should build");

        assert_eq!(spec.executable, std::path::PathBuf::from("codex"));
        assert_eq!(
            spec.args,
            vec![
                "exec".to_owned(),
                "--skip-git-repo-check".to_owned(),
                "--sandbox".to_owned(),
                "workspace-write".to_owned(),
                prompt,
            ]
        );
    }

    #[test]
    fn rejects_a_prompt_over_the_windows_command_line_limit() {
        let (adapter, target, attempt, context) = launch_inputs("x".repeat(24_001));

        let error = adapter
            .build_launch(&target, &attempt, &context)
            .expect_err("oversized prompt must fail closed");

        assert!(matches!(error, ProviderError::LaunchBuildFailed(_)));
    }
}
