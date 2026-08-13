# ADR-0004: Provider-Agnostic Core, Provider-Native Stacks

Status: Accepted

## Context

A lowest-common-denominator runtime abstraction can prevent optimized use of provider-specific capabilities.

Hardcoding provider behavior into core destroys adaptability.

## Decision

Keep the core provider-agnostic.

Allow runtimes/stacks to use provider-native features and namespaced options.

Use capability checks in shared behavior.

## Consequences

Codex support can be excellent without making Relay an OpenAI-only application.

A second provider will validate and refine the shared abstractions.
