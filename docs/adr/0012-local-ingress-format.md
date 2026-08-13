# ADR-0012: Local Ingress Uses Canonical JSON with Optional Markdown Wrapper

Status: Accepted

## Context

Relay needs an offline/no-network ingress that is convenient for humans and deterministic for machines.

Markdown is readable but ambiguous if Relay must infer semantics from prose. JSON is straightforward to validate but less pleasant as a human document.

## Decision

Use canonical Relay JSON for local machine input.

Support an optional Markdown wrapper that embeds the identical canonical JSON block between Relay markers.

The TUI supports both file import and direct paste.

## Consequences

- one semantic protocol across GitHub, local, paste, and future ingresses;
- deterministic JSON Schema validation;
- readable Markdown artifacts remain possible;
- planners can explicitly generate local Relay files without GitHub;
- importing a task never bypasses the normal review/Run boundary.
