# Relay Architecture v2 — Implementation Roadmap & Status

**Architecture Generation:** v2  
**Authority:** [ADR-0020: Relay Architecture v2](../adr/0020-relay-architecture-v2.md)  
**Historical Lineage:** M0–M9 lineage frozen (branch `feat/m9-relay-direct-mcp` at commit `0d03bed`)  
**Status Date:** 2026-09-18  

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

Per [02-IMPLEMENTATION-SEQUENCE.md](02-IMPLEMENTATION-SEQUENCE.md):

| Phase | Description | Status | Evidence / Deliverables |
|---|---|---|---|
| **00** | **Governance Rebaseline & Historical Freeze** | **DONE** | ADR-0020 accepted, ADR-0017/0018 updated, M10 pack marked on-hold, v2 docs in-tree, roadmap established, preflight gates verified. |
| **01** | **Canonical Daemon Ownership + Durable Effects / Events** | **DONE** | Single writer daemon process (`relay daemon`), client protocol (`DaemonRequest`/`DaemonResponse`), durable command idempotency, `PendingEffect`/`EffectResult` lifecycle, `EventRecord` monotonic sequencing, snapshot sync, lease fencing tokens + heartbeat renewal, full workspace tests PASS. |
| **02** | **Project / Checkout / Trust Foundation** | **DONE** | `DeclaredProjectIdentity`, `TrustedProjectRegistration`, `CheckoutId`, `RemoteFingerprint`, `Checkout`, `.relay/` portable config loader (`PortableRepositoryConfig`), security tests for hijack prevention, path injection rejection, remote mismatch fail-closed, and policy clamping. Full workspace tests PASS. |
| **03** | **Approved Intent, Execution Grant, Workflow, Roles, Policy** | **DONE** | `ApprovedIntent`, `ExecutionGrant`, `WorkflowRun`, `RoleInstance`, `RoleAttempt`, `ScopeExpansionRequest`, multi-layer policy compiler (`safety floor ∩ user ∩ project ∩ repo ∩ role`). Legacy adapter for `HandoffTask v1` (generation 1, single implementer node). Integration tests passing with 0 warnings. |
| **04** | **Repository / Worktree, Candidate, Lease, Artifact Foundation** | **DONE** | Isolated worktrees (`WorktreeManager`), boundary traversal rejection, `Candidate` generation with full repo fingerprint (`base_head`, `current_head`, `index_digest`, `tracked_diff_digest`, `relevant_untracked_digest`), verification invalidation on manual mutations, monotonic `Lease` fencing tokens, immutable content-addressed `ArtifactStore` with tamper detection. Full workspace tests PASS. |
| **05** | **Deterministic End-to-End Tracer Bullet (FakeBackplane)** | **DONE** | Implementer -> `Candidate 1` -> Verifier (FAIL) -> Remediation loop -> `Candidate 2` -> fresh Verifier (PASS) -> COMPLETE verified end-to-end. Process success alone does not complete role, candidate mutations break verification, budget caps attempts, cancellation leaves truthful state. Full workspace tests PASS. |
| **06** | **RuntimeBackplane + LocalProcessBackplane** | **DONE** | Decoupled `RuntimeBackplane` seam (`provision`, `observe`, `send`, `interrupt`, `dispose`, `snapshot`, `reconcile`), async-safe `LocalProcessBackplane` with streaming I/O limits, and `FakeBackplane`. Integration tests passing with 0 warnings, full workspace tests PASS. |
| **07** | **HerdrBackplane One-Role Vertical Slice** | **DONE** | Herdr client and transport protocol, capability negotiation and version checks, private internal topology planner (`Checkout -> Workspace`, `Task -> Tab`, `RoleAttempt -> Pane`), Relay metadata tags without secret exposure, `RuntimeBindingStore`, manual user intervention detection, bounded output capture, fake Herdr protocol test suite passing with 0 warnings, full workspace tests PASS. |
| **08** | **Provider Adapters, Execution Targets, Resolver, Fallback** | **DONE** | `ProviderAdapter` contract, deterministic resolver, immutable `ExecutionTarget`, provider retry/fallback, Codex + Mock adapters, 8 integration tests passing cleanly. |
| **09** | **Relay Exchange + Role-Session Capabilities** | **DONE** | Typed role communication (`ExchangeMessage`, `RoleInbox`), attempt-scoped ephemeral capability tokens (`RoleSessionCapability`), cross-task & cross-project isolation, structured result validation & candidate binding, provenance preservation (`ProviderClaim` vs `RelayObserved`), materialized session projection without secret token leakage, atomic outbox consumption. |
| **10** | **Delegation, Consultation, Budgets, Deadlock Prevention** | **DONE** | Role dependency graph, deadlock/cycle detection, execution budget deduction, consultation read-only write protection, bounded delegation depth & count, cancellation cascade across children. |
| **11** | **Multi-Project / Provider Scheduler v2** | **DONE** | Multi-project fair scheduling with starvation-preventing aging weight, global & per-project limits, exclusive checkout write serialization, provider capacity tracking & instant release, cancellation isolation across queues. |
| **12** | **Web UI + Relay TUI as Daemon Clients** | **DONE** | `DaemonClient` trait, `InProcessDaemonClient`, version-negotiating handshake, multi-client state agreement, monotonic event catch-up, persistent state preservation across disconnects, 3 integration tests passing. |
| **13** | **Privileged Action Executor + Exact Approvals** | **DONE** | `ApprovalRequest`/`ApprovalGrant`, SHA-256 parameter hashing, replay protection, deterministic executor, `GitOperationRunner`, 6 integration tests passing. |
| **14** | **SemanticDecisionEngine + Optional Jev Shadow Mode** | **DONE** | Provider-neutral `SemanticDecisionEngine` interface (`Recommend \| Abstain \| Escalate`), `DecisionManager` with shadow mode, `RuleDecisionEngine`, `FakeDecisionEngine`, optional `JevDecisionAdapter`, 6 integration tests passing. |
| **15** | **Integrated Recovery, Retention, Security Truthfulness, Docs Cutover** | **DONE** | Crash recovery with pending effect reconciliation, lease fencing verification, candidate working-tree integrity invalidation, `RetentionManager` with safe worktree pruning and expired artifact collection, 5 integration tests passing. |

---

## 3. Preserved Invariants & Boundary Commitments

1. `HandoffTask v1` wire compatibility is preserved through internal adaptation.
2. `relay serve-mcp` (`submit_handoff`) continues to mean *validate -> resolve trusted project -> persist -> READY*. No auto-execution.
3. Explicit human Run approval is mandatory before any workflow execution begins.
4. No automatic Git push.
5. Local trusted project registration is authoritative; remote tasks cannot provide arbitrary execution paths.
6. Fail-closed safety for all unknown protocols, malformed inputs, or security mismatches.
