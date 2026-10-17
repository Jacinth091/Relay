//! Phase 08: Deterministic Provider Resolver.
//!
//! Resolves the `ExecutionTarget` for a `RoleAttempt` following strict
//! deterministic precedence:
//!   1. explicit valid pin (if specified);
//!   2. hard compatibility / capability filter;
//!   3. policy restrictions (project/user/repo allowlist);
//!   4. health / auth / quota observable facts;
//!   5. deterministic preference order.
//!
//! An unavailable pinned provider fails/escalates according to policy; it
//! is never silently swapped for another provider.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use thiserror::Error;

use crate::{ExecutionTarget, ProviderAdapter, ProviderHealthSnapshot, ProviderId, RoleAttempt};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ResolverError {
    #[error("pinned provider '{0}' is not registered")]
    PinnedProviderNotRegistered(ProviderId),
    #[error("pinned provider '{0}' does not support the required role/capabilities")]
    PinnedProviderIncompatible(ProviderId),
    #[error("pinned provider '{0}' is denied by policy")]
    PinnedProviderPolicyDenied(ProviderId),
    #[error("pinned provider '{0}' is unavailable: {1:?}")]
    PinnedProviderUnavailable(ProviderId, ProviderHealthSnapshot),
    #[error("no eligible provider found for role '{0}'")]
    NoEligibleProvider(String),
}

/// Deterministic provider resolver. Resolution outcome depends only on
/// registered adapters' declared state, the explicit preference order, and
/// the policy allowlist -- never on registration timing or randomness.
pub struct ProviderResolver {
    adapters: BTreeMap<ProviderId, Arc<dyn ProviderAdapter>>,
    preference_order: Vec<ProviderId>,
    policy_allowlist: Option<BTreeSet<ProviderId>>,
}

impl ProviderResolver {
    pub fn new() -> Self {
        Self {
            adapters: BTreeMap::new(),
            preference_order: Vec::new(),
            policy_allowlist: None,
        }
    }

    /// Registers a provider adapter. Preference order position defaults to
    /// registration call order unless overridden by `set_preference_order`.
    pub fn register(&mut self, adapter: Arc<dyn ProviderAdapter>) {
        let id = adapter.provider_id();
        if !self.preference_order.contains(&id) {
            self.preference_order.push(id.clone());
        }
        self.adapters.insert(id, adapter);
    }

    /// Overrides the deterministic preference order explicitly.
    pub fn set_preference_order(&mut self, order: Vec<ProviderId>) {
        self.preference_order = order;
    }

    /// Restricts eligible providers to an explicit allowlist
    /// (project/user/repo policy). `None` (the default) allows any
    /// registered provider.
    pub fn set_policy_allowlist(&mut self, allowlist: BTreeSet<ProviderId>) {
        self.policy_allowlist = Some(allowlist);
    }

    fn policy_allows(&self, provider_id: &ProviderId) -> bool {
        match &self.policy_allowlist {
            Some(allowlist) => allowlist.contains(provider_id),
            None => true,
        }
    }

    /// Resolves the immutable `ExecutionTarget` for a role attempt.
    pub fn resolve(&self, attempt: &RoleAttempt) -> Result<ExecutionTarget, ResolverError> {
        match &attempt.provider_pin {
            Some(pin) => self.resolve_pinned(pin, attempt),
            None => self.resolve_by_preference(attempt),
        }
    }

    fn resolve_pinned(
        &self,
        pin: &ProviderId,
        attempt: &RoleAttempt,
    ) -> Result<ExecutionTarget, ResolverError> {
        let adapter = self
            .adapters
            .get(pin)
            .ok_or_else(|| ResolverError::PinnedProviderNotRegistered(pin.clone()))?;

        if !adapter
            .capabilities()
            .satisfies(&attempt.required_capabilities)
        {
            return Err(ResolverError::PinnedProviderIncompatible(pin.clone()));
        }

        if !self.policy_allows(pin) {
            return Err(ResolverError::PinnedProviderPolicyDenied(pin.clone()));
        }

        let health = adapter.health_check();
        if !health.is_eligible() {
            // Invariant: never silently swap an explicit pin for another provider.
            return Err(ResolverError::PinnedProviderUnavailable(
                pin.clone(),
                health,
            ));
        }

        Ok(build_target(adapter.as_ref(), attempt))
    }

    fn resolve_by_preference(
        &self,
        attempt: &RoleAttempt,
    ) -> Result<ExecutionTarget, ResolverError> {
        for provider_id in &self.preference_order {
            if attempt.excluded_providers.contains(provider_id) {
                continue;
            }
            let Some(adapter) = self.adapters.get(provider_id) else {
                continue;
            };
            if !adapter
                .capabilities()
                .satisfies(&attempt.required_capabilities)
            {
                continue;
            }
            if !self.policy_allows(provider_id) {
                continue;
            }
            if !adapter.health_check().is_eligible() {
                continue;
            }
            return Ok(build_target(adapter.as_ref(), attempt));
        }
        Err(ResolverError::NoEligibleProvider(
            attempt.required_capabilities.required_role.clone(),
        ))
    }
}

impl Default for ProviderResolver {
    fn default() -> Self {
        Self::new()
    }
}

fn build_target(adapter: &dyn ProviderAdapter, attempt: &RoleAttempt) -> ExecutionTarget {
    ExecutionTarget {
        provider_id: adapter.provider_id(),
        runtime_profile_id: attempt.runtime_profile_id.clone(),
        model: attempt.model.clone(),
        reasoning_effort: attempt.reasoning_effort,
        capability_snapshot: adapter.capabilities(),
        provider_adapter_version: env!("CARGO_PKG_VERSION").to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MockProviderAdapter, ProviderCapabilities, ProviderCapabilityRequirements};

    fn caps(roles: &[&str]) -> ProviderCapabilities {
        ProviderCapabilities {
            supported_roles: roles.iter().map(|r| r.to_string()).collect(),
            filesystem_write: true,
            network: false,
            streaming_output: true,
            cancellation: true,
            supported_models: vec!["default".to_owned()],
        }
    }

    #[test]
    fn resolves_by_deterministic_preference_order() {
        let mut resolver = ProviderResolver::new();
        resolver.register(MockProviderAdapter::new("alpha", caps(&["implementer"])).arc());
        resolver.register(MockProviderAdapter::new("beta", caps(&["implementer"])).arc());

        let attempt = RoleAttempt::new(
            "role-1",
            1,
            ProviderCapabilityRequirements::new("implementer"),
        );

        let target = resolver.resolve(&attempt).unwrap();
        assert_eq!(target.provider_id, ProviderId::new("alpha"));
    }

    #[test]
    fn hard_capability_filter_excludes_incompatible_providers() {
        let mut resolver = ProviderResolver::new();
        resolver.register(MockProviderAdapter::new("alpha", caps(&["verifier"])).arc());
        resolver.register(MockProviderAdapter::new("beta", caps(&["implementer"])).arc());

        let attempt = RoleAttempt::new(
            "role-1",
            1,
            ProviderCapabilityRequirements::new("implementer"),
        );

        let target = resolver.resolve(&attempt).unwrap();
        assert_eq!(target.provider_id, ProviderId::new("beta"));
    }

    #[test]
    fn policy_allowlist_restricts_eligible_providers() {
        let mut resolver = ProviderResolver::new();
        resolver.register(MockProviderAdapter::new("alpha", caps(&["implementer"])).arc());
        resolver.register(MockProviderAdapter::new("beta", caps(&["implementer"])).arc());
        resolver.set_policy_allowlist(BTreeSet::from([ProviderId::new("beta")]));

        let attempt = RoleAttempt::new(
            "role-1",
            1,
            ProviderCapabilityRequirements::new("implementer"),
        );

        let target = resolver.resolve(&attempt).unwrap();
        assert_eq!(target.provider_id, ProviderId::new("beta"));
    }

    #[test]
    fn pinned_unavailable_provider_never_silently_swaps() {
        let mut resolver = ProviderResolver::new();
        let alpha = MockProviderAdapter::new("alpha", caps(&["implementer"])).arc();
        alpha.set_health(ProviderHealthSnapshot::QuotaExhausted);
        resolver.register(alpha);
        resolver.register(MockProviderAdapter::new("beta", caps(&["implementer"])).arc());

        let attempt = RoleAttempt::new(
            "role-1",
            1,
            ProviderCapabilityRequirements::new("implementer"),
        )
        .with_pin(ProviderId::new("alpha"));

        let err = resolver.resolve(&attempt).unwrap_err();
        assert_eq!(
            err,
            ResolverError::PinnedProviderUnavailable(
                ProviderId::new("alpha"),
                ProviderHealthSnapshot::QuotaExhausted
            )
        );
    }

    #[test]
    fn unregistered_pin_fails_closed() {
        let mut resolver = ProviderResolver::new();
        resolver.register(MockProviderAdapter::new("beta", caps(&["implementer"])).arc());

        let attempt = RoleAttempt::new(
            "role-1",
            1,
            ProviderCapabilityRequirements::new("implementer"),
        )
        .with_pin(ProviderId::new("alpha"));

        let err = resolver.resolve(&attempt).unwrap_err();
        assert_eq!(
            err,
            ResolverError::PinnedProviderNotRegistered(ProviderId::new("alpha"))
        );
    }

    #[test]
    fn no_eligible_provider_when_none_registered() {
        let resolver = ProviderResolver::new();
        let attempt = RoleAttempt::new(
            "role-1",
            1,
            ProviderCapabilityRequirements::new("implementer"),
        );

        let err = resolver.resolve(&attempt).unwrap_err();
        assert_eq!(
            err,
            ResolverError::NoEligibleProvider("implementer".to_owned())
        );
    }
}
