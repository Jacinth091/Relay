# Design Principles

These are product-level invariants.

## 1. Planning is a conversation

Do not treat planning as a hidden pre-execution stage.

The developer should be able to participate meaningfully in the reasoning process.

## 2. Execution is a handoff

Planning and execution are separated by an explicit boundary.

## 3. There is one Relay

Planner integrations, ingresses, runtimes, and presentation clients all connect to the same Relay engine.

Do not create separate competing Relay implementations per provider.

## 4. Human approval owns intent

The user decides what should be built.

## 5. Controlled execution autonomy is desirable

After handoff, Relay may autonomously repair and retry implementation when doing so remains inside approved scope.

Do not escalate ordinary compiler errors and test failures to the user.

## 6. Explain before escalating

When execution must stop for planning input, Relay must explain:

- what it was doing;
- what succeeded;
- why it stopped;
- what decision is needed;
- what options/evidence exist;
- how work can resume.

A vague `NEEDS_INPUT` is insufficient.

## 7. Relay Core is deterministic

Core behavior should be understandable without an LLM.

Do not add an LLM call where typed state and deterministic logic can solve the problem.

## 8. Provider-agnostic core, provider-native edges

Do not reduce every coding runtime to the same weakest feature set.

Core abstractions should be small; runtime/stack implementations may be deeply native.

## 9. Capabilities, not provider-name branches

Prefer:

```text
runtime.capabilities.resume
```

over:

```text
runtime == "codex"
```

in shared code.

## 10. Local trust is explicit

Local project configuration is trusted.

Remote tasks are untrusted.

## 11. Fail closed

Unknown project, invalid remote, malformed protocol, unsupported unsafe option: do not execute.

## 12. Relay is not its first UI

Ratatui is a client.

All important behavior must be usable without Ratatui-specific types in core code.

## 13. Lightweight by default

Do not add always-running services, browser engines, or database servers to solve a local control-plane problem.

## 14. Learn abstractions from real integrations

Do not implement a universal plugin architecture before a second runtime/ingress proves what is actually common.

## 15. Durable state beats conversational memory

Execution history, requests, resolutions, and task state belong in Relay's local data store.

## 16. Native authentication before embedded credentials

Prefer existing trusted provider authentication before asking Relay to store tokens.

## 17. No silent source-control escalation

Editing is different from committing.

Committing is different from pushing.

v0.1 stops at local edits and review.

## 18. Agentic usage should be deliberate

Coding-runtime capacity may be scarce or metered.

Do not invoke a coding runtime merely to continue exploratory planning that belongs upstream.

This is an efficiency principle, not a mechanism for bypassing provider limits.

## 19. Respect provider usage controls

Relay must not scrape unsupported interfaces, evade billing, bypass quotas, or circumvent provider usage controls.

Relay chooses when to invoke a runtime; it does not alter how that runtime is metered.

## 20. Documentation is part of architecture

If a change modifies an invariant, protocol, state transition, recovery rule, or extension boundary, documentation changes are part of completion.
