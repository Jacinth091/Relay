# Relay Architecture v2 — Implementation Roadmap & Status

**Architecture Generation:** v2  
**Authority:** [ADR-0020: Relay Architecture v2](../adr/0020-relay-architecture-v2.md)  
**Historical Lineage:** M0–M9 lineage frozen (branch `feat/m9-relay-direct-mcp` at commit `0d03bed`)  
**Status Date:** 2026-10-18  

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

> **Status update (2026-10-18):** R12-02 is implemented on draft PR #31 (`0cf46f8`), and Wave C's R12-09/R12-11 plus the local-process Run path and TUI cutover are implemented on stacked draft PR #32 (`9b3ae87`); neither PR is merged. In that draft stack, the v2 Run persists its approved intent, grant and config snapshot, drives `OrchestratorV2`, validates the registered project through `TrustedRunContextResolver`, and launches the local Codex adapter through `LocalProcessBackplane`; TUI Run sends `RunTask` to the daemon by default, with the v1 scheduler opt-in behind `RELAY_LEGACY_V1_RUN=1`. Windows CI is green on runs 39094441207, 39126016714 and 39128455124. The proof uses `FakeBackplane`/mock providers and a local PowerShell stand-in; it has not exercised the real Codex CLI. R12-10 (production Herdr backplane) is split into a separate future run. Wave C remains **PARTIAL**, and no phase below is `DONE`.
> The default v2 Run path cannot yet raise structured clarification or decision escalations: `OrchestratorV2` completes or fails with a typed error, and only legacy v1 escalates.

Per [02-IMPLEMENTATION-SEQUENCE.md](02-IMPLEMENTATION-SEQUENCE.md):

| Phase | Description | Status | Evidence-based assessment |
|---|---|---|---|
| **00** | **Governance Rebaseline & Historical Freeze** | **PARTIAL** | ADR-0020 and v2 docs exist, but current-status docs and scope claims conflicted (being corrected in this pass). |
| **01** | **Canonical Daemon Ownership + Durable Effects / Events** | **PARTIAL (Wave A landed; Wave B/C Run cutover on unmerged drafts)** | Wave A daemon ownership and command/effect durability remain landed. R12-02 implements the v2 Run start and orchestrator on draft PR #31 (`af4b84a`, `623b9d5`); Wave C routes TUI Run to daemon `RunTask` by default, leaving v1 behind `RELAY_LEGACY_V1_RUN=1` (`7a0bd93`). PRs #31 and #32 are unmerged, so the promoted path is not yet cut over. |
| **02** | **Project / Checkout / Trust Foundation** | **PARTIAL (Wave B trust on draft Run path)** | R12-05 verifies the expected Git remote. The draft local Run composition calls `validate_and_classify` through `TrustedRunContextResolver` (`26b63e3`); `approved_run_completes_through_local_process_backplane_and_trusted_resolver` covers that path with a PowerShell stand-in. The PR stack is unmerged; this is not production proof from a real provider. |
| **03** | **Approved Intent, Execution Grant, Workflow, Roles, Policy** | **PARTIAL (Wave B durable Run path on unmerged drafts)** | R12-06 completes the workflow/intent/policy validation contract. R12-02 persists workflow runs, role instances and attempts (`0010`/`0011`, `f8db7f2`), then persists approved intent, execution grant and config snapshot (`0012`, `af4b84a`); `623b9d5` drives the approved run through `OrchestratorV2`. Broader policy enforcement truthfulness remains open in R12-14. |
| **04** | **Repository / Worktree, Candidate, Lease, Artifact Foundation** | **PARTIAL (Wave B/C durable Run pieces; broader foundation incomplete)** | R12-07 supplies stable SHA-256 digests and R12-08 durable fenced leases. R12-02 adds durable workflow runs, roles, attempts and typed result artifacts (`f8db7f2`, `af4b84a`, `623b9d5`); Wave C stabilizes worktree reuse (`1f1196a`). Remaining repository/worktree/candidate durability is incomplete, and the PR stack is unmerged. |
| **05** | **Deterministic End-to-End Tracer Bullet (FakeBackplane)** | **PARTIAL (Wave B test proof)** | `3c95021` adds `crates/relay-core/tests/wave_b_run_path_v2.rs`: daemon/store/orchestrator completion with `FakeBackplane`, bounded remediation, result validation, restart reconciliation, replay convergence and fencing. This is deterministic fake-backed coverage, not a live provider run. |
| **06** | **RuntimeBackplane + LocalProcessBackplane** | **PARTIAL (Wave C local-process path)** | R12-09 hardens environment sanitization, process-tree termination, orphan reporting and process-identity checks (`7cddeb9`, `2af3cb6`, `760a25d`). Wave C wires `LocalProcessBackplane` into the daemon Run path (`26b63e3`) and bounds output drain/stabilizes worktree reuse (`1f1196a`). Windows CI covers the boundary tests and a PowerShell stand-in; a real Codex CLI smoke run is still absent. |
| **07** | **HerdrBackplane One-Role Vertical Slice** | **PARTIAL / TEST-ONLY (R12-10 split out)** | Topology/client abstractions remain test-only; no production Herdr transport or durable RuntimeBinding recovery. Owner decision split R12-10 (production Herdr backplane) into a separate future run. |
| **08** | **Provider Adapters, Execution Targets, Resolver, Fallback** | **PARTIAL (Wave C adapter semantics and Codex wiring)** | R12-11 requires a typed published role result rather than treating process exit as success (`415337c`) and launches attempts through provider adapters using approved-intent prompts (`d226a9b`). The draft Run path wires only `CodexProviderAdapter` (`26b63e3`); a missing executable fails closed (`missing_codex_executable_fails_the_attempt_and_run_with_a_clear_reason`). There is no real Codex CLI run; the Antigravity target has no production adapter and fails closed. |
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
| **B** | Establish a real v2 execution vertical slice (trust classification, workflow/policy contracts, integrity digests, lease fencing, Run-path cutover) | **PARTIAL** — R12-05, R12-06, R12-07 and R12-08 landed; R12-02 is implemented on draft PR #31 (`0cf46f8`, Windows CI run 39094441207 green) but unmerged. |
| **C** | Harden runtime/provider semantics (process containment, exit-vs-result semantics, Herdr production backplane) | **PARTIAL** — R12-09 and R12-11 plus the local-process Run path/TUI cutover are on draft PR #32 (`9b3ae87`, Windows CI runs 39126016714 and 39128455124 green); R12-10 is split into a separate future run. PR #32 is unmerged. |
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
