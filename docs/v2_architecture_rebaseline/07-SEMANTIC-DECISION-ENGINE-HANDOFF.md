# Relay v2 Semantic Decision Engine + Optional Jev Handoff

# 1. Objective

Add bounded semantic recommendations only where deterministic rules cannot reliably express preference and where a full reasoning-agent workflow would be disproportionate.

Semantic decisions are **not** part of the v2 correctness foundation.

# 2. Core Rule

> **Intelligence recommends. Policy authorizes.**

Pipeline:

```text
durable deterministic facts
-> hard compatibility/policy filtering
-> legal bounded choice set
-> semantic decision needed?
   no -> deterministic choice
   yes -> SemanticDecisionEngine
          -> Recommend / Abstain / Escalate
          -> confidence policy
-> Relay policy/workflow gate
-> action or human attention
```

A semantic engine cannot create a new capability, provider, path, command, or privileged operation outside the supplied legal choices.

# 3. DecisionManager

Do not model the human as another decision-engine implementation.

Use a deterministic `DecisionManager` that:

- decides whether semantic assistance is enabled/allowed;
- builds sanitized bounded requests;
- invokes configured engine;
- applies confidence/fallback policy;
- records provenance;
- escalates to normal `DecisionRequest` when human judgment is required.

# 4. Provider-Neutral Interface

Conceptual:

```rust
trait SemanticDecisionEngine {
    fn id(&self) -> DecisionEngineId;

    async fn decide(
        &self,
        request: SemanticDecisionRequest,
    ) -> Result<SemanticDecisionOutcome, SemanticDecisionError>;
}
```

Keep Jev-specific client/types outside relay-core.

# 5. Engine Implementations

## RuleDecisionEngine

Always available deterministic baseline.

Examples:

- explicit pinned target -> choose/fail;
- one compatible target -> choose;
- hard retry ceiling -> escalate;
- configured provider preference -> choose first healthy compatible target.

## FakeDecisionEngine

Deterministic test adapter.

## JevDecisionEngine

Optional external adapter, only after v2 deterministic path is stable.

## ReasoningDecisionEngine

Future optional adapter if a decision class demonstrably needs broader reasoning. It must use the same bounded choice contract.

No `UserDecisionEngine`. Humans receive normal DecisionRequests.

# 6. Request Contract

```rust
struct SemanticDecisionRequest {
    id: SemanticDecisionId,
    kind: SemanticDecisionKind,
    project_id: ProjectId,
    task_id: TaskId,
    choices: Vec<DecisionChoice>,
    sanitized_state: SanitizedDecisionState,
    policy: SemanticDecisionPolicy,
}
```

Deterministic code produces `choices` first.

# 7. Outcome Contract

```rust
struct SemanticDecisionOutcome {
    engine_id: DecisionEngineId,
    recommendation: SemanticRecommendation,
    probabilities: Vec<ChoiceProbability>,
    confidence: Option<f64>,
    model_version: Option<String>,
    latency_ms: Option<u64>,
}
```

Recommendation:

```text
Recommend(choice_id)
Abstain
Escalate(reason)
```

Do not require every engine to expose calibrated probabilities if it cannot honestly do so. Optional fields are better than fabricated confidence.

# 8. Initial Decision Classes

## ProviderFit

Only after deterministic compatibility/policy filtering.

Possible inputs:

- role type;
- compact task label/summary;
- project stack/context labels;
- compatible execution targets;
- provider health/quota category;
- independence requirement;
- recent relevant outcomes;
- user preference.

## RetryEscalation

Legal choices may include:

```text
retry_fresh_same_target
switch_target
spawn_debugger
replan_within_scope
ask_user
fail
```

## AdditionalReviewNeeded

Choices may include:

```text
none
general_review
security_review
database_review
ask_user
```

This only recommends a review class allowed by workflow/grant.

## SkillRelevance

Rank/filter only already trusted/compatible optional skills.

Mandatory role/workflow context cannot be silently dropped.

## ArtifactRelevance

Rank optional context. ApprovedIntent, candidate evidence, and mandatory acceptance criteria are never removed as "irrelevant."

# 9. Explicit Initial Exclusions

Do not use semantic decisions for:

- scheduler fairness/tie-break;
- permission/authority grants;
- Git push/deploy/database authorization;
- sandbox bypass;
- secret access;
- provider installed/authenticated fact;
- process/pane existence;
- test exit status;
- Git hash/candidate fingerprint equality;
- artifact checksum validity;
- SQLite transition conflicts;
- lease ownership;
- cancellation truth.

These remain deterministic/policy facts.

# 10. Why Scheduler Tie-Break Is Deferred

Scheduler fairness is control-plane behavior. Deterministic priority/aging/capacity rules are easier to audit, reproduce, and recover.

Only consider semantic scheduling later if real evidence shows deterministic scheduling is inadequate and the fairness contract remains deterministic around any advisory signal.

# 11. Jev Adapter Position

Jev is optional and external.

Before implementation, verify the current TypeSafe AI/Jev:

- API/SDK shape;
- authentication;
- supported typed decision/result model;
- data handling/privacy terms;
- availability/rate limits;
- model/version provenance fields.

Do not freeze adapter code from stale documentation assumptions.

# 12. Jev Sanitized Projection

Send the minimum data needed for a decision.

Prefer:

```text
role = verifier
project_stack = rust-cli
candidate_targets = [codex-medium, claude-default]
independence = fresh_session
health = ...
recent_outcomes = compact categorical facts
```

Avoid by default:

- repository source;
- raw terminal logs;
- full conversation history;
- secrets/tokens;
- absolute local paths;
- unrelated task/project data;
- user-private text not required for the decision.

# 13. Confidence / Fallback Policy

Decision policy may specify:

- minimum confidence if engine reports confidence;
- whether recommendation can influence automatically;
- fallback deterministic order;
- when to escalate to human;
- timeout budget.

On:

```text
engine unavailable
timeout
invalid response
unknown choice
low confidence
abstain
```

Relay falls back deterministically or creates a DecisionRequest. No semantic-service failure may corrupt workflow state.

# 14. Shadow Mode First

Initial Jev use is shadow/evaluation only:

```text
Relay deterministic/user path selects actual action
Jev receives same sanitized choice problem
Jev recommendation recorded but not executed
```

Measure:

- agreement with accepted choices;
- calibration where meaningful;
- latency;
- availability;
- invalid/abstain frequency;
- whether recommendations improve provider/retry decisions.

Only enable influence for one decision class after explicit acceptance.

# 15. Decision Audit Record

Persist:

- decision ID/kind;
- legal choice IDs;
- sanitized-state digest;
- engine/version;
- recommendation;
- probabilities/confidence if supplied;
- shadow vs active mode;
- policy threshold/fallback;
- final Relay action;
- whether human overrode/selected differently;
- latency/error class.

Do not persist secrets merely for reproducibility.

# 16. Policy Interaction

Semantic engine runs after hard filtering and cannot broaden candidates.

Example:

```text
Provider A denied by project policy
Provider B legal
Provider C legal

semantic engine sees [B, C]
never [A, B, C]
```

A recommendation never converts `DENY/HARD_DENY` into `ALLOW`.

# 17. Scope Interaction

A semantic engine cannot decide that expanding ApprovedIntent is okay.

It may recommend `ask_user`/`escalate` when none of the legal in-scope choices are suitable.

# 18. Testing

## Contract tests

- output choice must belong to input set;
- unknown choice rejected;
- abstain/escalate supported;
- missing confidence does not fabricate value;
- adapter errors normalized.

## Policy tests

- denied candidate never reaches engine;
- recommendation cannot broaden capability;
- semantic result cannot bypass approval;
- scope expansion always returns to DecisionRequest.

## Privacy tests

- token/secret/path fields absent from sanitized projection;
- cross-task/project data absent unless explicitly authorized;
- logs/source not included by default.

## Failure tests

- timeout;
- unavailable service;
- malformed response;
- low confidence;
- stale model/version metadata;
- Relay restart around pending semantic request.

# 19. Acceptance Criteria

1. Relay works fully with semantic decisions disabled.
2. RuleDecisionEngine covers deterministic fallback.
3. human fallback uses DecisionRequest, not an AI-engine abstraction.
4. engine sees only deterministic legal choices.
5. semantic choice never grants authority.
6. initial decision classes exclude scheduler fairness and privileged operations.
7. Jev adapter is isolated from Relay Core.
8. external data is minimized/sanitized.
9. shadow mode records value before enabling influence.
10. timeout/low-confidence/unavailability degrades safely.
