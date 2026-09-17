# Relay v2 Architecture Rebaseline — Historical Baseline and Migration Audit

## Overall Verdict

**PROCEED WITH ARCHITECTURE V2 REBASELINE**

The previously audited `feat/m9-relay-direct-mcp` implementation is a coherent historical fixed point for Relay's original deterministic handoff-engine architecture. It should be preserved as history, not stretched into the new orchestration model by incremental terminology changes.

Relay v2 is intentionally a product and architecture rebaseline. The correct migration strategy is:

```text
preserve accepted implementation history
        +
preserve compatible external behavior
        +
introduce new v2 domain/orchestration contracts
        +
adapt legacy handoffs into v2
        !=
pretend old milestones already implemented v2
```

This document is a migration-impact audit and planning fixed point. It does not claim a fresh exact-current-HEAD repository audit on 2026-09-17. Before coding begins, the implementation branch must be pinned to the current accepted repository HEAD and the normal quality gates rerun.

## Historical Fixed Point

Prior handoff/audit evidence used:

```text
Repository: Jacinth091/Relay
Historical branch: feat/m9-relay-direct-mcp
Historical audited HEAD: 86ce0205098338eab7979a2ec117420ba9675f75
Historical staging merge-base: 3f906576c4ac0ef8cbfd9871cd406543957e5618
```

Historical capabilities to preserve through compatibility or migration:

- Rust workspace/core separation;
- trusted local project resolution;
- Git/repository safety checks;
- `HandoffTask v1` canonical JSON ingress;
- local file/paste ingress;
- `relay serve-mcp` planner submission surface;
- durable SQLite task state;
- Ready state after submission;
- explicit Run before execution;
- bounded scheduler/concurrency foundations;
- provider-neutral runtime edge;
- no automatic push;
- fail-closed handling of unsafe/unknown conditions.

## Source-of-Truth Order for v2 Work

Once ADR-0019 is accepted, implementers and reviewers should use this order:

1. accepted v2 ADRs and this rebaseline pack;
2. repository-local contribution/governance rules;
3. accepted implementation snapshots for already-shipped historical behavior;
4. current source and tests at the pinned implementation HEAD;
5. historical ADRs/roadmaps for lineage only;
6. inference.

If an old architecture document conflicts with an accepted v2 contract, do not force v2 code backward to match the old document. Record the document as historical/superseded.

## Architecture Generation Boundary

### Relay v1 lineage — historical

The original architecture can be summarized as:

```text
Planner
  -> HandoffTask v1
  -> Relay Core
  -> Scheduler / Runtime
  -> coding agent
  -> repository
```

Its defining product identity was a deterministic local handoff engine/control plane.

### Relay v2 — forward target

```text
Approved Intent
  -> explicit Execution Grant
  -> WorkflowRun
  -> RoleInstance(s)
  -> RoleAttempt(s)
  -> Provider Resolution
  -> Runtime Backplane
  -> Provider CLI
  -> typed Result / Candidate / Artifact
  -> policy-checked workflow transition
```

Relay v2 owns the task and the semantic lifecycle. Runtime infrastructure and providers perform bounded work; they do not become orchestration authorities.

# Migration Impact Audit

## A. Governance and Product Identity

### Historical assumption

Relay explicitly excluded broader intent/workflow orchestration.

### v2 change

Relay becomes a policy-governed workflow/delegation orchestrator while retaining deterministic authority over lifecycle, persistence, permissions, hard constraints, and safety.

### Required action

- add ADR-0019;
- mark conflicting portions of ADR-0017/0018 superseded;
- mark the old unimplemented provider-distribution M10 pack historical/on hold;
- do not edit old accepted implementation snapshots to describe v2.

**Status: REQUIRED BEFORE IMPLEMENTATION.**

---

## B. Submission Compatibility

### Preserve

`submit_handoff` continues to mean:

```text
validate
-> resolve trusted project
-> durably create task
-> Ready
```

It must not auto-run or silently choose an execution workflow with privileged effects.

### v2 adaptation

A legacy `HandoffTask v1` is converted internally into an `ApprovedIntent` and a compatibility workflow only after explicit Run creates an `ExecutionGrant`.

**Status: COMPATIBILITY REQUIREMENT.**

---

## C. Human Intent and Run Authorization

### Gap in the first Herdr proposal

The initial orchestration handoff allowed dynamic role creation without fully defining what one Run action authorizes.

### v2 requirement

Introduce:

- `ApprovedIntent` — immutable approved objective/constraints/non-goals/version;
- `ExecutionGrant` — exact authorization for a workflow run, allowed role/delegation classes, policy snapshot, and budget;
- `ScopeExpansionRequest` — explicit path back to the human when work would materially broaden intent.

A provider or semantic engine cannot silently widen the approved intent.

**Status: BLOCKING DOMAIN CONTRACT.**

---

## D. Task / Workflow / Role / Attempt Separation

### Gap

The first proposal used `RoleInstance` for both a semantic responsibility and a live provider execution.

### v2 requirement

Separate:

```text
Task
WorkflowRun
RoleInstance       semantic responsibility/generation
RoleAttempt        one concrete provider/runtime execution attempt
```

Provider replacement, quota fallback, crash retry, or fresh-session retry creates a new `RoleAttempt`, not a different semantic role unless the workflow actually creates a new role.

**Status: BLOCKING DOMAIN CONTRACT.**

---

## E. Durable Command / Effect / Event Semantics

### Gap

A normal in-memory event bus cannot safely represent external effects such as process creation or Herdr operations.

### v2 requirement

Use durable effect intent and reconciliation:

```text
command
-> transactional state transition + PendingEffect
-> external effect
-> durable EffectResult
-> ordered EventRecord
```

Every retryable external effect gets a stable idempotency/reconciliation key. Client events are sequenced durable facts, not the only record of state change.

**Status: BLOCKING INFRASTRUCTURE CONTRACT.**

---

## F. Runtime Backplane Seam

### Gap

The first backplane interface exposed Herdr concepts such as workspace/tab/pane creation directly.

### v2 requirement

The external seam is semantic process/session provisioning:

```text
provision
observe
send
interrupt
dispose
snapshot/reconcile
```

Herdr topology is internal to `HerdrBackplane` through a topology planner.

First-party implementations should include:

- `LocalProcessBackplane` — minimal non-Herdr path and migration bridge;
- `HerdrBackplane` — default/reference interactive orchestration path;
- `FakeBackplane` — deterministic tests.

**Status: BLOCKING INTERFACE CORRECTION.**

---

## G. Provider Adapter Ownership

### v2 rule

`ProviderAdapter` understands provider-specific semantics:

- launch specification;
- readiness/auth/quota normalization;
- provider-native skill/config hooks;
- role assignment delivery;
- result extraction/validation assistance.

`RuntimeBackplane` only hosts and observes the process/session.

Herdr never decides what a Verifier means. A provider adapter never owns Relay task state.

**Status: REQUIRED SEAM DISCIPLINE.**

---

## H. Project Identity and Repository Trust

### Gap

A repository-controlled `.relay/project.toml` cannot by itself be trusted as local project identity.

### v2 requirement

Distinguish:

```text
DeclaredProjectIdentity   repository claim
TrustedProjectRegistration local approved binding
CheckoutId                one trusted local clone/root
```

Registration binds the logical project claim to locally trusted evidence such as expected remote and approved root.

Portable repository configuration may restrict behavior but cannot grant trusted filesystem paths, secrets, or privileged authority.

**Status: SECURITY REQUIREMENT.**

---

## I. Worktree / Candidate / Lease Ownership

### Gap

The first proposal let Herdr materialize worktrees and introduced Candidate later than consumers that already needed it.

### v2 requirement

Relay owns a repository/worktree module. The runtime receives an already trusted working directory.

Candidate verification is generation-bound. Leases include fencing generations/tokens. Candidate fingerprints explicitly cover the repository state Relay promises to verify, including:

- HEAD;
- index;
- tracked worktree changes;
- relevant untracked files according to policy.

Unexpected/manual/stale writes invalidate a candidate even if Relay did not schedule them.

**Status: BLOCKING CORRECTNESS REQUIREMENT.**

---

## J. Policy Enforcement Strength

### Gap

The first proposal risked describing Relay policy as stronger than the operating-system/process sandbox actually makes it.

### v2 requirement

Every sensitive capability declares enforcement class:

```text
RELAY_ENFORCED
SANDBOX_ENFORCED
PROVIDER_ENFORCED
ADVISORY_ONLY
UNAVAILABLE
```

`HARD_DENY` may only be claimed as hard for operations Relay or an active sandbox can genuinely prevent. A same-user unrestricted shell process must not be described as fully confined by Relay policy text alone.

**Status: SECURITY REQUIREMENT.**

---

## K. Approvals and Privileged Effects

### Gap

Approvals existed as persistence concepts without a precise authorization contract.

### v2 requirement

Approval binds to the exact action and immutable parameters, scope, project/task, candidate where relevant, expiry, and replay behavior.

Preferred privileged architecture:

```text
AI role proposes IntegrationPlan / OperationPlan
-> Relay policy validates
-> human approval where required
-> deterministic privileged executor performs exact approved operation
```

Do not give an AI role an open-ended "approved integrator" capability.

**Status: HIGH-RISK CONTRACT.**

---

## L. Exchange and Artifact Trust

### v2 requirement

Artifacts are immutable, scoped, checksummed, and provenance-aware.

Trust classes distinguish:

- `ProviderClaim`;
- `RelayObserved`;
- `Derived`.

Exchange access control is a mediated Relay guarantee. It is not equivalent to OS filesystem sandboxing unless a sandbox is active.

Default access is same task/workflow only; cross-task and cross-project access require explicit mounts/grants.

**Status: REQUIRED COMMUNICATION CONTRACT.**

---

## M. Delegation

### v2 requirement

Delegation is always:

```text
role request
-> Relay validation
-> policy/budget/dependency checks
-> RoleInstance creation
-> provider resolution
-> RoleAttempt
```

Providers never directly spawn one another as task authorities.

Blocking delegation graphs must be cycle-checked. Execution budgets bound attempts, activations, delegation depth/count, concurrency, and wall-clock/runtime limits where enforceable.

**Status: REQUIRED ORCHESTRATION CONTRACT.**

---

## N. Cancellation and Suspension

### v2 requirement

Preserve M6-style truthful lifecycle semantics.

Cancellation propagates from Task/WorkflowRun to pending/active RoleAttempts, revokes role-session capabilities, suppresses unauthorized pending effects, releases leases, and preserves completed artifacts/history.

Define three separate operations:

- `SuspendWorkflow` — stop new scheduling; do not pretend OS process suspension;
- `InterruptAttempt` — ask the runtime to interrupt one attempt;
- `CancelWorkflow` — terminal cancellation intent with bounded cleanup/reconciliation.

**Status: REQUIRED LIFECYCLE CONTRACT.**

---

## O. Recovery

### v2 requirement

Recovery is designed into each phase, not added only at the end.

Relay recovers from durable semantic state and reconciles physical runtime. It must not reconstruct authoritative workflow state by parsing long terminal histories.

**Status: CROSS-CUTTING REQUIREMENT.**

---

## P. UI / Client Model

### v2 requirement

Web/TUI/CLI/MCP are daemon clients. They consume a current snapshot plus monotonic durable events.

The early Web UI is operational/read-only for workflow graphs. Visual workflow authoring is deferred until the workflow schema is stable; repository writes from the UI must never be an implicit side effect of viewing/editing a draft.

**Status: REQUIRED CLIENT CONTRACT.**

---

## Q. Semantic Decisions / Jev

### v2 requirement

Semantic intelligence is optional and advisory.

- deterministic filters produce legal candidates;
- decision engine may select/recommend/abstain/escalate;
- policy authorizes;
- human decisions are not modeled as an AI decision engine;
- scheduling fairness remains deterministic in the initial rollout;
- Jev starts in shadow/evaluation mode before it can influence supported decision classes.

**Status: DEFERRED UNTIL DETERMINISTIC V2 PATH WORKS.**

# Rebaseline Implementation Preflight

Before Phase 01 coding:

1. pin the exact current repository HEAD and parent branch;
2. run and record:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets --all-features
cargo test --workspace
```

3. identify the current accepted M9 implementation snapshot and migration state;
4. add/accept ADR-0019;
5. mark old future plans as historical/superseded without rewriting accepted implementation records;
6. create the v2 implementation tracking area/roadmap;
7. verify the legacy MCP/file/paste submission tests remain green before structural migration.

# Routing Decision

**FORWARD TO RELAY V2 IMPLEMENTATION PLANNING** after ADR-0019 and exact-current-HEAD preflight evidence are accepted.

No requirement in this rebaseline authorizes immediate Herdr/Jev implementation before the v2 core contracts and first deterministic vertical slice exist.
