# ADR-0014: Host-Native Planner Adapters with Shared Conformance

Status: Accepted

## Context

A provider-neutral planner specification alone is not enough for good user experience. Different planner hosts expose different Skill directories, upload flows, connectors, MCP tools, and invocation behavior.

Making every provider use a generic instruction file would sacrifice native capabilities. Allowing each adapter to redefine the task contract would fragment Relay.

## Decision

Keep one provider-neutral Relay Handoff Planner Specification and ship host-native first-party adapters.

Track support per **planner host**, not only model/provider identity.

Each first-party adapter must:

- use the host's documented native integration mechanism;
- document installation and supported ingress behavior;
- preserve canonical Relay protocol semantics;
- support local handoff as a fallback;
- report remote submission truthfully;
- pass shared planner-adapter conformance tests before being promoted to first-class.

## Consequences

Relay can exploit native provider features without changing Relay Core.

New planner hosts can be added independently.

Provider-specific integration drift is isolated at the adapter edge and can be re-verified without changing task semantics.
