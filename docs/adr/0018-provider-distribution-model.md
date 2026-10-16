# ADR-0018: Provider Distribution Model

## Status

Partially superseded by [ADR-0020](0020-relay-architecture-v2.md) for Relay Architecture v2 (2026-09-17). Unimplemented provider fan-out and M10 provider-distribution directions are superseded by the v2 role-based workflow and attempt model. Historical v0.1 record preserved.

## Context

Relay's identity is a deterministic handoff engine ([ADR-0017](0017-relay-identity-deterministic-handoff-engine.md)). The owner's mental model — a manager agent that assigns work to sub-workers, consolidates their output, verifies it, and reports back — applies here to coding-runtime providers: Relay accepts handoffs and hands them to a chosen provider or providers, acting as the deterministic distributor and supervisor.

The word *distribution* covers several distinct capabilities that must not be conflated, because they exist in very different states today:

1. **Routing one execution to one of several registered runtimes by policy.** This exists. `RuntimeRouter` registers multiple providers and resolves the target from each request's `profile_id` prefix (`crates/relay-runtime/src/router.rs`); every queued execution carries its own `ExecutionRunPolicy` frozen by `build_execution_plan` (`crates/relay-core/src/execution_planning.rs`). The production scheduler runs over a router with Codex and Antigravity registered (`apps/relay-tui/src/state.rs`).

2. **Running multiple executions concurrently across different providers.** This exists, bounded by scheduler slots: a global `max_parallel` and a per-project `per_project_max_parallel` (`crates/relay-core/src/execution.rs`, `ExecutionLimits`; tests confirm project slots avoid head-of-line blocking and the global limit holds across projects). The slots are global/project-scoped, not provider-scoped; there is no notion of per-provider capacity.

3. **Fanning out one task to several providers simultaneously.** This does not exist. `submit_with_policy` accepts exactly one policy per task; a task produces at most one execution on one runtime.

4. **Provider health-aware routing.** This does not exist. Routing is a pure profile-prefix lookup that fails closed for unregistered providers; integration probes feed capability/Doctor display only and do not influence routing.

5. **Cross-provider result aggregation.** Partial, display-only. Reports are per-execution (`ExecutionReport`); the TUI's DISTRIBUTION section counts running/completed work per profile from durable records but aggregates nothing beyond presentation.

This ADR separates what Relay does today from where its orchestration posture points, so aspirational text is never mistaken for shipped behavior.

## Decision

Relay's orchestration posture is **deterministic provider distribution**:

- Handoffs are routed to chosen runtime(s) via `RuntimeRouter` under `ExecutionRunPolicy` produced by `build_execution_plan`. Selection is deterministic and human/planner-supplied through approved intent; Relay never picks a provider by judgment.
- Concurrency across providers is bounded by global and per-project execution slots. Slot policy remains global/project-scoped; per-provider slot accounting may be added later as capability metadata, not as core decision-making authority.
- Recorded as design intent, not built:
  - fan-out of one task to multiple providers (for comparison or redundancy);
  - health-aware routing based on probe/capability results;
  - cross-provider result aggregation into a consolidated verdict.

These intents require explicit design and milestones before any code claims them. Until then, no documentation, UI text, or protocol surface may present them as working features.

Boundary conditions carried forward unchanged:

- Distribution changes *where* approved work runs, never *what* is approved. Fan-out would multiply execution of the same approved intent; it would not widen intent.
- All routing stays capability-based where facts exist (invariant 15): the stack allowlist seam in `execution_planning.rs` is scheduled for replacement by capability checks, and future health-aware routing must key on capabilities/probes rather than provider names.
- Relay Core gains data (provider health snapshots, per-provider slot metadata), not reasoning authority (invariants 1 and 7; [ADR-0017]).
- Remote-originated tasks still cannot select arbitrary execution paths; provider choice arrives only through validated handoff fields and local configuration (invariant 12).
- Every execution still crosses the local Run approval boundary (invariant 13); distribution happens after approval, never instead of it.
- Coding runtimes remain implementation executors, never exploratory planners (invariant 18).

This ADR elaborates invariants 1–23 and refines [ADR-0016](0016-mcp-first-submission-and-v0-1-local-descope.md) (submission layering is unaffected) without changing any invariant.

## Consequences

- "Distribute to multiple providers" is accurate product language only for routing among registered runtimes plus bounded concurrency; it is not accurate for fan-out until fan-out ships.
- Per-provider capacity, fan-out semantics (identical task vs. partitioned subtasks), and aggregation verdicts are open design questions that must each get their own ADR or milestone pack before implementation.
- The scheduler's slot model may grow provider-scoped limits; this is additive to `ExecutionLimits` and must fail closed like existing limit validation.
- Health-aware routing, if built, must consume plain-data probe/capability snapshots supplied to core, consistent with the `ExecutionPlanEnvironment` pattern.
- Documentation should reference this ADR whenever Relay's manager-like role is described, to keep exists-today and design-intent clearly separated.
