# ADR-0015: Canonical Local State and Optional External Bindings

## Status

Accepted for Foundation v1.6.

## Context

Relay's first practical planner-to-Relay path uses a planner host that can create a GitHub Issue. That is useful for v0.1, but other planner hosts may not have GitHub write access.

A future direct Relay submission interface such as MCP also makes it unnecessary to require a GitHub Issue for every handoff.

If GitHub remained the canonical task store, Relay would be unnecessarily coupled to one external service even when the planner can submit directly to Relay.

## Decision

After a task is accepted, Relay's durable local task record is canonical.

Separate:

- **submission transport** — how the task reached Relay; and
- **external binding** — an optional remote record synchronized with the Relay task.

Prefer direct Relay submission when a planner host supports it. Provider-native GitHub submission remains a compatibility path. Local file/paste remains the universal fallback.

GitHub Issues are optional external bindings once direct submission exists.

## Consequences

- Relay can operate without GitHub after direct submission is available.
- GitHub remains useful for remote history and collaboration.
- Planner hosts do not all need GitHub access.
- Relay owns ongoing GitHub synchronization using its own configured GitHub integration.
- Relay task IDs remain independent of GitHub Issue numbers.
- The persistence model must retain submission provenance separately from external bindings.
