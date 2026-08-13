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
