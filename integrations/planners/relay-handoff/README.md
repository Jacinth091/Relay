# Relay Handoff Planner Integration

This directory contains the provider-neutral planner-side specification for Relay and concrete first-party host adapters.

The canonical concept is **Relay Handoff Planner Specification**, not a ChatGPT-only Skill.

```text
                       Relay Handoff Planner Specification
                                     |
                   +-----------------+-----------------+
                   |                 |                 |
                   v                 v                 v
                ChatGPT          Claude Code       OpenCode / Copilot
                 Skill              Skill               Skills
                   |                 |                 |
                   +-----------------+-----------------+
                                     |
                              Handoff Task v1
                                     |
                                     v
                                   Relay
```

## Source of Truth

Read `core/` first.

Provider adapters may copy core references into self-contained installable directories, but those copies must remain semantically aligned with the provider-neutral specification.

## First-Party Adapters

- `adapters/chatgpt/` — first-class reference adapter.
- `adapters/claude-code/` — experimental first-party adapter.
- `adapters/opencode/` — experimental first-party adapter.
- `adapters/github-copilot/` — experimental first-party adapter.
- `adapters/generic/` — portable fallback instructions.

See each adapter's `INSTALL.md` or `README.md` and `docs/provider-support.md`.

## Core Rule

Adapters may change **how** Relay handoff behavior is installed and how authorized provider-native tools are invoked.

They may not change **what a Relay handoff means**.
