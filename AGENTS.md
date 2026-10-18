# Relay Agent Instructions

This file is for coding agents working **on Relay itself**.

It is designed to be **self-sufficient**: an agent that reads only this file should understand what Relay is, how it is structured, where things live, what is done vs planned, and how to work here without breaking anything. Browse deeper docs only when a task needs more detail than the summaries below (each section says which docs to open).

---

## 1. Identity Snapshot

**Relay Architecture v2 is a local-first, policy-governed orchestration control plane for user-approved software work** (ADR-0020, superseding ADR-0017).

- **The human owns intent** and privileged decisions.
- **Relay (Daemon)** owns durable tasks, approved intent, execution grants, workflows, roles, attempts, policy, provider resolution, artifacts, delegation, approvals, candidates, leases, scheduling, recovery, and user-facing semantic state.
- **Runtime backplanes** host processes and sessions without owning tasks (Herdr is the reference interactive backplane; `LocalProcessBackplane` provides the minimal non-Herdr execution path; `FakeBackplane` provides deterministic testing).
- **Provider adapters** translate provider-specific launch, readiness, and result behavior (Codex, Claude, AGY, OpenCode).
- **Provider CLIs** perform assigned role attempts.
- **Semantic decision engines** (e.g., Jev) are optional advisory helpers after deterministic filtering; they never create authority.
- One-sentence anchor: *"The human owns intent. Relay owns controlled orchestration. Runtime backplanes host execution. Providers perform assigned roles."*

Normative sources: `docs/adr/0020-relay-architecture-v2.md`, `docs/v2_architecture_rebaseline/`.

---

## 2. Current Status Snapshot

Relay Architecture v2 (ADR-0020) is organized as 16 dependency-ordered phases
(00–15). The current roadmap marks no phase `DONE`.

**Authoritative phase status is the table in `docs/v2_architecture_rebaseline/ROADMAP.md`**
(status date 2026-10-18). Wave A is **LANDED** (governed re-review pending).
Wave B is **PARTIAL**: R12-05 through R12-08 landed; R12-02 is implemented on
unmerged draft PR #31. Wave C is **PARTIAL** on unmerged draft PR #32: R12-09,
R12-11, and the local-process Run/TUI cutover are implemented; R12-10
(production Herdr backplane) is split into a separate future run. The proof
uses fake/mock providers and a local PowerShell stand-in; it has not exercised
the real Codex CLI and does not establish a promoted production path. Read the
roadmap for current truth and do not present a module or test as proof that a
path is production-ready.

| Item | State |
|---|---|
| Historical v1 lineage (M0–M9) | Frozen as historical evidence; ADR-0017 superseded by ADR-0020 |
| v2 phase table (00–15) | **No phase is `DONE`.** The audit reset the dated roadmap's `DONE` markings; every phase now carries an evidence-based verdict — read `ROADMAP.md` |
| Working end-to-end path today | On unmerged draft PRs #31/#32: local paste/file import or `relay serve-mcp` `submit_handoff` → TUI review → explicit Run approval → daemon `RunTask` (approved intent, execution grant, config snapshot, and execution target frozen) → `OrchestratorV2` → `TrustedRunContextResolver` (registered project, trust classification, isolated `relay-run-<id>` worktree) → `LocalProcessBackplane` → `CodexProviderAdapter` → typed role result → `COMPLETE` or typed failure. Windows CI uses fakes/mocks and a local PowerShell stand-in; no real Codex CLI run has occurred. The v1 `ExecutionScheduler` is available only with `RELAY_LEGACY_V1_RUN=1`. |
| A real Web UI | **Does not exist.** Phase 12's evidence was an in-process `DaemonClient` abstraction and integration tests, not a web application |
| Daemon | `relay daemon` and daemons embedded by the TUI and `relay serve-mcp` run the v2 workflow driver (`run_full_daemon_with_workflow_driver` / `connect_or_embed_with_workflow_driver`). The daemon owns its store lease and fencing token, renews its heartbeat, and serves clients over a **Windows named pipe**. Windows-only at this revision — see issue #17 |
| Herdr backplane | Topology/client abstractions remain test-only; there is no production Herdr transport or durable RuntimeBinding recovery. R12-10 (production Herdr backplane) is split into a separate future run |
| Jev / semantic decisions | Optional advisory adapter behind a provider-neutral seam; not required for correctness |
| Ingress reality | Local file/paste plus `relay serve-mcp` (local stdio). GitHub ingress descoped from v0.1 by ADR-0016 |
| Provider distribution | Route-to-chosen-provider and slot-bounded concurrent executions exist; fan-out, health-aware routing, and cross-provider aggregation are design intent only (unimplemented portion of ADR-0018, superseded by ADR-0020) |
| Planner adapters | Documentation/skill documents in `integrations/planners/`; no executable code, no conformance harness yet |
| Release status | Source-buildable only. No published installers, binaries, or packages |

Key decisions recorded: ADR-0015 (local state canonical, bindings optional),
ADR-0016 (MCP-first submission layering; v0.1 local-ingress-only descope),
ADR-0020 (v2 identity and ownership; supersedes ADR-0017 and incompatible
ADR-0018 portions). Archived v1 status: `docs/archive/legacy_v1/status-2026-08.md`.

Implementation packs (archived): `docs/archive/historical_milestones/M8_1_PACK/...`,
`docs/archive/historical_milestones/M9_PACK/...`, `.../M10_PACK/...`.

> Do not "correct" a status table by editing it to match the code, and do not
> mark a phase `DONE` on the strength of a module or test alone. If code and a
> status document disagree, report the drift to the maintainer.

---

## 3. Product Invariants

Do not violate these without an explicit ADR:

1. There is one Relay engine.
2. Planning is external and human-directed by default.
3. Execution crosses an explicit user-controlled handoff boundary.
4. After handoff, bounded implementation repair may be autonomous.
5. Material changes to approved intent require `NEEDS_CLARIFICATION` or `NEEDS_DECISION`.
6. Escalation must produce a structured packet, not a vague question.
7. Relay Core contains no LLM reasoning requirement.
8. Relay Core is provider-agnostic.
9. Provider-specific behavior belongs in stacks, runtime implementations, ingresses, or planner integrations.
10. Relay is not architecturally coupled to Ratatui.
11. Local project configuration is authoritative for filesystem paths and expected Git remotes.
12. Remote tasks cannot provide arbitrary execution paths or arbitrary shell commands.
13. v0.1 requires explicit local Run approval.
14. v0.1 never automatically pushes code.
15. Prefer capability checks over provider-name conditionals.
16. Prefer typed domain models over unstructured maps in core code.
17. Security failures fail closed.
18. Coding runtimes should not be invoked for exploratory planning by default.
19. Relay must never bypass provider quotas, billing, authentication, or usage controls.
20. First-party planner adapters must pass the shared planner-adapter conformance cases before being called first-class.
21. Treat planner host identity separately from model/provider identity when integration behavior differs by product surface.
22. After task acceptance, Relay local durable state is canonical; external bindings such as GitHub Issues are optional and must not replace Relay task identity.
23. Planner adapters prefer Relay Direct when available, then supported provider-native compatibility transports, then local artifact/paste fallback.

---

## 4. Technology Baseline

Use:

- Rust stable, Edition 2024.
- Cargo workspace with resolver 3.
- Tokio for async/process orchestration.
- Ratatui + Crossterm for the first UI.
- Serde for serialization.
- JSON for canonical task/clarification wire formats.
- TOML for local configuration.
- SQLite + SQLx for durable local state.
- clap for command-line flags/subcommands (wired into the binary entry point since M9 phase 01; default invocation still starts the TUI).
- tracing for diagnostics.
- thiserror for library/domain errors.
- anyhow only at application boundaries.

---

## 5. Repository Map

```text
apps/relay-tui/        The terminal UI — first presentation client of Relay Core
crates/
  relay-core/          Domain engine: lifecycle, planning, policy, ingress validation
  relay-protocol/      HandoffTask v1 wire format and validation
  relay-runtime/       Runtime abstraction layer: traits, profiles, router, activity
  relay-store/         SQLite persistence (SqliteStore)
runtimes/
  relay-runtime-codex/       v1 Codex app-server runtime for the legacy Run path
  relay-runtime-antigravity/ Antigravity adapter
  relay-runtime-fake/        Scriptable runtime + fake backplane for tests
  relay-decision-jev/        Optional advisory semantic decision adapter
integrations/planners/  Planner specification + host adapters (documents/skills, no code)
schemas/                Wire format schemas
docs/                   Architecture docs, ADRs, v2 rebaseline pack, archive
contexts/               Project-level context documents
```

### crates/relay-core — the engine (provider-agnostic, LLM-free)

| Module | Responsibility |
|---|---|
| `task_ingress.rs` | Local task intake: preview/validate paste & file imports (size caps, duplicate-key rejection, project resolution, fail-closed). Entry point for ALL transports |
| `execution.rs` | Legacy v1 `ExecutionScheduler`: queueing, global/project concurrency slots, permission broker (scoped grants), bounded repair loop, verification, escalation events, cancellation. Used for Run only with `RELAY_LEGACY_V1_RUN=1`; M6 semantics are FROZEN — additive amendments only |
| `execution_planning.rs` | Legacy v1 `plan_execution` / `build_execution_plan`: turns user *intent* (profile/model/effort preferences) into a validated, clamped plan. The default daemon v2 Run path sends `RunTask`; trusted workspace derives ONLY from registered projects |
| `project_registry.rs` | Trusted local project registry: filesystem paths, expected Git remotes, default stacks. Remote/task input may reference projects by key only — never paths |
| `repository_safety.rs` | Pre-run repository inspection/eligibility checks via git |
| `durable_state.rs` | Stored-task state machine over relay-store; corrupt rows are quarantined, not fatal to listing |
| `run_context.rs` | `TrustedRunContextResolver`: resolves a Run against the registered project, trust classification, and isolated run worktree |
| `orchestrator_v2.rs` | `OrchestratorV2`: drives approved workflows through role attempts, runtime backplane, and typed role results on the default daemon Run path |
| `legacy_adapter.rs` | Adapts accepted task data into the v2 workflow/run contract used by the default `RunTask` path; also provides the v1 compatibility execution target |
| `daemon.rs` | v2 Phase 01 daemon ownership: client protocol types, typed commands, monotonic events, durable effects, and a lease + fencing token. `relay daemon` acquires the store lease, renews a heartbeat, and serves `DaemonClient` requests |
| `daemon_runtime.rs`, `daemon_transport.rs` | Wave A daemon client/lifecycle seam: `DaemonClientHandle`, `run_workflow_driver`, `run_full_daemon_with_workflow_driver`, `connect_or_embed_with_workflow_driver`, and Windows named-pipe transport/framing. TUI and `serve-mcp` embedded daemons attach the v2 workflow driver. Both modules are `#[cfg(windows)]`; see issue #17 |
| v2 domain modules (`workflow`, `policy`, `candidate`, `lease`, `artifact`, `exchange`, `delegation`, `privileged`, `scheduler_v2`, …) | `workflow`, `orchestrator_v2`, and `legacy_adapter` are on the default Run path. `OrchestratorV2` uses `SchedulerV2` for per-run role slots and write leases; multi-project/provider scheduling durability, Exchange/outbox, delegation, and privileged-action enforcement remain partial or outside the production Run path under Waves D/E. Check `ROADMAP.md` before relying on them |

### apps/relay-tui — presentation (must not own policy)

Post-split module layout (was a monolith):

| Module | Contents |
|---|---|
| `main.rs` | Startup and daemon wiring; `production_workflow_driver` composes `LocalProcessBackplane`, `CodexProviderAdapter`, and `TrustedRunContextResolver`, and selects `RunPath` |
| `state.rs` | `AppState` and `RunPath::{DaemonV2, LegacyV1}`; default `submit_selected_task_v2` sends `RunTask` to the daemon |
| `input.rs` | Key handling per screen; Ctrl+C quits globally; Enter prefers exactly-typed commands over suggestions |
| `commands.rs` | Command palette command dispatch (`RelayCommand::execute_relay_command`) |
| `command_completion.rs` | Palette catalog, suggestions, autocomplete eligibility |
| `ui/` | Render functions split per screen group: `dashboard.rs` (escalation inbox FIRST, distribution view second, queue summary below), `tasks.rs`, `execution.rs`, `runtime.rs`, `projects.rs`, `mod.rs` (dispatch + palette overlay) |
| `util.rs` | Shared helpers (status text, preflight strings, distribution rows) |
| `tests.rs` | All TUI tests |

UI language rule: user-facing copy speaks handoff language ("Handoff status", escalations, distribution), never orchestrator/AI-manager framing. Internal identifiers keep scheduler/runtime names unchanged.

Presentation must not own: task execution, ingress logic, project routing, runtime selection, repository validation, persistence policy, repair policy. The default path sends `RunTask` to the daemon; `plan_execution` belongs to the legacy v1 path behind `RELAY_LEGACY_V1_RUN=1`.

### crates/relay-runtime — the seam between core and providers

| Module | Responsibility |
|---|---|
| `lib.rs` | Runtime trait, session/control channels, permission request/grant types (with scope Once/Turn/Session + granted paths), model catalog types, activity types, `RuntimeControl` |
| `integration.rs` | Integration facade used by hosts to resolve profiles/models/capabilities |
| `router.rs` | `RuntimeRouter`: routes each execution to a registered runtime by profile prefix (capability-based dispatch is the goal; name-prefix is the current mechanism) |
| `profile.rs` | Runtime profiles, capability enums (filesystem/network), effective-profile resolution |
| `activity.rs` | Provider-neutral activity normalization |
| `backplane.rs` | `RuntimeBackplane` contract for launching, observing, and controlling runtime processes |
| `local_process_backplane.rs` | `LocalProcessBackplane`, the local process implementation used by the default daemon Run composition |
| `process_tree.rs` | Process-tree containment and identity-aware orphan handling |
| `role_result.rs` | Parses and validates typed published role-result artifacts; process exit alone is insufficient for completion |
| `provider/mod.rs` | Provider adapter contract and exports |
| `provider/codex.rs` | `CodexProviderAdapter` for the v2 local Run path |
| `provider/mock.rs` | Mock provider adapter for deterministic tests |

### Runtimes (edges)

The v2 `CodexProviderAdapter` in `crates/relay-runtime/src/provider/codex.rs` launches one-shot `codex exec`. The separate `runtimes/relay-runtime-codex` app-server stdio JSONL runtime serves the legacy v1 path behind `RELAY_LEGACY_V1_RUN=1`.
- `relay-runtime-fake`: scriptable outcomes for tests; recovery-loop tests depend on it.

---

## 6. How A Run Flows (mental model)

```text
Default daemon v2 Run path (implemented on unmerged draft PRs #31/#32):
paste/file import or `serve-mcp` `submit_handoff` -> TUI review -> explicit Run approval
   -> daemon `RunTask` (approved intent, execution grant, config snapshot, execution target)
   -> `OrchestratorV2` -> `TrustedRunContextResolver` (registered project, trust classification,
      isolated `relay-run-<id>` worktree) -> `LocalProcessBackplane` -> `CodexProviderAdapter`
   -> typed role result -> COMPLETE or typed failure

Legacy v1 Run path (`RELAY_LEGACY_V1_RUN=1` only):
task review -> `plan_execution(intent)` -> `ExecutionScheduler` -> legacy runtime
   -> DONE | FAILED | NEEDS_CLARIFICATION | NEEDS_DECISION (structured Markdown packet)
```

Legacy v1 clarification and decision packets remain an important engine output and surface in the Dashboard ESCALATION INBOX. The current `OrchestratorV2` path completes a workflow or returns a typed failure; it does not emit `NEEDS_CLARIFICATION` or `NEEDS_DECISION` packets.

---

## 7. Architecture Rules

Presentation code must not own: task execution; ingress logic; project routing; runtime selection; repository validation; persistence policy; repair policy.

The TUI calls Relay Core.

Runtime implementations receive validated local execution context.

Ingress implementations move protocol data/status and do not decide implementation strategy.

Planner integrations are clients of Relay; they do not replicate core orchestration.

---

## 8. Recovery / Clarification

Ordinary implementation failures should be repaired automatically within configured bounds.

When the runtime crosses the approved-intent boundary, create a typed Clarification or Decision Request containing enough context for a planner/user to decide.

Executor recommendations are not approvals.

Resolution Packets must be validated before resuming work.

Deep detail: `docs/clarification-protocol.md`.

---

## 9. Process Execution & Security

Never concatenate untrusted text into shell command strings.

Prefer direct process spawning with explicit executable and argument arrays.

Treat runtime output as untrusted display/log content.

Do not log secrets.

Security posture: fail closed everywhere (unknown projects, unsupported extensions, oversized inputs, unknown protocols). Tasks carry no filesystem paths; trusted workspace comes from the registry only.

Deep detail: `docs/security-model.md`.

---

## 10. Where to Look When Doing X

| Task | Read first |
|---|---|
| Anything touching task acceptance/validation | `crates/relay-core/src/task_ingress.rs`, `docs/task-protocol.md`, `docs/local-ingress.md` |
| Scheduler/lifecycle/repair changes | `crates/relay-core/src/execution.rs`; M6 semantics are FROZEN — additive amendments only |
| New user-facing UI copy/screens | `apps/relay-tui/src/ui/`, `docs/tui-ux.md`, ADR-0020 language and ownership rules |
| Runtime/provider work | `crates/relay-runtime/src/lib.rs` (trait + permission/catalog types), relevant runtime crate, `docs/runtime-stack-model.md` |
| Submission transports / MCP | `docs/adr/0016-*.md`, `docs/archive/historical_milestones/M9_PACK/...`, `docs/planner-hosts/` (host registration recipes), `docs/submission-and-bindings.md` |
| Multi-provider distribution / fan-out ideas | `docs/adr/0018-*.md` — fan-out is PLANNED, not built; do not present as shipped |
| Planning surfaces / ChatGPT skill | `integrations/planners/relay-handoff/`, `docs/planner-integrations.md` |
| Persistence/store changes | `crates/relay-store/`, `docs/data-persistence.md`; two-process access (TUI + future serve-mcp) hardening is an M9 gate |
| Roadmap/status questions | `docs/v2_architecture_rebaseline/ROADMAP.md`, `docs/archive/legacy_v1/status-2026-08.md` |

---

## 11. Workflows & Conventions

**Branch/milestone model:** one branch per milestone (`feat/mN-slug`); PRs go through `staging` to `main`. Commit messages follow `type[Jacinth]: imperative summary` (feat/fix/docs/refactor/test/style).

**Quality gates before considering ANY implementation complete:**

- `cargo fmt --check`
- `cargo clippy --workspace --all-targets --all-features`
- `cargo test --workspace`

Add focused integration tests for process boundaries, repository safety, recovery limits, and clarification/resolution flows.

**Testing conventions:** fake runtime for scheduler tests; temp-dir fixtures for store/registry tests; integration tests spawn real binaries for process-boundary work. Do not weaken tests to make refactors pass — move/update assertions deliberately.

**Docs discipline:** honest status labels only — exists today / partial / planned, never aspirational text presented as shipped. Architectural decisions get ADRs (`docs/adr/`, follow 0015–0018 format); non-trivial changes update affected docs in the same change.

**Changes checklist (non-trivial):**
1. identify affected invariant;
2. update/add an ADR;
3. update relevant docs;
4. add tests for the boundary;
5. preserve protocol compatibility unless a version change is intentional.

**Third-party material:** architectural ideas may be independently implemented from public references. Never copy/adapt third-party source or docs without checking license and preserving notices — record material in `THIRD_PARTY_NOTICES.md`, inspirations in `ACKNOWLEDGMENTS.md`.

---

## 12. Next Up (so agents don't guess)

1. **Relay Architecture v2 Rebaseline** — follow the 16-phase sequential implementation plan in `docs/v2_architecture_rebaseline/02-IMPLEMENTATION-SEQUENCE.md`. Per-phase status is tracked authoritatively in `docs/v2_architecture_rebaseline/ROADMAP.md` — read that table for current truth, do not rely on this section for phase status, and do not mark a phase `DONE` here or there until its production exit gate is proven.
2. **Wave A is LANDED** (governed re-review remains deferred). Wave B R12-02 is on draft PR #31; Wave C R12-09, R12-11, the local-process Run path, and TUI cutover are on stacked draft PR #32. Both PRs are unmerged and await the owner's merge decision; the governed re-review is also deferred. This stack is not a promoted production build.
3. Next is **Wave D** (R12-12/13/14). Production Herdr backplane work, R12-10, is split into a separate future run.
4. Residual gaps: Exchange/outbox idempotency, durable delegation and multi-project scheduler recovery, truthful policy enforcement, privileged/failure correctness, no v2 cancel command or per-run model/effort selection, no live execution output or provider-health display, only Codex is wired in the local composition, no real-provider smoke run, no production Herdr transport, and the v1 scheduler remains available behind its opt-in.
5. If committed work on a branch contradicts `ROADMAP.md`, **report the drift to the maintainer**; do not rewrite the status tables yourself.

## 13. Working on this repository as a public project

Relay is prepared for public, experimental hobby/development use. Anyone (including an agent) preparing or reviewing public-facing changes should read:

- `CONTRIBUTING.md` and `docs/DEVELOPMENT.md` — contribution policy and contributor onboarding.
- `docs/PUBLIC_RELEASE_CHECKLIST.md` — the owner-controlled publication gate.
- `SECURITY.md` — private vulnerability reporting (do not disclose in public issues).

Do not change repository visibility, default branch, branch protection, releases, or history as part of an ordinary change; those are owner decisions with their own approval path. Never commit secrets, private logs, `.env` files, local SQLite state, or `.relay/` privileged state.
