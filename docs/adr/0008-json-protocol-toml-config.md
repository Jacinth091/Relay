# ADR-0008: JSON Task Protocol and TOML Local Configuration

Status: Accepted

## Context

Relay needs a cross-language canonical handoff format and a human-editable local configuration format.

## Decision

Use JSON as the canonical handoff wire format.

Use TOML for trusted local Relay configuration.

## Consequences

Ingress renderings can remain human-friendly while preserving a deterministic machine payload.

Local paths/policies remain separate from remote task data.
