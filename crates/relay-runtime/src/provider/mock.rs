//! Deterministic, fully scriptable `ProviderAdapter` for unit/integration
//! tests (the "MockProviderAdapter" / "TestProviderAdapter" of the Phase 08
//! spec). Health and capabilities can be mutated at runtime to simulate
//! quota exhaustion, auth loss, or unavailability without spawning real
//! processes.

use std::sync::{Arc, Mutex};

use super::{
    ExecutionTarget, LaunchContext, ProviderAdapter, ProviderCapabilities, ProviderError,
    ProviderHealthSnapshot, ProviderId, ProviderReadiness, RoleAttempt,
    interpret_process_observation,
};
use crate::{LaunchSpecification, RuntimeObservation};

pub struct MockProviderAdapter {
    provider_id: ProviderId,
    capabilities: ProviderCapabilities,
    health: Mutex<ProviderHealthSnapshot>,
    launch_executable: String,
}

impl MockProviderAdapter {
    pub fn new(provider_id: impl Into<String>, capabilities: ProviderCapabilities) -> Self {
        let provider_id = ProviderId::new(provider_id);
        Self {
            launch_executable: format!("mock-{}", provider_id.0),
            provider_id,
            capabilities,
            health: Mutex::new(ProviderHealthSnapshot::Healthy),
        }
    }

    pub fn set_health(&self, health: ProviderHealthSnapshot) {
        *self.health.lock().expect("mock health lock poisoned") = health;
    }

    pub fn arc(self) -> Arc<Self> {
        Arc::new(self)
    }
}

impl ProviderAdapter for MockProviderAdapter {
    fn provider_id(&self) -> ProviderId {
        self.provider_id.clone()
    }

    fn capabilities(&self) -> ProviderCapabilities {
        self.capabilities.clone()
    }

    fn health_check(&self) -> ProviderHealthSnapshot {
        self.health
            .lock()
            .expect("mock health lock poisoned")
            .clone()
    }

    fn build_launch(
        &self,
        _target: &ExecutionTarget,
        attempt: &RoleAttempt,
        context: &LaunchContext,
    ) -> Result<LaunchSpecification, ProviderError> {
        Ok(LaunchSpecification::new(
            self.launch_executable.clone(),
            vec![
                "--role-attempt".to_owned(),
                attempt.id.0.clone(),
                "--cwd".to_owned(),
                context.trusted_cwd.display().to_string(),
            ],
        ))
    }

    fn interpret_observation(&self, obs: &RuntimeObservation) -> ProviderReadiness {
        interpret_process_observation(obs, |obs| {
            String::from_utf8_lossy(&obs.stdout_chunk).contains("WAITING_FOR_USER")
        })
    }
}
