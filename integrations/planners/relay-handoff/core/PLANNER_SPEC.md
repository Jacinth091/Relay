# Relay Handoff Planner Specification

Status: Foundation v1.7

This is the provider-neutral behavioral specification for any conversational planner that interoperates with Relay.

## Purpose

Teach a planner host how to:

1. remain in discussion during exploratory planning;
2. recognize an explicit user-controlled handoff boundary;
3. convert finalized intent into the canonical Relay task protocol;
4. submit that task using the strongest supported Relay submission transport, or export it when automatic submission is unavailable;
5. help the user reason about Relay clarification/decision packets;
6. return a structured Resolution Packet after the user decides.

A planner adapter is a client of Relay. It is not another Relay engine.

## Planner Host

A planner **provider** and a planner **host** are not the same concept.

Examples:

```text
Provider      Host
OpenAI        ChatGPT
Anthropic     Claude Code
GitHub        GitHub Copilot
OpenCode      OpenCode
```

Relay adapters are host-specific because installation paths, tool availability, skill discovery, and connector behavior belong to the host.

## Explicit Handoff Boundary

Do not send work merely because implementation is being discussed.

Examples of explicit handoff intent include:

```text
Send this to Relay.
Hand this off to Relay.
Queue this for Relay.
Send this to Codex through Relay.
This is final; hand it off.
```

Equivalent clear wording is acceptable.

If intent is ambiguous, keep planning.

## Task Construction

Construct only the approved implementation intent.

Include:

- project key;
- concise title;
- goal;
- requirements;
- constraints;
- verification expectations;
- requested/default stack when known;
- source metadata;
- ambiguity policy;
- supported namespaced extensions when useful.

Exclude:

- local filesystem paths;
- credentials or secrets;
- arbitrary shell commands;
- rejected alternatives unless they are necessary constraints;
- unrelated conversation history.

Use `task-protocol.md`.

## Submission Selection

When the user explicitly asks to **Send this to Relay**, build a valid HandoffTask first, then choose the strongest supported transport.

Use this preference order unless the user explicitly requests a different mode:

### 1. Relay Direct

If the planner host has an authorized direct Relay interface, such as a configured Relay MCP server, submit the HandoffTask directly to Relay.

The direct interface is preferred because the planner does not need to know about Relay's optional external bindings.

Only report success after Relay confirms acceptance.

### 2. Provider-native remote transport

If direct Relay submission is unavailable but the host has another supported authorized transport, use it.

The initial compatibility example is a write-capable GitHub integration that creates an Issue in the configured **user-owned Relay mailbox repository**. The recommended mailbox is private and is created/selected by the user or their team during setup.

Never assume a Relay maintainer's example mailbox is a shared destination for users.

This is a transport choice, not the definition of a Relay handoff.

Only report success after the remote system confirms creation.

### 3. Local artifact / paste fallback

If no automatic submission transport is available, do not pretend the handoff completed.

Provide either:

- canonical `.relay.json`;
- readable `.relay.md` containing the identical canonical JSON block; or
- canonical JSON suitable for Relay's paste/import flow.

Tell the user that the task is prepared for Relay but has not yet been received by Relay.

## External Bindings Are Relay-Owned

A planner adapter does not require an external binding to complete a direct handoff.

After Relay receives the task, Relay may create or synchronize an optional external record such as a GitHub Issue according to local Relay configuration.

The planner should not assume that every Relay task has a GitHub Issue.

## Local artifact requests

When the user explicitly asks for a local/offline Relay task, produce a `.relay.json` canonical artifact or a readable `.relay.md` carrier containing the identical canonical JSON payload even if automatic submission is otherwise available.

For direct terminal paste, prefer raw canonical JSON.

## Clarification / Decision Loop

When Relay returns `NEEDS_CLARIFICATION` or `NEEDS_DECISION`:

- treat executor assessment as evidence, not authority;
- help the user understand the blocker;
- challenge weak assumptions where useful;
- do not silently choose a material product/architecture decision for the user;
- after the user decides, produce a structured Resolution Packet.

If a direct Relay resolution interface is available, prefer it. Otherwise use an authorized bound-channel write-back when appropriate, or provide a local Resolution Packet.

Use `clarification-resolution.md`.

## Agentic Usage Principle

Keep exploratory product/architecture reasoning in the planner by default.

Do not invoke a coding runtime simply to continue discussion.

This improves coding-agent usage efficiency but must never be represented as bypassing provider billing, quotas, authentication, or rate controls.

## Provider Adapter Contract

A provider/host adapter conforms only when it:

- preserves explicit handoff semantics;
- produces protocol-valid Relay tasks;
- prefers direct Relay submission when a supported authorized direct interface exists;
- supports provider-native compatibility transports when appropriate;
- supports local artifact output when automatic submission is unavailable or explicitly requested;
- preserves the clarification/resolution model;
- never embeds trusted local repository paths;
- never claims successful submission without receiver confirmation;
- does not assume GitHub is required for every Relay task;
- uses provider-native integration mechanisms without moving provider-specific logic into Relay Core;
- documents installation paths and host-specific capabilities;
- passes the planner-adapter conformance cases in `docs/planner-adapter-conformance.md`.
