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

Preferred long-term path:

```text
ChatGPT -> Relay Direct/MCP -> Relay
```

The v0.1 compatibility path may use a connected write-capable GitHub integration:

```text
ChatGPT -> GitHub handoff Issue -> Relay GitHub ingress
```

If neither automatic path is available, the adapter must generate local/pasteable Relay JSON instead of claiming success. A successful direct handoff does not require a GitHub Issue; Relay may create an optional binding afterward.

## Local ingress

Always supported through generated `.relay.json`, readable `.relay.md`, or pasteable canonical JSON.

## Core alignment

The adapter conforms to `../../core/PLANNER_SPEC.md`. ChatGPT-specific UI metadata and tool routing must never redefine Relay task semantics.

## GitHub mailbox bootstrap

If ChatGPT cannot reach Relay Direct/MCP but has authorized GitHub issue-write capability, configure the adapter to use a mailbox repository owned by the user/team. Private visibility is the recommended default. Do not use a Relay maintainer's example repository as a shared service.

The same mailbox may carry tasks for multiple Relay projects because every HandoffTask identifies its logical `project` key. Users with Relay Direct/MCP do not need a mailbox.
