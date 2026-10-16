# M9 Phase 04 — Conformance Tests and Host Integration Docs

## Goal

Prove the transport end-to-end and make it usable from real planner hosts with zero Relay-side code per host.

## Conformance tests

Spawn the actual binary (not in-process) in integration tests:

1. happy path: spawn `relay serve-mcp` -> initialize -> tools/list -> `submit_handoff` valid task -> assert TaskId -> terminate server -> open store directly and assert task exists in `Ready`;
2. error cases over the wire: malformed JSON-RPC, unknown tool, invalid task payload, unknown project — each returns structured errors, server stays alive for subsequent requests;
3. lifecycle: client disconnect closes the process cleanly; two sequential sessions against one store both work.

## Host recipes (documents only)

Add under `integrations/planners/` or `docs/` following existing conventions:

- `claude-code.md` — registering a local stdio MCP server, agent instructions for using `submit_handoff`, "Send to Relay" phrasing;
- `opencode.md` — same;
- `codex-cli.md` — same.

Each recipe must state:

- the exact registration config snippet;
- that submission lands the task in `Ready` and requires TUI Run approval (invariant 13);
- fallback: if MCP is unavailable, use paste/file import (ADR-0016 layering).

Update the ChatGPT adapter document to point at Layer 1/3 paths per ADR-0016; remove any implication of host-action coupling.

## Doc reconciliation

- `docs/submission-and-bindings.md`: mark Direct/MCP as implemented, adjust preference order text (invariant 23).
- `docs/status-2026-08.md`: add M9 outcome.
- `docs/roadmap.md`: tick the Direct/MCP item; GitHub remains descoped by ADR-0016.
- `docs/installation.md` / `docs/first-run-setup.md`: mention `relay serve-mcp` setup where transports are listed.

## Done when

- All conformance tests pass repeatedly on Windows.
- Docs updated; full workspace gates pass.
- A human has completed at least one real handoff from a local planner host through MCP into Relay review.
