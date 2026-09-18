# Relay M9 — Relay Direct (MCP) Sequential Implementation Pack

**Status:** DEV implementation sequence
**Decision basis:** ADR-0016 (`docs/adr/0016-mcp-first-submission-and-v0-1-local-descope.md`)
**Purpose:** Deliver `relay serve-mcp` as the first automatic submission transport, in dependency order, without collapsing the work into one oversized document.

## Milestone goal

A planner host running on the same machine can start `relay serve-mcp` over stdio and call a `submit_handoff` tool that returns a real Relay `TaskId` after durable persistence. The TUI remains the review/Run surface. No task auto-executes from MCP submission.

## Non-goals for M9

- No remote/network MCP transport (deferred; see ADR-0016 consequences).
- No headless execution: the server accepts tasks, it does not run them.
- No GitHub ingress or mailbox code.
- No clarification/resolution round-trip over MCP in phase 01–03 (designed for by schemas; deferred to a later pack).

## Implementation order

### 01 — CLI and process model

File: `01-cli-process-model.md`

Introduce clap subcommands to the binary without breaking raw-mode TUI startup:

```text
relay            -> existing TUI behavior (default)
relay serve-mcp  -> long-lived headless stdio JSON-RPC process
```

The MCP server owns no scheduler. It shares only what it must: configuration, project registry, and the SQLite store. Its lifetime is owned by the planner host that spawned it.

Exit paths must be clean: stdin close ends the process gracefully; no orphaned locks.

### 02 — Store concurrency hardening

File: `02-store-concurrency.md`

Today `SqliteStore` assumes a single owner because all state lived inside the TUI. Two processes (TUI + MCP server) will now open the same database file.

Implement and prove:

- WAL journal mode with busy timeout on every connection path;
- retry-on-locked behavior for transient contention;
- an integration test that opens two stores against one file and interleaves writes without corruption or lost updates;
- documented behavior when the schema needs migration under a second live reader.

Do not proceed to 03 until this is proven. This is the largest engineering unknown of M9.

### 03 — MCP protocol layer and submit_handoff

File: `03-mcp-protocol-submit-handoff.md`

Implement the MCP stdio surface:

- initialize handshake and capability negotiation;
- a `submit_handoff` tool accepting exactly one canonical HandoffTask v1 payload;
- acceptance semantics per ADR-0016: machine-initiated submission is single-phase at the transport boundary but lands in `Ready` state through the same validation and persistence path used by local import (`preview` semantics inline, then `confirm_task_import_with_store`);
- response contract: return the assigned `TaskId`; never claim success unless the store confirmed durability; map every failure branch to structured errors (parse, version, unknown project, unsupported extensions, store failure);
- fail closed on every error; no partial states.

Protocol implementation choice (hand-rolled minimal subset vs SDK such as `rmcp`) is made here; if an SDK is chosen, its license goes into `THIRD_PARTY_NOTICES.md`.

### 04 — Conformance tests and host integration docs

File: `04-conformance-and-host-docs.md`

- End-to-end test: spawn `relay serve-mcp`, perform handshake, submit a valid task, assert persisted state matches what the TUI would list.
- Negative conformance cases: bad version, unknown fields, duplicate keys, unknown project, oversized input, store-closed mid-write.
- Host recipes as documents only (no code): Claude Code, OpenCode, Codex CLI — how to register the local MCP server and instruct the agent to use `submit_handoff`.
- Update `docs/submission-and-bindings.md` and `docs/status-2026-08.md` once the transport works end-to-end.

## Quality gates (all phases)

- `cargo fmt --check`
- `cargo clippy --workspace --all-targets --all-features`
- `cargo test --workspace`
- Focused integration tests at each new boundary (process spawn, concurrent store access, protocol error handling) per AGENTS.md quality requirements.

## Invariant notes

- Invariants 3/5 (explicit handoff): satisfied because the human crossed the boundary inside the planner before invoking the tool; recorded by ADR-0016.
- Invariant 7 (no LLM reasoning in core): the server is deterministic plumbing.
- Invariant 22 (canonical local state): MCP submission writes canonical local state; nothing external is created.
- Invariant 23 (transport preference order): after M9, update the preference text — Direct first, then fallbacks.
