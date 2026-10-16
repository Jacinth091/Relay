# Installation

> [!NOTE]
> Relay is a source-buildable experimental prototype. There is **no published
> installer, package-manager entry, or prebuilt binary release** yet. Build from
> source. The sections about future packages below describe intent, not shipped
> artifacts.

## Available today: build from source

Relay builds and tests from source with a stable Rust toolchain. See
[DEVELOPMENT.md](DEVELOPMENT.md) for the full guide.

```sh
git clone https://github.com/Jacinth091/Relay.git
cd Relay
cargo build --workspace --locked
cargo run --locked -p relay-tui
```

`relay-tui` produces the `relay` executable. Building requires:

- Git;
- Rust stable 1.85 or newer with Cargo (Edition 2024);
- on Windows with the MSVC toolchain, the Microsoft C++ build tools and Windows SDK
  (use a Developer Command Prompt).

A coding-agent CLI, API key, hosted service, or GitHub mailbox is **not**
required to build, test, or start the TUI. Executing a real provider run
additionally requires that provider's CLI, installed and authenticated by you.

## Goal

Installing Relay should be separate from configuring a planner, a submission
transport, an external binding, or a coding runtime.

The normal entry point should be:

```text
relay
```

Users should not need to memorize a large command surface for everyday use.
For the command surface that exists today, see [cli.md](cli.md).

## Future release packages

A future release may publish prebuilt binaries for supported platforms through
GitHub Releases or another official channel. Conceptual examples:

```text
relay-v0.x.y-windows-x86_64.zip
relay-v0.x.y-linux-x86_64.tar.gz
relay-v0.x.y-macos-aarch64.tar.gz
```

Package-manager distribution such as Cargo, Homebrew, Scoop, or WinGet may be
added later, after the release process is stable.

> None of these exist yet. Do not document an installer command as working until
> it is actually published and tested.

## First launch

If no Relay configuration exists, launching `relay` should open a first-run
setup flow rather than fail with a list of required commands. That flow is a
design target — see [first-run-setup.md](first-run-setup.md).

A GitHub-mailbox bootstrap transport is **not** part of v0.1 (descoped by
[ADR-0016](adr/0016-mcp-first-submission-and-v0-1-local-descope.md)). If it
returns as a future optional path, its setup guidance is preserved at
[archive/legacy_v1/github-mailbox-setup.md](archive/legacy_v1/github-mailbox-setup.md).
Direct/MCP users do not need a mailbox.

## Planner submission transport (`relay serve-mcp`)

Relay ships a headless MCP **stdio** server alongside the TUI:

```text
relay serve-mcp
```

Planner hosts that can spawn a local stdio MCP server register this command to
submit handoffs directly (one tool: `submit_handoff`). Per-host registration
recipes live under [planner-hosts/](planner-hosts/). Submission only persists
the task in `Ready` state; execution still requires explicit Run approval in the
TUI.

`relay serve-mcp` is local-only: a cloud-hosted planner that cannot spawn local
processes cannot reach your Relay instance through it. Such hosts use local
file/paste import of the same canonical task payload instead.

## Troubleshooting commands

A small optional CLI surface is a design target:

```text
relay doctor
relay version
relay config
```

> These are **not implemented** as CLI subcommands. The TUI exposes a `/doctor`
> command palette entry for diagnostics, and `relay --version` prints the
> version. Do not advertise `relay doctor` or `relay config` as working until
> they exist in `apps/relay-tui/src/cli.rs`.
