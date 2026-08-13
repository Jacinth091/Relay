# TUI UX

The TUI is Relay's first client.

It should feel like an application, not a list of commands the user has to memorize.

---

## Launch

Normal use:

```text
relay
```

---

## Primary Screens

### Dashboard

Shows:

- projects;
- project health;
- task counts;
- runtime health;
- runtime usage/allowance status when supported;
- running execution;
- tasks waiting for clarification/decision.

### Task Review

Show:

- title;
- project;
- source;
- selected stack/runtime;
- goal;
- requirements;
- constraints;
- verification;
- preflight state.

### Execution

Show:

- target repository;
- runtime;
- elapsed time;
- current phase;
- repair attempt count;
- live bounded output;
- cancel action when supported.

Possible phases:

```text
RUNNING
VERIFYING
REPAIRING
```

### Needs Clarification / Decision

Example:

```text
+------------------------------------------------------+
| Relay                                                |
+------------------------------------------------------+
| Task #42 needs a decision                            |
|                                                      |
| Project: PractiCore                                  |
| Runtime: Codex                                       |
| Reason: Architecture decision required               |
|                                                      |
| Codex completed part of the task but continuing      |
| would require changing approved intent.              |
|                                                      |
| [Enter] Read Packet                                  |
| [C] Copy for Planner                                 |
| [O] Open Issue                                       |
+------------------------------------------------------+
```

Packet view should clearly show:

- work completed;
- blocker;
- evidence;
- question;
- options;
- executor assessment;
- repository/verification state;
- resume point.

### Resolution

When a Resolution Packet arrives, show:

- decision;
- rationale;
- work disposition;
- whether execution will continue, revise, restart, revert, or cancel.

### Result

Show:

- Done / Failed / Cancelled;
- changed files;
- Git status;
- verification result;
- runtime summary;
- View Diff action.

### Projects

Allow trusted local project registration/editing.

### Settings

Configure:

- ingress;
- task repository/channel;
- runtime profiles;
- recovery limits;
- local policies;
- output/log retention.

---

## Interaction Rules

- arrow keys for navigation;
- Enter for primary action;
- visible key hints;
- `?` opens help;
- destructive/escalating actions require clear confirmation;
- no hidden key combinations for essential operations.

The user should not need to remember subcommands for normal operation.

---


## Local Task Entry

The TUI should support a no-network ingress:

```text
New Task
  > Import File
    Paste Task
```

`Import File` accepts canonical Relay JSON and readable Markdown wrappers containing an embedded Relay JSON block.

`Paste Task` accepts the same formats directly from the clipboard/editor.

After parsing, always show a task preview before importing. Importing does not equal running.

See `docs/local-ingress.md`.

## Clipboard Workflow

For clarification/decision packets, `Copy for Planner` should produce ready-to-paste Markdown.

The user should not have to reconstruct what the runtime was doing.

---

## Core Boundary

The TUI receives:

```text
domain state
domain events
commands/results
```

It does not directly spawn Codex, call GitHub, inspect Git remotes, or write SQLite.


## Runtime Usage Display

If a runtime exposes supported usage metadata, Relay may show it without making it a required cross-provider feature.

Concept:

```text
Codex
Status: Available
Included/credit usage: provider-reported value
Reset: provider-reported value, when available
```

If the provider does not expose reliable information, Relay should show `Unavailable` or omit the metric rather than estimate it.

Usage display is informational and must not become quota-circumvention logic.
