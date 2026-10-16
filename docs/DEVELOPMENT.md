# Developing Relay

Relay is an experimental, local-first Rust application. The source tree is available for learning, experimentation, and contributions; it is **not** a stable, supported release.

## Requirements

- Git.
- Rust stable **1.85 or newer**, with Cargo (Rust Edition 2024).
- A normal interactive terminal for the TUI.
- On Windows when using the MSVC Rust toolchain, the required Microsoft C++ build tools and Windows SDK.

A coding-agent CLI, API key, hosted service, or GitHub mailbox is **not** required merely to compile and run the existing source prototype.

## Build and explore the current default branch

From the repository root:

```sh
cargo build --workspace --locked
cargo test --workspace --locked
cargo run --locked -p relay-tui
```

The binary package is named `relay-tui` and produces the `relay` executable. The current default-branch TUI uses the fake runtime for execution testing. Do not assume that other runtime integrations described in the design documents are implemented on `main`.

Relay stores configuration and task state locally. Review `docs/data-persistence.md`, `docs/security-model.md`, and `.gitignore` before using an existing, important repository. Start with a disposable Git repository.

## Quality checks

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked
cargo test --workspace --locked
```

These are also the proposed GitHub Actions checks on Ubuntu and Windows. A green build checks the source; it is not a security certification.

## Architecture guide

- `AGENTS.md`: invariants for coding agents working **on Relay**.
- `contexts/PROJECT_CONTEXT.md`: project definition and boundaries.
- `docs/architecture.md` and `docs/design-principles.md`: responsibilities and separation of concerns.
- `docs/adr/`: recorded architectural decisions.
- `crates/`: core, protocol, runtime interfaces, and storage.
- `apps/relay-tui/`: the terminal client.
- `runtimes/relay-runtime-fake/`: deterministic fake execution for development.
- `integrations/planners/`: planner-side handoff integrations.

Keep provider-specific behavior at integration/runtime edges. Preserve the explicit human approval boundary and fail-closed security behavior.

## Contributions and limits

Read `CONTRIBUTING.md` before opening a PR. Small bug fixes, focused tests, documentation improvements, and reproducible reports are particularly welcome.

This is a personal, experimental project with **no compatibility, uptime, or maintainer response-time commitment**. Please do not paste secrets, personal data, or private repository content into public issues, logs, or reproductions.

## Planned versus implemented

The repository includes architecture documents and planned workflows. They are **not** proof that a capability currently works. Inspect the code on the chosen branch, its tests, and the branch's recent pull requests before relying on a feature. In particular, release binaries, unattended operation, and provider integrations should not be advertised as supported until validated end to end.
