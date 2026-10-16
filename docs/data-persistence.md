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

> **v2 status (2026-10-16):** this section previously described the current
> (v1/M9) topology as final. It is superseded by the Relay Architecture v2
> daemon model (ADR-0020), and the Wave A cutover has landed: the daemon is the
> sole canonical semantic writer and TUI/`relay serve-mcp` submit durable
> mutations through it (see `docs/v2_architecture_rebaseline/ROADMAP.md`
> Phase 01 and `REMEDIATION-WAVES.md`). The v1 description below remains
> accurate for the parts of Relay that have not yet been cut over (notably the
> v1 Run/scheduler path — Wave B, R12-02), but it is no longer the target
> end-state for task submission/mutation.

Historically (v1/M9), Relay supported concurrent access to the same local
database file from exactly two writer classes: the TUI and the
`relay serve-mcp` process. That two-process behavior was proven by tests:
interleaved writes across two store handles on one database file, busy-timeout
retry under contention, and compare-and-set loser rejection (M9 phase 02). It
does not claim anything beyond what those tests cover.

Under Relay Architecture v2, the **daemon (`relay daemon`) is the sole
canonical semantic writer**. TUI and `relay serve-mcp` are `DaemonClient`
consumers for durable mutations (submitting commands such as
`SubmitHandoff`) and keep local read-only `SqliteStore` handles only for
reading snapshots/history. Direct production writes from TUI/MCP were removed
by Wave A, which has landed. The one known, intentional exception is the v1
`ExecutionScheduler` Run path (task execution state), which still writes
directly to the store pending the Wave B v2 orchestrator cutover (R12-02) —
this is a tracked gap, not an oversight.

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

Two processes starting against the same unmigrated database at the same moment
are a supported case, not an error: sqlx does not serialize applied-version
bookkeeping across processes, so the loser of that race can surface either a
`UNIQUE` conflict on `_sqlx_migrations` or an `already exists` DDL error after
the winner commits. Both symptoms are retried within a bounded budget (5
attempts with linear backoff), which re-reads the completed history instead of
re-running DDL. Migrations still avoid `IF NOT EXISTS` deliberately, so a
failure that reproduces on every attempt — a genuinely partial or conflicting
schema — fails closed as described above.

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
