# Future Frontends

## Architectural Rule

Relay is not a TUI application.

Relay is one handoff engine whose first client is a TUI.

---

## TUI

Initial client:

```text
apps/relay-tui
```

Technology:

```text
Ratatui + Crossterm
```

---

## Desktop

Possible future client:

```text
apps/relay-desktop
```

Preferred shell:

```text
Tauri 2
```

The desktop client calls the same Relay Core.

---

## Local Web

Possible future mode:

```text
relay serve
```

Preferred Rust HTTP layer:

```text
Axum
```

Potential architecture:

```text
Browser
   |
HTTP / SSE / WebSocket
   |
relay-server
   |
Relay Core
```

---

## Relay Direct Planner Submission

A future MCP/direct planner integration is the preferred automatic planner submission path when available. It reaches the same Relay engine without requiring a GitHub Issue.

It does not require a second Relay implementation.

```text
ChatGPT
   |
   v
Relay Direct / MCP submission
   |
   v
Relay Core
```

The existing GitHub ingress may remain available as a compatibility path. Separately, GitHub may be configured as an optional external binding for durability/collaboration.

---

## Multiple Clients

If Relay later needs simultaneous clients, introduce a local service/daemon only when that requirement is real.

Possible future:

```text
relay daemon
   |
   +-- TUI client
   +-- Desktop client
   +-- Browser client
   +-- Relay Direct/MCP submission
   +-- GitHub ingress compatibility
   `-- optional external bindings
```

Do not begin with a daemon merely because it might be useful later.
