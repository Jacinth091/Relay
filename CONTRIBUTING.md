# Contributing to Relay

Relay is an **experimental personal hobby/development project**. It is shared so
people can learn from it, test ideas, and propose improvements. Issues and pull
requests are welcome, but there is **no guarantee of review, acceptance, support,
or response time**.

Be constructive and respectful. Small, focused contributions are easiest to
review.

## Start here

Read, in this order:

1. `README.md` — what Relay is and what currently works.
2. `docs/DEVELOPMENT.md` — build, test, and source layout.
3. `AGENTS.md` — invariants for coding agents working **on Relay**.
4. `docs/adr/0020-relay-architecture-v2.md` — the accepted architecture authority.
5. `docs/v2_architecture_rebaseline/ROADMAP.md` — the honest per-phase status table.

Check existing issues before starting substantial work, and open an issue to
discuss anything large before writing code.

## Supported platforms

**Windows with the MSVC toolchain is the only supported host today**, and CI
gates merges on it. The other leg of the CI matrix (`ubuntu-latest`) is
*advisory*: it is labelled `[advisory]`, its build steps tolerate their own
failure, and it never blocks a merge. Read its logs for the real errors, not its
check mark.

Linux fails for one concrete reason. The local Relay Daemon transport is a
Windows named pipe, gated `#![cfg(windows)]`
(`crates/relay-core/src/daemon_transport.rs` and the `daemon_runtime` module),
while `apps/relay-tui` (`main.rs`, `state.rs`, `mcp/*`) imports those types
unconditionally. Removing that blocker means either adding a platform-neutral
transport or compiling the daemon path out behind a platform gate.

A portability contribution is welcome. Until it lands, do not claim Linux or
macOS support in code, documentation, or pull request descriptions.

## Architecture and boundaries

Decide which layer owns a change before implementing it:

- `crates/relay-core` — deterministic domain engine (provider-agnostic, no LLM reasoning);
- `crates/relay-protocol` — `HandoffTask v1` wire format and validation;
- `crates/relay-store` — SQLite persistence;
- `crates/relay-runtime` — runtime/backplane seam and router;
- `runtimes/*` — provider adapters (Codex, Antigravity, fake);
- `integrations/planners/*` — planner-side specifications and adapter documents;
- `apps/relay-tui` — presentation client (must not own policy).

Preserve the product invariants in `AGENTS.md`: explicit human approval, trusted
local project resolution, no arbitrary execution paths, no automatic push, and
fail-closed behavior for unknown protocols or malformed input. Do not put
provider-specific shortcuts in `relay-core`.

Changes to contracts, product invariants, persistence, or the security boundary
require a corresponding ADR (`docs/adr/`), updated documentation, and boundary
tests in the same change.

## Validation

Run the workspace gates before opening a pull request:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked
cargo test --workspace --locked
```

State clearly which checks you ran, skipped, or that failed. Do not claim
compatibility with an OS, provider, or integration you did not actually test.
Tests are expected to stay hermetic: use the fake runtime and temporary
directories, never real provider accounts, quotas, or credentials.

## Pull requests

Keep PRs small and coherent. Describe the problem, the scope, the affected
architectural layers, any security or invariant implications, the tests you ran,
and the documentation you updated. Split large changes rather than expanding one
PR indefinitely.

## Licensing

The repository is released under the [MIT License](LICENSE). Unless you state
otherwise, contributions submitted for inclusion are offered under the same MIT
terms. By contributing you represent that you have the right to submit the
material and agree to its distribution under that license. No CLA is required.

Do not submit secrets, credentials, private logs, personal data, private task
handoffs, or unlicensed third-party content.

## Security

Do not report vulnerabilities in public issues. Follow [SECURITY.md](SECURITY.md).
