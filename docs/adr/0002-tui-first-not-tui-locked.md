# ADR-0002: TUI First, Not TUI Locked

Status: Accepted

## Context

Relay should begin as an interactive terminal application, but future desktop or browser clients may become desirable.

## Decision

Ratatui is the first presentation client.

Relay Core must not depend on Ratatui or terminal concepts.

## Consequences

A future Tauri desktop or Axum-backed browser client can reuse core execution logic.

Presentation clients may evolve independently.
