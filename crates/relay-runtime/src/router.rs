use std::collections::HashMap;
use std::sync::Arc;

use crate::{Runtime, RuntimeIntegration};

/// Routes execution requests to the appropriate provider runtime.
pub struct RuntimeRouter {
    runtimes: HashMap<String, Arc<dyn Runtime>>,
    integrations: HashMap<String, Arc<dyn RuntimeIntegration>>,
    default_profile_id: Option<String>,
}

impl Default for RuntimeRouter {
    fn default() -> Self {
        Self::new()
    }
}

impl RuntimeRouter {
    pub fn new() -> Self {
        Self {
            runtimes: HashMap::new(),
            integrations: HashMap::new(),
            default_profile_id: None,
        }
    }

    /// Register a runtime provider.
    pub fn register(
        &mut self,
        provider_id: impl Into<String>,
        runtime: Arc<dyn Runtime>,
        integration: Arc<dyn RuntimeIntegration>,
    ) {
        let id = provider_id.into();
        self.runtimes.insert(id.clone(), runtime);
        self.integrations.insert(id, integration);
    }

    /// Set the default profile ID.
    pub fn set_default_profile(&mut self, profile_id: impl Into<String>) {
        self.default_profile_id = Some(profile_id.into());
    }

    /// Resolve a provider from a profile ID.
    pub fn resolve_provider<'a>(&self, profile_id: &'a str) -> Option<&'a str> {
        // Profile IDs are "provider/name" format
        profile_id
            .split('/')
            .next()
            .filter(|&provider| self.runtimes.contains_key(provider))
    }

    /// Get the runtime for a provider.
    pub fn runtime(&self, provider_id: &str) -> Option<&Arc<dyn Runtime>> {
        self.runtimes.get(provider_id)
    }

    /// Get the integration for a provider.
    pub fn integration(&self, provider_id: &str) -> Option<&Arc<dyn RuntimeIntegration>> {
        self.integrations.get(provider_id)
    }

    /// List all registered provider IDs.
    pub fn providers(&self) -> Vec<String> {
        self.runtimes.keys().cloned().collect()
    }

    /// Get the default profile ID.
    pub fn default_profile_id(&self) -> Option<&str> {
        self.default_profile_id.as_deref()
    }

    /// List all integrations for health/capability display.
    pub fn integrations_iter(&self) -> impl Iterator<Item = (&str, &Arc<dyn RuntimeIntegration>)> {
        self.integrations.iter().map(|(k, v)| (k.as_str(), v))
    }
}
