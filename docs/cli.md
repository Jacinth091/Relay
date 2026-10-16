# Relay CLI & Command Reference

## Binary Commands

The executable is named `relay` (crate `relay-tui`).

```text
relay                  Start the TUI (default behavior with no arguments)
relay run-tui          Explicitly start the TUI
relay serve-mcp        Run the Relay Direct MCP server over stdio (headless)
relay daemon           Run the durable Relay daemon (store-lease ownership + named-pipe client transport)
relay --help           Show usage
relay --version        Show version
```

Unknown subcommands or flags produce a clap usage error on stderr and exit code 2.

> There is **no** top-level `relay doctor` or `relay config` subcommand. `/doctor`
> is a TUI command-palette entry (below), not a CLI subcommand.

### `relay serve-mcp`

Long-lived headless process speaking MCP over stdio (newline-delimited JSON-RPC 2.0). Its lifetime is owned by whichever planner host spawns it; it exits 0 when stdin closes.

- Exposes one tool: `submit_handoff` — accepts one canonical HandoffTask v1 JSON object and returns `{ task_id, state: "Ready" }` after durable persistence.
- Submitted tasks never auto-execute; running them requires explicit Run approval in the TUI.
- Diagnostics go to stderr only; stdout carries protocol messages exclusively.
- Host registration recipes: [Claude Code](planner-hosts/claude-code.md), [OpenCode](planner-hosts/opencode.md), [Codex CLI](planner-hosts/codex-cli.md).

This server is **local stdio**. A cloud-hosted planner that cannot spawn local
processes cannot reach a local Relay instance through it.

### `relay daemon`

Long-lived process that takes durable ownership of the local store: it acquires a
lease, prints its fencing token, and renews a heartbeat until it is shut down.
This is the v2 Phase 01 daemon-ownership slice, extended by Wave A so that it is
also the **sole canonical semantic writer** for task submission: the TUI and
`relay serve-mcp` submit durable mutations through it instead of writing the
store directly. The client transport is a **Windows named pipe** at this
revision; portability is tracked separately (see
[Supported platforms](CONTRIBUTING.md#supported-platforms)).

| Flag | Default | Purpose |
|---|---|---|
| `--lease-id <id>` | `relay-daemon-primary` | Lease identity for the daemon's write ownership |
| `--lease-ttl <seconds>` | `15` | Lease time-to-live |

```sh
cargo run --locked -p relay-tui -- daemon
cargo run --locked -p relay-tui -- daemon --lease-id relay-daemon-primary --lease-ttl 30
```

Starting the daemon is **not required** for ordinary local use: when no daemon is
already running, the TUI and `relay serve-mcp` embed one in-process
(`connect_or_embed`), so neither needs a separately started daemon.

## Environment Variables

| Variable | Purpose |
|---|---|
| `RELAY_CONFIG_PATH` | Override config file location |
| `RELAY_DATABASE_PATH` | Override SQLite database location |
| `RELAY_CODEX_TRANSPORT` | Set to `exec` to opt out of the default Codex `app-server` transport |
| `RELAY_TUI_INPUT_TRACE` | Enable TUI input diagnostics |

## TUI Command Palette

Press `/` in the TUI to open the palette. Type an exact command and press Enter to execute it; suggestions autocomplete otherwise.

### Navigation

| Command | Description |
|---|---|
| `/home` | Go to home dashboard |
| `/tasks` | Browse durable tasks |
| `/projects` | Browse trusted projects |
| `/runtimes` | Browse available coding runtimes |
| `/settings` | Open Relay configuration settings |
| `/doctor` | Run system diagnostics & health check |
| `/help` | Show command reference |
| `/quit` | Exit Relay TUI |

### Tasks

| Command | Description |
|---|---|
| `/new` | Create or import a Relay handoff task |
| `/run <task>` | Approve and execute a READY task |
| `/cancel <task>` | Cancel a running or queued task |
| `/diff [task]` | View read-only repository git diff |
| `/logs [task]` | View execution logs for a task |

Task arguments accept short ids; completion suggests eligible tasks per command (e.g., only READY tasks for `/run`).

### Projects

| Command | Description |
|---|---|
| `/project add [path]` | Register a local repository project |

### Runtime Selection

| Command | Description |
|---|---|
| `/runtime <provider>` | View runtime provider details & health |
| `/runtime use <profile>` | Select global default runtime profile |
| `/runtime model <model>` | Select global default runtime model |
| `/runtime reasoning <effort>` | Select global default runtime reasoning effort |
| `/runtime test <provider>` | Recheck provider prerequisites & probe health |

One-run overrides are also available from the Task Review screen's model/reasoning selectors; they apply to a single approved run without changing global settings.

## Common Flows

```text
# Plan elsewhere, submit via MCP (task arrives Ready)
<planner host> -> relay serve-mcp -> submit_handoff

# Local import
relay          -> /new -> import file / paste -> review -> R to approve Run

# Inspect results
/tasks         -> select task -> /diff, /logs, result screen
```
