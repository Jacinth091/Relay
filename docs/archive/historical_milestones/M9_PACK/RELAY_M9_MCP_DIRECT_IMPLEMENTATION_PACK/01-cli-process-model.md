# M9 Phase 01 — CLI and Process Model

## Goal

`relay serve-mcp` exists as a headless, long-lived stdio process. The TUI remains the default behavior with no flags.

## Current state

- The single binary (`apps/relay-tui`, bin name `relay`) unconditionally enters raw-mode TUI in `main()`.
- clap is a declared technology baseline (AGENTS.md) but is not yet wired into the binary entry point.

## Requirements

1. Add clap parsing at the top of `main()`:
   - no arguments -> existing TUI startup path, byte-for-byte unchanged;
   - `relay serve-mcp` -> MCP server path (stub in this phase: read stdin lines, echo nothing, exit cleanly on EOF);
   - unknown subcommands/flags -> usage error, exit non-zero.
2. `serve-mcp` must NOT initialize crossterm raw mode, alternate screen, or any TUI state.
3. Logging via `tracing` to stderr only (stdout belongs to the protocol).
4. Graceful shutdown on stdin EOF; process exit code 0.
5. Configuration loading (TOML) and store path resolution are shared helpers used by both modes — extract if currently TUI-local.
6. Tests:
   - subcommand routing unit tests;
   - integration test spawning the binary with `serve-mcp`, writing nothing, closing stdin, asserting clean exit;
   - integration test that the TUI default still launches is out of scope for automation (raw mode); cover via the routing layer instead.

## Boundaries

- No protocol work in this phase.
- No store access in this phase beyond what shared config loading already does.
- Presentation code stays untouched apart from the entry-point split.

## Done when

- `relay --help` lists `serve-mcp`.
- `relay serve-mcp < /dev/null` (or `echo "" | relay serve-mcp`) exits 0 without touching the terminal.
- Full workspace gates pass.
