# ADR-0003: Human-Directed Planning

Status: Accepted

## Context

Adjacent development orchestrators may perform autonomous planning and continuous agent loops.

Relay is motivated by a different desired interaction.

## Decision

External human-directed conversational planning is the default.

Relay begins at the explicit handoff boundary.

Autonomous planning may exist later as an optional stack, never as a required core behavior.

## Consequences

The product remains useful to developers who want to participate in design decisions and preserve coding-runtime usage for implementation.
