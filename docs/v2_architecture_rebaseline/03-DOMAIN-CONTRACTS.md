# Relay v2 Orchestration Domain Contracts

**Status:** planning contracts to stabilize before implementation  
**Rule:** these are semantic contracts; exact Rust shapes may differ if invariants remain intact.

# 1. Identity Types

Use opaque strongly typed IDs rather than interchangeable strings:

```text
ProjectId
CheckoutId
TaskId
ApprovedIntentId / generation
ExecutionGrantId
WorkflowDefinitionId / version
WorkflowRunId
RoleDefinitionId
RoleInstanceId
RoleAttemptId
ProviderId
RuntimeProfileId
RuntimeHandleId
ArtifactId
CandidateId
LeaseId
ApprovalRequestId
ClarificationRequestId
DecisionRequestId
SemanticDecisionId
EffectId
EventId
```

# 2. Project

```rust
struct Project {
    id: ProjectId,
    display_name: String,
    status: ProjectStatus,
}
```

Logical identity is not a filesystem path.

# 3. Declared Project Identity

Repository claim:

```rust
struct DeclaredProjectIdentity {
    schema_version: u32,
    project_id: ProjectId,
    name: String,
}
```

This object is untrusted until matched to a local registration.

# 4. Trusted Project Registration / Checkout

```rust
struct Checkout {
    id: CheckoutId,
    project_id: ProjectId,
    canonical_root: TrustedAbsolutePath,
    expected_remote: Option<RemoteFingerprint>,
    allowed_worktree_root: TrustedAbsolutePath,
    trust_state: CheckoutTrustState,
    registered_at: Timestamp,
}
```

Repository input may not populate trusted absolute paths directly.

# 5. Task

```rust
struct Task {
    id: TaskId,
    project_id: ProjectId,
    checkout_id: CheckoutId,
    title: String,
    state: TaskState,
    current_intent_generation: u32,
    created_at: Timestamp,
}
```

Suggested states:

```text
READY
RUNNING
SUSPENDED
NEEDS_CLARIFICATION
NEEDS_DECISION
NEEDS_APPROVAL
COMPLETED
FAILED
CANCELLED
```

Task state is aggregate/user-facing state, not a direct mirror of one process.

# 6. ApprovedIntent

```rust
struct ApprovedIntent {
    id: ApprovedIntentId,
    task_id: TaskId,
    generation: u32,
    objective: String,
    acceptance_criteria: Vec<String>,
    constraints: Vec<String>,
    non_goals: Vec<String>,
    scope: IntentScope,
    source_refs: Vec<ArtifactRef>,
    approved_by: ApprovalPrincipal,
    approved_at: Timestamp,
    content_hash: Digest,
}
```

Immutable. Scope change creates a new generation.

# 7. ExecutionGrant

```rust
struct ExecutionGrant {
    id: ExecutionGrantId,
    task_id: TaskId,
    approved_intent_id: ApprovedIntentId,
    approved_intent_generation: u32,
    workflow_definition: WorkflowDefinitionRef,
    policy_snapshot_id: ConfigSnapshotId,
    execution_budget: ExecutionBudget,
    allowed_delegation_classes: Vec<RoleDefinitionId>,
    project_scope: ProjectExecutionScope,
    granted_by: ApprovalPrincipal,
    granted_at: Timestamp,
    revoked_at: Option<Timestamp>,
}
```

A Run command creates/activates this contract. It never implies unrestricted privileged effects.

# 8. WorkflowDefinition

```rust
struct WorkflowDefinition {
    id: WorkflowDefinitionId,
    version: WorkflowVersion,
    nodes: Vec<WorkflowNodeDefinition>,
    transitions: Vec<WorkflowTransitionDefinition>,
    policy_ref: Option<PolicyRef>,
    default_budget: ExecutionBudget,
    content_hash: Digest,
}
```

Repository definitions are validated declarations, not trusted executable code.

# 9. WorkflowRun

```rust
struct WorkflowRun {
    id: WorkflowRunId,
    task_id: TaskId,
    execution_grant_id: ExecutionGrantId,
    workflow_definition: WorkflowDefinitionRef,
    state: WorkflowRunState,
    config_snapshot_id: ConfigSnapshotId,
    started_at: Timestamp,
    completed_at: Option<Timestamp>,
}
```

States:

```text
PENDING
RUNNING
SUSPENDED
WAITING_FOR_ATTENTION
COMPLETED
FAILED
CANCELLED
```

# 10. Workflow Transition

Transitions use typed/deterministic facts, not free-form provider prose.

Examples:

```text
VerificationResult.outcome == PASS
ReviewResult.blocking_findings == 0
DecisionResponse.choice == APPROVE_SCOPE
CandidateChanged == false
BudgetRemaining == true
```

If a condition requires arbitrary code, it belongs in a trusted locally registered action/evaluator, not an untrusted repository expression.

# 11. RoleDefinition

```rust
struct RoleDefinition {
    id: RoleDefinitionId,
    purpose: String,
    required_inputs: Vec<ArtifactKind>,
    required_result: ResultSchemaRef,
    capability_requirements: CapabilityRequirements,
    independence: IndependenceRequirement,
    allowed_delegations: Vec<RoleDefinitionId>,
    write_mode: RoleWriteMode,
}
```

A role defines responsibility, not provider identity.

# 12. RoleInstance

```rust
struct RoleInstance {
    id: RoleInstanceId,
    workflow_run_id: WorkflowRunId,
    role_definition_id: RoleDefinitionId,
    generation: u32,
    state: RoleInstanceState,
    parent_role_instance_id: Option<RoleInstanceId>,
    accepted_result_artifact: Option<ArtifactId>,
}
```

Suggested states:

```text
CREATED
WAITING_INPUTS
READY
ACTIVE
WAITING_CHILDREN
RESULT_PENDING
COMPLETED
BLOCKED
FAILED
CANCELLED
```

# 13. RoleAttempt

```rust
struct RoleAttempt {
    id: RoleAttemptId,
    role_instance_id: RoleInstanceId,
    attempt_no: u32,
    execution_target: ExecutionTarget,
    runtime_binding_id: Option<RuntimeBindingId>,
    state: RoleAttemptState,
    runtime_outcome: Option<RuntimeOutcome>,
    result_artifact_id: Option<ArtifactId>,
    started_at: Option<Timestamp>,
    ended_at: Option<Timestamp>,
}
```

States:

```text
QUEUED
PROVISIONING
STARTING
RUNNING
RESULT_PENDING
COMPLETED
INTERRUPTED
FAILED
CANCELLED
LOST
```

A completed runtime attempt is not automatically a completed RoleInstance.

# 14. ExecutionTarget

```rust
struct ExecutionTarget {
    provider_id: ProviderId,
    runtime_profile_id: RuntimeProfileId,
    capability_snapshot_id: ProviderSnapshotId,
    provider_adapter_version: String,
}
```

Immutable for one attempt.

# 15. RuntimeProfile

Trusted local configuration selecting provider-specific mode/model/capabilities.

Must not contain secrets in repository-owned config unless referenced indirectly through trusted local secret configuration.

# 16. RuntimeProvisionRequest

```rust
struct RuntimeProvisionRequest {
    effect_key: EffectKey,
    role_attempt_id: RoleAttemptId,
    trusted_cwd: TrustedAbsolutePath,
    launch: LaunchSpecification,
    environment: SanitizedEnvironment,
    metadata: RuntimeMetadata,
}
```

The backplane does not receive arbitrary untrusted shell command text.

# 17. RuntimeHandle / Binding

```rust
struct RuntimeBinding {
    id: RuntimeBindingId,
    role_attempt_id: RoleAttemptId,
    backplane_id: BackplaneId,
    runtime_handle: RuntimeHandle,
    created_at: Timestamp,
    last_reconciled_at: Timestamp,
    state: RuntimeBindingState,
}
```

Herdr-specific physical IDs stay inside the adapter-specific handle payload/storage, not Relay Core domain identity.

# 18. Runtime Outcome vs Semantic Result

`RuntimeOutcome` examples:

```text
EXITED_SUCCESS
EXITED_FAILURE
INTERRUPTED
LOST
TIMED_OUT
AUTH_REQUIRED
QUOTA_EXHAUSTED
```

A RoleInstance completes only when its required result artifact is accepted.

# 19. ProviderAdapter Contract

Conceptual:

```rust
trait ProviderAdapter {
    fn provider_id(&self) -> ProviderId;
    fn probe(&self) -> ProviderSnapshot;
    fn build_launch(&self, assignment: RoleAssignment) -> LaunchSpecification;
    fn normalize_observation(&self, runtime: RuntimeObservation) -> ProviderObservation;
    fn validate_result(&self, published: PublishedResult) -> ProviderResultValidation;
}
```

Do not put workflow transitions or policy grants in this interface.

# 20. ExecutionBudget

```rust
struct ExecutionBudget {
    max_role_activations: u32,
    max_attempts_per_role: u32,
    max_delegations: u32,
    max_delegation_depth: u32,
    max_concurrent_attempts: u32,
    max_wall_clock: Option<Duration>,
}
```

Optional quota/cost hints may be tracked separately and must not be described as exact enforcement if provider telemetry is unreliable.

# 21. EffectivePolicy

```rust
struct PolicyDecision {
    capability: Capability,
    disposition: PolicyDisposition,
    enforcement: EnforcementClass,
    reason: PolicyReason,
}
```

Dispositions:

```text
ALLOW
REQUIRE_APPROVAL
DENY
HARD_DENY
```

Enforcement:

```text
RELAY_ENFORCED
SANDBOX_ENFORCED
PROVIDER_ENFORCED
ADVISORY_ONLY
UNAVAILABLE
```

# 22. ApprovalRequest / Grant

```rust
struct ApprovalRequest {
    id: ApprovalRequestId,
    task_id: TaskId,
    workflow_run_id: WorkflowRunId,
    operation: PrivilegedOperation,
    parameter_digest: Digest,
    candidate_id: Option<CandidateId>,
    requested_by: Option<RoleAttemptId>,
    expires_at: Option<Timestamp>,
}
```

```rust
struct ApprovalGrant {
    request_id: ApprovalRequestId,
    parameter_digest: Digest,
    granted_by: ApprovalPrincipal,
    granted_at: Timestamp,
    expires_at: Option<Timestamp>,
    replay_policy: ApprovalReplayPolicy,
}
```

Changing material parameters invalidates the grant.

# 23. ClarificationRequest

Used when facts are missing but intent need not change.

```rust
struct ClarificationRequest {
    task_id: TaskId,
    question: String,
    required_fields: Vec<ClarificationField>,
    resume_point: WorkflowResumePoint,
}
```

# 24. DecisionRequest

Used when a human choice changes/selects design, intent, or scope.

```rust
struct DecisionRequest {
    task_id: TaskId,
    kind: DecisionKind,
    options: Vec<DecisionOption>,
    context_artifacts: Vec<ArtifactId>,
    scope_impact: DecisionScopeImpact,
}
```

# 25. ScopeExpansionRequest

Specialized decision that describes:

- requested expanded objective/scope;
- why current intent is insufficient;
- affected constraints/non-goals;
- expected additional work;
- whether current work can safely remain paused.

Accepted expansion creates a new ApprovedIntent generation and, if required, a new ExecutionGrant.

# 26. Candidate

```rust
struct Candidate {
    id: CandidateId,
    project_id: ProjectId,
    checkout_id: CheckoutId,
    workflow_run_id: WorkflowRunId,
    generation: u32,
    producer_role_instance_id: RoleInstanceId,
    producer_attempt_id: RoleAttemptId,
    fingerprint: CandidateFingerprint,
    created_at: Timestamp,
}
```

Fingerprint:

```rust
struct CandidateFingerprint {
    base_head: GitOid,
    current_head: GitOid,
    index_digest: Digest,
    tracked_diff_digest: Digest,
    relevant_untracked_digest: Digest,
}
```

Exact implementation may optimize this but must preserve equivalent covered-state guarantees.

# 27. VerificationResult

```rust
struct VerificationResult {
    candidate_id: CandidateId,
    candidate_generation: u32,
    verifier_role_instance_id: RoleInstanceId,
    verifier_attempt_id: RoleAttemptId,
    outcome: VerificationOutcome,
    checks: Vec<VerificationCheck>,
    evidence_artifacts: Vec<ArtifactId>,
    observed_fingerprint: CandidateFingerprint,
}
```

If observed fingerprint does not match the candidate being verified, outcome cannot be accepted as PASS.

# 28. ReviewResult

```rust
struct ReviewResult {
    candidate_id: CandidateId,
    findings: Vec<ReviewFinding>,
    blocking_count: u32,
    reviewer_role_instance_id: RoleInstanceId,
    reviewer_attempt_id: RoleAttemptId,
}
```

Review conclusions remain provider claims unless Relay can independently observe specific evidence.

# 29. Lease

```rust
struct Lease {
    id: LeaseId,
    resource: LeaseResource,
    kind: LeaseKind,
    holder: LeaseHolder,
    fencing_token: u64,
    acquired_at: Timestamp,
    expires_at: Timestamp,
    state: LeaseState,
}
```

A later fencing token supersedes all earlier holders.

# 30. Artifact

```rust
struct ArtifactMetadata {
    id: ArtifactId,
    kind: ArtifactKind,
    schema_version: u32,
    project_id: ProjectId,
    task_id: TaskId,
    workflow_run_id: WorkflowRunId,
    producer_role_instance_id: Option<RoleInstanceId>,
    producer_attempt_id: Option<RoleAttemptId>,
    candidate_id: Option<CandidateId>,
    trust_class: ArtifactTrustClass,
    checksum: Digest,
    size_bytes: u64,
    storage_ref: ArtifactStorageRef,
    created_at: Timestamp,
    sensitivity: ArtifactSensitivity,
    retention: RetentionClass,
}
```

Trust:

```text
ProviderClaim
RelayObserved
Derived
```

# 31. Message

Small scoped coordination object. Large/important output should be an Artifact.

Messages must identify project/task/workflow and authorized sender/recipients.

# 32. RoleSessionCapability

```rust
struct RoleSessionCapabilityClaims {
    project_id: ProjectId,
    checkout_id: CheckoutId,
    task_id: TaskId,
    workflow_run_id: WorkflowRunId,
    role_instance_id: RoleInstanceId,
    role_attempt_id: RoleAttemptId,
    allowed_ops: Vec<RoleSessionOperation>,
    expires_at: Timestamp,
}
```

Caller-supplied IDs cannot widen these claims.

# 33. DelegationRequest

```rust
struct DelegationRequest {
    parent_role_instance_id: RoleInstanceId,
    parent_attempt_id: RoleAttemptId,
    requested_role: RoleDefinitionId,
    mode: DelegationMode,
    purpose: String,
    input_artifacts: Vec<ArtifactId>,
    requested_independence: IndependenceRequirement,
}
```

Relay validates against ApprovedIntent, ExecutionGrant, policy, budget, and dependency graph.

# 34. DelegationMode

```text
BLOCKING_CHILD
NONBLOCKING_CHILD
CONSULTATION
```

Blocking relationships must remain acyclic.

# 35. IndependenceRequirement

```text
NoIndependenceRequirement
FreshSessionRequired
DifferentProviderPreferred
DifferentProviderRequired
ProducerContextExcluded
```

Requirements may compose where meaningful.

# 36. ConfigSnapshot

Captures content hashes/versions of the effective run configuration so recovery/audit does not depend on whatever config exists later.

Minimum:

- workflow definition;
- role definitions;
- policy layers/effective result;
- approved intent;
- skills/contexts mounted;
- trusted action registry;
- budget.

# 37. PendingEffect / EffectResult

```rust
struct PendingEffect {
    id: EffectId,
    effect_key: EffectKey,
    kind: EffectKind,
    aggregate_ref: AggregateRef,
    parameters_digest: Digest,
    state: EffectState,
    attempt_count: u32,
}
```

Effects must support `PENDING`, `EXECUTING`, `SUCCEEDED`, `FAILED`, and `AMBIGUOUS` or equivalent reconciliation state.

# 38. EventRecord

```rust
struct EventRecord {
    id: EventId,
    sequence: u64,
    type_name: String,
    schema_version: u32,
    aggregate_ref: AggregateRef,
    command_id: Option<CommandId>,
    effect_id: Option<EffectId>,
    timestamp: Timestamp,
    payload: SanitizedEventPayload,
}
```

# 39. Cancellation Contract

Cancellation is a durable intent. Completion racing cancellation must be resolved transactionally/reconciled so Relay never reports mutually inconsistent terminal outcomes.

Cancellation:

- revokes new scheduling;
- requests interruption of active attempts according to policy;
- revokes role-session capabilities;
- releases or marks leases for safe recovery;
- keeps immutable artifacts/history;
- does not report success until durable state agrees.

# 40. Suspension Contract

`SuspendWorkflow` stops new scheduling. It is not a claim that provider processes are OS-suspended.

UI must expose attempt interruption separately.

# 41. SemanticDecisionRequest

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

Choices are precomputed legal options. The engine cannot invent provider IDs, commands, paths, capabilities, or authority.

# 42. SemanticDecisionOutcome

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

Human decisions are handled through `DecisionRequest`, not `UserDecisionEngine`.

# 43. Legacy Handoff Adapter

```text
HandoffTask v1
-> canonical validation/project resolution
-> Task READY
-> human Run
-> ApprovedIntent generated from already-approved handoff
-> ExecutionGrant
-> compatibility workflow
```

Do not silently add planning that can broaden the handoff.

# 44. Compatibility Workflow

Minimum software-change compatibility workflow:

```text
IMPLEMENT
  -> Candidate N
VERIFY
  -> PASS -> COMPLETE
  -> FAIL -> REMEDIATE
REMEDIATE
  -> Candidate N+1
  -> VERIFY fresh attempt
```

Review may be optional/policy-controlled. Privileged Git integration remains separate.

# 45. Contract Acceptance Checklist

Before real provider orchestration, tests must prove:

- immutable intent generation;
- bounded ExecutionGrant;
- workflow static validation;
- RoleInstance vs RoleAttempt retry semantics;
- typed result required for semantic completion;
- project declaration cannot self-trust local path;
- config snapshot immutability;
- candidate fingerprint invalidation;
- lease fencing;
- artifact immutability/checksum/trust class;
- role capability scope;
- delegation cycle/budget enforcement;
- durable effect idempotency;
- event sequence reconnect;
- cancellation/recovery truthfulness.
