# Relay v2 Architecture Handoff
## Policy-Governed Workflow Orchestration with Provider-Neutral Roles, Durable Exchange, Herdr Backplane, and Optional Semantic Decisions

**Status:** proposed architecture baseline **as written on 2026-09-17; not implemented at that time**  
**Current status:** partially implemented — see [ROADMAP.md](ROADMAP.md) for the authoritative, evidence-based per-phase table. The "not implemented" line above is the historical status when the proposal was written and is deliberately not rewritten.  
**Architecture generation:** v2  
**Historical predecessor:** Relay deterministic handoff-engine / v0.1 milestone lineage  

# 1. Objective

Relay v2 evolves the product from a deterministic handoff/execution engine into a **local-first, policy-governed orchestration control plane for user-approved software work**.

The product must work for:

- one user, one project, one AI CLI;
- one user with several providers;
- several projects running concurrently;
- workflows that need independent verification, review, remediation, clarification, and bounded delegation;
- operation with Herdr as the primary interactive runtime backplane;
- operation without Herdr for a minimal local-process path;
- operation without any external semantic decision service.

Canonical identity:

> **The human owns intent. Relay owns controlled orchestration. Runtime backplanes host execution. Providers perform assigned roles.**

# 2. Version / History Policy

Relay v2 is an architecture generation, not a claim that every historical release was semantically called v1.

Historical records remain immutable evidence:

```text
old ADRs
old M0-M9 plans
accepted implementation snapshots
historical branch audits
```

A new ADR explicitly supersedes incompatible architectural assumptions. Do not edit old accepted records to retroactively describe v2.

# 3. Compatibility Commitments

Unless a later explicit ADR changes them, v2 preserves:

- Rust stable and repository quality gates;
- one provider-neutral Relay semantic engine;
- `HandoffTask v1` wire compatibility;
- local file/paste ingress;
- `relay serve-mcp` planner submission behavior;
- `submit_handoff -> durable Ready`, not automatic execution;
- explicit Run authorization;
- trusted local path resolution;
- repository safety checks;
- provider-owned auth/subscription/quota enforcement;
- no automatic Git push;
- fail-closed treatment of unknown/unsafe states;
- bounded global/per-project concurrency;
- durable SQLite canonical orchestration state.

Do not mutate `HandoffTask v1` into the new orchestration protocol. Adapt it internally.

# 4. Authority Model

```text
Human
  -> approves intent
  -> grants execution
  -> resolves scope-changing decisions
  -> approves privileged effects when policy requires

Relay
  -> owns durable task/workflow state
  -> owns role/delegation semantics
  -> owns policy evaluation
  -> owns provider resolution
  -> owns candidate/lease/worktree rules
  -> owns Exchange/artifacts
  -> owns approvals and audit trail
  -> owns recovery/reconciliation

RuntimeBackplane
  -> hosts process/session resources
  -> reports observations
  -> does not decide task semantics

ProviderAdapter
  -> translates provider-specific launch/readiness/result behavior
  -> does not own authority

Provider CLI
  -> performs one RoleAttempt
```

# 5. Target Layer Model

```text
┌─────────────────────────────────────────────────────────────┐
│ CLIENTS                                                     │
│ Web · TUI · CLI · planner MCP                               │
└───────────────────────────────┬─────────────────────────────┘
                                │ local authenticated protocol
┌───────────────────────────────▼─────────────────────────────┐
│ RELAY DAEMON                                                │
│                                                             │
│ Task / Intent / Execution Grants                            │
│ Workflow Engine · Role Registry · Delegation Manager        │
│ Policy / Approval / Decision Manager                        │
│ Provider Resolver · Runtime Coordinator                     │
│ Exchange · Artifact Store · Candidate / Lease Manager       │
│ Project / Checkout / Worktree Manager                       │
│ Scheduler · Budget Manager · Recovery / Reconciliation      │
│ Durable Effects · Ordered Event Log                         │
└───────────────────────────────┬─────────────────────────────┘
                                │
        ┌───────────────────────┴───────────────────────┐
        ▼                                               ▼
 LocalProcessBackplane                            HerdrBackplane
        │                                               │
 provider process                                 Herdr topology
        │                                               │
        └──────────────────┬────────────────────────────┘
                           ▼
                  ProviderAdapter
                           ▼
               Codex / Claude / AGY / ...
```

`relay-core` must not know Herdr workspace/tab/pane IDs.

# 6. Core Domain Separation

Keep these concepts distinct:

| Concept | Ownership / meaning |
|---|---|
| `Project` | logical project identity |
| `Checkout` | one trusted local clone/root |
| `Task` | user-visible work objective |
| `ApprovedIntent` | immutable approved scope/constraints/non-goals generation |
| `ExecutionGrant` | explicit authorization to run one workflow under bounded authority |
| `WorkflowDefinition` | reusable declarative workflow schema |
| `WorkflowRun` | one execution of a workflow for one task/intent generation |
| `RoleDefinition` | semantic responsibility contract |
| `RoleInstance` | one semantic activation/generation of a role |
| `RoleAttempt` | one provider/runtime attempt for a RoleInstance |
| `Provider` | available AI CLI or deterministic executor family |
| `RuntimeProfile` | provider configuration/model/capability profile |
| `ExecutionTarget` | immutable provider + runtime profile chosen for an attempt |
| `Skill` | reusable knowledge/procedure, never authority |
| `Policy` | allowed/restricted actions |
| `Approval` | bounded authorization for one known privileged effect |
| `Artifact` | immutable typed work product |
| `Message` | small scoped coordination object |
| `Candidate` | versioned repository state under evaluation |
| `Lease` | scheduled ownership token with fencing generation |
| `RuntimeBinding` | mapping from RoleAttempt to physical runtime handle |
| `SemanticDecision` | optional advisory bounded choice |

Invariant:

```text
Provider != Role != RoleAttempt != Skill != Policy != RuntimeProfile
```

# 7. Approved Intent and Execution Grant

## ApprovedIntent

An `ApprovedIntent` captures exactly what the human approved:

- objective;
- acceptance criteria;
- constraints;
- explicit exclusions/non-goals;
- affected project/checkout scope;
- source handoff/spec references;
- generation/version;
- approval timestamp/provenance.

It is immutable. A revised scope creates a new generation.

## ExecutionGrant

Run does not mean "allow Relay to do anything useful." It creates a bounded grant containing:

- task + approved-intent generation;
- selected workflow definition/version;
- effective policy snapshot;
- allowed role/delegation classes;
- execution budget;
- project/checkout scope;
- privileged effects still requiring separate approval;
- expiry/cancellation semantics where applicable.

Dynamic delegation must remain inside this grant.

## Scope expansion

When a plan/provider discovers materially broader work:

```text
RoleAttempt
-> ScopeExpansionRequest
-> WorkflowRun NEEDS_DECISION
-> human approves/rejects revised intent
-> new ApprovedIntent generation if accepted
```

No model can self-approve scope expansion.

# 8. Workflow Model

Workflows are declarative graphs with typed inputs/outputs and explicit transitions.

They define:

- semantic role nodes;
- preconditions;
- typed required inputs;
- typed required results;
- transitions based on validated facts;
- permitted delegation targets;
- retry/remediation bounds;
- completion/failure/cancellation rules.

Repository-provided workflow definitions must be statically validated before execution.

Validation includes:

- schema/version support;
- unique node IDs;
- reachable start/completion nodes;
- typed input/output compatibility;
- legal transition conditions;
- bounded/reviewed cycles;
- allowed delegation graph;
- referenced roles/skills/policy names;
- no arbitrary executable expression language.

The workflow engine is Relay's deterministic `Orchestrator`; it is not a normal AI `Coordinator` role.

# 9. Role Model

Built-in semantic roles may include:

- Planner;
- Implementer;
- Verifier;
- Reviewer;
- Remediator;
- Debugger;
- Consultant;
- IntegrationPlanner;
- OperationPlanner.

A role definition declares:

- purpose;
- required input artifact kinds;
- required result schema;
- capability requirements;
- write/read expectations;
- independence requirements;
- allowed delegation classes;
- default attempt policy;
- optional skill requirements.

A `RoleInstance` persists even if its first provider attempt fails.

Examples:

```text
Verifier RoleInstance V1
  attempt 1 -> Codex -> quota exhausted
  attempt 2 -> Claude -> completed
```

The workflow consumes the accepted role result, not provider exit status.

# 10. Role Attempt and Semantic Completion

`RoleAttempt` freezes:

- RoleInstanceId;
- ExecutionTarget;
- runtime profile snapshot;
- attempt number;
- provider adapter version;
- runtime binding;
- session capability;
- start/end timestamps;
- runtime outcome;
- result publication outcome.

Runtime success is not role success.

Canonical completion:

```text
process/runtime outcome
-> required result published
-> Relay validates schema + scope + candidate generation
-> durable accepted artifact
-> RoleInstance semantic state advances
```

Exit code `0`, provider "idle", or Herdr pane survival cannot independently complete a semantic role.

# 11. Project and Checkout Trust

Repository-owned portable configuration:

```text
<repo>/.relay/
├── project.toml
├── policies.toml
├── workflows/
├── skills/
└── contexts/
```

Repository content is a declaration, not trusted local authority.

Separate:

```text
DeclaredProjectIdentity
TrustedProjectRegistration
CheckoutId
```

Local registration stores trusted machine evidence:

- logical ProjectId accepted by user;
- CheckoutId;
- canonical absolute path;
- expected remote/fingerprint;
- allowed worktree root;
- trust state and timestamp.

A repository cannot grant itself trusted filesystem paths, credentials, provider access, or privileged policy.

# 12. Portable Config Trust

Repository `.relay/` may define:

- project labels/context;
- declarative workflow structure;
- project-local skills/context;
- additional restrictions;
- references to locally trusted command/action identifiers.

It must not directly grant:

- arbitrary shell execution authority;
- secret access;
- machine-local paths;
- provider credentials;
- sandbox bypass;
- automatic push/deploy;
- unrestricted external endpoints.

Executable verification/actions from repository configuration must reference a locally trusted action registry or require the policy/sandbox path defined for executable repository input. Never blindly execute arbitrary repo-supplied command strings as trusted verification.

# 13. Configuration Snapshot

Every `WorkflowRun` captures immutable hashes/snapshots of the effective inputs that define its behavior:

- workflow definition/version;
- role definitions;
- policy layers/effective policy;
- approved intent generation;
- skills/contexts actually mounted;
- runtime profile IDs/versions as attempts are created;
- trusted action registry version;
- execution budget.

Editing `.relay/` or user config during a run affects future runs unless an explicit controlled reconfiguration transition exists.

# 14. Policy Model

Effective policy is an intersection, never privilege union:

```text
built-in safety floor
∩ user policy
∩ trusted local project registration policy
∩ repository project restrictions
∩ workflow policy
∩ role policy
∩ execution grant
= EffectivePolicy
```

Policy decisions:

```text
ALLOW
REQUIRE_APPROVAL
DENY
HARD_DENY
INHERIT
```

Add `EnforcementClass`:

```text
RELAY_ENFORCED
SANDBOX_ENFORCED
PROVIDER_ENFORCED
ADVISORY_ONLY
UNAVAILABLE
```

Do not describe advisory policy as a hard security boundary when an unrestricted same-user process can bypass it.

# 15. Approval Model

An approval is first-class domain data, not a generic boolean.

It binds to:

- operation kind;
- exact immutable parameters/hash;
- project/task/workflow;
- candidate generation where relevant;
- requesting role/attempt;
- policy reason;
- approver;
- expiration;
- single-use/replay rules.

Changing material parameters invalidates/requires a new approval.

Prefer:

```text
AI role -> proposed IntegrationPlan/OperationPlan
Relay -> deterministic validation
Human -> exact approval if required
Relay privileged executor -> exact operation
```

# 16. Clarification, Decision, and Approval Are Different

Use separate objects:

- `ClarificationRequest` — missing factual input;
- `DecisionRequest` — human choice that changes/selects intent/design/scope;
- `ApprovalRequest` — authorization for a known side effect.

Do not collapse these into one "needs human" blob; the UI may aggregate them in one Attention Inbox while preserving type semantics.

# 17. Durable Daemon and Store Ownership

Target:

```text
Web/TUI/CLI/MCP
      ↓
Relay daemon
      ↓
single canonical writer/orchestrator
      ↓
SQLite + artifact filesystem
```

The daemon must have singleton/state-root ownership rules and local protocol versioning.

Direct database writers are compatibility debt to remove after clients are migrated.

# 18. Durable Command / Effect / Event Model

External side effects cannot be atomically committed with SQLite.

Use explicit durable effect state:

```text
Client Command
  -> validate/version check
  -> SQLite transaction:
       semantic state transition
       PendingEffect(effect_key)
       EventRecord(sequence)
  -> effect executor
  -> external runtime/git/action call
  -> SQLite EffectResult
  -> follow-up semantic transition/event
```

Requirements:

- stable idempotency/reconciliation key;
- bounded retry policy;
- effect ownership/lease;
- ambiguous-success state when external outcome cannot be known;
- reconciliation instead of optimistic success;
- no double-launch/double-push caused by client retry.

# 19. Durable Event Stream

Events are committed facts used for audit/client synchronization.

Each event has:

- monotonic sequence per daemon state root;
- event ID;
- aggregate IDs;
- type/version;
- timestamp;
- causal command/effect reference;
- sanitized payload.

Reconnect flow:

```text
client gets snapshot at sequence N
-> subscribes/requests events > N
```

An ephemeral broadcast channel may accelerate delivery but is not the canonical record.

# 20. Runtime Backplane

Use a deep provider-neutral seam.

Conceptual interface:

```rust
trait RuntimeBackplane {
    async fn provision(req: RuntimeProvisionRequest) -> RuntimeHandle;
    async fn observe(handle: RuntimeHandle) -> RuntimeObservation;
    async fn send(handle: RuntimeHandle, input: RuntimeInput) -> SendReceipt;
    async fn interrupt(handle: RuntimeHandle) -> InterruptReceipt;
    async fn dispose(handle: RuntimeHandle) -> DisposeReceipt;
    async fn snapshot() -> BackplaneSnapshot;
    async fn reconcile(binding: RuntimeBinding) -> ReconcileOutcome;
}
```

Implementations:

```text
FakeBackplane          deterministic tests
LocalProcessBackplane minimal direct process hosting
HerdrBackplane         default/reference interactive hosting
```

Workspace/tab/pane creation is an internal Herdr implementation detail, not part of the Relay-facing interface.

# 21. Provider Adapter

Conceptual responsibility:

```text
role assignment + execution target
      ↓
ProviderAdapter
      ↓
launch spec / readiness rules / result strategy
      ↓
RuntimeBackplane
```

Provider adapter owns provider-specific knowledge such as:

- executable/args/env shape;
- safe invocation mode;
- readiness/auth/quota/error normalization;
- provider-native instruction/skill injection where supported;
- structured result publishing path;
- bounded recovery prompts where appropriate.

It does not own task state, leases, workflow transitions, or policy authorization.

# 22. Execution Target

A provider choice must resolve to more than a provider family.

```text
ExecutionTarget {
  provider_id,
  runtime_profile_id,
  capability_snapshot_id,
  provider_adapter_version
}
```

The target is immutable for one RoleAttempt.

# 23. Worktree Manager

Relay owns repository/worktree lifecycle through a dedicated module.

Responsibilities:

- validate trusted checkout;
- create/select isolated worktree where policy allows;
- enforce allowed root;
- record branch/base/head evidence;
- provide trusted CWD to runtime;
- clean up only when no active/auditable object depends on it.

Herdr may be used internally by an adapter only if the ownership remains behind this module; `HerdrBackplane` itself does not become the semantic worktree owner.

# 24. Candidate Model

A Candidate is immutable evidence of repository state produced for evaluation.

Fingerprint includes policy-defined repository state:

- base HEAD;
- current HEAD;
- index tree/status;
- tracked worktree diff hash;
- relevant untracked-file manifest/hash;
- worktree/checkout identity;
- producer RoleInstance/RoleAttempt;
- generation.

If any covered state changes during verification, the verification result is invalidated.

# 25. Lease and Fencing

Lease kinds may include:

- `WORKTREE_WRITE`;
- `CANDIDATE_VERIFY`;
- `GIT_INTEGRATE`;
- `ENVIRONMENT_OPERATION`;
- provider capacity reservation.

Every acquired lease receives a monotonically increasing fencing token/generation. A stale holder cannot regain validity after expiration/recovery.

Relay guarantees that it will not intentionally schedule conflicting holders. External/manual writes that Relay cannot technically prevent are detected and invalidate affected candidates/results.

# 26. Exchange and Artifacts

Relay Exchange is the semantic communication fabric.

Channels:

- Messages — small coordination/consultation data;
- Artifacts — immutable typed work products.

Artifacts carry:

- ID/schema/version;
- project/task/workflow/role/attempt provenance;
- candidate generation if relevant;
- checksum/size/storage reference;
- trust class;
- creation timestamp;
- sensitivity/retention flags.

Trust class:

```text
ProviderClaim
RelayObserved
Derived
```

Downstream policy can require Relay-observed evidence rather than trusting provider claims.

# 27. Exchange Isolation

Default authorization:

```text
RoleAttempt
  -> own Task + WorkflowRun
  -> explicitly mounted project context
  -> declared input artifacts/messages
```

No global mailbox/list-projects capability for normal role sessions.

Cross-task and cross-project data require explicit Relay grants/mounts.

Exchange isolation is a Relay-mediated interface guarantee. Full filesystem isolation additionally requires OS/runtime sandbox enforcement.

# 28. Role Session Capability

A role attempt receives a short-lived opaque capability bound to:

- ProjectId;
- CheckoutId;
- TaskId;
- WorkflowRunId;
- RoleInstanceId;
- RoleAttemptId;
- allowed Exchange/delegation operations;
- expiry.

It is revoked when the attempt completes/cancels/revokes.

Do not embed the token in durable artifacts, normal prompts, raw logs, or repository files.

# 29. Delegation

Providers request delegation; Relay decides whether it is legal.

```text
DelegationRequest
-> schema/scope validation
-> execution-grant check
-> policy check
-> budget check
-> blocking dependency-cycle check
-> RoleDefinition compatibility
-> create RoleInstance
-> provider resolution
-> RoleAttempt
```

No provider-to-provider direct spawning as orchestration authority.

Consultation is a bounded request/response relationship and may not require a blocking child workflow node.

# 30. Execution Budget

Every ExecutionGrant carries a budget appropriate to the workflow:

- max role activations;
- max attempts per role;
- max delegation depth/count;
- max concurrent attempts;
- max wall-clock/runtime duration where enforceable;
- provider-specific capacity categories where observable;
- optional user-defined cost/quota hints.

Do not claim exact token/currency enforcement when providers do not expose reliable measurements.

# 31. Scheduler

Preserve deterministic fairness and concurrency as core control-plane behavior.

Inputs:

- user priority;
- project fairness;
- per-project limit;
- provider capacity;
- checkout/lease conflicts;
- workflow dependency readiness;
- execution budget;
- explicit provider pinning.

Semantic AI should not control fairness in v2 initial rollout.

# 32. Independence Requirements

Independence is explicit and multidimensional:

- `FreshSessionRequired`;
- `DifferentProviderRequired`;
- `DifferentProviderPreferred`;
- `ProducerContextExcluded`;
- `NoIndependenceRequirement`.

A verifier can be independent by session/context even when only one provider exists, while some policies may require actual provider diversity.

# 33. Cancellation / Suspension / Interrupt

Separate actions:

## SuspendWorkflow

- stop scheduling new work;
- let policy decide whether currently running attempts continue;
- does not claim OS process suspension.

## InterruptAttempt

- runtime-level interrupt for one active attempt;
- semantic role becomes interrupted/needs retry/reconciliation according to policy.

## CancelWorkflow

- durable terminal cancellation intent;
- propagate cancellation to pending/active attempts;
- revoke role capabilities;
- suppress not-yet-authorized pending effects;
- release leases safely;
- preserve completed artifacts/audit history;
- reconcile race with concurrent completion before reporting final state.

# 34. Recovery and Reconciliation

Recovery reconstructs from durable semantic state, then observes physical reality.

On daemon restart:

```text
load unfinished aggregates/effects
-> validate checkout/worktree registrations
-> query LocalProcess/Herdr snapshots
-> reconcile RuntimeBindings
-> verify lease generations
-> verify candidate fingerprints
-> resume/retry/escalate effects according to recorded state
```

Possible runtime reconciliation states:

```text
ATTACHED
EXITED
MISSING
ORPHANED
MISMATCHED
AMBIGUOUS
```

Do not infer authoritative workflow completion from terminal logs.

# 35. Herdr Position

Herdr is the default/reference interactive runtime backplane, not Relay's task owner and not a domain dependency.

Herdr owns physical concepts such as:

- server/session;
- workspace/tab/pane topology;
- PTY/terminal process;
- input/output transport;
- raw terminal attachment;
- runtime observations.

Relay owns all semantic concepts.

# 36. LocalProcessBackplane Position

Keep a minimal direct-process backplane because:

- Relay already has direct runtime lineage;
- it proves the backplane seam is real rather than Herdr-shaped;
- it allows simple one-provider operation without Herdr;
- it provides a lower-complexity test/migration path.

It does not need to duplicate Herdr's interactive topology features.

# 37. Store Evolution

Retain migrations; add versioned schema only as each phase needs it.

Likely v2 persisted concepts:

```text
projects / trusted checkouts
approved_intents
execution_grants
workflow_definitions or version refs
workflow_runs
role_instances
role_attempts
role_dependencies
runtime_bindings
pending_effects / effect_results
events
messages
artifacts / artifact_edges
candidates
leases
approval_requests / approvals
clarification_requests / responses
decision_requests / responses
provider_snapshots
configuration_snapshots
semantic_decisions
```

Large artifacts/logs remain filesystem-backed with atomic write + checksum; SQLite stores metadata and references.

Do not create every table in Phase 01. Migrations follow the vertical slice that consumes the concept.

# 38. Client Architecture

Primary semantic UI:

```text
local Web UI
```

Power-user semantic UI:

```text
Relay TUI
```

Raw runtime/debugging UI:

```text
Herdr TUI/session
```

All Relay clients read/write through daemon commands. Clients do not own state transitions.

# 39. UI Workflow Authoring Policy

Start with read-only workflow visualization and configuration inspection.

A visual Workflow Builder is deferred until schema/contracts stabilize.

When authoring arrives:

- user-local drafts are safe by default;
- writing `.relay/workflows/*` must be an explicit repository modification;
- schema validation occurs before save/activation;
- UI never silently dirties the project repository.

# 40. Semantic Decisions

Decision pipeline:

```text
deterministic facts
-> hard compatibility/policy filtering
-> legal candidate set
-> optional SemanticDecisionEngine
-> recommendation / abstain / escalate
-> confidence policy
-> Relay policy gate
-> action or human attention
```

Humans are handled by Decision/Attention workflow, not by a `UserDecisionEngine` implementation.

Initial semantic classes may include:

- provider fit among already-compatible targets;
- retry escalation strategy;
- optional additional-review recommendation;
- skill relevance among trusted compatible skills;
- optional artifact relevance for non-mandatory context.

Initial rollout excludes scheduler fairness and all authority decisions.

# 41. Jev Position

Jev is an optional adapter candidate, never a correctness dependency.

Requirements:

- Relay builds/tests/runs without Jev;
- send compact sanitized projections only;
- never send secrets or arbitrary repository/log/chat content by default;
- persist engine/version/confidence/outcome provenance;
- begin in shadow/evaluation mode;
- low confidence/unavailable/timeout -> deterministic fallback or human escalation;
- verify the current TypeSafe/Jev API and contractual/privacy behavior immediately before implementation because it is an external evolving integration.

# 42. Security Truthfulness

Relay v2 documentation and UI must distinguish:

- what Relay prevents;
- what an active sandbox prevents;
- what a provider promises/enforces;
- what is only advisory/detected after the fact.

Never present a policy toggle as filesystem/process isolation if the provider process can technically bypass it.

# 43. Cleanup / Retention Ownership

Define retention for:

- expired role-session capabilities;
- stale runtime bindings;
- Herdr panes created by Relay;
- LocalProcess handles;
- temporary worktrees;
- raw logs;
- artifacts;
- old candidates;
- expired leases;
- semantic decision records.

Never delete a resource while referenced by an active workflow, unresolved effect, approval/audit requirement, or configured retention rule.

# 44. Suggested Code Ownership

Prefer modules first; extract crates only when a stable seam has multiple consumers/implementations.

Potential eventual layout:

```text
crates/relay-core        domain + orchestration contracts
crates/relay-protocol    public/legacy wire contracts
crates/relay-store       durable state/migrations
crates/relay-runtime     runtime/provider contracts
crates/relay-exchange    only if artifact/exchange complexity earns extraction
crates/relay-herdr       Herdr adapter when stable
crates/relay-decision    only when semantic engine integration is implemented
crates/relay-api         daemon client protocol if separation earns a crate

apps/relay-daemon
apps/relay-tui
apps/relay-cli or existing binary mode
web/
```

Do not create empty future crates for symmetry.

# 45. Architecture Acceptance Criteria

Relay Architecture v2 is proven when all of the following are true:

1. ADR-0019 explicitly authorizes and versions the rebaseline.
2. Historical v0.1/M0-M9 records remain identifiable as history.
3. `HandoffTask v1` and `submit_handoff` remain compatible and Ready-only.
4. explicit Run creates a bounded `ExecutionGrant`.
5. `ApprovedIntent` prevents silent scope expansion.
6. Task/WorkflowRun/RoleInstance/RoleAttempt are separate durable concepts.
7. daemon is canonical semantic state/store owner.
8. external effects are durable/idempotent/reconcilable.
9. clients can reconnect by snapshot + event sequence.
10. project declarations cannot self-authorize local paths or privileges.
11. repository workflow/config executable trust is bounded.
12. FakeBackplane proves the workflow semantics without Herdr.
13. LocalProcessBackplane proves a minimal non-Herdr real-runtime path.
14. HerdrBackplane hides topology behind the generic runtime interface.
15. provider adapters own provider-specific semantics, not workflow state.
16. a RoleInstance can survive provider replacement through multiple attempts.
17. Candidates are immutable generation-bound fingerprints.
18. leases use fencing and stale holders cannot validate new work.
19. typed role results, not process exit, drive semantic completion.
20. Exchange/artifact access is task-scoped by default.
21. delegation is Relay-mediated, bounded, and cycle-checked.
22. cancellation/suspension/interrupt semantics are distinct and recoverable.
23. policy UI/docs accurately state enforcement strength.
24. privileged effects bind to exact approvals.
25. at least one end-to-end workflow runs with one provider.
26. at least two providers can fill the same semantic role where installed.
27. two projects can execute concurrently without Relay-mediated context leakage.
28. Web and TUI reflect the same daemon truth.
29. Herdr may be unavailable without corrupting Relay state.
30. Jev may be disabled/unavailable without breaking workflow correctness.
