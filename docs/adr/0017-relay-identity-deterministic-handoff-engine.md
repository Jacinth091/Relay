# ADR-0017: Relay Identity — Deterministic Handoff Engine

## Status

Superseded by [ADR-0020](0020-relay-architecture-v2.md) for Relay Architecture v2 (2026-09-17). Historical v0.1 record preserved.

## Context

As Relay gained runtimes, a permission broker, model catalogs, and planned transports (MCP), the question arose whether Relay is an "orchestrator", "an MCP server", or something broader. The word *orchestrator* covers two unrelated activities, and the ambiguity invites scope creep in both directions: adding intent-level reasoning to core, or mistaking a transport edge for the product.

## Decision

Relay is a **deterministic handoff engine**. It performs execution orchestration only:

- validate, route, persist, spawn runtime processes, stream events, bounded repair, structured escalation.

Relay does not perform intent orchestration — no planning, reasoning, or strategy selection. Those belong to the human and planner surfaces (invariants 3 and 7).

Transports and presentation surfaces (TUI, MCP server, GitHub ingress, paste/file) are replaceable doors around one core; none of them is what Relay *is*.

Classification test for any proposed capability:

1. deterministic policy/lifecycle/persistence/validation → Relay Core;
2. data movement to/from external hosts/providers → edge crate;
3. intent or strategy decisions → not part of Relay.

Scope growth widens the *data* available to core (capability metadata) rather than core's *authority*.

This statement is documented normatively in [docs/architecture.md](../architecture.md) ("What Relay Is / Is Not") and elaborates invariants 1 and 7 without changing them.

## Consequences

- New surfaces are evaluated as doors, never as second engines.
- Provider/runtime abstraction work should add capability facts to profiles rather than decision-making power to core.
- Proposals that blur identity (e.g., autonomous planning agents inside Relay, hosted multi-user services) require revisiting this ADR first.
