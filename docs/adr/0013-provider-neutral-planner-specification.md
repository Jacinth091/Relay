# ADR-0013: Provider-Neutral Planner Specification

Status: Accepted

## Context

The first Relay planner integration was packaged as a ChatGPT Skill.

Treating that package as the canonical integration would unnecessarily couple Relay's planner-side behavior to one provider's packaging format.

Other providers may use Skills, instruction files, agent definitions, extensions, plugins, MCP, or other mechanisms.

## Decision

Define a provider-neutral **Relay Handoff Planner Specification** as the source of truth.

Implement provider-native adapters that package those semantics for each supported planning provider.

The ChatGPT Skill becomes the first adapter, not the canonical specification.

## Consequences

- Relay handoff semantics remain stable across providers.
- Providers can use native installation/integration mechanisms.
- New providers can be added without changing Relay Core.
- Adapter packaging can evolve independently of the task protocol.
- The project must avoid claiming an adapter is installable until its provider's current integration mechanism is verified.


Foundation v1.5+ clarifies that the neutral specification is paired with concrete host-native adapters and shared conformance cases; see ADR-0014.
