# M9 Phase 03 — MCP Protocol Layer and `submit_handoff`

## Goal

A planner host speaks MCP over stdio to `relay serve-mcp` and receives a real Relay `TaskId` for a durably persisted HandoffTask.

## Protocol surface (minimal subset)

1. `initialize` handshake: respond with server name/version and capabilities; reject unsupported protocol versions with a structured error.
2. Notifications the spec requires (e.g. `initialized`) are accepted silently.
3. Tool: `submit_handoff`
   - Input: one canonical HandoffTask v1 JSON object (same schema as local paste import — `crates/relay-protocol`, `docs/local-ingress.md`). No alternate shapes, no templates.
   - Behavior:
     a. parse + validate through the same protocol path as local ingress (duplicate-key rejection, `deny_unknown_fields`, version check, size caps);
     b. resolve `project` against the trusted local project registry — unknown project fails closed;
     c. reject non-empty `extensions` at the boundary;
     d. persist via the existing confirm/import persistence path, assigning a Relay-owned `TaskId`;
   - Output: `{ "task_id": "<relay task id>", "state": "Ready" }`.
4. Error contract: every failure maps to a structured JSON-RPC error with a stable code and human-readable message. Never claim success unless the store confirmed durability (`submission-and-bindings.md` rule).

## Acceptance semantics (ADR-0016)

Local import is two-phase (preview -> human confirms). Machine-initiated submission collapses preview into an inline validation step; the human review moves to the TUI before Run. The task lands in `Ready`; nothing executes because of an MCP call. Record this in the tool's documentation text returned by the MCP `tools/list` description.

## Implementation choice

- Prefer a minimal hand-rolled stdio JSON-RPC layer if it stays small (~initialize + tools/list + tools/call); otherwise adopt an SDK crate (e.g. `rmcp`) and record license + notices in `THIRD_PARTY_NOTICES.md`.
- Whichever is chosen: no `unsafe`, no blocking reads on the async runtime, framed per the MCP stdio convention.

## Tests

- Unit: request/response serialization round-trips, error-code mapping table.
- Integration (in-process): handshake -> tools/list shows `submit_handoff` -> valid submit returns TaskId -> store contains the task exactly as TUI listing would show it.
- Negative conformance: bad version, unknown field, duplicate key, unknown project, oversized payload, extensions present, store failure injection.
- The scheduler must not be constructible by the server path; assert acceptance never enqueues execution.

## Done when

- A real planner host can complete the flow manually against a locally running server.
- Full workspace gates pass.
