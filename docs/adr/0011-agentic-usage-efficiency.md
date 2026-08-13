# ADR-0011: Agentic Usage Efficiency Without Circumvention

Status: Accepted

## Context

Relay was partly motivated by workflows where a conversational planning surface is readily available while coding-agent execution is constrained by quotas, rate limits, credits, or workspace policy.

Using a coding runtime for long exploratory planning can consume capacity that the developer would prefer to reserve for implementation.

Provider usage systems can change over time.

## Decision

Relay treats coding-runtime usage as a resource that should begin at the explicit handoff boundary by default.

Relay may expose provider-reported usage metadata when available.

Relay must not bypass, evade, manipulate, or circumvent provider billing, authentication, quotas, or usage controls.

## Consequences

Usage efficiency becomes an explicit product motivation.

Public messaging must describe Relay as workflow optimization rather than quota circumvention.

Provider-specific quota models remain outside Relay Core and must not be hard-coded as universal assumptions.
