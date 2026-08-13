# ADR-0005: External-Process Extension Direction

Status: Accepted Direction; wire protocol deferred

## Context

Long-term community runtimes/ingresses should be cross-platform and should not require contributors to use Rust.

## Decision

Prefer out-of-process community extensions over native dynamic-library plugins.

Do not freeze the exact extension wire protocol until a real external extension is implemented.

## Consequences

Potential extensions may be written in Rust, Go, TypeScript, Python, C#, or other languages.

The process boundary provides isolation but requires protocol/version design later.
