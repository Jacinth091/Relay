# Submission Transports and External Bindings

## Decision

Relay separates **how a task reaches Relay** from **where that task may also be mirrored or synchronized**.

After a task is accepted, Relay's local durable task record is canonical.

```text
Planner
  |
  | HandoffTask v1
  v
Submission Transport
  |
  v
Relay Core
  |
  +--> SQLite / local durable task state   <- canonical
  |
  +--> Optional External Binding
          |
          +--> GitHub Issue
          +--> future GitLab/Jira/Linear/etc.
```

An external system must not be required for Relay to execute a task once Relay can receive that task directly.

## Submission Transport

A submission transport describes how the approved `HandoffTask v1` reaches Relay.

Initial/future transport identifiers may include:

```text
relay-direct
relay-mcp        <- IMPLEMENTED (M9): `relay serve-mcp`, persisted provenance "relay-mcp"
provider-github
local-file       <- implemented (local ingress)
local-paste      <- implemented (local ingress)
github-ingress   <- descoped from v0.1 by ADR-0016; not started
```

The exact persisted names may evolve, but the distinction must remain.

### Preferred order for planner handoff

Per invariant 23, a planner adapter prefers transports in this order:

1. **Relay Direct** when available — submit through Relay's own interface.
   This is `relay serve-mcp` today: an MCP stdio server whose single tool
   `submit_handoff` accepts one canonical HandoffTask v1 JSON object and,
   after validation and durable persistence, answers with `{task_id, state:
   "Ready"}`. Host registration recipes live under
   [docs/planner-hosts/](planner-hosts/).
2. **Provider-native remote compatibility transport** — for example, create an Issue in the user's configured private Relay mailbox when the planner host has authorized issue-write capability. The mailbox is user-owned configuration, not a Relay-maintainer service.
3. **Local artifact/paste fallback** — produce a canonical Relay task for import when no automatic transport is available ([docs/local-ingress.md](local-ingress.md)).

A planner must not claim that a task was sent unless an external receiver confirms success. Submission never executes anything: running requires explicit Run approval in the Relay TUI.

## External Binding

An external binding is an optional association between a canonical Relay task and a record in another system.

Examples:

```text
none
github-issue
future-gitlab-issue
future-jira-ticket
future-linear-issue
```

A binding may be used for:

- shared task visibility;
- durable remote history;
- collaboration;
- clarification/decision publication;
- result/status mirroring.

It is not Relay's canonical task identity.

## Examples

### Current ChatGPT reference transport

```text
ChatGPT
  |
  | provider GitHub capability
  v
GitHub Issue
  |
  | GitHub ingress
  v
Relay
```

Conceptually:

```text
submission_transport = provider-github
external_binding     = github-issue
```

This remains a valid v0.1 reference path.

### Direct MCP with no GitHub binding

```text
Planner
  |
  | relay.submit_handoff
  v
Relay
  |
  v
SQLite
```

Conceptually:

```text
submission_transport = relay-mcp
external_binding     = none
```

This is a complete Relay workflow. GitHub is not required.

### Direct MCP with GitHub mirroring

```text
Planner
  |
  | relay.submit_handoff
  v
Relay
  |
  +--> SQLite
  |
  `--> GitHub Issue
```

Conceptually:

```text
submission_transport = relay-mcp
external_binding     = github-issue
```

Relay creates and manages the GitHub Issue using Relay's own configured GitHub integration.

### Local paste promoted to GitHub-backed task

```text
Planner
  |
  | copy canonical JSON
  v
Relay Paste Task
  |
  +--> canonical local task
  |
  `--> optional GitHub Issue
```

Conceptually:

```text
submission_transport = local-paste
external_binding     = github-issue
```

The provenance remains `local-paste`; creating a later GitHub binding must not rewrite history to pretend the task originally arrived through GitHub.

## GitHub Ownership

Relay should own its GitHub integration independently of planner-host GitHub capabilities.

The planner may use its own GitHub capability as a compatibility transport, but Relay must not depend on that capability for ongoing lifecycle synchronization.

Relay's GitHub adapter may handle:

- mailbox access checks;
- issue creation for optional bindings;
- issue discovery when GitHub is the ingress;
- labels/status synchronization;
- clarification/decision comments;
- result publication.

The first local implementation should use the authenticated `gh` executable rather than asking users to paste GitHub tokens into Relay.

When a GitHub mailbox is used, each user/team should configure its own repository. Private visibility is the recommended default, and a single mailbox may carry handoffs for multiple logical Relay projects. See `github-mailbox-setup.md`.

## Canonical Identity

Relay assigns its own task ID after acceptance.

External identities are bindings:

```text
Relay task UUID
    |
    +-- GitHub Issue #42
    +-- future external record
```

Deleting or disconnecting an external binding must not silently delete Relay's canonical local history.
