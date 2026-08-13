# Relay Contributor Context

Relay should grow by preserving boundaries rather than accumulating shortcuts in core code.

There is one Relay engine.

Before implementing a feature, decide which layer owns it:

```text
Core?
Ingress?
Runtime?
Stack?
Planner integration?
Presentation client?
```

Examples:

- GitHub issue parsing -> GitHub ingress; GitHub status mirroring -> external binding adapter.
- Codex reasoning option -> Codex runtime/stack.
- Task state transition -> Core.
- Clarification/decision packet domain model -> Core/protocol.
- Rendering clarification Markdown -> protocol/presentation helper.
- Open local repo in VS Code -> client/platform action using core-safe project resolution.
- Render task list -> presentation client.
- Resolve project key to filesystem path -> trusted project registry/core.
- Create a task from ChatGPT -> ChatGPT planner adapter implementing the Relay Handoff Planner Specification.
- Automatic repair loop -> runtime/core execution policy.
- Deciding whether a blocker changes intent -> recovery policy + runtime classification, with escalation when uncertain.

Avoid designing for imaginary providers.

Implement the first real stack well, then generalize when a second implementation proves the abstraction.
