# ADR-0010: Controlled Execution Autonomy and Structured Escalation

Status: Accepted

## Context

Requiring a user decision for every failed test or implementation mistake would make Relay tedious.

Allowing a coding runtime to silently make architecture/product decisions would undermine human ownership of intent.

## Decision

After explicit handoff, Relay may autonomously repair and retry implementation within approved scope.

When continuing requires materially changing or clarifying approved intent, Relay enters:

- `NEEDS_CLARIFICATION`, or
- `NEEDS_DECISION`.

Escalation must produce a structured packet explaining work completed, blocking reason, evidence, options, executor assessment, repository state, verification state, and resume point.

The user/planner returns a structured Resolution Packet.

## Consequences

Relay can reduce coding-agent babysitting without becoming an autonomous product planner.

Recovery loops require local limits.

Clarification/resolution becomes a first-class protocol and persistence concern.
