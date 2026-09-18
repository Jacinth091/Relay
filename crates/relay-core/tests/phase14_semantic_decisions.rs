//! Phase 14: SemanticDecisionEngine + Optional Jev Shadow Mode Tests.
//!
//! Verifies:
//! 1. Disabling semantic engines leaves a fully functional deterministic system.
//! 2. `Recommend | Abstain | Escalate` outcomes are handled and audited.
//! 3. Semantic decisions never override deterministic filters (cannot select
//!    an ineligible/illegal provider outside the supplied `choices`).
//! 4. Shadow mode records recommendations without altering the selected decision.

use std::sync::Arc;

use relay_core::{
    ChoiceProbability, DecisionChoice, DecisionManager, DecisionManagerConfig, DecisionOutcome,
    FakeDecisionEngine, ProjectId, SanitizedDecisionState, SemanticDecisionKind,
    SemanticDecisionOutcome, SemanticDecisionPolicy, SemanticDecisionRequest, TaskId,
};

fn request_with(
    choices: Vec<DecisionChoice>,
    policy: SemanticDecisionPolicy,
) -> SemanticDecisionRequest {
    SemanticDecisionRequest::new(
        SemanticDecisionKind::ProviderFit,
        ProjectId::parse("proj-1").unwrap(),
        TaskId::generate(),
        choices,
        SanitizedDecisionState {
            role: Some("implementer".to_owned()),
            project_stack: Some("rust-cli".to_owned()),
            candidate_targets: vec!["codex".to_owned(), "mock".to_owned()],
            independence: Some("fresh_session".to_owned()),
            health: Some("healthy".to_owned()),
            recent_outcomes: vec!["last_attempt: implementation_failure".to_owned()],
        },
        policy,
    )
}

fn outcome_recommending(
    engine_id: &str,
    choice_id: &str,
    confidence: Option<f64>,
) -> SemanticDecisionOutcome {
    SemanticDecisionOutcome {
        engine_id: relay_core::DecisionEngineId::new(engine_id),
        recommendation: DecisionOutcome::Recommend(choice_id.to_owned()),
        probabilities: vec![ChoiceProbability {
            choice_id: choice_id.to_owned(),
            probability: confidence.unwrap_or(1.0),
        }],
        confidence,
        model_version: Some("test-v1".to_owned()),
        latency_ms: Some(5),
    }
}

#[tokio::test]
async fn disabling_semantic_engines_leaves_a_fully_functional_deterministic_system() {
    let config = DecisionManagerConfig {
        semantic_enabled: false,
        shadow_mode: false,
    };
    // Even though an engine IS configured, `semantic_enabled: false` must
    // fully bypass it -- proving the system works with semantic assistance
    // disabled, not merely "unconfigured."
    let engine = Arc::new(FakeDecisionEngine::new(
        "should-never-run",
        Ok(outcome_recommending(
            "should-never-run",
            "codex",
            Some(0.99),
        )),
    ));
    let manager = DecisionManager::new(config, Some(engine));

    let request = request_with(
        vec![
            DecisionChoice::new("codex", "Codex"),
            DecisionChoice::new("mock", "Mock"),
        ],
        SemanticDecisionPolicy {
            min_confidence: None,
            allow_auto_influence: true,
        },
    );

    let resolution = manager.decide(request).await;

    // Multiple legal choices -> deterministic RuleDecisionEngine abstains,
    // and since semantic assistance is disabled, the final decision abstains
    // too -- the system is fully functional (no panic, no crash, no hang),
    // just conservatively deterministic.
    assert_eq!(resolution.final_decision, DecisionOutcome::Abstain);
    assert!(resolution.semantic.is_none());
    assert_eq!(resolution.audit.engine_id, None);
}

#[tokio::test]
async fn recommend_abstain_and_escalate_outcomes_are_handled_and_audited() {
    let choices = vec![
        DecisionChoice::new("codex", "Codex"),
        DecisionChoice::new("mock", "Mock"),
    ];

    // Recommend
    let engine = Arc::new(FakeDecisionEngine::new(
        "fake",
        Ok(outcome_recommending("fake", "mock", Some(0.9))),
    ));
    let manager = DecisionManager::new(
        DecisionManagerConfig {
            semantic_enabled: true,
            shadow_mode: false,
        },
        Some(engine),
    );
    let resolution = manager
        .decide(request_with(
            choices.clone(),
            SemanticDecisionPolicy {
                min_confidence: Some(0.5),
                allow_auto_influence: true,
            },
        ))
        .await;
    assert_eq!(
        resolution.final_decision,
        DecisionOutcome::Recommend(DecisionChoice::new("mock", "Mock"))
    );
    assert_eq!(manager.audit_log().len(), 1);
    assert!(!manager.audit_log()[0].shadow_mode);

    // Abstain
    let engine = Arc::new(FakeDecisionEngine::new(
        "fake",
        Ok(SemanticDecisionOutcome {
            engine_id: relay_core::DecisionEngineId::new("fake"),
            recommendation: DecisionOutcome::Abstain,
            probabilities: vec![],
            confidence: None,
            model_version: None,
            latency_ms: Some(3),
        }),
    ));
    let manager = DecisionManager::new(
        DecisionManagerConfig {
            semantic_enabled: true,
            shadow_mode: false,
        },
        Some(engine),
    );
    let resolution = manager
        .decide(request_with(
            choices.clone(),
            SemanticDecisionPolicy {
                min_confidence: None,
                allow_auto_influence: true,
            },
        ))
        .await;
    // Semantic engine abstained -> falls back to the deterministic engine's
    // own abstain (two legal choices, genuine ambiguity).
    assert_eq!(resolution.final_decision, DecisionOutcome::Abstain);
    assert_eq!(
        resolution.semantic.unwrap().recommendation,
        DecisionOutcome::Abstain
    );

    // Escalate
    let engine = Arc::new(FakeDecisionEngine::new(
        "fake",
        Ok(SemanticDecisionOutcome {
            engine_id: relay_core::DecisionEngineId::new("fake"),
            recommendation: DecisionOutcome::Escalate("ambiguous provider fit".to_owned()),
            probabilities: vec![],
            confidence: None,
            model_version: None,
            latency_ms: Some(3),
        }),
    ));
    let manager = DecisionManager::new(
        DecisionManagerConfig {
            semantic_enabled: true,
            shadow_mode: false,
        },
        Some(engine),
    );
    let resolution = manager
        .decide(request_with(
            choices,
            SemanticDecisionPolicy {
                min_confidence: None,
                allow_auto_influence: true,
            },
        ))
        .await;
    // Escalate always propagates directly: asking for human attention never
    // grants automatic authority, so it needs no confidence gate.
    assert_eq!(
        resolution.semantic.unwrap().recommendation,
        DecisionOutcome::Escalate("ambiguous provider fit".to_owned())
    );
    assert_eq!(
        resolution.final_decision,
        DecisionOutcome::Escalate("ambiguous provider fit".to_owned())
    );
}

#[tokio::test]
async fn abstaining_semantic_engine_never_overrides_a_good_deterministic_recommendation() {
    // Only one legal choice -> the deterministic engine confidently
    // recommends it. A semantic engine that abstains must NOT blank that
    // out to Abstain -- it contributed nothing, so the deterministic pick stands.
    let engine = Arc::new(FakeDecisionEngine::new(
        "fake",
        Ok(SemanticDecisionOutcome {
            engine_id: relay_core::DecisionEngineId::new("fake"),
            recommendation: DecisionOutcome::Abstain,
            probabilities: vec![],
            confidence: None,
            model_version: None,
            latency_ms: Some(1),
        }),
    ));
    let manager = DecisionManager::new(
        DecisionManagerConfig {
            semantic_enabled: true,
            shadow_mode: false,
        },
        Some(engine),
    );

    let request = request_with(
        vec![DecisionChoice::new("codex", "Codex")],
        SemanticDecisionPolicy {
            min_confidence: None,
            allow_auto_influence: true,
        },
    );
    let resolution = manager.decide(request).await;

    assert_eq!(
        resolution.final_decision,
        DecisionOutcome::Recommend(DecisionChoice::new("codex", "Codex"))
    );
}

#[tokio::test]
async fn semantic_decision_cannot_select_an_ineligible_choice() {
    // Deterministic/policy filtering already excluded "denied-provider" --
    // it is never in `choices`. A misbehaving/compromised engine recommends
    // it anyway; the manager must reject it, never letting it become the
    // final decision.
    let engine = Arc::new(FakeDecisionEngine::new(
        "fake",
        Ok(outcome_recommending("fake", "denied-provider", Some(0.99))),
    ));
    let manager = DecisionManager::new(
        DecisionManagerConfig {
            semantic_enabled: true,
            shadow_mode: false,
        },
        Some(engine),
    );

    let request = request_with(
        vec![DecisionChoice::new("codex", "Codex")],
        SemanticDecisionPolicy {
            min_confidence: None,
            allow_auto_influence: true,
        },
    );
    let resolution = manager.decide(request).await;

    // The illegal recommendation is discarded entirely; the deterministic
    // engine's own choice (the single legal option) wins instead.
    assert_eq!(
        resolution.final_decision,
        DecisionOutcome::Recommend(DecisionChoice::new("codex", "Codex"))
    );
    assert!(resolution.semantic.is_none());
    assert_eq!(
        resolution.audit.error_class,
        Some("unknown_choice".to_owned())
    );
}

#[tokio::test]
async fn shadow_mode_records_recommendation_without_altering_the_decision() {
    let engine = Arc::new(FakeDecisionEngine::new(
        "fake",
        Ok(outcome_recommending("fake", "mock", Some(0.99))),
    ));
    let manager = DecisionManager::new(
        DecisionManagerConfig {
            semantic_enabled: true,
            shadow_mode: true,
        },
        Some(engine),
    );

    let request = request_with(
        vec![
            DecisionChoice::new("codex", "Codex"),
            DecisionChoice::new("mock", "Mock"),
        ],
        SemanticDecisionPolicy {
            min_confidence: None,
            // Even with auto-influence allowed by policy, shadow mode must
            // still win -- shadow mode is a manager-level override.
            allow_auto_influence: true,
        },
    );
    let resolution = manager.decide(request).await;

    // Two legal choices -> deterministic engine abstains; shadow mode keeps
    // that as the final decision even though the semantic engine confidently
    // recommended "mock".
    assert_eq!(resolution.final_decision, DecisionOutcome::Abstain);

    // But the recommendation WAS recorded for calibration/audit purposes.
    let semantic = resolution.semantic.expect("semantic outcome recorded");
    assert_eq!(
        semantic.recommendation,
        DecisionOutcome::Recommend("mock".to_owned())
    );
    let audit = &manager.audit_log()[0];
    assert!(audit.shadow_mode);
    assert_eq!(
        audit.semantic_recommendation,
        Some(DecisionOutcome::Recommend("mock".to_owned()))
    );
    assert_eq!(audit.final_decision, DecisionOutcome::Abstain);
}

#[tokio::test]
async fn missing_confidence_never_fabricated_to_pass_a_bar() {
    // Engine recommends without reporting confidence at all, but the policy
    // requires a minimum -- must NOT be treated as passing.
    let engine = Arc::new(FakeDecisionEngine::new(
        "fake",
        Ok(outcome_recommending("fake", "mock", None)),
    ));
    let manager = DecisionManager::new(
        DecisionManagerConfig {
            semantic_enabled: true,
            shadow_mode: false,
        },
        Some(engine),
    );

    let request = request_with(
        vec![
            DecisionChoice::new("codex", "Codex"),
            DecisionChoice::new("mock", "Mock"),
        ],
        SemanticDecisionPolicy {
            min_confidence: Some(0.5),
            allow_auto_influence: true,
        },
    );
    let resolution = manager.decide(request).await;

    assert_eq!(resolution.final_decision, DecisionOutcome::Abstain);
}
