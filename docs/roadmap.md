# Roadmap

Milestone tracking has moved to the M-ladder (M1 through M8.1). See
[docs/status-2026-08.md](status-2026-08.md) for a current end-to-end status
summary and [docs/M8_1_PACK/RELAY_M8_1_SEQUENTIAL_IMPLEMENTATION_PACK/00-IMPLEMENTATION-SEQUENCE.md](M8_1_PACK/RELAY_M8_1_SEQUENTIAL_IMPLEMENTATION_PACK/00-IMPLEMENTATION-SEQUENCE.md)
for the M8.1 implementation sequence.

## Current Status — Milestone Ladder

Tracked by commits, in order:

- M1 — TUI shell + core boundary: **done**.
- M2 — project registry: **done**.
- M3 — repository safety: **done**.
- M4 — task semantics: **done**.
- M5 — SQLite durable state: **done**.
- M6 — scheduler: **done**.
- M7 — explicit Run approval: **done**.
- M8 — Codex runtime: **done**, including both `codex exec` and `codex app-server`
  transports; app-server is now the default rich-client path.
- M8.1 — multi-runtime orchestration: **mostly landed**. Per the M8.1 pack,
  items 01–03 are complete and items 04–05 are partial:
  - RuntimeRouter and ExecutionRunPolicy are implemented;
  - a second runtime (Antigravity) is wired through RuntimeRouter;
  - command-palette TUI overhaul, model/reasoning selection, Diff/Logs screens,
    Doctor view, and the permission broker with scoped grants are shipped;
  - provider-neutral Markdown rendering and full scrollable execution/result UX
    polish remain incomplete (pack items 04 and 05).

Capabilities shipped ahead of earlier plan documents: second runtime
(Antigravity) + RuntimeRouter, Codex app-server transport as default,
command palette, model/reasoning selectors, permission broker with scoped
grants, Diff/Logs screens, Doctor view.

Not started, despite appearing in older plans:

- GitHub ingress / `gh` adapter: **descoped from v0.1 by
  [ADR-0016](adr/0016-mcp-first-submission-and-v0-1-local-descope.md)**. GitHub
  remains an optional future compatibility path whose issue-writer must be
  host-agnostic. It currently appears only as expected-remote metadata in
  repository safety checks.
- Relay Direct / MCP submission transport: **implemented (M9) on
  `feat/m9-relay-direct-mcp`, pending merge**. `relay serve-mcp` provides the
  MCP stdio submission transport (`submit_handoff`); host recipes under
  [docs/planner-hosts/](planner-hosts/). See
  [docs/M9_PACK](M9_PACK/RELAY_M9_MCP_DIRECT_IMPLEMENTATION_PACK/00-IMPLEMENTATION-SEQUENCE.md).
- Executable planner-adapter conformance harness: **planned, not started**.
  The conformance cases exist only as documentation
  ([docs/planner-adapter-conformance.md](planner-adapter-conformance.md)).

## v0.1 — Reliable Human-Controlled Handoff

Primary stack:

```text
ChatGPT -> local file/paste ingress -> Relay -> Codex CLI
```

The originally documented primary stack assumed a GitHub compatibility
transport. That leg is planned but not started; the working end-to-end ingresses
today are local file/paste plus the M9 `relay serve-mcp` MCP submission
transport (on `feat/m9-relay-direct-mcp`, pending merge).

Must include:

- one Relay engine — met;
- project registry — met;
- repository safety — met;
- task protocol — met;
- local JSON/Markdown file and paste ingress — met;
- durable state — met;
- GitHub ingress and optional GitHub binding — **descoped by
  [ADR-0016](adr/0016-mcp-first-submission-and-v0-1-local-descope.md)**; v0.1
  ships with local file/paste as the only ingress; automation arrives via the
  M9 Relay Direct/MCP transport instead;
- Codex runtime — met;
- live execution — met;
- bounded automatic repair — met;
- `NEEDS_CLARIFICATION` — met;
- `NEEDS_DECISION` — met;
- structured Markdown packets — met;
- Resolution Packets — met;
- no automatic push — met.

The GitHub bullet is descoped by ADR-0016: v0.1 ships local-ingress-only, and
the first automatic submission transport is Relay Direct/MCP (M9).

## v0.2 — Daily Driver

Some items already have early implementations shipped during M8/M8.1
(Diff/Logs screens, Doctor view). The remaining work is depth, not existence:

- stronger task history;
- project locks;
- better diff experience (basic Diff screen exists);
- clarification/resolution history;
- retry/restart controls;
- validation profiles;
- runtime diagnostics (Doctor view exists);
- `relay doctor`;
- bounded log retention.

## v0.3 — Native Stack Quality

- provider-neutral Relay Handoff Planner Specification;
- ChatGPT Skill as the first provider adapter;
- Codex-native capability improvements (partially delivered via the app-server
  transport and model/reasoning selection);
- project Skills/instruction awareness;
- optional provider-reported usage/limit display where reliable;
- better GitHub status synchronization (**blocked: GitHub ingress does not
  exist yet**).

## v0.4 — Second Ecosystem

- run planner-adapter conformance against Claude Code/OpenCode/Copilot as applicable;
- promote only adapters that pass real end-to-end validation;

Support a meaningfully different planner/runtime/ingress combination.

Use it to validate:

- runtime contracts;
- ingress contracts;
- stack composition;
- capability model;
- provider-native extensions;
- clarification/resolution portability.

## v0.5 — Extension Boundary

Design public out-of-process extension protocol from real integration needs.

## v1.0 — Open-Source Stable Core

Before v1.0:

- public license chosen;
- stable task/clarification protocol;
- stable config migration policy;
- runtime/ingress contributor docs;
- Windows/Linux/macOS release story;
- security documentation reviewed;
- upgrade path documented.

## Possible Later Directions

Optional, not promises:

- Tauri desktop client;
- local Axum server/browser client;
- Relay Direct/MCP submission (implemented in M9 on
  `feat/m9-relay-direct-mcp`);
- optional external binding configuration independent of submission transport;
- multiple execution lanes;
- remote Relay node;
- team task sources;
- richer source-control workflows;
- optional autonomous planning stacks — meaning external planner/runtime
  integrations consumed by Relay as clients (ADR-0014). This does NOT mean
  Relay Core embeds autonomous planning or strategy generation; that would
  conflict with ADR-0017 and require a new architectural decision.

Autonomous planning may be added as an option, but must not replace human-directed planning as Relay's default philosophy.
