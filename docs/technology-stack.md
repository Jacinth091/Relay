# Technology Stack

Status: **Foundation v1 decision**

Relay optimizes for:

- long-term code quality;
- low idle/resource overhead;
- safe process orchestration;
- cross-platform distribution;
- future desktop/server clients;
- provider/runtime extensibility;
- strong compile-time guarantees.

---

## Core Language: Rust

**Decision:** Rust stable, Edition 2024.

Why:

- native binaries;
- no mandatory garbage-collected runtime;
- strong type system;
- explicit ownership and concurrency semantics;
- suitable for process orchestration and long-lived local tools;
- reusable core for TUI, desktop, and server clients.

The steeper learning curve is accepted because Relay is intended as a durable application rather than a disposable script.

### Workspace

Use a Cargo workspace with resolver 3.

Initial target layout:

```text
apps/relay-tui
crates/relay-core
crates/relay-protocol
crates/relay-store
crates/relay-runtime
crates/relay-ingress
runtimes/relay-runtime-codex
ingresses/relay-ingress-github
```

Do not split trivial code into crates before the boundary has real value.

---

## Async / Process Runtime: Tokio

Use Tokio for:

- child process execution;
- stdout/stderr streaming;
- cancellation;
- timers;
- background ingress polling;
- event channels;
- future local server networking.

Coding runtime processes should be spawned through explicit executable/argument APIs, never by interpolating untrusted data into shell strings.

---

## First UI: Ratatui + Crossterm

Ratatui is the first presentation client.

Crossterm provides cross-platform terminal input/output support.

The TUI should use domain events and core commands.

Ratatui types must not appear in Relay Core public APIs.

---

## Serialization: Serde

Use Serde for typed serialization/deserialization.

### Task protocol

Canonical wire format: **JSON**. The local ingress accepts this JSON directly as a file or pasted payload; Markdown is an optional readable wrapper containing the canonical JSON block.

Reasons:

- deterministic machine format;
- strong tooling support;
- easy schema validation;
- portable across languages;
- avoids YAML-specific parsing/ambiguity concerns;
- suitable for future external-process protocols.

A human-facing transport/binding such as GitHub Issues may wrap the canonical JSON in Markdown.

### Local configuration

Use **TOML**.

Reasons:

- readable;
- appropriate for local application configuration;
- familiar in the Rust ecosystem;
- supports strongly typed deserialization through Serde.

---

## Persistence: SQLite + SQLx

Use embedded SQLite for durable local state.

Use SQLx with Tokio for async access.

Store:

- imported task metadata;
- task state;
- execution attempts;
- timestamps;
- runtime/profile identity;
- durable execution summaries;
- ingress/source identifiers;
- clarification/failure metadata.

Do not store entire provider transcripts by default.

Large runtime output should use bounded retention or log files rather than creating an ever-growing database.

No external database server is required.

---

## CLI / Startup Interface: clap

Relay's normal UX is interactive:

```text
relay
```

clap should support lightweight operational commands such as:

```text
relay
relay doctor
relay version
relay config
relay import <file>
```

Do not make the normal product depend on users remembering a large command vocabulary.

---

## Diagnostics: tracing

Use structured tracing for application diagnostics.

Default logs should avoid:

- secrets;
- auth tokens;
- complete task bodies;
- complete runtime output.

Debug modes may increase verbosity with explicit user awareness.

Use tracing spans around:

- ingress polling;
- repository validation;
- runtime preflight;
- process execution;
- persistence operations.

---

## Error Strategy

Use `thiserror` for stable library/domain error types.

Use `anyhow` only near application boundaries for contextual error aggregation where a stable typed API is not required.

Core state transitions should return explicit domain errors rather than opaque strings.

---

## Git Integration

Initial implementation: invoke the user's installed `git` executable.

Why:

- matches the user's actual Git configuration;
- avoids reimplementing credentials and edge cases;
- predictable behavior across existing repositories;
- easy to inspect and debug.

Use explicit arguments.

Do not use shell command concatenation.

Possible later optimization: a native read-only Git library may be evaluated for high-frequency status operations, but it is not required for v0.1.

---

## GitHub Integration

Initial implementation: invoke `gh`.

Why:

- GitHub CLI already owns authentication;
- no OAuth application is required for Relay v0.1;
- avoids storing GitHub tokens;
- easy to inspect manually;
- supports private repositories when the user is authorized.

Long-term, a native GitHub API ingress may be added without changing Relay Core.

---

## First Coding Runtime

Codex CLI.

The runtime implementation owns:

- availability/version detection;
- authentication health reporting where discoverable;
- provider-native invocation options;
- stdin/task preparation;
- stdout/stderr parsing;
- exit handling;
- cancellation;
- supported capability declaration.

Relay Core does not build Codex command lines.

---

## Future Desktop Client: Tauri 2

Tauri is a future option, not a v0.1 dependency.

A future desktop client should consume the same Relay Core.

Frontend technology can be chosen later.

---

## Future Local Server: Axum

Axum is the preferred future Rust HTTP layer if a local API/browser UI becomes useful.

Potential use:

```text
relay serve
    ->
localhost API / event stream
    ->
browser client
```

Do not add a server to the default runtime before there is a user need.

---

## Future Extension Boundary

Community runtime/ingress extensions should eventually be external processes with a versioned protocol.

Do not use Rust dynamic plugins as the primary public extension mechanism.

Built-in Rust integrations remain appropriate for first-party runtimes.

---

## Dependency Policy

Prefer:

- mature crates;
- small dependency surfaces;
- maintained projects;
- default-safe configurations.

Before adding a dependency:

1. identify the requirement;
2. confirm standard library/current dependency cannot solve it cleanly;
3. inspect maintenance/security posture;
4. keep feature flags narrow;
5. update dependency documentation when architecturally important.

`Cargo.lock` should be committed for the application workspace.

---

## Quality Tooling

Required local checks:

```text
cargo fmt --check
cargo clippy --workspace --all-targets --all-features
cargo test --workspace
```

Later public-release checks may add:

- dependency auditing;
- license policy;
- release builds for Windows/Linux/macOS;
- reproducible packaging.
