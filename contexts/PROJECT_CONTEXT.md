# Relay — Canonical Project Context

Use this document to recontextualize a new human or AI conversation.

## What Relay Is

Relay is one local handoff engine for developers who want to plan collaboratively in a conversational AI surface and deliberately choose when implementation work is delegated to a coding agent.

Its architectural aim: accept handoffs; distribute approved implementation work to a chosen or multiple coding-runtime providers; supervise deterministically; aggregate and verify results within approved scope; relay outcomes and structured escalations back to the human. Distribution beyond one chosen provider is design intent, not shipped capability — see [ADR-0018](../docs/adr/0018-provider-distribution-model.md) and [docs/status](../docs/status-2026-08.md) for what works today.

### Manager / Sub-worker Mapping

Relay's role maps onto a manager-and-sub-workers model, applied to coding runtimes rather than to LLM subagents:

```text
manager                 -> Relay Core (deterministic)
sub-workers             -> coding runtimes reached via RuntimeRouter
work assignment         -> approved handoff tasks under ExecutionRunPolicy
consolidation           -> execution reports + activity normalization
verification relay-back -> DONE/FAILED outcomes or NEEDS_* packets
```

The manager role is strictly deterministic. Relay never becomes the reasoning manager: it does not decide intent, split work semantically, or judge results beyond configured verification. Planning stays with humans and planner surfaces ([ADR-0017](../docs/adr/0017-relay-identity-deterministic-handoff-engine.md)).

Core flow:

```text
Human + Planner
    -> explicit approval
    -> Planner Integration
    -> Relay Ingress
    -> Relay Core
    -> local execution approval
    -> Coding Runtime
    -> Target Repository
```

The first supported path is:

```text
ChatGPT -> local file/paste ingress -> Relay -> Codex CLI -> local Git repo
```

The supported submission transports today (ADR-0016):

```text
Planner host with local stdio MCP support -> relay serve-mcp -> Relay
Planner host without usable MCP access    -> local file/paste -> Relay
```

Relay Direct / MCP is implemented (M9: `submit_handoff` over `relay serve-mcp`, dual-era MCP protocol). Local file/paste remains the hard fallback that always works. The GitHub compatibility transport was descoped from v0.1 and is a future optional ingress/external-binding path, not current behavior.

GitHub may return as an additional durable ingress bridge or optional external binding in a future decision; it would not be a second Relay implementation.

The local ingress accepts canonical JSON files/paste, with optional Markdown wrappers containing the same JSON block, and it is the path that works end to end today.

## Why It Exists

Relay rejects the assumption that planning should always be performed autonomously by the same agent that implements the code.

The developer should be free to:

- discuss;
- disagree;
- ask for counterarguments;
- revise intent;
- stop;
- decide when implementation is ready.

Relay begins at the explicit handoff boundary.

## Agentic Usage / Quota Motivation

A major original motivation is that coding-agent execution may be subject to finite quotas, rate limits, credits, or workspace allowances.

Relay allows long exploratory reasoning to remain in the conversational planning surface and starts the coding runtime only after explicit approval.

For the first ChatGPT + Codex stack, the intended behavior is:

```text
Normal ChatGPT planning
    ->
approved handoff
    ->
Relay
    ->
Codex implementation
```

This is usage optimization, not limit circumvention.

Relay never bypasses provider billing, usage controls, or authentication. Provider-specific limits are external policy and may change.

## Execution Autonomy

After handoff, Relay may autonomously pursue the approved intent.

It may repair:

- compiler/type errors;
- failed tests;
- lint failures;
- implementation mistakes;
- verification failures;

when the repair stays within approved scope.

It must stop when continuing would require materially changing intent.

The canonical rule is:

> The human owns intent. Relay owns controlled execution.

## Clarification Boundary

When execution crosses back into planning territory, Relay creates a structured packet.

States:

```text
NEEDS_CLARIFICATION
NEEDS_DECISION
```

A packet explains:

- work attempted;
- work completed;
- blocking reason;
- relevant evidence;
- decision/question;
- options;
- executor assessment;
- repository/verification state;
- resume point.

The planner returns a Resolution Packet.

For the first GitHub-based integration (planned, not yet implemented), these packets can be posted to the task issue and copied as Markdown into a normal planning conversation. Today, packets are copied as Markdown into a normal planning conversation via local file/paste.

## Hard Invariants

- There is one Relay engine.
- Relay Core does not require an LLM.
- Planning remains external by default.
- User approval is explicit.
- Execution may be autonomous only within approved scope.
- Core is provider-agnostic.
- Provider-native stacks may be highly specialized.
- Local config owns repository paths.
- Remote tasks cannot choose arbitrary local paths.
- Runtime execution must be validated before spawn.
- First UI is a TUI; the product is not TUI-locked.
- No automatic push in v0.1.
- Unknown or unsafe conditions fail closed.
- Clarification/decision states must explain why execution stopped.
- Relay should avoid invoking coding runtimes for exploratory planning.
- Relay must not circumvent provider quotas, billing, or usage controls.

## Foundation Technology

- Rust stable / Edition 2024.
- Cargo workspace.
- Tokio.
- Ratatui + Crossterm.
- Serde.
- JSON task/clarification protocol.
- TOML local config.
- SQLite + SQLx.
- clap.
- tracing.
- Git CLI initially.
- GitHub CLI initially.
- Codex CLI first runtime.

Future frontends may use Tauri 2 or an Axum-based local server.

## Architectural Vocabulary

Planner: where intent is formed.

Planner Specification: provider-neutral rules for converting approved intent into Relay protocol.

Planner Adapter: provider-native packaging of the planner specification.

Handoff Task: approved implementation specification.

Ingress: how work reaches Relay.

Provider: ecosystem behind a runtime.

Runtime: coding agent being executed.

Runtime Transport: CLI/SDK/API mechanism used to communicate with a runtime.

Runtime Profile: local configured instance/options.

Capabilities: runtime features that can be safely used.

Stack: opinionated composition of planner adapter/specification, ingress, runtime, and provider-native behavior.

Clarification Request: asks for explanation of existing approved intent.

Decision Request: asks for a new material choice.

Resolution Packet: approved answer used to resume/restart execution.

## First Stack

`chatgpt-codex`

- planner: normal ChatGPT conversation;
- planner specification: Relay Handoff Planner Specification;
- planner adapter: installable ChatGPT `relay-handoff` Skill;
- v0.1 submission: Relay Direct/MCP (`relay serve-mcp`, implemented) preferred when the host supports local MCP; local file/paste is the hard fallback;
- GitHub ingress: descoped from v0.1; future optional compatibility path;
- external binding: optional GitHub Issue;
- runtime: Codex;
- runtime transport: CLI;
- local execution approval: required;
- implementation repair: automatic within policy;
- planning ambiguity: escalate;
- code push: manual.

## Positioning

Relay is not trying to outperform autonomous orchestration systems at autonomous planning.

Its differentiated default is:

> Planning is a conversation. Execution is a handoff.

Its execution principle is:

> After handoff, Relay may autonomously pursue approved intent without inventing new intent.
\n\n## Planner-host adapters\n\nRelay Handoff has one provider-neutral planner specification with host-native adapters.\n\nCurrent adapter statuses:\n\n```text\nChatGPT          first-class reference\nClaude Code      experimental first-party\nOpenCode         experimental first-party\nGitHub Copilot   experimental first-party\nGeneric          fallback\n```\n\nProvider-native installation/tool behavior may differ, but all adapters must emit the same Relay task and clarification/resolution semantics.\n