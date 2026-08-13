# Build Plan

This is the implementation sequence for Relay.

The goal is to prove boundaries in small vertical slices.

---

## Phase 0 — Repository Foundation

Done when:

- Foundation v1.6 documentation is committed;
- Rust toolchain is installed;
- Cargo workspace root exists;
- empty architecture is not over-generated.

---

## Phase 1 — TUI Shell + Core Boundary

Build:

```text
relay
```

that opens a Ratatui screen and exits cleanly.

Create minimum:

```text
apps/relay-tui
crates/relay-core
```

Prove that the TUI consumes core state rather than owning business logic.

---

## Phase 2 — Local Project Registry

Add typed TOML config.

User can register:

```text
project key
display name
local path
expected remote
default stack
```

TUI shows healthy/unhealthy state.

---

## Phase 3 — Repository Safety

Implement Git CLI wrapper.

Checks:

- directory exists;
- Git worktree is valid;
- origin can be read;
- expected remote matches;
- working-tree state is visible.

---

## Phase 4 — Persistence

Add SQLite + migrations.

Persist:

- known tasks;
- task states;
- executions;
- important events;
- clarification/decision requests;
- resolutions.

---

## Phase 5 — Task Protocol

Implement typed v1 JSON envelope.

Add local ingress for development:

```text
relay import task.relay.json
```

The TUI should also support **Import File** and **Paste Task** using canonical JSON or a Markdown wrapper with embedded Relay JSON.

This gives task-flow testing without GitHub and becomes a supported offline ingress, not just a test hack.

---

## Phase 6 — Fake Runtime + Recovery Loop

Before Codex, build a fake runtime executable.

Test:

- success;
- failure;
- verification failure;
- auto-repair;
- repair limits;
- clarification request;
- decision request;
- cancellation;
- huge output.

Implement structured clarification/decision packets here.

---

## Phase 7 — Runtime Registry + Core Recovery Policy

Create runtime abstractions from fake-runtime lessons.

Implement:

- capabilities;
- execution phases;
- bounded repair policy;
- intent-boundary escalation;
- Resolution Packet ingestion/resume behavior.

---

## Phase 8 — GitHub Integration and Binding Boundary

Implement the initial `gh` GitHub adapter for GitHub Issue ingestion and establish the separate external-binding abstraction.

Capabilities:

- fetch tasks;
- parse canonical Relay task block;
- map issue identity to Relay task;
- publish status;
- publish clarification/decision Markdown;
- ingest Resolution Packets when represented in the issue.

GitHub identity does not become core task identity.

---

## Phase 9 — Codex Runtime

Add `relay-runtime-codex`.

Codex integration:

- discover executable;
- report version/health;
- prepare task;
- run in validated cwd;
- stream output;
- capture exit state;
- support bounded repair;
- produce structured escalation;
- cancel when possible;
- declare capabilities.

---

## Phase 10 — End-to-End Reference Loop

Prove:

```text
ChatGPT-created GitHub Issue (v0.1 compatibility transport)
    ->
Relay
    ->
Review
    ->
Run
    ->
Codex
    ->
auto-repair if needed
    ->
DONE
```

and:

```text
Codex
    ->
NEEDS_DECISION
    ->
Relay packet
    ->
GitHub / Copy for Planner
    ->
Resolution
    ->
Relay resumes
```

No automatic push.

---

## Phase 11 — Planner Specification + Host Adapters

Finalize the provider-neutral Relay Handoff Planner Specification and conformance cases.

Ship the ChatGPT adapter as the first-class reference integration and keep concrete first-party adapters for Claude Code, OpenCode, and GitHub Copilot aligned with the same protocol.

The specification defines:

- explicit handoff semantics;
- task extraction/formatting;
- local artifact behavior;
- clarification/decision handling;
- Resolution Packets.

Host adapters add native installation and authorized tool/connector behavior. They remain clients of Relay, not separate implementations.

Before promoting an experimental adapter to first-class, run the shared conformance cases and an end-to-end handoff against that host.

---

## Phase 12 — Relay Direct / MCP Submission

Expose a direct planner-to-Relay submission interface without requiring GitHub.

Start from a transport-neutral core submission boundary, then add MCP where supported.

Required behavior:

- `submit_handoff` accepts HandoffTask v1;
- Relay assigns canonical local task identity;
- success is confirmed to the planner;
- GitHub binding is optional and Relay-managed;
- resolution submission can use the same direct path;
- local file/paste remains available.

The exact MCP packaging should be verified against supported planner hosts at implementation time.

---

## Phase 13 — Daily-Use Improvements

From real use, add:

- task history;
- diff viewer;
- better clarification UX;
- runtime usage visibility when reliable;
- validation profiles;
- project locks;
- richer repair telemetry.

---

## Phase 14 — Validate Agnosticism

Promote at least one non-ChatGPT planner adapter through real end-to-end validation, and implement one genuinely different runtime/ingress path.

Candidates:

- Claude Code planner -> Relay -> Codex;
- OpenCode planner -> Relay -> Codex;
- ChatGPT -> Relay -> Claude Code;
- local-file ingress.

Refactor only where the second implementation proves it is needed.
