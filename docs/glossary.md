# Glossary

## Relay

The local handoff engine/control plane that receives approved implementation intent and coordinates controlled coding-agent execution.

## Planner

The conversational or manual surface where implementation intent is formed.

## Planner Host

The concrete product surface in which the planner integration runs, such as ChatGPT, Claude Code, OpenCode, or GitHub Copilot.

## Planner Integration / Adapter

A provider-host-specific client that recognizes explicit handoff intent, constructs the Relay protocol object, and submits or exports it without changing Relay semantics.

## Handoff Task

The canonical user-approved implementation specification accepted by Relay.

## Submission Transport

How a HandoffTask reaches Relay. Implemented transports: Relay Direct/MCP (`relay serve-mcp`) and local file/paste. Future optional: GitHub ingress (descoped from v0.1 by ADR-0016).

## Ingress

A Relay-side receiving mechanism for tasks or protocol objects. Implemented today: local import and the Relay Direct MCP server. GitHub Issue ingestion is a future optional compatibility path, not current behavior.

## External Binding

An optional association between a canonical Relay task and a remote record such as a GitHub Issue. It may mirror status/history but is not Relay's canonical identity.

## Relay Direct

The implemented planner-to-Relay submission path in which a planner host calls Relay itself through MCP (`relay serve-mcp`) instead of writing to an external mailbox. M9 delivers Phase 1 — submission (`submit_handoff`) only; bidirectional capabilities such as status/result retrieval, trusted diff reading, and clarification/resolution loops are future phases, specified deliberately before implementation.

## Provider

The model/tool ecosystem behind a planner host or coding runtime.

## Runtime

The coding agent Relay executes.

## Runtime Transport

How Relay talks to that runtime: CLI, SDK, API, or another mechanism.

## Runtime Profile

Local configuration selecting a runtime, provider, transport, and native options.

## Capability

A feature a planner host, ingress, binding adapter, or runtime can actually support.

## Stack

An opinionated composition of planner adapter, submission preference, optional external binding policy, runtime profile, and provider-native behavior.

## Clarification Request

A structured request for explanation of already-approved intent.

## Decision Request

A structured request for a new material decision discovered during implementation.

## Resolution Packet

The approved answer that allows Relay to resume or restart execution.
