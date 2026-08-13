# Claude Code Adapter Installation

Status: **experimental first-party adapter; native mechanism verified, Relay end-to-end validation pending**.

## Native mechanism

Claude Code Skills (`SKILL.md`).

## Personal installation

Copy the `relay-handoff` directory to:

```text
~/.claude/skills/relay-handoff/
```

On Windows, `~/.claude` resolves under the user's profile directory unless Claude Code configuration changes it.

## Project installation

Copy the directory to:

```text
<project>/.claude/skills/relay-handoff/
```

## Invocation

Claude Code can discover the skill automatically from its description or the user can invoke:

```text
/relay-handoff
```

## Relay submission

Prefer a configured Relay Direct/MCP connection when Relay exposes one. If that is unavailable, an authorized GitHub MCP server or authenticated `gh` CLI may be used as a compatibility transport. Local Relay JSON/Markdown remains the universal fallback.

A direct Relay handoff does not require a GitHub Issue. Relay may create an optional GitHub binding afterward according to local configuration.

## Official reference

https://code.claude.com/docs/en/skills
