# ADR-0001: Rust Core

Status: Accepted

## Context

Relay should remain lightweight, cross-platform, safe around process/concurrency boundaries, and reusable by future TUI/desktop/server clients.

## Decision

Build Relay Core in Rust stable using Edition 2024.

## Consequences

Positive:

- native executable;
- strong type/concurrency guarantees;
- suitable for long-lived local tooling;
- reusable by multiple Rust-hosted frontends.

Cost:

- steeper learning curve;
- slower early development while contributors learn ownership/borrowing.

The learning cost is accepted in exchange for long-term foundation quality.
