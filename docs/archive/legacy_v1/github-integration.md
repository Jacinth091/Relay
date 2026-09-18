# GitHub Integration

> **Status: FUTURE / OPTIONAL — not implemented, not a v0.1 blocker.**
> GitHub ingress was descoped from v0.1 by ADR-0016; Relay Direct/MCP
> (`relay serve-mcp`) and local file/paste are the implemented submission
> transports. This document is preserved as future compatibility and optional
> external-binding design material.

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

## Mailbox ownership

When GitHub is used as the bootstrap planner-to-Relay transport, the mailbox must be a repository selected or created by the Relay user/team. Relay must never assume a maintainer-owned example repository is available for other users.

Private visibility is the recommended default because handoff Issues may contain implementation and planning context. The repository name is user-defined; names such as `relay-handoff`, `relay-mailbox`, or `ai-handoff` are examples only.

One private mailbox may receive tasks for many local Relay projects because every HandoffTask carries exactly one logical `project` key. Relay resolves that key through its trusted local Project Registry.

See `github-mailbox-setup.md`.

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
