# Relay v2 Architecture Rebaseline Handoff Pack

**Status:** accepted forward architecture, partially implemented (see [ADR-0020](../adr/0020-relay-architecture-v2.md))  
**Date:** 2026-09-17  
**Product:** Relay  
**Architecture generation:** v2  
**Implementation status:** per-phase status tracked in [ROADMAP.md](ROADMAP.md); no phase is complete until its production exit gate is proven  

> **Reading note:** this pack is the 2026-09-17 proposal. The documents below
> describe the intended v2 architecture, not a claim about current behavior.
> Check [ROADMAP.md](ROADMAP.md) (status-dated) for what has actually been
> implemented and wired into the production path. Individual handoff documents
> may carry a "not implemented" status line from the date they were written;
> that is historical and is intentionally left as-is.

## Why v2

Relay has one clear architectural predecessor: the deterministic handoff/execution engine built through the original v0.1 milestone lineage. Calling this rebaseline **Relay Architecture v2** gives the project an explicit generational break without pretending that a third architecture already existed.

The original architecture remains historical evidence. Accepted milestone snapshots, old ADRs, and the M0-M9 implementation lineage must not be rewritten to make them look as though they always described v2.

## Read order

1. `00-GOVERNED-BRANCH-AUDIT.md`
2. `01-ARCHITECTURE-HANDOFF.md`
3. `02-IMPLEMENTATION-SEQUENCE.md`
4. `03-DOMAIN-CONTRACTS.md`
5. `04-UI-UX-HANDOFF.md`
6. `05-HERDR-INTEGRATION-HANDOFF.md`
7. `06-EXCHANGE-DELEGATION-HANDOFF.md`
8. `07-SEMANTIC-DECISION-ENGINE-HANDOFF.md`

## v2 architecture in one paragraph

Relay v2 is a local-first, policy-governed orchestration control plane for user-approved software work. The human owns intent and privileged decisions. Relay owns durable tasks, approved intent, execution grants, workflows, roles, attempts, policy, provider resolution, artifacts, delegation, approvals, candidates, leases, scheduling, recovery, and user-facing semantic state. Runtime backplanes host processes but do not own tasks; Herdr is the default/reference interactive backplane, while a local-process backplane preserves a minimal non-Herdr execution path. AI CLI providers fill Relay roles through provider adapters. Typed Relay Exchange is the normal communication mechanism. Optional semantic decision engines such as TypeSafe AI Jev may advise bounded choices after deterministic filtering, but they cannot create authority or become required for correctness.

## Canonical ownership

```text
Human
  owns approved intent and privileged decisions

Relay
  owns task/workflow/role semantics and durable orchestration truth

Runtime backplane
  owns process/session hosting and observations

Provider adapter
  understands provider-specific launch/readiness/result behavior

Provider CLI
  performs one Relay-assigned role attempt
```

## Governance action before implementation

This action has been completed: [ADR-0020: Relay Architecture v2](../adr/0020-relay-architecture-v2.md) is **Accepted** (2026-09-17) and:

- declares Relay Architecture v2;
- marks ADR-0017 and the incompatible portions of ADR-0018 as superseded while preserving their history;
- freezes the original v0.1/M0-M9 lineage as historical implementation evidence;
- marks the old unimplemented provider-distribution M10 direction superseded/on hold;
- defines the v2 compatibility obligations for `HandoffTask v1`, `relay serve-mcp`, explicit Run, no automatic push, trusted local path resolution, and provider-neutral core semantics.

Note: `ADR-0019` was already assigned to an unrelated, still-`Proposed` ADR (`docs/adr/0019-multi-runtime-orchestration.md`, renumbered from 0009) before this pack was written. ADR-0020 is the correct and current authority for this architecture generation; do not create a duplicate ADR-0019 for it.

## Implementation principle

Do not implement the whole architecture horizontally. Build v2 as dependency-ordered vertical slices. The first new end-to-end proof must work with deterministic/fake infrastructure before Herdr or Jev is required.

## Non-goals of this pack

This pack does not:

- claim that v2 is already implemented;
- rewrite accepted historical milestone snapshots;
- authorize automatic Git push, production deploy, secret access, destructive database changes, or sandbox bypass;
- require Herdr for Relay core correctness;
- require Jev or any external semantic service;
- define provider billing/authentication workarounds;
- create uncontrolled recursive AI-to-AI spawning.
