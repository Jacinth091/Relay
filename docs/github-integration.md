# GitHub Integration

## Principle

Relay's GitHub integration is local Relay functionality. It must not depend on the selected planner host having its own GitHub connector.

Planner-host GitHub access is only an optional compatibility submission path.

## Local detection

The initial implementation should use the installed GitHub CLI (`gh`).

During setup/diagnostics Relay may check, through direct process spawning:

```text
gh executable available?
gh authentication valid?
which GitHub account is active?
can the configured mailbox repository be read?
can it be written when a binding/ingress requires writes?
```

Relay should use supported `gh` commands/APIs and must not scrape planner-host sessions or private authentication state.

## Separate environments

These are independent capabilities:

```text
Planner host
    GitHub connector/tool?     maybe

Local Relay
    authenticated gh?          maybe
```

Relay can reliably inspect its own local GitHub configuration. It generally cannot and should not infer whether a cloud planner currently has a GitHub connector unless that planner exposes the capability through a supported interface.

## v0.1 compatibility path

The first ChatGPT reference flow may be:

```text
ChatGPT
  -> planner GitHub write capability
  -> Relay mailbox Issue
  -> Relay GitHub ingress
```

This is useful before Relay Direct exists.

## After Relay Direct exists

The preferred flow may become:

```text
Planner
  -> Relay Direct / MCP
  -> Relay
  -> optional GitHub Issue binding
```

In this flow the planner does not need GitHub access.

Relay creates/synchronizes the optional Issue using Relay's own local GitHub integration.

## GitHub adapter responsibilities

The Relay GitHub adapter may eventually support both roles:

### Ingress role

- discover mailbox Issues;
- parse canonical Relay task blocks;
- map remote identity to Relay task identity.

### External binding role

- create an Issue for a task already received through another transport;
- mirror task state;
- publish clarification/decision packets;
- publish result summaries;
- retain binding health/error metadata.

The core should depend on interfaces/traits rather than invoking `gh` throughout domain code.
