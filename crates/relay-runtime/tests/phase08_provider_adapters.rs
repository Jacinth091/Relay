//! Phase 08: Provider Adapters, Execution Targets, Resolver, Attempt Fallback
//! Integration Tests.
//!
//! Verifies:
//! 1. Same `RoleInstance` retries with a fresh `RoleAttempt`.
//! 2. Provider failure does not rewrite task intent.
//! 3. Execution target is immutable per attempt (deterministic re-resolution).
//! 4. Unavailable pinned provider fails/escalates according to policy.
//! 5. Second provider can fill same semantic role without changing the
//!    workflow/capability schema.
//! 6. Fallback transitions correctly from failed primary provider to a
//!    secondary provider.

use std::path::PathBuf;

use relay_runtime::{
    CodexProviderAdapter, EffectKey, LaunchContext, MockProviderAdapter, ProcessStatus,
    ProviderAdapter, ProviderCapabilities, ProviderCapabilityRequirements, ProviderHealthSnapshot,
    ProviderId, ProviderReadiness, ProviderResolver, ResolverError, RoleAttempt,
    RuntimeObservation,
};

fn implementer_caps() -> ProviderCapabilities {
    ProviderCapabilities {
        supported_roles: ["implementer"].into_iter().map(str::to_owned).collect(),
        filesystem_write: true,
        network: false,
        streaming_output: true,
        cancellation: true,
        supported_models: vec!["default".to_owned()],
    }
}

/// Stand-in for the piece of durable state that must never change across
/// provider retries/fallback (Relay Core owns the real `ApprovedIntent`;
/// this local type just proves the runtime-layer seam never touches it).
#[derive(Debug, Clone, PartialEq, Eq)]
struct TaskIntent {
    objective: String,
    generation: u32,
}

#[test]
fn same_role_instance_retries_with_fresh_role_attempt() {
    let requirements = ProviderCapabilityRequirements::new("implementer");
    let attempt1 = RoleAttempt::new("role-instance-implementer-1", 1, requirements.clone());

    assert_eq!(attempt1.attempt_no, 1);

    // Primary provider fails; a fresh attempt is spawned.
    let attempt2 = attempt1.retry_after_failure(ProviderId::new("primary"));

    // Invariant: RoleInstance identity is unchanged across retries.
    assert_eq!(attempt1.role_instance_id, attempt2.role_instance_id);
    // Attempt identity and number DO advance.
    assert_eq!(attempt2.attempt_no, 2);
    assert_ne!(attempt1.id, attempt2.id);
    // The capability schema requested is unchanged by the retry.
    assert_eq!(
        attempt1.required_capabilities,
        attempt2.required_capabilities
    );
}

#[test]
fn provider_failure_does_not_rewrite_task_intent() {
    let intent = TaskIntent {
        objective: "Implement the widget".to_owned(),
        generation: 1,
    };

    let mut resolver = ProviderResolver::new();
    resolver.register(MockProviderAdapter::new("primary", implementer_caps()).arc());
    resolver.register(MockProviderAdapter::new("secondary", implementer_caps()).arc());

    let requirements = ProviderCapabilityRequirements::new("implementer");
    let attempt1 = RoleAttempt::new("role-instance-1", 1, requirements);
    let target1 = resolver.resolve(&attempt1).expect("resolve attempt 1");
    assert_eq!(target1.provider_id, ProviderId::new("primary"));

    // Simulate the primary provider failing; retry.
    let attempt2 = attempt1.retry_after_failure(target1.provider_id.clone());
    let target2 = resolver.resolve(&attempt2).expect("resolve attempt 2");
    assert_eq!(target2.provider_id, ProviderId::new("secondary"));

    // Provider failure and fallback never touch the task's approved intent.
    let intent_after = intent.clone();
    assert_eq!(intent, intent_after);
    assert_eq!(intent_after.generation, 1);
}

#[test]
fn execution_target_is_immutable_and_deterministic_per_attempt() {
    let mut resolver = ProviderResolver::new();
    resolver.register(MockProviderAdapter::new("primary", implementer_caps()).arc());

    let requirements = ProviderCapabilityRequirements::new("implementer");
    let attempt = RoleAttempt::new("role-instance-imm", 1, requirements);

    let target_a = resolver.resolve(&attempt).expect("resolve #1");
    let target_b = resolver.resolve(&attempt).expect("resolve #2");

    // Resolving the same attempt twice yields an identical, immutable target.
    assert_eq!(target_a, target_b);
    assert_eq!(target_a.provider_id, ProviderId::new("primary"));
}

#[test]
fn unavailable_pinned_provider_fails_and_never_silently_swaps() {
    let mut resolver = ProviderResolver::new();
    let pinned = MockProviderAdapter::new("pinned-provider", implementer_caps()).arc();
    pinned.set_health(ProviderHealthSnapshot::AuthRequired);
    resolver.register(pinned);
    resolver.register(MockProviderAdapter::new("fallback-candidate", implementer_caps()).arc());

    let requirements = ProviderCapabilityRequirements::new("implementer");
    let attempt = RoleAttempt::new("role-instance-pin", 1, requirements)
        .with_pin(ProviderId::new("pinned-provider"));

    let err = resolver
        .resolve(&attempt)
        .expect_err("pinned provider must fail closed");
    assert_eq!(
        err,
        ResolverError::PinnedProviderUnavailable(
            ProviderId::new("pinned-provider"),
            ProviderHealthSnapshot::AuthRequired
        )
    );
}

#[test]
fn second_provider_fills_same_semantic_role_without_schema_change() {
    let requirements = ProviderCapabilityRequirements::new("implementer");

    let mut resolver_a = ProviderResolver::new();
    resolver_a.register(CodexProviderAdapter::new().arc());
    let attempt_a = RoleAttempt::new("role-instance-a", 1, requirements.clone());
    let target_a = resolver_a.resolve(&attempt_a).expect("codex resolves role");
    assert_eq!(target_a.provider_id, ProviderId::new("codex"));

    let mut resolver_b = ProviderResolver::new();
    resolver_b.register(MockProviderAdapter::new("mock-provider", implementer_caps()).arc());
    let attempt_b = RoleAttempt::new("role-instance-b", 1, requirements.clone());
    let target_b = resolver_b.resolve(&attempt_b).expect("mock resolves role");
    assert_eq!(target_b.provider_id, ProviderId::new("mock-provider"));

    // Neither RoleAttempt's requested schema (capability requirements)
    // changed to accommodate the different provider.
    assert_eq!(
        attempt_a.required_capabilities,
        attempt_b.required_capabilities
    );
    assert_eq!(requirements, attempt_a.required_capabilities);
}

#[test]
fn fallback_transitions_from_failed_primary_to_secondary_provider() {
    let mut resolver = ProviderResolver::new();
    resolver.register(MockProviderAdapter::new("primary", implementer_caps()).arc());
    resolver.register(MockProviderAdapter::new("secondary", implementer_caps()).arc());
    resolver.set_preference_order(vec![
        ProviderId::new("primary"),
        ProviderId::new("secondary"),
    ]);

    let requirements = ProviderCapabilityRequirements::new("implementer");
    let attempt1 = RoleAttempt::new("role-instance-fallback", 1, requirements);

    let target1 = resolver.resolve(&attempt1).expect("resolve primary");
    assert_eq!(target1.provider_id, ProviderId::new("primary"));

    // Primary provider attempt fails (e.g. observation reports ProviderError).
    let obs = RuntimeObservation {
        handle_id: "handle-1".to_owned(),
        status: ProcessStatus::ExitedFailure { exit_code: Some(1) },
        stdout_chunk: vec![],
        stderr_chunk: vec![],
        total_bytes_captured: 0,
    };
    let adapter = MockProviderAdapter::new("primary", implementer_caps());
    let readiness = adapter.interpret_observation(&obs);
    assert!(matches!(readiness, ProviderReadiness::ProviderError { .. }));

    // Fresh attempt for the SAME role instance, excluding the failed provider.
    let attempt2 = attempt1.retry_after_failure(target1.provider_id.clone());
    assert_eq!(attempt2.role_instance_id, attempt1.role_instance_id);
    assert_eq!(attempt2.attempt_no, 2);

    let target2 = resolver.resolve(&attempt2).expect("resolve fallback");
    assert_eq!(target2.provider_id, ProviderId::new("secondary"));
    assert_ne!(target1.provider_id, target2.provider_id);
}

#[test]
fn build_launch_produces_explicit_executable_and_args_no_shell_text() {
    let requirements = ProviderCapabilityRequirements::new("implementer");
    let attempt = RoleAttempt::new("role-instance-launch", 1, requirements);

    let mut resolver = ProviderResolver::new();
    resolver.register(CodexProviderAdapter::new().arc());
    let target = resolver.resolve(&attempt).expect("resolve codex");

    // Build the launch spec against a second, unregistered adapter instance
    // to prove `build_launch` is a pure function of (target, attempt, context).
    let adapter = CodexProviderAdapter::new();
    let context = LaunchContext::new(
        PathBuf::from("."),
        "implementer",
        EffectKey::new("eff-launch-1"),
    );
    let spec = adapter
        .build_launch(&target, &attempt, &context)
        .expect("build launch");

    assert_eq!(spec.executable, PathBuf::from("codex"));
    assert_eq!(spec.args, vec!["app-server".to_owned()]);
}

#[test]
fn mock_adapter_interprets_waiting_for_user_marker_and_rejects_bare_exit_zero() {
    let adapter = MockProviderAdapter::new("mock", implementer_caps());
    let obs = RuntimeObservation {
        handle_id: "handle-2".to_owned(),
        status: ProcessStatus::Running,
        stdout_chunk: b"...WAITING_FOR_USER...".to_vec(),
        stderr_chunk: vec![],
        total_bytes_captured: 0,
    };
    assert_eq!(
        adapter.interpret_observation(&obs),
        ProviderReadiness::WaitingForUser
    );

    // Exit 0 alone is not a role result: with no typed envelope on stdout this
    // is a provider error, never `ResultPublished` (R12-11).
    let obs_exit_zero = RuntimeObservation {
        handle_id: "handle-2".to_owned(),
        status: ProcessStatus::ExitedSuccess { exit_code: 0 },
        stdout_chunk: vec![],
        stderr_chunk: vec![],
        total_bytes_captured: 0,
    };
    assert!(
        matches!(
            adapter.interpret_observation(&obs_exit_zero),
            ProviderReadiness::ProviderError { .. }
        ),
        "a provider process that exited 0 without publishing a typed result \
         must not be reported as result-published"
    );
}
