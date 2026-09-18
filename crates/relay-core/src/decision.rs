//! Phase 14: SemanticDecisionEngine + Optional Jev Shadow Mode.
//!
//! Core rule: **intelligence recommends, policy authorizes.**
//!
//! ```text
//! durable deterministic facts
//!   -> hard compatibility/policy filtering
//!   -> legal bounded choice set (`DecisionChoice`)
//!   -> semantic decision needed?
//!        no  -> deterministic choice
//!        yes -> SemanticDecisionEngine -> Recommend / Abstain / Escalate
//!   -> Relay policy/confidence gate (`DecisionManager`)
//!   -> action or human attention
//! ```
//!
//! A semantic engine can only ever recommend one of the `choices` it was
//! given; it cannot invent a provider, path, command, or capability, and it
//! never runs before deterministic/policy filtering has already produced the
//! legal set. See
//! `docs/v2_architecture_rebaseline/07-SEMANTIC-DECISION-ENGINE-HANDOFF.md`.

use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};

use thiserror::Error;

use crate::{ProjectId, TaskId};

// ---------------------------------------------------------------------------
// Identifiers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SemanticDecisionId(pub String);

impl SemanticDecisionId {
    pub fn generate() -> Self {
        Self(format!("dec-{}", uuid::Uuid::new_v4().simple()))
    }
}

impl std::fmt::Display for SemanticDecisionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DecisionEngineId(pub String);

impl DecisionEngineId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl std::fmt::Display for DecisionEngineId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// Decision classes
// ---------------------------------------------------------------------------

/// Initial bounded decision classes. Deliberately excludes scheduler
/// fairness/tie-break, permission/authority grants, and any Git/deploy/
/// database authorization -- those remain deterministic/policy facts (see
/// handoff doc section 9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticDecisionKind {
    /// Selecting a provider from an already deterministically-filtered,
    /// policy-eligible set of execution targets.
    ProviderFit,
    /// Whether to retry fresh, switch target, replan, ask the user, or fail.
    RetryEscalation,
    /// Whether to recommend an additional (optional) human review class.
    AdditionalReviewNeeded,
    /// Ranking/filtering already-trusted optional skills.
    SkillRelevance,
    /// Ranking optional artifact context (never removes mandatory evidence).
    ArtifactRelevance,
}

/// One precomputed legal option. The engine cannot invent choices beyond
/// this set -- `DecisionManager` rejects any recommendation whose id is not
/// present here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionChoice {
    pub id: String,
    pub label: String,
}

impl DecisionChoice {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
        }
    }
}

/// Minimum sanitized data needed for a decision. Structurally typed (not a
/// free-form map) so repository source, raw terminal logs, full
/// conversation history, secrets/tokens, and absolute local paths have no
/// field to be smuggled into -- the privacy boundary is enforced by the
/// shape of this type, not by a runtime scrubber.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SanitizedDecisionState {
    pub role: Option<String>,
    pub project_stack: Option<String>,
    pub candidate_targets: Vec<String>,
    pub independence: Option<String>,
    pub health: Option<String>,
    /// Compact categorical facts only (e.g. "last_attempt: verification_failure").
    pub recent_outcomes: Vec<String>,
}

/// Confidence/fallback policy attached to one decision request.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SemanticDecisionPolicy {
    /// Minimum confidence required before a recommendation may influence the
    /// final decision. `None` means no confidence bar is enforced.
    pub min_confidence: Option<f64>,
    /// Whether a passing recommendation may actually change the deterministic
    /// choice in non-shadow mode. `false` means shadow-mode-equivalent
    /// behavior even if the manager itself is not globally in shadow mode.
    pub allow_auto_influence: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SemanticDecisionRequest {
    pub id: SemanticDecisionId,
    pub kind: SemanticDecisionKind,
    pub project_id: ProjectId,
    pub task_id: TaskId,
    pub choices: Vec<DecisionChoice>,
    pub sanitized_state: SanitizedDecisionState,
    pub policy: SemanticDecisionPolicy,
}

impl SemanticDecisionRequest {
    pub fn new(
        kind: SemanticDecisionKind,
        project_id: ProjectId,
        task_id: TaskId,
        choices: Vec<DecisionChoice>,
        sanitized_state: SanitizedDecisionState,
        policy: SemanticDecisionPolicy,
    ) -> Self {
        Self {
            id: SemanticDecisionId::generate(),
            kind,
            project_id,
            task_id,
            choices,
            sanitized_state,
            policy,
        }
    }
}

// ---------------------------------------------------------------------------
// Outcomes
// ---------------------------------------------------------------------------

/// Generic recommendation shape shared by the trait boundary (`T = String`,
/// a choice id) and the resolved manager result (`T = DecisionChoice`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecisionOutcome<T> {
    Recommend(T),
    Abstain,
    Escalate(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChoiceProbability {
    pub choice_id: String,
    pub probability: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SemanticDecisionOutcome {
    pub engine_id: DecisionEngineId,
    /// A choice id, never a fabricated new option.
    pub recommendation: DecisionOutcome<String>,
    pub probabilities: Vec<ChoiceProbability>,
    /// Optional on purpose: an engine that cannot honestly calibrate
    /// confidence must leave this `None` rather than fabricate a value.
    pub confidence: Option<f64>,
    pub model_version: Option<String>,
    pub latency_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SemanticDecisionError {
    #[error("semantic decision engine timed out")]
    Timeout,
    #[error("semantic decision engine unavailable: {0}")]
    Unavailable(String),
    #[error("semantic decision engine returned a malformed response: {0}")]
    InvalidResponse(String),
}

// ---------------------------------------------------------------------------
// Engine trait
// ---------------------------------------------------------------------------

pub type DecisionFuture<'a> = Pin<
    Box<dyn Future<Output = Result<SemanticDecisionOutcome, SemanticDecisionError>> + Send + 'a>,
>;

/// Provider-neutral advisory interface. Implementations never create
/// authority: they only ever recommend, abstain, or escalate over the
/// `choices` they were handed.
pub trait SemanticDecisionEngine: Send + Sync {
    fn id(&self) -> DecisionEngineId;
    fn decide<'a>(&'a self, request: &'a SemanticDecisionRequest) -> DecisionFuture<'a>;
}

// ---------------------------------------------------------------------------
// RuleDecisionEngine: always-available deterministic baseline
// ---------------------------------------------------------------------------

/// Deterministic baseline/fallback. Always available, never errors, never
/// requires configuration. Mechanical rules only:
/// - zero legal choices -> escalate (nothing safe to pick);
/// - exactly one legal choice -> recommend it (nothing to decide);
/// - more than one -> abstain (genuine ambiguity is not this engine's job).
#[derive(Debug, Default)]
pub struct RuleDecisionEngine;

impl RuleDecisionEngine {
    pub fn new() -> Self {
        Self
    }
}

impl SemanticDecisionEngine for RuleDecisionEngine {
    fn id(&self) -> DecisionEngineId {
        DecisionEngineId::new("rule-engine")
    }

    fn decide<'a>(&'a self, request: &'a SemanticDecisionRequest) -> DecisionFuture<'a> {
        let recommendation = match request.choices.len() {
            0 => DecisionOutcome::Escalate(
                "no legal choices available for deterministic selection".to_owned(),
            ),
            1 => DecisionOutcome::Recommend(request.choices[0].id.clone()),
            _ => DecisionOutcome::Abstain,
        };
        let outcome = SemanticDecisionOutcome {
            engine_id: self.id(),
            recommendation,
            probabilities: Vec::new(),
            confidence: None,
            model_version: None,
            latency_ms: Some(0),
        };
        Box::pin(async move { Ok(outcome) })
    }
}

// ---------------------------------------------------------------------------
// FakeDecisionEngine: deterministic test adapter
// ---------------------------------------------------------------------------

/// Fully scriptable engine for tests. Returns whatever result was last set,
/// independent of the request -- callers assert against known input.
pub struct FakeDecisionEngine {
    id: DecisionEngineId,
    result: Mutex<Result<SemanticDecisionOutcome, SemanticDecisionError>>,
}

impl FakeDecisionEngine {
    pub fn new(
        id: impl Into<String>,
        result: Result<SemanticDecisionOutcome, SemanticDecisionError>,
    ) -> Self {
        Self {
            id: DecisionEngineId::new(id),
            result: Mutex::new(result),
        }
    }

    pub fn set_result(&self, result: Result<SemanticDecisionOutcome, SemanticDecisionError>) {
        *self.result.lock().expect("fake engine lock poisoned") = result;
    }
}

impl SemanticDecisionEngine for FakeDecisionEngine {
    fn id(&self) -> DecisionEngineId {
        self.id.clone()
    }

    fn decide<'a>(&'a self, _request: &'a SemanticDecisionRequest) -> DecisionFuture<'a> {
        let result = self
            .result
            .lock()
            .expect("fake engine lock poisoned")
            .clone();
        Box::pin(async move { result })
    }
}

// ---------------------------------------------------------------------------
// DecisionManager
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecisionManagerConfig {
    /// Master switch. `false` means the system behaves exactly as if no
    /// semantic engine were configured at all -- fully deterministic.
    pub semantic_enabled: bool,
    /// `true`: semantic recommendations are recorded in the audit trail but
    /// NEVER change the selected decision. `false`: a passing recommendation
    /// (per `SemanticDecisionPolicy`) may become the final decision.
    pub shadow_mode: bool,
}

impl Default for DecisionManagerConfig {
    fn default() -> Self {
        Self {
            semantic_enabled: false,
            shadow_mode: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DecisionAuditRecord {
    pub decision_id: SemanticDecisionId,
    pub kind: SemanticDecisionKind,
    pub legal_choice_ids: Vec<String>,
    pub engine_id: Option<DecisionEngineId>,
    pub semantic_recommendation: Option<DecisionOutcome<String>>,
    pub confidence: Option<f64>,
    pub shadow_mode: bool,
    pub final_decision: DecisionOutcome<DecisionChoice>,
    pub policy_min_confidence: Option<f64>,
    pub human_overrode: bool,
    pub latency_ms: Option<u64>,
    /// Set when the semantic engine failed or returned an illegal
    /// recommendation; never set merely because the engine abstained.
    pub error_class: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DecisionResolution {
    pub deterministic: SemanticDecisionOutcome,
    pub semantic: Option<SemanticDecisionOutcome>,
    pub final_decision: DecisionOutcome<DecisionChoice>,
    pub audit: DecisionAuditRecord,
}

fn is_recommendation_legal(rec: &DecisionOutcome<String>, choices: &[DecisionChoice]) -> bool {
    match rec {
        DecisionOutcome::Recommend(id) => choices.iter().any(|c| &c.id == id),
        DecisionOutcome::Abstain | DecisionOutcome::Escalate(_) => true,
    }
}

fn meets_confidence(confidence: Option<f64>, min: Option<f64>) -> bool {
    match (confidence, min) {
        (Some(c), Some(m)) => c >= m,
        // Never fabricate confidence: an engine that didn't report one does
        // not clear an explicit bar.
        (None, Some(_)) => false,
        _ => true,
    }
}

fn resolve_choice(
    rec: DecisionOutcome<String>,
    choices: &[DecisionChoice],
) -> DecisionOutcome<DecisionChoice> {
    match rec {
        DecisionOutcome::Recommend(id) => match choices.iter().find(|c| c.id == id) {
            Some(choice) => DecisionOutcome::Recommend(choice.clone()),
            None => DecisionOutcome::Escalate(format!(
                "resolved recommendation '{id}' is not a legal choice"
            )),
        },
        DecisionOutcome::Abstain => DecisionOutcome::Abstain,
        DecisionOutcome::Escalate(reason) => DecisionOutcome::Escalate(reason),
    }
}

fn classify_error(error: &SemanticDecisionError) -> String {
    match error {
        SemanticDecisionError::Timeout => "timeout".to_owned(),
        SemanticDecisionError::Unavailable(_) => "unavailable".to_owned(),
        SemanticDecisionError::InvalidResponse(_) => "invalid_response".to_owned(),
    }
}

/// Deterministic orchestrator around zero-or-one optional semantic engine.
/// Never models a human as another engine implementation -- humans are
/// reached through the ordinary `DecisionRequest` escalation path, not
/// through this manager.
pub struct DecisionManager {
    config: DecisionManagerConfig,
    rule_engine: RuleDecisionEngine,
    semantic_engine: Option<Arc<dyn SemanticDecisionEngine>>,
    audit_log: Mutex<Vec<DecisionAuditRecord>>,
}

impl DecisionManager {
    pub fn new(
        config: DecisionManagerConfig,
        semantic_engine: Option<Arc<dyn SemanticDecisionEngine>>,
    ) -> Self {
        Self {
            config,
            rule_engine: RuleDecisionEngine::new(),
            semantic_engine,
            audit_log: Mutex::new(Vec::new()),
        }
    }

    pub fn audit_log(&self) -> Vec<DecisionAuditRecord> {
        self.audit_log
            .lock()
            .expect("audit log lock poisoned")
            .clone()
    }

    /// Resolves one decision. Always succeeds with a `DecisionResolution` --
    /// engine failures degrade to the deterministic fallback rather than
    /// propagating as an error, so a semantic-service outage can never
    /// corrupt workflow state.
    pub async fn decide(&self, request: SemanticDecisionRequest) -> DecisionResolution {
        // The deterministic engine is always consulted; it never legitimately errors.
        let deterministic = match self.rule_engine.decide(&request).await {
            Ok(outcome) => outcome,
            Err(_) => SemanticDecisionOutcome {
                engine_id: self.rule_engine.id(),
                recommendation: DecisionOutcome::Escalate(
                    "deterministic engine failed unexpectedly".to_owned(),
                ),
                probabilities: Vec::new(),
                confidence: None,
                model_version: None,
                latency_ms: None,
            },
        };

        let mut semantic_result: Option<SemanticDecisionOutcome> = None;
        let mut error_class: Option<String> = None;

        if self.config.semantic_enabled {
            if let Some(engine) = &self.semantic_engine {
                match engine.decide(&request).await {
                    Ok(outcome) => {
                        if is_recommendation_legal(&outcome.recommendation, &request.choices) {
                            semantic_result = Some(outcome);
                        } else {
                            error_class = Some("unknown_choice".to_owned());
                        }
                    }
                    Err(err) => {
                        error_class = Some(classify_error(&err));
                    }
                }
            }
        }

        let final_recommendation = if self.config.shadow_mode {
            // Shadow mode: the semantic recommendation is recorded (below)
            // but NEVER changes the selected decision.
            deterministic.recommendation.clone()
        } else {
            match semantic_result.as_ref().map(|sem| &sem.recommendation) {
                // Asking for human attention is always safe to honor
                // directly -- it narrows to a human, it never grants
                // automatic authority, so it needs no confidence gate.
                Some(DecisionOutcome::Escalate(reason)) => {
                    DecisionOutcome::Escalate(reason.clone())
                }
                // Only a concrete Recommend may replace the deterministic
                // choice, and only if policy explicitly allows automatic
                // influence AND the confidence bar (if any) is honestly met.
                Some(DecisionOutcome::Recommend(choice_id)) => {
                    let sem = semantic_result.as_ref().expect("checked Some above");
                    if request.policy.allow_auto_influence
                        && meets_confidence(sem.confidence, request.policy.min_confidence)
                    {
                        DecisionOutcome::Recommend(choice_id.clone())
                    } else {
                        deterministic.recommendation.clone()
                    }
                }
                // An abstaining (or absent/failed) semantic engine
                // contributes nothing; the deterministic choice stands.
                Some(DecisionOutcome::Abstain) | None => deterministic.recommendation.clone(),
            }
        };

        let final_decision = resolve_choice(final_recommendation, &request.choices);

        let record = DecisionAuditRecord {
            decision_id: request.id.clone(),
            kind: request.kind,
            legal_choice_ids: request.choices.iter().map(|c| c.id.clone()).collect(),
            engine_id: semantic_result.as_ref().map(|o| o.engine_id.clone()),
            semantic_recommendation: semantic_result.as_ref().map(|o| o.recommendation.clone()),
            confidence: semantic_result.as_ref().and_then(|o| o.confidence),
            shadow_mode: self.config.shadow_mode,
            final_decision: final_decision.clone(),
            policy_min_confidence: request.policy.min_confidence,
            human_overrode: false,
            latency_ms: semantic_result.as_ref().and_then(|o| o.latency_ms),
            error_class,
        };
        self.audit_log
            .lock()
            .expect("audit log lock poisoned")
            .push(record.clone());

        DecisionResolution {
            deterministic,
            semantic: semantic_result,
            final_decision,
            audit: record,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choices(ids: &[&str]) -> Vec<DecisionChoice> {
        ids.iter().map(|id| DecisionChoice::new(*id, *id)).collect()
    }

    #[tokio::test]
    async fn rule_engine_recommends_the_only_legal_choice() {
        let request = SemanticDecisionRequest::new(
            SemanticDecisionKind::ProviderFit,
            ProjectId::parse("proj-1").unwrap(),
            TaskId::generate(),
            choices(&["codex"]),
            SanitizedDecisionState::default(),
            SemanticDecisionPolicy::default(),
        );
        let engine = RuleDecisionEngine::new();
        let outcome = engine.decide(&request).await.unwrap();
        assert_eq!(
            outcome.recommendation,
            DecisionOutcome::Recommend("codex".to_owned())
        );
    }
}
