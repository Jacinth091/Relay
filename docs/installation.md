# Installation

> [!NOTE]
> This page describes **future release installation goals**, not currently published installer packages. The default branch contains an experimental Rust TUI prototype; to build and run it today, use [DEVELOPMENT.md](DEVELOPMENT.md). The capabilities below are not a promise that a packaged or production-ready release exists.

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

## Troubleshooting commands

A small optional CLI surface is still useful:

```text
relay doctor
relay version
relay config
```

These are support tools, not the primary product UX.
