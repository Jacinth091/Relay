# Relay v2 — Sequential Implementation Plan

**Planning class:** large / high-risk architecture rebaseline  
**Execution rule:** implement one dependency-complete vertical slice at a time.  
**Historical rule:** old milestones remain history; do not renumber them into v2.

# Sequence Overview

```text
00 Governance + history freeze
01 Daemon/store ownership + durable effects/events
02 Project/checkout/trust foundation
03 Intent + execution grant + workflow/role/policy contracts
04 Repository/worktree + candidates + leases + artifacts
05 Deterministic FakeBackplane end-to-end tracer bullet
06 RuntimeBackplane + LocalProcessBackplane
07 HerdrBackplane one-role vertical slice
08 Provider adapters + resolver + RoleAttempt fallback
09 Exchange + role-session capabilities
10 Delegation + consultation + budgets/deadlock checks
11 Multi-project/provider scheduling
12 Web/TUI daemon clients
13 Privileged action/approval execution path
14 Optional SemanticDecisionEngine + Jev shadow mode
15 Recovery/reconciliation hardening + v2 documentation cutover
```

Recovery, cancellation, security, and tests are not deferred to Phase 15. Phase 15 is the final integrated hardening pass.

# Phase 00 — Governance Rebaseline and Historical Freeze

## Goal

Make v2 authoritative without falsifying v1 history.

## Work

- add ADR-0019 declaring Relay Architecture v2;
- explicitly supersede incompatible portions of ADR-0017/0018;
- mark old unimplemented provider-distribution M10 material historical/on hold;
- preserve accepted M0-M9 implementation snapshots as historical records;
- create a v2 roadmap/status area rather than rewriting old module history;
- pin the exact implementation starting HEAD;
- record current schema/migration version;
- run baseline quality gates.

## Required decisions

ADR-0019 must explicitly freeze:

- human owns intent;
- Relay owns controlled orchestration;
- explicit Run/ExecutionGrant;
- no automatic push;
- trusted local path model;
- Herdr is runtime backplane, not task authority;
- semantic engines are optional advisers;
- old wire compatibility commitments.

## Gate

- ADR accepted;
- exact HEAD recorded;
- `cargo fmt --check` PASS;
- `cargo clippy --workspace --all-targets --all-features` PASS;
- `cargo test --workspace` PASS;
- existing `submit_handoff`/Ready behavior covered and green.

# Phase 01 — Canonical Daemon Ownership + Durable Effects / Events

## Goal

Create the infrastructure all later orchestration state can safely depend on without adding workflow complexity yet.

## Owning seams

Current app/store bootstrap, relay-store, current scheduler/state ownership, MCP/TUI process split.

## Work

1. extract/establish one Relay daemon process as canonical semantic writer;
2. define local client protocol/version negotiation;
3. preserve TUI and `serve-mcp` behavior through daemon clients/proxies;
4. implement daemon singleton/state-root ownership and stale-lock recovery;
5. introduce durable command IDs/idempotency;
6. add `PendingEffect`/`EffectResult` infrastructure for external operations;
7. add monotonic durable `EventRecord` sequencing;
8. add snapshot + events-after-sequence client sync;
9. migrate direct DB writes behind daemon incrementally;
10. preserve current task state machine behavior at this phase.

## Do not add yet

- workflows/roles;
- Herdr;
- Jev;
- delegation;
- visual Web UI.

## Recovery requirements

- daemon crash between semantic commit and external effect execution;
- effect success with response loss;
- client retry of same command;
- migration exclusivity/single writer.

## Tests

- duplicate command ID does not duplicate state/effect;
- pending effect resumes/reconciles after restart;
- event sequence monotonic and reconnectable;
- TUI + MCP compatibility through daemon;
- daemon singleton enforcement;
- existing Ready import semantics unchanged.

## Gate

Legacy Relay behavior works through the daemon with no semantic regression.

# Phase 02 — Project / Checkout / Trust Foundation

## Goal

Establish v2 identity/trust before workflows can execute anything.

## Add

- `DeclaredProjectIdentity`;
- `TrustedProjectRegistration`;
- `CheckoutId`;
- trusted expected-remote evidence;
- allowed worktree root;
- registration/trust state;
- repository `.relay/` loader with explicit trust classification;
- safe portable config validation.

## Migration

Adapt current Project Registry entries into trusted local registrations where evidence is sufficient. Do not trust a repository UUID/path automatically during migration.

## Security tests

- cloned project claiming another ProjectId cannot silently hijack registration;
- repo-supplied path rejected as trusted path authority;
- remote mismatch fails closed;
- repository policy can restrict but cannot broaden local hard policy;
- repo workflow cannot invoke arbitrary trusted executable action by string unless action trust rules permit it.

## Gate

Relay can identify a logical project and a trusted local checkout without filesystem path becoming the durable logical ID.

# Phase 03 — Approved Intent, Execution Grant, Workflow, Roles, Policy

## Goal

Introduce the semantic heart of v2 without real providers.

## Add domain contracts

- `ApprovedIntent`;
- `ExecutionGrant`;
- `WorkflowDefinition` + version;
- `WorkflowRun`;
- `RoleDefinition`;
- `RoleInstance`;
- `RoleAttempt` shell/state contract;
- `ExecutionBudget`;
- policy compiler + enforcement class;
- `ClarificationRequest`;
- `DecisionRequest`;
- `ScopeExpansionRequest`;
- approval request contract (execution comes later);
- configuration snapshot.

## Workflow compiler/validator

Validate before starting:

- graph structure;
- typed inputs/outputs;
- role references;
- transition conditions;
- allowed delegation edges;
- cycle bounds;
- budget constraints;
- policy references;
- no arbitrary executable expressions.

## Legacy adapter

Create the v1 handoff compatibility mapping:

```text
HandoffTask v1 + explicit Run
-> ApprovedIntent generation 1
-> ExecutionGrant
-> compatibility WorkflowRun
```

No hidden planner role is added to already-approved implementation intent.

## Tests

- Run without Ready/approved intent rejected;
- changed scope cannot mutate ApprovedIntent in place;
- workflow version/config snapshots remain stable during run;
- policy intersection cannot broaden built-in/user hard deny;
- semantic Orchestrator is not represented as provider RoleInstance;
- attempt retry does not create a new semantic role;
- cancellation transition truthfully persists.

## Gate

A pure in-memory/fake workflow can create roles and transition using validated typed results without starting a real process.

# Phase 04 — Repository / Worktree, Candidate, Lease, Artifact Foundation

## Goal

Build the correctness primitives required before Implementer/Verifier workflows.

## Repository/worktree module

- trusted checkout validation;
- optional isolated worktree creation;
- allowed-root enforcement;
- cleanup ownership;
- current repo fingerprint helpers.

## Candidate

Add immutable candidate generation with explicit fingerprint:

- base/current HEAD;
- index state;
- tracked diff checksum;
- policy-relevant untracked file manifest/checksum;
- checkout/worktree;
- producer role/attempt;
- generation.

## Lease

Add lease kinds plus monotonic fencing token.

Test stale holder behavior after expiry/reacquisition.

## Artifact store

Add immutable metadata + filesystem payload path where needed:

- checksum;
- atomic write;
- trust class;
- provenance;
- retention/sensitivity metadata.

Initial artifact kinds needed for tracer bullet:

- Specification/ApprovedIntent reference;
- ImplementationResult;
- CandidateReference;
- VerificationResult;
- RawLog.

## Gate

Relay can produce Candidate N, freeze/fingerprint it, prevent/identify conflicting scheduled writes, and invalidate verification when covered repository state changes.

# Phase 05 — Deterministic End-to-End Tracer Bullet with FakeBackplane

## Goal

Prove v2 orchestration before any Herdr/provider complexity.

## Scenario

```text
legacy HandoffTask v1
-> Ready
-> human Run
-> ApprovedIntent / ExecutionGrant
-> Implementer RoleInstance
-> FakeBackplane RoleAttempt
-> ImplementationResult + Candidate 1
-> Verifier RoleInstance
-> independent FakeBackplane RoleAttempt
-> VerificationResult PASS
-> COMPLETE
```

Failure branch:

```text
Verifier FAIL
-> Remediator
-> Candidate 2
-> fresh Verifier
-> PASS
```

## Required semantics

- process success alone never completes role;
- malformed result -> bounded correction/failure;
- candidate change invalidates verification;
- execution budget stops unbounded remediation;
- cancellation at every transition leaves durable truthful state;
- restart can resume from any persisted boundary.

## Gate

The v2 domain model is demonstrably sufficient without Herdr, provider-specific logic, or Jev.

# Phase 06 — RuntimeBackplane + LocalProcessBackplane

## Goal

Make the runtime seam real with a production process host that is not Herdr.

## Add

Deep `RuntimeBackplane` interface:

- provision;
- observe;
- send;
- interrupt;
- dispose;
- snapshot;
- reconcile.

Implement:

- `FakeBackplane` aligned to final contract;
- `LocalProcessBackplane` using explicit executable + argument arrays and trusted CWD.

## Important boundary

The backplane receives a prepared/provenanced runtime request. It does not decide project, role, provider, policy, or worktree.

## Tests

- process start/exit;
- output bounds;
- interrupt/cancel;
- lost process/restart reconciliation;
- duplicate provisioning effect key does not spawn twice;
- no shell interpolation from untrusted task text.

## Gate

One real provider can later be hosted without Herdr and without changing relay-core semantics.

# Phase 07 — HerdrBackplane One-Role Vertical Slice

## Goal

Integrate Herdr behind the already-proven runtime seam.

## Work

- Herdr transport/client module;
- capability negotiation/version checks;
- internal topology planner;
- stable Relay metadata on Herdr-created resources where supported;
- provisioning/reconciliation mapping;
- input/output/interrupt/dispose mapping;
- manual-intervention detection;
- bounded raw-log capture;
- runtime binding persistence.

## Topology policy

Suggested default mapping remains internal:

```text
Checkout -> workspace
Task -> tab
RoleAttempt -> pane/process
```

Do not expose this mapping as Relay identity.

## First proof

Provision one RoleAttempt for one provider in one trusted worktree. Do not add dynamic delegation in this phase.

## Tests

- fake Herdr protocol tests;
- duplicate effect reconciliation;
- Relay restart / Herdr session survives;
- Herdr restart / Relay survives;
- missing/mismatched resources become explicit reconciliation outcomes;
- manual user termination is reflected but does not fabricate semantic completion.

## Gate

Herdr can be removed/disabled without changing workflow contracts, and Herdr topology remains an adapter concern.

# Phase 08 — Provider Adapters, Execution Targets, Resolver, Attempt Fallback

## Goal

Separate provider semantics from runtime hosting and prove role portability.

## Add

- `ProviderAdapter` contract;
- provider capability/health snapshots;
- `RuntimeProfile`;
- immutable `ExecutionTarget`;
- deterministic provider resolver;
- `RoleAttempt` retry/fallback state transitions.

## Migration targets

Start with the provider already best supported by historical Relay. Add a second provider only after the first adapter passes contract tests.

Potential providers from the architecture direction include Codex, Claude, AGY, OpenCode, but implement only adapters that can be tested in the repository/environment.

## Resolver order

1. explicit valid pin;
2. hard compatibility/capability filter;
3. policy restrictions;
4. health/auth/quota facts where observable;
5. deterministic preference order;
6. optional semantic recommendation only in later phase.

## Tests

- same RoleInstance retries with fresh RoleAttempt;
- provider failure does not rewrite task intent;
- execution target immutable per attempt;
- unavailable pinned provider fails/escalates according to policy;
- second provider can fill same role without changing workflow schema.

## Gate

At least two provider adapters can satisfy one semantic role contract when installed, or one real + one deterministic test adapter proves the portability contract pending environment availability.

# Phase 09 — Relay Exchange + Role-Session Capabilities

## Goal

Move role communication to typed scoped Relay interfaces.

## Add

- messages;
- artifact edges/attachments;
- role-scoped inbox;
- structured result publication;
- role-session capability/token;
- CLI exchange surface;
- role-scoped MCP/tool surface where needed;
- materialized session fallback.

## Capability requirements

Token is:

- opaque/random;
- attempt-scoped;
- task/project-scoped;
- operation-scoped;
- short-lived;
- revoked on completion/cancel;
- never written to artifact content or normal logs.

## Materialized fallback

Session projection is ephemeral and minimum-access. Use restrictive local permissions where the OS supports them.

## Tests

- cannot enumerate other projects/tasks;
- cross-task access denied by default;
- token cannot widen scope with caller-supplied IDs;
- expired/revoked token denied;
- provider claim vs Relay-observed artifact provenance preserved;
- malformed result never advances role.

## Gate

A real role can consume typed inputs and publish its typed result without downstream roles parsing terminal conversation text.

# Phase 10 — Delegation, Consultation, Budgets, Deadlock Prevention

## Goal

Allow dynamic orchestration without uncontrolled recursive spawning.

## Add

- `DelegationRequest`;
- `ConsultationRequest`;
- role dependency graph;
- blocking/nonblocking semantics;
- budget accounting;
- cycle/deadlock checks;
- delegation policy checks;
- attempt/activation/depth/count limits.

## Flow

```text
provider requests role
-> Relay validates request
-> checks ApprovedIntent/ExecutionGrant
-> checks policy and budget
-> checks dependency cycle
-> creates RoleInstance
-> resolves provider
-> RoleAttempt
-> typed result returns to parent/workflow
```

## Tests

- child cannot broaden approved intent;
- cyclic blocking delegation rejected;
- budget exhaustion escalates/terminates predictably;
- consultation does not accidentally create write authority;
- provider cannot directly name executable/path to bypass resolver;
- cancellation propagates through outstanding children.

## Gate

Dynamic delegation is bounded, recoverable, and entirely Relay-mediated.

# Phase 11 — Multi-Project / Provider Scheduler v2

## Goal

Scale v2 semantics across concurrent projects without semantic AI controlling fairness.

## Extend scheduling inputs

- global max parallel;
- per-project max parallel;
- provider capacity;
- checkout/worktree leases;
- provider pinning;
- user priority;
- workflow dependency readiness;
- execution budget;
- project fairness/aging;
- privileged operation serialization where needed.

## Tests

- one blocked project does not starve another;
- same worktree write conflict serializes;
- independent checkouts may run concurrently if policy permits;
- provider capacity release after crash/cancel;
- fairness deterministic under repeated runs;
- cancellation of one project does not corrupt another queue.

## Gate

Two real or realistically simulated projects can run concurrently with correct isolation and recover after daemon restart.

# Phase 12 — Web UI + Relay TUI as Daemon Clients

## Goal

Expose semantic orchestration without duplicating Herdr's terminal UI.

## Web first

Implement:

- global dashboard;
- project/task views;
- task detail + approved intent;
- workflow graph (read-only operational view);
- roles/attempts/providers;
- candidate/verification history;
- artifact viewer;
- Attention Inbox split by clarification/decision/approval;
- policy/enforcement labels;
- decision/event log;
- runtime binding status + open Herdr/raw terminal action.

## TUI

Refactor into daemon client with concise:

- queue/running/attention screens;
- task/workflow detail;
- Run/Cancel/Suspend/Interrupt commands with explicit semantics;
- artifact/result inspection.

## Defer

- full visual workflow builder;
- arbitrary repository editing;
- terminal emulation in Web UI.

## Client consistency

Clients use snapshot + event sequence and must render equivalent semantic state.

## Gate

Closing Web/TUI does not affect active workflows, and both clients agree on daemon state after reconnect.

# Phase 13 — Privileged Action Executor + Exact Approvals

## Goal

Add sensitive operations without delegating open-ended privilege to AI roles.

## Add

- `ApprovalRequest`/`ApprovalGrant` full lifecycle;
- privileged action registry;
- deterministic executor for approved Git integration/operation classes;
- exact parameter hashing;
- replay/expiry rules;
- policy enforcement-class reporting.

## Initial scope

Keep initial privileged operations narrow. Automatic push remains off unless separately approved by a future explicit product decision.

## Tests

- approval for operation A cannot authorize B;
- parameter change invalidates approval;
- expired/replayed approval rejected;
- provider cannot execute privileged Relay action by publishing prose;
- ambiguous external effect becomes reconcile/escalate, not optimistic success.

## Gate

One approved privileged action can be executed exactly once with auditable evidence and no open-ended provider authority.

# Phase 14 — SemanticDecisionEngine + Optional Jev Shadow Mode

## Goal

Add bounded semantic recommendations only after deterministic v2 works.

## Add

- `DecisionManager`;
- `RuleDecisionEngine`;
- `FakeDecisionEngine`;
- provider-neutral `SemanticDecisionEngine` interface;
- outcome `Recommend | Abstain | Escalate` semantics;
- sanitized decision projection;
- decision audit record;
- optional Jev adapter behind feature/config gating.

## Initial decision classes

- provider fit among already-legal candidates;
- retry escalation strategy;
- optional additional review recommendation;
- trusted skill relevance;
- optional artifact relevance.

## Explicit exclusions

- permission grants;
- Git/deploy/database authority;
- candidate validity;
- test exit status;
- provider installation/auth facts;
- scheduler fairness/tie-break in initial rollout.

## Jev rollout

1. verify current external API/privacy/credential model;
2. shadow mode only;
3. compare recommendation against deterministic/user outcome;
4. measure usefulness/calibration/failure rate;
5. enable influence per decision class only after explicit acceptance.

## Gate

Disabling Jev produces a fully functional Relay. Low confidence/unavailability yields deterministic fallback or human attention.

# Phase 15 — Integrated Recovery, Retention, Security Truthfulness, Documentation Cutover

## Goal

Prove the whole architecture under failure and formally cut documentation to v2.

## Failure matrix

Exercise at minimum:

- daemon crash before/after effect execution;
- client disconnect/reconnect;
- provider crash;
- role result missing/malformed;
- Herdr unavailable/restarted;
- LocalProcess child lost;
- candidate mutated during verification;
- stale lease holder;
- project path/remote mismatch;
- token expiry/revocation;
- cancellation racing completion;
- approval racing parameter change;
- delegation child failure;
- budget exhaustion;
- artifact checksum mismatch;
- storage write failure;
- SQLite contention/migration failure.

## Retention / cleanup

Implement auditable cleanup for:

- runtime resources created by Relay;
- temporary worktrees;
- session projections/tokens;
- old raw logs;
- expired leases;
- old candidates/artifacts according to retention policy.

Never delete active/referenced/audit-required data.

## Documentation cutover

Update current architecture/security/persistence/UI/context docs to v2. Historical v1 docs remain clearly marked historical/superseded.

## Final quality gates

```bash
cargo fmt --check
cargo clippy --workspace --all-targets --all-features
cargo test --workspace
```

Add relevant Web lint/typecheck/test/build gates once Web exists.

# Cross-Phase Rules

Every phase must:

- have an exact parent/fixed point;
- change only the dependency cone needed for its exit condition;
- add migrations only when the phase consumes the new data;
- include crash/recovery tests for any new durable/external effect;
- update v2 docs when a contract intentionally changes;
- preserve no-auto-run and no-auto-push unless explicitly superseded;
- refuse speculative future crates/tables/frameworks not needed by the phase;
- finish with exact quality-gate evidence.

# Recommended Branch Strategy

Use one reviewed branch/PR per phase or tightly bounded subphase rather than one enormous v2 branch.

Each accepted phase records:

- parent SHA;
- accepted candidate SHA;
- migrations introduced;
- public/domain contract changes;
- tests/gates;
- recovery assumptions;
- known deferred work;
- whether the next phase may rely on the new contract.
