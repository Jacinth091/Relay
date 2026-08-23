# Data and Persistence

## Decision

Use embedded SQLite for durable Relay state.

Relay does not require a database server.

After Relay accepts a task, the local Relay task record is canonical. External systems are optional bindings, not the source of truth.

## Why Durable State Matters

Relay should survive:

- application restart;
- terminal closure;
- runtime failure;
- ingress reconnect;
- external binding outage;
- machine reboot;
- clarification/decision cycles.

Task history must not depend on conversational memory or continued GitHub availability.

## Data Categories

### Projects

Project configuration remains in local TOML.

The database may cache project health metadata but TOML is authoritative for routing.

### Tasks

Store:

- internal Relay task ID;
- submission transport/provenance;
- source identity/metadata;
- project key;
- title;
- validated protocol payload or normalized task data;
- state;
- timestamps;
- requested stack.

### External Bindings

Store optional remote associations separately from task provenance:

- binding ID;
- Relay task ID;
- binding kind, such as `github-issue`;
- remote identity/reference;
- synchronization status;
- last sync/error metadata where useful;
- creation/update timestamps.

A task may have no external binding.

Creating a GitHub binding for a locally pasted task must not change its original submission transport.

### Executions

Store:

- execution ID;
- task ID;
- runtime profile;
- start/end timestamps;
- execution phase;
- repair attempt count;
- exit outcome;
- failure category;
- repository snapshot metadata where useful.

### Clarification / Decision Requests

Store:

- request ID;
- task/execution ID;
- request kind;
- category;
- structured packet;
- creation/resolution timestamps;
- publication/binding metadata when relevant.

### Resolutions

Store:

- resolution ID;
- request ID;
- structured resolution;
- work disposition;
- resume/restart outcome.

### Events

Store only durable events useful for recovery/history.

Do not persist every high-volume stdout chunk indefinitely.

## Output Retention

Runtime output can grow quickly.

Recommended approach:

- live output: bounded in-memory ring buffer;
- recent execution log: optional local file;
- SQLite: summary and important event metadata.

## Schema Evolution

Use explicit database migrations from the beginning.

Database schema version and Relay protocol versions are separate.

## Multi-Process Access

Relay supports concurrent access to the same local database file from exactly
two writer classes: the TUI and a future `relay serve-mcp` process. Anything
more (additional writer processes, networked/shared databases) is out of scope
until re-examined.

Every Relay connection sets:

- WAL journal mode, so readers never block writers and vice versa;
- a 5000 ms busy timeout, so short cross-process write contention resolves by
  waiting rather than failing;
- foreign key enforcement.

Write paths additionally retry transient `SQLITE_BUSY`/`SQLITE_LOCKED` errors
with bounded exponential backoff (5 attempts). If contention persists beyond
the retry budget, the write fails with a typed busy error instead of hanging or
corrupting state. Concurrent writes to the same task row are arbitrated by
compare-and-set state transitions: exactly one writer wins; the loser receives
a typed conflict error.

A second process that opens a database written by an incompatible Relay build
(unknown migration version, modified applied migration, or partially applied /
dirty migration) fails closed at open time with a typed schema-version error.
It does not crash, migrate blindly, or continue silently.

## Current Implementation

M5 initializes Relay's embedded SQLite store through SQLx migrations owned by
the `relay-store` crate in `crates/relay-store/migrations`.

Accepted local file and paste imports are persisted as canonical `ready` task
records before the TUI enters task review. Relay Core converts validated domain
objects into store DTOs, and validates stored DTOs back into domain objects on
load. The validated `HandoffTask v1` payload is stored with Relay-owned task
identity, submission transport, project key, task state, source metadata, and
timestamps.

Task creation and its initial `task_ready` durable event are written in one
transaction. Task state updates use compare-and-set storage semantics surfaced
through a typed Core adapter, preserving stale-state conflicts for later
orchestration policy. Reload validates stored canonical JSON through the
protocol parser and rejects project/title metadata drift.

On restart, the TUI lists durable tasks with task ID, title, project, state,
submission transport, and loaded task content. The M5 task view is read-only
and exposes no run action.

The initial schema also creates tables for optional external bindings,
executions, clarification/decision requests, resolutions, and durable events so
later milestones can add lifecycle behavior without replacing the canonical
task store.

## Export / Backup

A future export command may package:

- sanitized config;
- task history;
- submission provenance;
- external binding metadata;
- execution metadata;
- clarification/resolution history;
- selected logs.

Secrets and local paths should be optionally redacted.
