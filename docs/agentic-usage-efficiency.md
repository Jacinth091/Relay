# Agentic Usage Efficiency

## Motivation

One of Relay's original motivations is that coding-agent execution is not always an unlimited resource.

Depending on provider and plan, coding runtimes may be constrained by:

- included usage allowances;
- rate limits;
- credit pools;
- workspace spend controls;
- reset windows;
- runtime availability.

Meanwhile, a developer may already have a conversational AI surface that is well suited to long-form architecture and requirements discussion.

Relay intentionally separates those phases.

```text
Planner Surface
    |
    | explore freely
    | challenge assumptions
    | refine requirements
    v
Explicit approval
    |
    v
Relay
    |
    | coding runtime starts
    v
Implementation
```

The intended result is that coding-agent capacity is concentrated on:

- repository inspection;
- implementation;
- tests;
- verification;
- bounded repair.

rather than:

- repeated architecture debate;
- rejected ideas;
- product clarification;
- exploratory conversation that never becomes code.

## First Reference Stack

For the initial ChatGPT + Codex stack, Relay is motivated in part by a workflow where normal ChatGPT is used as the planning surface and Codex is invoked after approval for implementation.

Provider usage models can change, so this is a reference workflow rather than a permanent billing assumption.

## Non-Goal: Circumventing Limits

Relay must not:

- evade provider billing;
- bypass quotas;
- manipulate usage accounting;
- scrape unsupported authenticated interfaces to defeat restrictions;
- rotate identities/accounts to evade limits;
- fake provider usage state.

Relay does not reduce provider-side metering for work that actually runs.

Relay improves efficiency by avoiding unnecessary coding-runtime invocation before implementation is ready.

## Runtime Usage Metadata

When a provider exposes usage information through supported mechanisms, Relay may normalize and display it.

Possible metadata:

```text
available
throttled
remaining allowance
credits
reset time
provider message
```

This information should be treated as volatile provider state.

Relay must not hard-code assumptions such as a universal "weekly quota."

## Product Language

Preferred:

> Relay helps reserve coding-agent capacity for implementation.

> Relay separates conversational planning from agentic execution.

> Relay helps developers use metered coding runtimes deliberately.

Avoid:

> Relay bypasses Codex limits.

> Relay gives unlimited Codex.

> Relay beats provider quotas.

Those statements would misrepresent what Relay does.
