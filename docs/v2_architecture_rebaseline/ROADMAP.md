# Relay Architecture v2 — Implementation Roadmap & Status

**Architecture Generation:** v2  
**Authority:** [ADR-0020: Relay Architecture v2](../adr/0020-relay-architecture-v2.md)  
**Historical Lineage:** M0–M9 lineage frozen (branch `feat/m9-relay-direct-mcp` at commit `0d03bed`)  
**Status Date:** 2026-10-16  

---

## 1. Baseline Preflight Evidence (Fixed Point)

- **Parent branch:** `feat/m9-relay-direct-mcp`
- **Fixed-point start commit:** `0d03bed` (`fix(mcp): repair comment encoding, tighten scrubbing doc, drop dead EOF branch`)
- **Workspace Quality Gates:**
  - `relay-core` (85 unit & integration tests): **PASS**
  - `relay-protocol` (16 wire tests): **PASS**
  - `relay-runtime` (5 tests): **PASS**
  - `relay-runtime-antigravity` (3 tests): **PASS**
  - `relay-runtime-fake` (recovery loop tests): **PASS**
  - `relay-store` (SQLite concurrency & state machine): **PASS**
  - `relay-tui` (124 tests): **PASS**
  - `serve_mcp` integration tests (9 tests): **PASS**
  - *Toolchain requirement:* Windows MSVC `link.exe` via Visual Studio Build Tools (`vcvarsall.bat x64`).

---

## 2. Implementation Sequence Status

> **Status correction (2026-09-18):** the table below previously marked every phase `DONE`. An external governed review of PR #12 (fixed head `8fc1566d`) found that claim false against the phases' own exit gates — new v2 modules were largely built horizontally alongside the v1 store/scheduler/runtime topology rather than replacing it on the production path. The statuses below are reset to the audit's evidence-based assessment. See [REMEDIATION-WAVES.md](REMEDIATION-WAVES.md) for the ordered correction plan. Do not mark a phase `DONE` again until its exit gate is proven on the production path, not merely covered by a unit/integration test of an unwired module.

> **Status update (2026-10-16):** four of the five Wave B findings have since landed — R12-05 (trust classification verifies the expected remote), R12-06 (workflow/intent/policy validation contract), R12-07 (SHA-256 integrity digests) and R12-08 (durable, fenced write leases). Phases 02, 03 and 04 are re-graded below to reflect that. **R12-02 is still outstanding**: the user-facing Run path still executes the v1 `ExecutionScheduler`, so phase 01 and the "What works today" summary are unchanged on that point. Waves C–F are not started, and the governed re-review remains deferred (see [REMEDIATION-WAVES.md](REMEDIATION-WAVES.md)).

Per [02-IMPLEMENTATION-SEQUENCE.md](02-IMPLEMENTATION-SEQUENCE.md):

| Phase | Description | Status | Evidence-based assessment |
|---|---|---|---|
| **00** | **Governance Rebaseline & Historical Freeze** | **PARTIAL** | ADR-0020 and v2 docs exist, but current-status docs and scope claims conflicted (being corrected in this pass). |
| **01** | **Canonical Daemon Ownership + Durable Effects / Events** | **PARTIAL (Wave A landed)** | Wave A is implemented: `relay daemon` now runs a real Windows named-pipe request listener (not just a lease heartbeat), TUI and `relay serve-mcp` submit task creation through a `DaemonClient` (`connect_or_embed`) instead of writing `SqliteStore` directly, commands are crash-safe (deterministic task ids derived from the command id, idempotent inserts, `needs_reconciliation` for corrupted payloads, startup reconciliation), and a daemon-owned effect worker reconciles stale `in_flight` effects. Still **not DONE**: the v1 `ExecutionScheduler` Run path is unchanged and still writes directly to the store (Wave B / R12-02), and no v2 aggregate beyond commands/effects/leases is durable yet (by design - nothing on the corrected path needs it yet). |
| **02** | **Project / Checkout / Trust Foundation** | **PARTIAL (Wave B: R12-05 landed)** | Core types and portable config exist, and trusted classification now verifies the expected Git remote end-to-end (R12-05), so the previous gate failure no longer applies. Still **not DONE**: the trust decision is not exercised on the production Run path (R12-02). |
| **03** | **Approved Intent, Execution Grant, Workflow, Roles, Policy** | **PARTIAL (Wave B: R12-06 landed)** | The workflow/intent/policy validation contract is now complete (R12-06), so the previous gate failure no longer applies. Still **not DONE**: canonical state for these aggregates is not durable yet, and nothing here is on the production Run path (R12-02). |
| **04** | **Repository / Worktree, Candidate, Lease, Artifact Foundation** | **PARTIAL (Wave B: R12-07, R12-08 landed)** | Integrity digests are stable SHA-256 (R12-07) and write leases are durable, fenced and scheduler-enforced (R12-08), so the previous `UNSAFE` verdict no longer applies. Still **not DONE**: durability for the remaining repository/worktree/candidate aggregates is incomplete and none of it is on the production Run path (R12-02). |
| **05** | **Deterministic End-to-End Tracer Bullet (FakeBackplane)** | **NOT IMPLEMENTED AS SPECIFIED** | Existing test simulates state/files manually; no FakeBackplane + daemon/store + restart vertical slice yet. Not in Wave A scope. |
| **06** | **RuntimeBackplane + LocalProcessBackplane** | **PARTIAL** | Seam and adapters exist; containment, environment, and idempotency semantics need hardening. Not in Wave A scope. |
| **07** | **HerdrBackplane One-Role Vertical Slice** | **PARTIAL / TEST-ONLY** | Topology/client abstractions exist; production transport and durable RuntimeBinding recovery are absent. Not in Wave A scope. |
| **08** | **Provider Adapters, Execution Targets, Resolver, Fallback** | **PARTIAL / SEMANTIC BUG** | Codex + mock resolver foundation exists; process exit success is conflated with semantic result publication. Not in Wave A scope. |
| **09** | **Relay Exchange + Role-Session Capabilities** | **PARTIAL / PROTOTYPE** | Capability/result logic exists; communication is memory-only and outbox ingestion is not crash-idempotent. Not in Wave A scope. |
| **10** | **Delegation, Consultation, Budgets, Deadlock Prevention** | **PARTIAL** | Algorithms/tests exist; state/budgets are memory-only and not restart-recoverable. Not in Wave A scope. |
| **11** | **Multi-Project / Provider Scheduler v2** | **PARTIAL** | Fairness/capacity algorithm exists; queues/allocations are memory-only and real leases are not used. Not in Wave A scope. |
| **12** | **Web UI + Relay TUI as Daemon Clients** | **NOT IMPLEMENTED AS SPECIFIED** | No Web app; tests used `InProcessDaemonClient` only. Wave A wires TUI/MCP task submission through a real `DaemonClient`, but the Web UI itself is out of scope until Waves A-E are accepted (see remediation policy). |
| **13** | **Privileged Action Executor + Exact Approvals** | **PARTIAL / SECURITY BUG** | Typed actions + SHA-256 exist; checkout binding and durable replay/audit are missing. Not in Wave A scope. |
| **14** | **SemanticDecisionEngine + Optional Jev Shadow Mode** | **FOUNDATION ONLY** | Decision seam/fakes exist and Jev safely degrades, but decision audit/integration is memory-only and upstream v2 path is not operational. Not in Wave A scope. |
| **15** | **Integrated Recovery, Retention, Security Truthfulness, Docs Cutover** | **NOT IMPLEMENTED AS GATE** | Small recovery/retention tests exist; recovery/cleanup logic has correctness defects, failure matrix is incomplete, and docs cutover did not happen (this pass begins the docs correction only). Not in Wave A scope. |

---

## 2a. Governed Audit Findings (Wave Tracking)

A governed external review of PR #12 (fixed head `8fc1566d`) produced 19 P1 findings grouped into 6 dependency waves. Full source material and one detailed correction plan per finding are archived outside this repository; the wave structure and Wave A scope are recorded in-tree at [REMEDIATION-WAVES.md](REMEDIATION-WAVES.md) so later work has an authoritative in-repo reference instead of relying on an out-of-band review.

| Wave | Theme | Status |
|---|---|---|
| **A** | Restore architectural truth and one owner (docs truth, daemon ownership, command/effect idempotency, durable v2 state) | **LANDED** (re-review pending; see acceptance checklist in REMEDIATION-WAVES.md) |
| **B** | Establish a real v2 execution vertical slice (trust classification, workflow/policy contracts, integrity digests, lease fencing, Run-path cutover) | **PARTIAL** — R12-05, R12-06, R12-07 and R12-08 landed; **R12-02 (Run-path cutover) outstanding** |
| **C** | Harden runtime/provider semantics (process containment, exit-vs-result semantics, Herdr production backplane) | **NOT STARTED** |
| **D** | Make orchestration durable/recoverable (Exchange, delegation/scheduler v2, policy enforcement truthfulness) | **NOT STARTED** |
| **E** | Privilege and failure correctness (approval binding, startup recovery uncertainty, retention safety) | **NOT STARTED** |
| **F** | User surfaces and final acceptance (exact-head quality-gate evidence) | **NOT STARTED** |

Do not implement the Phase 12 Web UI merely to make this table look complete while Waves B-E are outstanding — see the remediation policy in `REMEDIATION-WAVES.md`.

## 3. Preserved Invariants & Boundary Commitments

1. `HandoffTask v1` wire compatibility is preserved through internal adaptation.
2. `relay serve-mcp` (`submit_handoff`) continues to mean *validate -> resolve trusted project -> persist -> READY*. No auto-execution.
3. Explicit human Run approval is mandatory before any workflow execution begins.
4. No automatic Git push.
5. Local trusted project registration is authoritative; remote tasks cannot provide arbitrary execution paths.
6. Fail-closed safety for all unknown protocols, malformed inputs, or security mismatches.
