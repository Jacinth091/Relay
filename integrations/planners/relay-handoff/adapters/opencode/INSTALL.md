# OpenCode Adapter Installation

Status: **experimental first-party adapter; native mechanism verified, Relay end-to-end validation pending**.

## Native mechanism

OpenCode Skills (`SKILL.md`).

## Global installation

Copy `relay-handoff` to:

```text
~/.config/opencode/skills/relay-handoff/
```

OpenCode also discovers global compatibility skills under:

```text
~/.agents/skills/relay-handoff/
~/.claude/skills/relay-handoff/
```

## Project installation

Preferred project path:

```text
<project>/.opencode/skills/relay-handoff/
```

Compatibility paths include `.agents/skills/` and `.claude/skills/`.

## Invocation

With a useful description, OpenCode can advertise the Skill for model invocation. In the V2 CLI it is also available as:

```text
/relay-handoff
```

## Relay submission

Prefer a configured Relay Direct/MCP connection when Relay exposes one. If that is unavailable, an authorized GitHub tool/MCP integration or authenticated local `gh` CLI may be used as a compatibility transport. Otherwise use local Relay JSON/Markdown.

A direct Relay handoff does not require a GitHub Issue. Relay may create an optional GitHub binding afterward according to local configuration.

If GitHub is used as the bootstrap compatibility transport, configure a mailbox repository owned by the user/team. Private visibility is the recommended default. Do not use a Relay maintainer's example repository as a shared mailbox. One mailbox can carry handoffs for multiple Relay projects because each task includes its logical `project` key.

## Official reference

https://opencode.ai/v2/docs/skills
