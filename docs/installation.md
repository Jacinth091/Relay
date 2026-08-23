# Installation

> [!NOTE]
> Relay is not implemented yet. This document describes the intended installation experience and is a product/UX target, not a claim that release packages already exist.

## Goal

Installing Relay should be separate from configuring a planner, ingress/submission transport, external binding, or coding runtime.

The normal entry point should eventually be:

```text
relay
```

Users should not need to memorize a large command surface for everyday use.

## Intended release packages

A future release may publish prebuilt binaries for supported platforms through GitHub Releases or another official distribution channel.

Conceptual examples:

```text
relay-v0.x.y-windows-x86_64.zip
relay-v0.x.y-linux-x86_64.tar.gz
relay-v0.x.y-macos-aarch64.tar.gz
```

Package-manager distribution such as Cargo, Homebrew, Scoop, or WinGet may be added later after the basic release process is stable.

Do not document an installer command as working until it is actually published and tested.

## First launch

If no Relay configuration exists, launching `relay` should open the first-run setup flow rather than fail with a list of required commands.

See `first-run-setup.md`.

If the user selects the GitHub mailbox bootstrap transport, setup should also guide them to create or select their own private Relay mailbox repository. See `github-mailbox-setup.md`. Direct/MCP users do not need a mailbox.

## Planner submission transport (`relay serve-mcp`)

Relay ships a headless MCP stdio server alongside the TUI:

```text
relay serve-mcp
```

Planner hosts that speak MCP register this command as a local stdio MCP
server to submit handoffs directly (one tool: `submit_handoff`). Per-host
registration recipes live under `planner-hosts/`. Submission only persists the
task in Ready state; execution still requires explicit Run approval in the TUI.

## Troubleshooting commands

A small optional CLI surface is still useful:

```text
relay doctor
relay version
relay config
```

These are support tools, not the primary product UX.
