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

## 2. Current Status Snapshot (2026-08)

Milestone ladder actually tracked by development: M2 registry → M3 repo safety → M4 task semantics → M5 SQLite → M6 scheduler/recovery → M7 run approval UX → M8 Codex runtime → M8.1 multi-runtime orchestration/UX.

| Item | State |
|---|---|
| M2–M7 | Done, merged to main |
| **M8 / M8.1** | **Done on `feat/m8-codex-runtime`, pending merge** (branch is ahead of main by ~40 commits) |
| **M9 (Relay Direct/MCP)** | **Done on `feat/m9-relay-direct-mcp`, pending merge** — clap CLI entry point, `relay serve-mcp` MCP stdio server (`submit_handoff`), store concurrency hardening, conformance tests + host docs |
| Working end-to-end path today | Local paste/file import → TUI review → explicit Run approval → Codex (`app-server` transport default) → events/diff/logs/history; plus MCP submission via `relay serve-mcp` on the M9 branch |
| Ingress reality | Local file/paste plus `relay serve-mcp` (M9 branch). GitHub ingress descoped from v0.1 by ADR-0016 |
| Provider distribution | Route-to-chosen-provider and slot-bounded concurrent executions exist; fan-out to multiple providers, health-aware routing, cross-provider aggregation are design intent only (ADR-0018) |
| Planner adapters | Documentation/skill documents in `integrations/planners/`; no executable code, no conformance harness yet |

Key decisions recorded: ADR-0015 (local state canonical, bindings optional), ADR-0016 (MCP-first submission layering; v0.1 local-ingress-only descope), ADR-0017 (identity), ADR-0018 (deterministic provider-distribution model). Point-in-time status: `docs/archive/legacy_v1/status-2026-08.md`.

Implementation packs (archived): `docs/archive/historical_milestones/M8_1_PACK/...`, `docs/archive/historical_milestones/M9_PACK/...`.

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
  relay-runtime-codex/       Codex adapter (app-server stdio JSONL default; exec fallback)
  relay-runtime-antigravity/ Antigravity adapter
  relay-runtime-fake/        Scriptable runtime for tests
integrations/planners/  Planner specification + host adapters (documents/skills, no code)
schemas/                Wire format schemas
docs/                   Architecture docs, ADRs, implementation packs
contexts/               Project-level context documents
```

### crates/relay-core — the engine (provider-agnostic, LLM-free)

| Module | Responsibility |
|---|---|
| `task_ingress.rs` | Local task intake: preview/validate paste & file imports (size caps, duplicate-key rejection, project resolution, fail-closed). Entry point for ALL transports — MCP will reuse this path |
| `execution.rs` | `ExecutionScheduler`: queueing, global/project concurrency slots, permission broker (scoped grants), bounded repair loop, verification, escalation events, cancellation. Also `ExecutionRunPolicy`, limits, reports |
| `execution_planning.rs` | `plan_execution` / `build_execution_plan`: turns user *intent* (profile/model/effort preferences) into a validated, clamped plan. Owns stack allowlist, cross-provider model-leakage rejection, clamps duration/repair bounds. Trusted workspace derives ONLY from registered projects |
| `project_registry.rs` | Trusted local project registry: filesystem paths, expected Git remotes, default stacks. Remote/task input may reference projects by key only — never paths |
| `repository_safety.rs` | Pre-run repository inspection/eligibility checks via git |
| `durable_state.rs` | Stored-task state machine over relay-store; corrupt rows are quarantined, not fatal to listing |

### apps/relay-tui — presentation (must not own policy)

Post-split module layout (was a monolith):

| Module | Contents |
|---|---|
| `main.rs` (~140 lines) | Startup, store opening, event loop wiring |
| `state.rs` | `AppState` (all UI state incl. preflight cache, permission queue), health probing, router construction |
| `input.rs` | Key handling per screen; Ctrl+C quits globally; Enter prefers exactly-typed commands over suggestions |
| `commands.rs` | Command palette command dispatch (`RelayCommand::execute_relay_command`) |
| `command_completion.rs` | Palette catalog, suggestions, autocomplete eligibility |
| `ui/` | Render functions split per screen group: `dashboard.rs` (escalation inbox FIRST, distribution view second, queue summary below), `tasks.rs`, `execution.rs`, `runtime.rs`, `projects.rs`, `mod.rs` (dispatch + palette overlay) |
| `util.rs` | Shared helpers (status text, preflight strings, distribution rows) |
| `tests.rs` | All TUI tests |

UI language rule: user-facing copy speaks handoff language ("Handoff status", escalations, distribution), never orchestrator/AI-manager framing. Internal identifiers keep scheduler/runtime names unchanged.

Presentation must not own: task execution, ingress logic, project routing, runtime selection, repository validation, persistence policy, repair policy. It gathers **intent** and calls `plan_execution`; core owns everything downstream.

### crates/relay-runtime — the seam between core and providers

| Module | Responsibility |
|---|---|
| `lib.rs` | Runtime trait, session/control channels, permission request/grant types (with scope Once/Turn/Session + granted paths), model catalog types, activity types, `RuntimeControl` |
| `integration.rs` | Integration facade used by hosts to resolve profiles/models/capabilities |
| `router.rs` | `RuntimeRouter`: routes each execution to a registered runtime by profile prefix (capability-based dispatch is the goal; name-prefix is the current mechanism) |
| `profile.rs` | Runtime profiles, capability enums (filesystem/network), effective-profile resolution |
| `activity.rs` | Provider-neutral activity normalization |

### Runtimes (edges)

Each implements the Runtime trait against its provider's local CLI:
- `relay-runtime-codex`: `codex app-server` over stdio JSONL is the DEFAULT transport (`RELAY_CODEX_TRANSPORT=exec` opts out); handles turn/start with model+reasoning, paginated model/list catalogs, real approval-request parsing, process-tree containment (Windows job objects/taskkill).
- `relay-runtime-fake`: scriptable outcomes for tests; recovery-loop tests depend on it.

---

## 6. How A Run Flows (mental model)

```text
paste/file import -> protocol validate -> preview -> human confirms import
   -> task READY (SQLite, canonical)
review screen (cached preflight) -> explicit Run approval (invariant 13)
   -> plan_execution(intent) -> validated/clamped ExecutionRunPolicy
   -> scheduler queue (global/project slots) -> RuntimeRouter -> runtime process
   -> events: output chunks, activity, permission requests (TUI popup grants scoped)
   -> bounded auto-repair within approved scope
   -> DONE | FAILED | NEEDS_CLARIFICATION | NEEDS_DECISION (structured Markdown packet)
```

Escalation packets are the engine's most important output; they surface in the Dashboard ESCALATION INBOX.

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
| New user-facing UI copy/screens | `apps/relay-tui/src/ui/`, `docs/tui-ux.md`, ADR-0017 language rules |
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
2. A governed external review found PR #12 could not merge: v2 modules existed but were not wired into the production path and most v2 state was memory-only. The correction plan is tracked wave-by-wave in `docs/v2_architecture_rebaseline/REMEDIATION-WAVES.md`. **Wave A (in progress)**: canonical daemon ownership actually wired into TUI/MCP, crash-safe command/effect idempotency, and durable state for the paths that wave touches.
3. Do not start Wave B work (Run-path cutover to the v2 orchestrator, trust/workflow/policy/digest/lease corrections) until Wave A is accepted — see the remediation waves doc for the full ordering and dependency reasoning.
