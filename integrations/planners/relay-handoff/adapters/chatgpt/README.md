# ChatGPT Adapter

Status: **first-class reference planner adapter**.

## Native mechanism

ChatGPT Skills.

The installable Skill is:

```text
relay-handoff/
|-- SKILL.md
|-- agents/openai.yaml
`-- references/
```

## Installation

Upload the packaged `skill.zip` from ChatGPT's Skills interface, or create/install the Skill through supported ChatGPT Skill management.

ChatGPT Personal Skills may need to be installed separately on different ChatGPT surfaces according to current OpenAI product behavior.

## Submission behavior

Layering follows [ADR-0016](../../../../../docs/adr/0016-mcp-first-submission-and-v0-1-local-descope.md):

1. **Relay Direct/MCP** — submit through `relay serve-mcp` when an MCP-capable
   host surface is available (implemented in M9). ChatGPT surfaces without
   MCP client capability cannot use this layer; do not imply otherwise.
2. **Provider-native remote compatibility transport** — a user-owned GitHub
   mailbox Issue, only when the surface has authorized issue-write capability.
3. **Local artifact/paste fallback** — generate local/pasteable Relay JSON.

A successful direct handoff does not require a GitHub Issue; Relay may create an optional binding afterward. No layer implies host-action coupling: submission lands the task in `Ready`, and execution still requires explicit Run approval in the Relay TUI. The adapter must never claim success unless the chosen transport confirmed receipt (MCP structured result with a TaskId, or explicit local import by the user).

## Local ingress

Always supported through generated `.relay.json`, readable `.relay.md`, or pasteable canonical JSON.

## Core alignment

The adapter conforms to `../../core/PLANNER_SPEC.md`. ChatGPT-specific UI metadata and tool routing must never redefine Relay task semantics.

## GitHub mailbox bootstrap

If ChatGPT cannot reach Relay Direct/MCP but has authorized GitHub issue-write capability, configure the adapter to use a mailbox repository owned by the user/team. Private visibility is the recommended default. Do not use a Relay maintainer's example repository as a shared service.

The same mailbox may carry tasks for multiple Relay projects because every HandoffTask identifies its logical `project` key. Users with Relay Direct/MCP do not need a mailbox.
