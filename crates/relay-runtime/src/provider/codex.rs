//! Codex CLI provider adapter, aligned with `relay-runtime-codex`'s
//! `codex app-server` transport (stdio JSONL, the historical default; see
//! `runtimes/relay-runtime-codex`). Deterministic and synchronous: live
//! installation/authentication probing is owned by `RuntimeIntegration`,
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
        _context: &LaunchContext,
    ) -> Result<LaunchSpecification, ProviderError> {
        if target.provider_id != self.provider_id() {
            return Err(ProviderError::LaunchBuildFailed(format!(
                "execution target pinned to provider '{}', not codex",
                target.provider_id
            )));
        }
        // Aligned with relay-runtime-codex: app-server stdio JSONL transport
        // is the default. Model/reasoning are negotiated over the protocol
        // handshake, never passed as untrusted bare argv text.
        Ok(LaunchSpecification::new(
            self.executable.clone(),
            vec!["app-server".to_owned()],
        ))
    }

    fn interpret_observation(&self, obs: &RuntimeObservation) -> ProviderReadiness {
        interpret_process_observation(obs, |obs| {
            // Codex app-server signals an approval prompt via a
            // `*requestApproval*` JSON-RPC method name on stdout.
            String::from_utf8_lossy(&obs.stdout_chunk).contains("requestApproval")
        })
    }
}
