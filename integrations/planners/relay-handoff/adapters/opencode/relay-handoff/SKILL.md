---
name: relay-handoff
description: Create, submit, export, or resolve Relay handoff tasks from OpenCode. Use when the user explicitly asks to send, hand off, queue, or format approved implementation work for Relay; asks to create a local Relay file or pasteable Relay payload; or brings back a Relay NEEDS_CLARIFICATION / NEEDS_DECISION packet for discussion and resolution. Do not activate merely because software planning is happening; wait for explicit handoff or Relay-resolution intent.
---

# Relay Handoff

Implement the provider-neutral Relay Handoff Planner Specification for OpenCode.

This adapter changes only provider-native installation and tool use. It must not change Relay task semantics.

## Decide the operation

Determine which operation the user requested:

1. **Send to Relay** — build the approved task, then use the strongest supported submission transport: Relay Direct first, provider-native remote transport second, local artifact/paste fallback last.
2. **Create a local Relay artifact** — produce canonical `.relay.json`, or readable `.relay.md` containing the identical canonical JSON block.
3. **Format for copy/paste** — provide canonical JSON suitable for Relay's paste/import flow.
4. **Resolve a Relay blocker** — discuss a `NEEDS_CLARIFICATION` or `NEEDS_DECISION` packet, then produce a Resolution Packet only after the user decides.

Do not treat ordinary statements such as "that sounds good" as execution approval unless the surrounding context clearly expresses handoff intent.

## Build the handoff task

Summarize only approved implementation intent. Exclude rejected alternatives and unrelated conversation history unless they are needed as constraints.

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

Never include local filesystem paths, secrets, arbitrary shell commands, credentials, or instructions to bypass Relay safety policy.

Read `references/task-protocol.md` for the canonical shape.

## Submission behavior

Prefer a configured Relay MCP/direct tool. If Relay Direct is unavailable, use an authorized GitHub tool/MCP integration or authenticated local `gh` CLI when available in the selected OpenCode environment.

If no automatic transport is available, return a valid local/pasteable Relay artifact and clearly state that Relay has not received the task yet.

Never say "Sent to Relay" unless Relay or the selected remote receiver confirms success.

Do not require a GitHub Issue when direct Relay submission succeeds. Relay may create an optional GitHub binding afterward according to Relay-side configuration.

## Local ingress

When the user asks for a local file, offline handoff, downloadable Relay task, or copy/paste payload:

- use `.relay.json` for the canonical machine artifact;
- use `.relay.md` only as a human-readable carrier that embeds the identical canonical JSON block;
- use canonical JSON for direct terminal paste;
- never invent a separate Markdown semantic schema.

Read `references/local-ingress.md`.

## Clarification and decision packets

Treat executor assessment as evidence, not authority.

Discuss the blocker with the user. Challenge assumptions when appropriate. Once the user decides, create a structured Resolution Packet containing task/request identity, decision, rationale, implementation direction, scope, and work disposition.

Prefer direct Relay resolution submission when an authorized Relay direct interface exists. Otherwise use an authorized bound-channel write-back when appropriate, or provide the Resolution Packet locally.

Read `references/clarification-resolution.md`.

## Preserve the boundary

Keep exploratory planning in the planning surface. Do not launch or simulate a coding runtime merely to continue product reasoning.

Do not describe Relay as bypassing quotas, billing, rate limits, authentication, or provider controls. Relay changes when a coding runtime is invoked; it does not alter provider metering.
