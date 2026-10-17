# Installation

> [!NOTE]
> Relay is a source-buildable experimental prototype. **No release is published
> yet, so the one-line installer below cannot resolve a package yet.** A release package is produced by
> [`.github/workflows/release.yml`](../.github/workflows/release.yml) when the
> maintainer pushes a version tag (`v*`), so whether a download exists is a fact
> about the
> [Releases page](https://github.com/Jacinth091/Relay/releases) — check it there,
> and expect it to be empty until the first version tag is pushed. Until a release
> is listed, build from source (below) or
> `cargo install --git https://github.com/Jacinth091/Relay --locked relay-tui`.

## One-line installer (Windows, after a release is published)

Works once the repository is public and a non-prerelease release is listed on the
Releases page. Until then it fails to find a release; use the other paths on this
page.

```powershell
irm https://raw.githubusercontent.com/Jacinth091/Relay/main/install.ps1 | iex
```

The script ([`install.ps1`](../install.ps1)) downloads the release zip and its
`.sha256`, refuses to install on a checksum mismatch, copies `relay.exe`, `LICENSE`
and `README.md` into `%LOCALAPPDATA%\Programs\Relay`, and adds that folder to the
user `PATH` if it is absent. Re-running it upgrades. `-Uninstall` removes the folder
and its `PATH` entry and keeps `%APPDATA%\Relay`. Parameters (`-IncludePrerelease`,
`-Version`, `-InstallDir`) need the scriptblock form; the README has the commands
and the inspect-first alternative.

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

## Prebuilt releases (only when one is listed)

The release workflow
([`.github/workflows/release.yml`](../.github/workflows/release.yml)) builds and
publishes a package when the maintainer pushes a version tag (`v*`). Published
asset names:

```text
relay-<tag>-windows-x86_64.zip
relay-<tag>-windows-x86_64.zip.sha256
```

Windows x86_64 is the only platform built today; there is no Linux or macOS
package, no installer, and no package-manager entry. A tag containing `-` (for
example `v0.1.0-rc.1`) is published as a **prerelease**.

If the Releases page lists a release, download the zip and its `.sha256` file,
verify the checksum, unzip it and run `relay.exe` from Windows Terminal or
PowerShell. The full walkthrough — checksum verification, the SmartScreen prompt
for an unsigned binary, and adding the folder to `PATH` — is in
[README.md](../README.md#download-a-release-windows-x86_64). Running real work
still requires your provider's CLI (for example Codex) installed and
authenticated separately.

> **No prebuilt release exists yet.** None of the asset names above can be
> assumed to resolve until the Releases page shows one. Do not document an
> installer command as working until it is actually published and tested.

## Goal

Installing Relay should be separate from configuring a planner, a submission
transport, an external binding, or a coding runtime.

The normal entry point should be:

```text
relay
```

Users should not need to memorize a large command surface for everyday use.
For the command surface that exists today, see [cli.md](cli.md).

## Future distribution

Additional platforms (Linux, macOS) and package-manager distribution such as
Homebrew, Scoop or WinGet are **not implemented**. `cargo install --git` (above)
and building from a clone are the only paths besides a published Windows release.

> Do not document an installer or package-manager command as working until it is
> actually published and tested.

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
