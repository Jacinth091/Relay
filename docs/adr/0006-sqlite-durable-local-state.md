# ADR-0006: SQLite Durable Local State

Status: Accepted

## Context

Task and execution history must survive restarts without requiring a database server.

## Decision

Use embedded SQLite through SQLx for durable application state.

Keep local project routing configuration in TOML.

## Consequences

Relay gains reliable history/migrations with minimal runtime infrastructure.

High-volume raw runtime output should not be stored without bounds.
