# Relay M10 — Provider Distribution Sequential Implementation Pack

**Status:** DEV implementation sequence
**Decision basis:** ADR-0018 (`docs/adr/0018-provider-distribution-model.md`)
**Purpose:** Implement the three ADR-0018 design-intent capabilities — health-aware routing, fan-out of one approved task to multiple providers, and cross-provider result aggregation — in dependency order, without collapsing the work into one oversized document and without presenting any design-intent item as shipped before it works end-to-end.

## Milestone goal

One locally approved handoff can be fanned out to several chosen coding-runtime providers as sibling executions under one parent task identity. Routing honors capability metadata and plain-data health snapshots deterministically. When siblings disagree materially, Relay escalates with a structured `NEEDS_DECISION` packet instead of inventing a verdict. The Dashboard DISTRIBUTION view shows where work went across providers, grouped by parent handoff.

Nothing here widens approved intent (ADR-0018): fan-out multiplies *where* the same approved work runs, never *what* runs.

## Non-goals for M10

- No partitioned subtasks: every sibling executes the identical approved HandoffTask. Splitting one task into provider-specific subtasks remains an open design question per ADR-0015/0018 and needs its own ADR.
- No LLM-based result comparison or verdict synthesis. Aggregation is deterministic text/record comparison only (invariant 7).
- No automatic push of any sibling's output anywhere (invariant 14). Sibling outputs stay local; a human picks what survives.
- No per-provider slot accounting. Slots remain global/project-scoped; per-provider capacity stays additive-later per ADR-0018.
- No planner-facing fan-out surface. Fan-out intent originates at local Run approval only (invariant 13).
- No new transports. Submission layering is untouched by this pack (ADR-0016).

## Implementation order

### 01 — Capability metadata and health-aware routing

File: `01-capability-metadata-and-health-aware-routing.md`

Replace name-prefix dispatch in `RuntimeRouter` with capability/profile metadata dispatch. Health enters routing as plain-data snapshots supplied by the host (the `ExecutionPlanEnvironment` pattern); selection among healthy candidates follows deterministic preference from approved intent and local configuration, never judgment. Fails closed when no registered runtime satisfies the policy.

This phase is the foundation: fan-out (02) routes each sibling through the same seam.

### 02 — Fan-out execution model

File: `02-fan-out-execution-model.md`

Fan ONE approved task into N sibling executions under one parent task identity. Fan-out intent is expressed at Run approval time as local run configuration — never inside the HandoffTask v1 payload, which must remain byte-compatible with today's schema. Parent/sibling durable-state model, per-sibling plan derivation via `plan_execution` reuse, slot accounting rules, cancellation semantics, and deterministic sibling-disagreement handling are specified here.

Do not proceed to 03 until sibling lifecycle and cancellation are proven against the fake runtime. This is the hard gate of M10: aggregation without a sound fan-out model would present numbers about executions that do not reliably exist.

### 03 — Cross-provider aggregation and reports

File: `03-cross-provider-aggregation-and-reports.md`

Consolidated result view across siblings; deterministic conflict/diff summary between provider outputs; `NEEDS_DECISION` escalation on material disagreement; evolution of the Dashboard DISTRIBUTION section from per-profile counts to parent-grouped sibling rows.

### 04 — Conformance tests and documentation reconciliation

File: `04-conformance-and-tests.md`

Deterministic fake-runtime scenarios for router capability dispatch, slot accounting under fan-out, cancel-parent-cancels-siblings, and disagreement escalation; UI test expectations for the new Dashboard rows; doc reconciliation so status pages distinguish shipped distribution behavior from remaining design intent.

## Quality gates (all phases)

- `cargo fmt --check`
- `cargo clippy --workspace --all-targets --all-features`
- `cargo test --workspace`
- Focused integration tests at each new boundary (router dispatch, fan-out scheduling, cancellation propagation, escalation packet production) per AGENTS.md quality requirements.
- Scheduler/lifecycle changes respect the frozen M6 semantics: additive amendments only (`crates/relay-core/src/execution.rs`).

## Invariant notes

- Invariant 1/7 (one engine, no LLM reasoning): routing, fan-out, and aggregation are deterministic data movement and record-keeping; core gains snapshots, never opinions.
- Invariant 3/5/13 (explicit handoff, escalation on material change, explicit Run approval): fan-out is requested at Run approval and multiplies nothing before approval. Sibling disagreement over approved intent escalates as `NEEDS_DECISION`, it is never resolved autonomously.
- Invariant 12 (remote tasks cannot pick execution paths): provider choice still arrives only through validated handoff fields and local configuration; fan-out intent is local-only.
- Invariant 15 (capability checks over names): phase 01 exists precisely to retire the name-prefix seam.
- Invariant 18 (runtimes are executors, not planners): fan-out compares implementations of one approved intent; it does not ask runtimes to explore.
- Invariant 22 (canonical local state): the parent TaskId remains the single canonical identity; siblings are execution records beneath it, all in local SQLite.
