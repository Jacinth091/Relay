# Planner Integrations

Planner integrations teach a conversational host how to cross the explicit Relay handoff boundary.

They are clients of Relay, not separate Relay engines.

## Provider-Neutral Source of Truth

The canonical behavior is the **Relay Handoff Planner Specification**:

```text
integrations/planners/relay-handoff/core/
|-- PLANNER_SPEC.md
|-- task-protocol.md
|-- local-ingress.md
|-- clarification-resolution.md
`-- configuration.md
```

The specification defines semantics.

A host adapter defines native packaging and tool behavior.

```text
Relay Handoff Planner Specification
                 |
       +---------+----------+----------+
       |         |          |          |
       v         v          v          v
   ChatGPT   Claude Code  OpenCode   Copilot
    Skill       Skill       Skill      Skill
       |         |          |          |
       +---------+----------+----------+
                 |
          HandoffTask v1
                 |
                 v
        submission transport
                 |
                 v
               Relay
```

## Planner Host vs Provider

Adapters are host-specific.

Examples:

```text
provider=openai     planner_host=chatgpt
provider=anthropic  planner_host=claude-code
provider=github     planner_host=github-copilot
```

A provider may expose multiple hosts with different installation and tool capabilities.

## First-Party Adapter Support

See `docs/provider-support.md` for the current support matrix.

Current adapters:

```text
adapters/chatgpt/         first-class reference
adapters/claude-code/     experimental first-party
adapters/opencode/        experimental first-party
adapters/github-copilot/  experimental first-party
adapters/generic/         fallback instructions
```

Experimental means the provider-native Skill mechanism is packaged/documented, but Relay has not yet completed end-to-end validation on that planner host.

## Native specificity is expected

Provider-specific adapters should use native mechanisms rather than pretending every host is identical.

Those differences belong at the adapter edge and may not change the canonical Relay task or security semantics.

## Submission capability

Every adapter supports local JSON/Markdown generation.

For an explicit handoff, adapters prefer:

```text
Relay Direct / Relay MCP
          |
          | unavailable
          v
provider-native remote transport
          |
          | unavailable
          v
local file / paste
```

GitHub Issue creation by the planner is therefore a compatibility transport, not a universal requirement.

The adapter must never claim success when it could only format the task.

## External bindings

After Relay accepts a task, Relay may optionally bind it to a GitHub Issue or another external record.

That choice belongs to Relay's local configuration, not to the planner adapter.

See `docs/submission-and-bindings.md`.

## Conformance

All first-party adapters must satisfy `docs/planner-adapter-conformance.md`.

That is how Relay remains provider-adaptable without degrading into provider-specific task protocols.

## Future Relay UI

Relay should eventually expose:

```text
Planner Integrations

ChatGPT          Export / Install instructions
Claude Code      Install locally
OpenCode         Install locally
GitHub Copilot   Install locally
Generic          Copy instructions
```

Relay may install into documented local Skill directories after explicit user approval.

Cloud/UI-managed installations should be exported rather than automated through unsupported private APIs.
