# ADR-0016: MCP-First Submission Layering and v0.1 Local-Ingress Descope

## Status

Accepted for Foundation v1.8.

## Context

The v0.1 Definition of Done is satisfied by local file/paste ingress alone; no bullet requires GitHub or MCP. However, the originally documented primary stack assumed a GitHub compatibility transport (a planner host writing Issues into a user-owned mailbox repository), with Relay Direct/MCP as the preferred future path.

Two problems emerged before either transport was built:

1. **Host coupling.** The GitHub mailbox flow, as commonly imagined, depends on a specific planner host's feature surface (custom GPT actions, connectors). Planner hosts change these features unilaterally. Building Relay's first automatic submission path on one host's optional feature would couple Relay's headline experience to a vendor decision Relay cannot influence.
2. **Priority inversion.** The durable surfaces are open protocols and stable platforms. MCP is an open protocol supported by multiple independent planner hosts. GitHub is a stable platform, but the *writer* of a mailbox issue varies by host and era. Host-specific integration glue is the least durable layer of all.

This ADR records how Relay orders its submission investments so that no single vendor or host decision can strand the product.

## Decision

Relay Direct (MCP) becomes the first automatic submission transport Relay builds. GitHub ingress is descoped from v0.1 and remains an optional future compatibility path whose issue-writer must be host-agnostic.

Submission paths are layered by dependency durability:

```text
Layer 1 (universal fallback)   Local file / paste          always works, zero coupling
Layer 2 (primary automation)   Relay Direct / MCP          open protocol, many independent hosts
Layer 3 (optional bridge)      GitHub mailbox ingress      stable platform; writer must be replaceable
Layer 4 (never in core)        Host-specific glue          lives only in adapter documents/skills
```

Rules:

- Relay Core accepts tasks through typed ingress entry points only; transports are edges.
- The ChatGPT adapter (and every future host adapter) remains documentation/skill material inside `integrations/planners/`. No host-specific behavior enters Rust code.
- If a mailbox ingress is built later, Relay must not assume which client wrote the issue.
- v0.1 ships with local file/paste as the only working ingress. This is a deliberate descope recorded here, not an omission.

This refines, and does not contradict, ADR-0015 (canonical local state) and AGENTS.md invariants 20–23.

## Consequences

- The roadmap v0.1 checklist item "GitHub ingress and optional binding" moves out of v0.1 scope by explicit decision.
- Planner hosts that speak MCP (Claude Code, OpenCode, Codex CLI, others) gain automatic handoff without any per-host code in Relay.
- Web-hosted planners that cannot reach a local stdio server are served by Layer 1 today and may justify a remote/local-server MCP transport later; that decision is deferred.
- The M9 milestone implements `relay serve-mcp`, including the process model and store-concurrency groundwork it requires.
- Acceptance semantics for machine-initiated submission must be defined before M9 lands (the two-phase preview/confirm flow assumes a human at import time); this belongs to the M9 pack, backed by this ADR.
