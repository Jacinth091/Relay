# M10 Phase 01 — Capability Metadata and Health-Aware Routing

## Goal

`RuntimeRouter` dispatches each execution to a runtime that satisfies the policy's capability/profile requirements, using registered capability metadata plus plain-data health snapshots. When no registered runtime satisfies the policy, routing fails closed with a typed error.

## Current state

Exists today:

- `RuntimeRouter` (`crates/relay-runtime/src/router.rs`) resolves a provider by splitting the request's `profile_id` on `/` and looking up the prefix in a registration map. Unknown providers fail closed (`RuntimeError::StartFailed`).
- Profiles already carry capability facts: `RuntimeProfile` includes `provider`, `filesystem`, `network`, `extra_writable_roots`, `max_duration`, `max_repair_attempts` (`crates/relay-runtime/src/profile.rs`); `RuntimeIntegration::resolve_profile` and `verify_capabilities` produce per-profile effective snapshots.
- `build_execution_plan` (`crates/relay-core/src/execution_planning.rs`) freezes one profile per run and validates model/reasoning against host-supplied snapshots; the stack check is an explicit allowlist (`SUPPORTED_STACKS`) — the documented seam awaiting capability metadata (invariant 15).
- Health probing exists as display-only TUI behavior: `probe_version` / `collect_health` / `ToolHealth` in `apps/relay-tui/src/state.rs` and `RuntimeIntegration::probe`. Probes never influence routing (ADR-0018, item 4).

Does not exist:

- Any routing decision keyed on capabilities rather than the provider-name prefix.
- Any path from probe results into core routing decisions.

## Requirements

1. **Capability facts live with profiles, not names.** At registration time, the router derives a per-provider capability summary from its integration's declared profiles (`RuntimeIntegration::profiles` / `resolve_profile`). Dispatch matches the *requested profile id against resolved profiles and their capabilities*, not the string prefix. The name-prefix path may remain as a deterministic fallback only where exactly one registered provider declares the requested profile; it must be exercised by tests so the seam's retirement is deliberate.
2. **Health enters core as plain data.** Following the `ExecutionPlanEnvironment` pattern, hosts supply a health snapshot map (provider id -> installed/authenticated/version/compatible facts) to planning. The snapshot shape belongs in `crates/relay-runtime` (data types), the snapshot *population* stays at the edge (TUI probes today). Core never spawns probes itself.
3. **Deterministic selection, no judgment.** If approved intent names one provider, routing goes there or fails closed — degraded health of the named provider surfaces as a typed preflight/planning error, never silent rerouting. Only intent that explicitly requests a preference *set* (for example "any healthy provider satisfying this stack") consults health, and then selection follows a fixed order derived from local configuration. Relay never picks by assessment (ADR-0018 decision; invariant 15).
4. **Fail closed when nothing satisfies the policy.** A new typed error (planning or preflight layer) states which requirements no registered runtime met. No default-provider fallback for unsatisfiable policies.
5. **Stack allowlist replacement begins here.** With capability metadata available at the router, `SUPPORTED_STACKS` validation moves toward checking whether any registered integration resolves a profile whose provider satisfies the task/project stack field. Keep the allowlist semantics during migration; remove it only when every production stack is expressible via metadata.
6. **Tests:** router dispatch by capability/profile metadata (including a two-provider case where both declare overlapping profiles); fail-closed on unsatisfiable policy; health-snapshot filtering determinism; regression test that single-provider resolution behaves identically to today.

## Boundaries

- No fan-out behavior in this phase; one execution still maps to at most one runtime.
- Presentation keeps rendering probe results as Doctor-style display; it does not compute routes.
- Scheduler slot logic untouched.

## Done when

- A request naming a profile resolves through metadata even when the provider id cannot be guessed from the profile prefix.
- An unsatisfiable policy produces a typed fail-closed error before any process spawn.
- Full workspace gates pass; phase 02 builds on this seam without re-touching dispatch rules.
