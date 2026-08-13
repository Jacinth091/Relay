# ADR-0009: Single Relay Engine and Ingress Model

Status: Accepted; submission/binding terminology refined by ADR-0015

## Context

Early diagrams could be read as if planner-side handoff logic and the local orchestrator were separate Relay implementations.

That would create unnecessary conceptual and technical duplication.

## Decision

Relay is one engine.

Planner integrations are clients of Relay.

GitHub Issues, local files, and direct/MCP interfaces are receiving paths through which protocol objects reach the same Relay Core. ADR-0015 further separates planner submission transport from optional external bindings.

Presentation clients such as the TUI are also clients of that same core.

## Consequences

- ChatGPT-specific logic remains thin.
- GitHub is an implementation detail of the v0.1 compatibility path, not the product identity or canonical task store.
- Future MCP/direct integrations do not require replacing Relay Core.
- Desktop/web clients can reuse the same engine.
