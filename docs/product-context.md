# Product Context

## Problem

Developers increasingly use one AI surface for reasoning and another for repository implementation.

A common workflow is:

1. discuss a feature or bug with a conversational assistant;
2. explore architecture and trade-offs;
3. challenge or correct the assistant;
4. refine requirements;
5. decide what should actually happen;
6. hand the approved work to a coding agent;
7. supervise implementation;
8. return to planning only if implementation uncovers a genuine decision.

Without a handoff layer, the user often has to:

- copy and paste a final specification;
- switch applications;
- re-establish project context;
- choose the correct repository manually;
- remember coding-agent commands;
- manually track what has been handed off;
- translate runtime confusion back into human-readable planning context.

At the other extreme, autonomous orchestration systems may internalize planning, implementation, review, and rework into continuous agent loops.

There is also a practical resource problem: **coding-agent execution is often metered, rate-limited, credit-limited, or subject to plan allowances**.

Long exploratory planning conversations can consume the same scarce agentic resources that a developer would rather reserve for repository work.

A developer may already have access to a conversational planning surface through an existing subscription or workspace while their coding agent has a separate or more constrained usage allowance. In that situation, it is wasteful to spend coding-agent capacity debating architecture, revisiting rejected options, or clarifying product intent when that work can happen before execution.

Relay therefore treats agentic execution capacity as a resource worth using deliberately:

```text
Conversational surface
    |
    | reason / discuss / challenge / decide
    |
    | does not require Relay to start a coding runtime
    v
Approved implementation intent
    |
    v
Relay
    |
    | coding-agent usage starts here
    v
Implementation / verification / bounded repair
```

For Relay's first ChatGPT + Codex workflow, one motivating use case is to keep the long planning discussion in normal ChatGPT and use Codex after explicit handoff, so Codex/agentic capacity is concentrated on implementation rather than exploratory planning.

Relay chooses a different default.

## Product Aim

Relay should make this workflow natural:

```text
Think together
    ->
decide
    ->
hand off
    ->
spend coding-agent capacity on implementation
    ->
execute autonomously within approved scope
    ->
escalate only when intent must change
```

A secondary product aim is **usage efficiency**: avoid spending limited coding-agent capacity on planning work that can happen before the handoff.

## Primary User

Initially:

- an individual software developer;
- multiple local repositories;
- uses conversational AI for reasoning;
- uses one or more coding agents for implementation;
- values explicit control over intent;
- does not want to babysit ordinary implementation failures;
- does not want to memorize complex CLI commands;
- may have limited access to MCP/custom integrations;
- may have finite coding-agent quotas, rate limits, credits, or workspace allowances;
- wants to reserve coding-agent usage primarily for implementation;
- may use modest hardware.

## User Promise

Relay should make it easy to answer:

- What work is waiting?
- Which project will this run against?
- Which coding runtime will execute it?
- Is the target repository safe and correct?
- What is currently running?
- What has Relay repaired automatically?
- Why did execution stop?
- What decision does Relay need from me?
- What changed?
- Did it succeed?

## Differentiation

Relay's default workflow is not autonomous product planning.

Its default value is the boundary:

> The user remains part of planning, and Relay makes the transition to execution deliberate and reliable.

After that boundary, Relay should be capable enough that the user is not forced to approve every implementation correction.

## Success Criteria

Relay is useful when it reduces:

- context switching;
- command memorization;
- coding-agent babysitting;
- ambiguity during implementation escalation;
- unnecessary consumption of coding-agent usage during exploratory planning;

without reducing user ownership of product/architecture intent.

## Usage Efficiency, Not Limit Circumvention

Relay must never market or implement this goal as bypassing provider limits, evading billing, scraping unsupported interfaces, or circumventing usage controls.

Relay improves efficiency by choosing **when** a coding runtime is invoked.

It does not alter how a provider meters that runtime.

Provider plans, quotas, credit systems, and rate limits can change independently of Relay. Relay should therefore describe this benefit generically and treat provider-specific usage information as runtime metadata rather than a permanent assumption.

The intended principle is:

> **Use the planner for planning. Spend coding-agent capacity when there is approved coding work to do.**
