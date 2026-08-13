# Inspiration and Positioning

Relay was designed after considering both manual conversational-to-coding workflows and more autonomous agent orchestration systems.

## AIF Handoff

One specifically reviewed adjacent open-source project is **AIF Handoff** by `lee-to`:

https://github.com/lee-to/aif-handoff

AIF Handoff currently describes itself as an autonomous Kanban workflow in which AI agents plan, implement, review, and rework tasks.

Useful architectural ideas observed while researching it include:

- separating provider/runtime/transport concepts;
- capability-driven behavior;
- runtime profiles;
- durable task state;
- recovery/rework mechanisms;
- extension boundaries.

Relay intentionally differs in its default product philosophy.

```text
external human-directed conversation
    ->
explicit approved task
    ->
Relay
    ->
autonomous implementation within approved intent
    ->
structured escalation if intent must change
```

Relay does not require a human to approve every implementation repair.

It also does not allow the implementation agent to silently redefine product intent.

That middle position is deliberate.

## Learning From Other Projects

Open-source projects are an important part of how Relay's architecture is being learned and explored. General ideas and patterns can be studied and then implemented in a way that fits Relay's own goals.

If Relay later incorporates third-party source code, documentation, assets, or other material directly, the appropriate license and attribution notices should travel with that material.

See `ACKNOWLEDGMENTS.md` and `THIRD_PARTY_NOTICES.md`.

## Usage-Efficiency Positioning

Another defining motivation is **agentic usage efficiency**.

A developer may have broad access to a conversational planning surface while coding-agent execution is more constrained by included usage, rate limits, credits, or workspace policy.

Relay deliberately allows planning to remain external so the coding runtime can be reserved for implementation, verification, and bounded repair.

This must not be positioned as "beating," "bypassing," or "circumventing" a provider quota.

Relay does not change provider metering.

It changes the workflow boundary at which the coding runtime is invoked.
