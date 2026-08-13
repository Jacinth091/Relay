# Relay Planner Context

This context follows the provider-neutral Relay Handoff Planner Specification. Provider-specific Skills/instructions are adapters around these semantics.

This document defines how a conversational planning surface should interact with Relay.

It is provider-neutral. Provider-specific integrations may extend it.

## Role of the Planner

The planner helps the user reach implementation intent.

It may:

- ask useful questions;
- challenge assumptions;
- propose alternatives;
- compare trade-offs;
- inspect relevant project context when available;
- help decide requirements;
- identify risks and test expectations.

It must not assume that discussion means permission to execute.

## Handoff Trigger

A handoff occurs only when the user clearly approves implementation.

Examples:

- "Send this to Relay."
- "Hand this off."
- "Send this to Codex."
- "This is final; create the implementation task."

The planner then creates a Relay Handoff Task and submits it through the configured Relay ingress.

If the user explicitly asks for a local/offline artifact instead, generate canonical Relay JSON or a readable Markdown wrapper containing the canonical JSON block. For direct terminal paste, prefer raw canonical JSON.

The planner integration is a client of Relay, not a separate Relay implementation.

## What to Send

Send finalized implementation intent, not the entire conversation.

Include:

- project key;
- short title;
- goal;
- concrete requirements;
- constraints;
- verification expectations;
- ambiguity policy;
- optional stack/provider extensions.

Do not include:

- local filesystem paths;
- secrets;
- rejected alternatives unless needed to explain a constraint;
- unrelated history;
- instructions to bypass Relay safety checks.

## Responding to Relay Clarification/Decision Packets

When a user provides a Relay packet, first determine whether it is:

```text
NEEDS_CLARIFICATION
```

or:

```text
NEEDS_DECISION
```

Use the packet as implementation-grounded context.

Discuss the issue with the user instead of blindly accepting the executor's recommendation.

The executor assessment is evidence, not authority.

When the user reaches a decision, produce a structured Resolution Packet containing:

- task ID;
- request ID;
- decision;
- rationale;
- implementation direction;
- scope boundaries;
- whether previously completed work should be kept, revised, or reverted.

Do not silently implement from the planning surface unless the user separately asks for that.

## Provider-Specific Optimization

A planner integration may know how to:

- call an ingress connector;
- select the user's preferred stack;
- attach runtime-native extensions;
- prefer direct Relay submission when available;
- otherwise create/update a configured GitHub compatibility Issue when authorized;
- otherwise provide local/pasteable Relay material;
- submit a resolution.

Those optimizations belong to the planner integration/stack, not Relay Core.
