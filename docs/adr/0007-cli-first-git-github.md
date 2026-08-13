# ADR-0007: CLI-First Git and GitHub Integration

Status: Accepted for initial implementation

## Context

The v0.1 reference workflow needs Git repository validation and a GitHub Issues compatibility path; Relay may also use GitHub later as an optional external binding.

Embedding authentication and reimplementing Git behavior adds unnecessary v0.1 complexity.

## Decision

Use installed `git` and `gh` executables initially.

Spawn them directly with explicit argument arrays.

## Consequences

Relay reuses the user's existing configuration/authentication.

Native integrations may be added later behind the same boundaries.
