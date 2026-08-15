use std::collections::HashMap;
use std::sync::Arc;

use crate::{
    CancellationToken, Runtime, RuntimeCapabilities, RuntimeIdentity, RuntimeIntegration,
    RuntimeRequest, RuntimeStartFuture,
};

/// Routes execution requests to the appropriate provider runtime.
#[derive(Default)]
pub struct RuntimeRouter {
    runtimes: HashMap<String, Arc<dyn Runtime>>,
    integrations: HashMap<String, Arc<dyn RuntimeIntegration>>,
    default_profile_id: Option<String>,
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

    /// List all registered provider IDs deterministically.
    pub fn providers(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.runtimes.keys().cloned().collect();
        keys.sort();
        keys
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

impl Runtime for RuntimeRouter {
    fn identity(&self) -> RuntimeIdentity {
        let profile_id = self
            .default_profile_id
            .clone()
            .unwrap_or_else(|| "codex/implement".to_owned());
        let provider = self
            .resolve_provider(&profile_id)
            .unwrap_or("codex")
            .to_owned();
        RuntimeIdentity {
            runtime_id: provider.clone(),
            profile_id,
            display_name: format!("Relay Router ({provider})"),
        }
    }

    fn capabilities(&self) -> RuntimeCapabilities {
        let profile_id = self
            .default_profile_id
            .as_deref()
            .unwrap_or("codex/implement");
        if let Some(provider) = self.resolve_provider(profile_id) {
            if let Some(runtime) = self.runtime(provider) {
                return runtime.capabilities();
            }
        }
        RuntimeCapabilities::default()
    }

    fn start(
        &self,
        request: RuntimeRequest,
        cancellation: CancellationToken,
    ) -> RuntimeStartFuture {
        let profile_id = request.profile_id.clone();
        let provider = match self.resolve_provider(&profile_id) {
            Some(p) => p.to_owned(),
            None => {
                return Box::pin(async move {
                    Err(crate::RuntimeError::StartFailed(format!(
                        "Unknown provider for profile '{profile_id}'"
                    )))
                });
            }
        };

        if let Some(runtime) = self.runtime(&provider) {
            runtime.start(request, cancellation)
        } else {
            Box::pin(async move {
                Err(crate::RuntimeError::StartFailed(format!(
                    "Runtime provider '{provider}' not registered in router"
                )))
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DummyRuntime;
    impl Runtime for DummyRuntime {
        fn identity(&self) -> RuntimeIdentity {
            RuntimeIdentity {
                runtime_id: "dummy".to_owned(),
                profile_id: "dummy/default".to_owned(),
                display_name: "Dummy".to_owned(),
            }
        }
        fn capabilities(&self) -> RuntimeCapabilities {
            RuntimeCapabilities::default()
        }
        fn start(
            &self,
            _request: RuntimeRequest,
            _cancellation: CancellationToken,
        ) -> RuntimeStartFuture {
            Box::pin(async move { Err(crate::RuntimeError::StartFailed("dummy".to_owned())) })
        }
    }

    struct DummyIntegration;
    impl RuntimeIntegration for DummyIntegration {
        fn descriptor(&self) -> crate::RuntimeDescriptor {
            crate::RuntimeDescriptor {
                provider_id: "dummy".to_owned(),
                display_name: "Dummy".to_owned(),
                description: "Dummy".to_owned(),
                is_development_only: true,
            }
        }
        fn probe(
            &self,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = crate::RuntimeProbe> + Send + '_>>
        {
            Box::pin(async move {
                crate::RuntimeProbe {
                    installed: true,
                    version: None,
                    authenticated: true,
                    compatible: true,
                    account_identity: None,
                    notes: vec![],
                }
            })
        }
        fn profiles(&self) -> Vec<crate::RuntimeProfileDescriptor> {
            vec![]
        }
        fn resolve_profile(
            &self,
            _profile_id: &str,
        ) -> Result<crate::EffectiveRuntimeProfile, crate::RuntimeConfigError> {
            Err(crate::RuntimeConfigError::UnknownProfile {
                profile_id: "dummy".to_owned(),
            })
        }
        fn models(
            &self,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = crate::RuntimeModelCatalog> + Send + '_>,
        > {
            Box::pin(async move {
                crate::RuntimeModelCatalog {
                    default_model: "default".to_owned(),
                    available_models: vec![],
                }
            })
        }
        fn verify_capabilities(
            &self,
            _profile: &crate::EffectiveRuntimeProfile,
            _context: &crate::RuntimeCapabilityContext,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = crate::RuntimeCapabilityReport> + Send + '_>,
        > {
            Box::pin(async move {
                crate::RuntimeCapabilityReport {
                    filesystem_read: crate::CapabilityStatus::Verified,
                    filesystem_write: crate::CapabilityStatus::Verified,
                    command_execution: crate::CapabilityStatus::Verified,
                    network: crate::CapabilityStatus::Configured,
                    notes: vec![],
                }
            })
        }
    }

    #[test]
    fn routes_requests_by_profile_prefix() {
        let mut router = RuntimeRouter::new();
        let dummy = Arc::new(DummyRuntime);
        let integration = Arc::new(DummyIntegration);
        router.register("codex", dummy, integration);
        router.set_default_profile("codex/implement");

        assert_eq!(router.resolve_provider("codex/implement"), Some("codex"));
        assert_eq!(router.resolve_provider("codex/review"), Some("codex"));
        assert_eq!(router.resolve_provider("unknown/implement"), None);
    }
}
