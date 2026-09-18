//! Optional Jev semantic-decision adapter.
//!
//! Kept in its own crate and isolated from `relay-core`, per
//! `docs/v2_architecture_rebaseline/07-SEMANTIC-DECISION-ENGINE-HANDOFF.md`
//! section 11 ("Keep Jev-specific client/types outside relay-core") and
//! acceptance criterion 7 ("Jev adapter is isolated from Relay Core").
//!
//! The current TypeSafe AI/Jev API/SDK shape, authentication, typed
//! decision/result model, data-handling/privacy terms, and availability have
//! not been verified against this codebase. Per the handoff doc ("do not
//! freeze adapter code from stale documentation assumptions"), this adapter
//! deliberately does NOT call any external service yet. It exists to prove
//! two things:
//!
//! 1. the isolation boundary -- Jev-specific code never lives in `relay-core`;
//! 2. the safe-degradation contract -- while disabled/unconfigured (the
//!    default), this engine always reports `Unavailable`, which
//!    `relay_core::DecisionManager` treats exactly like "no semantic
//!    assistance configured," falling back to the deterministic engine.
//!
//! Wiring a real HTTP client belongs to a follow-up change made only after
//! step 1 of the handoff doc's rollout ("verify current external API/
//! privacy/credential model") has actually happened.

use std::sync::atomic::{AtomicBool, Ordering};

use relay_core::{
    DecisionEngineId, DecisionFuture, SemanticDecisionEngine, SemanticDecisionError,
    SemanticDecisionRequest,
};

/// Feature/config gate. Defaults to disabled: enabling requires an explicit
/// operator opt-in, and even then this build has no live transport wired up.
pub struct JevDecisionEngine {
    enabled: AtomicBool,
}

impl JevDecisionEngine {
    /// Constructs the adapter in its default, disabled state.
    pub fn new() -> Self {
        Self {
            enabled: AtomicBool::new(false),
        }
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::SeqCst);
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }
}

impl Default for JevDecisionEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl SemanticDecisionEngine for JevDecisionEngine {
    fn id(&self) -> DecisionEngineId {
        DecisionEngineId::new("jev")
    }

    fn decide<'a>(&'a self, _request: &'a SemanticDecisionRequest) -> DecisionFuture<'a> {
        let enabled = self.is_enabled();
        Box::pin(async move {
            if !enabled {
                return Err(SemanticDecisionError::Unavailable(
                    "Jev adapter is disabled".to_owned(),
                ));
            }
            // No real external transport is wired up yet -- see module doc.
            // Reporting Unavailable (never Recommend/Abstain) guarantees a
            // caller can never mistake this build for a live integration.
            Err(SemanticDecisionError::Unavailable(
                "Jev live integration not yet implemented pending API verification (see docs/v2_architecture_rebaseline/07-SEMANTIC-DECISION-ENGINE-HANDOFF.md section 11)".to_owned(),
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use relay_core::{
        ProjectId, SanitizedDecisionState, SemanticDecisionKind, SemanticDecisionPolicy, TaskId,
    };

    fn sample_request() -> SemanticDecisionRequest {
        SemanticDecisionRequest::new(
            SemanticDecisionKind::ProviderFit,
            ProjectId::parse("proj-1").unwrap(),
            TaskId::generate(),
            vec![],
            SanitizedDecisionState::default(),
            SemanticDecisionPolicy::default(),
        )
    }

    #[tokio::test]
    async fn disabled_by_default_and_never_recommends() {
        let engine = JevDecisionEngine::new();
        assert!(!engine.is_enabled());

        let request = sample_request();
        let err = engine.decide(&request).await.unwrap_err();
        assert!(matches!(err, SemanticDecisionError::Unavailable(_)));
    }

    #[tokio::test]
    async fn enabling_still_never_fabricates_a_live_recommendation() {
        let engine = JevDecisionEngine::new();
        engine.set_enabled(true);
        assert!(engine.is_enabled());

        let request = sample_request();
        // Even enabled, there is no real transport wired up in this build --
        // it must still degrade safely rather than invent a recommendation.
        let err = engine.decide(&request).await.unwrap_err();
        assert!(matches!(err, SemanticDecisionError::Unavailable(_)));
    }
}
