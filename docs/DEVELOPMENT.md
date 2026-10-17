# Developing Relay

This is the maintainer and contributor onboarding guide for **Relay
Architecture v2**. It covers how to build, test, and navigate the source.

Relay is an experimental, local-first Rust workspace. The source tree is
available for learning, experimentation, and contribution. It is **not** a
stable or supported release.

Before changing behavior, read:

1. [AGENTS.md](../AGENTS.md) — invariants for coding agents working on Relay.
2. [ADR-0020](../docs/adr/0020-relay-architecture-v2.md) — accepted architecture authority.
3. [`docs/v2_architecture_rebaseline/ROADMAP.md`](v2_architecture_rebaseline/ROADMAP.md) — authoritative per-phase status.

## Requirements

- **Git**
- **Rust stable 1.85+** with Cargo. The workspace is Edition 2024
  (`rust-version = "1.85"` in the root `Cargo.toml`); current stable is fine.
- A normal interactive terminal for the TUI.
- **Windows only:** the MSVC toolchain needs the Microsoft C++ build tools and
  Windows SDK, and the linker (`link.exe`) must be discoverable. Use a **Developer
  Command Prompt / Developer PowerShell for VS**. Do not hardcode a
  `vcvarsall.bat` path in scripts — the installation path is machine-specific.

A coding-agent CLI, API key, hosted service, Herdr server, or GitHub mailbox is
**not** required to compile and test the workspace. Real provider execution
requires the matching provider CLI to be installed and authenticated by you, and
is never exercised by CI.

The workspace pins dependencies with a checked-in `Cargo.lock`; use `--locked`.

## Build, test, and run

From the repository root:

```sh
cargo build --workspace --locked
cargo test --workspace --locked
cargo run --locked -p relay-tui -- --help
```

The `relay-tui` package produces the `relay` binary. Entry points:

```sh
cargo run --locked -p relay-tui                 # TUI (default)
cargo run --locked -p relay-tui -- run-tui      # TUI (explicit)
cargo run --locked -p relay-tui -- serve-mcp    # MCP stdio submission server
cargo run --locked -p relay-tui -- daemon       # durable daemon
```

Run the TUI only against a disposable repository with isolated local state.
Relay persists SQLite state and can launch coding-agent processes.

### Local state and configuration

| Variable | Effect |
|---|---|
| `RELAY_CONFIG_PATH` | Override the config file location |
| `RELAY_DATABASE_PATH` | Override the SQLite database location |
| `RELAY_CODEX_TRANSPORT` | Set to `exec` to opt out of the default Codex `app-server` transport |
| `RELAY_TUI_INPUT_TRACE` | Enable TUI input diagnostics |
| `RELAY_DAEMON_PIPE_SUFFIX` | Test-only override for the daemon's named-pipe suffix |

Both `RELAY_CONFIG_PATH` and `RELAY_DATABASE_PATH` are useful for hermetic
manual testing: point them at a temporary directory so a run cannot touch your
normal local state.

You do **not** need to isolate the daemon transport by hand. The daemon's named
pipe is derived from the durable identity of the database it serves (a value
generated inside that database by migration 0007), so two databases never share
one transport address and a client cannot reach a daemon serving different state
— every request opens a connection, performs a state-identity handshake, and is
refused if the identities disagree. The daemon enforces this on its side too: a
connection is not admitted until a matching handshake succeeds, so a peer that
speaks the raw framing protocol without one cannot mutate state. The daemon also
fences its own durable writes on its lease, so a command admitted before a
takeover cannot land after the successor took over. `RELAY_DAEMON_PIPE_SUFFIX`
exists only for tests that deliberately need to influence the pipe name; leaving
it unset is the production behavior.

Relay reads repository-local `.relay/project.toml` and `.relay/policies.toml`
as **untrusted portable declarations**. Such declarations may *restrict* what a
repository is allowed to do, but they can never expand permissions beyond the
trusted local project registration. `.relay/` is git-ignored on purpose: it can
hold privileged local trust and runtime state. Do not make the whole directory
trackable.

## Workspace layout

```text
apps/relay-tui/            TUI entry point, CLI parsing, screens, MCP service,
                           daemon CLI wiring, app state
crates/relay-core/         Deterministic domain engine (no LLM reasoning):
                           task ingress, project registry, repository safety,
                           execution planning, scheduler, durable state, v2
                           domain modules (workflow, trust, candidates, leases,
                           artifacts, delegation, policy), daemon + effects
crates/relay-protocol/     HandoffTask v1 wire format and validation
crates/relay-runtime/      Runtime trait, backplane seam, profiles, router,
                           activity normalization, model catalog types
crates/relay-store/        SqliteStore and migrations
runtimes/relay-runtime-fake/  Scriptable runtime + backplane for tests
runtimes/relay-runtime-codex/ Codex adapter (app-server stdio default)
runtimes/relay-runtime-antigravity/
runtimes/relay-decision-jev/  Optional advisory decision adapter
integrations/planners/     Planner specification and adapter documents
schemas/                   Wire format schemas
```

### Where to make a change

- Task acceptance/validation → `crates/relay-core/src/task_ingress.rs`
- Scheduler/lifecycle/repair → `crates/relay-core/src/execution.rs` (v1 scheduler
  semantics are frozen; additive amendments only — see AGENTS.md)
- User-facing copy/screens → `apps/relay-tui/src/ui/`
- Runtime/provider behavior → the relevant `runtimes/*` crate plus
  `crates/relay-runtime/src/lib.rs`
- Submission transports / MCP → `apps/relay-tui/src/mcp/`, `docs/planner-hosts/`
- Persistence → `crates/relay-store/`

Presentation code must not own execution, ingress, project routing, runtime
selection, repository validation, persistence policy, or repair policy.

## Quality gates

Run all three before considering a change complete:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked
cargo test --workspace --locked
```

These are the same gates the GitHub Actions workflow runs on Ubuntu and Windows.
A green build checks the source; it is **not** a security certification.

### Focused suites

The workspace suite is the default. During development you can run a subset:

```sh
# v2 phase/behaviour suites (crates/relay-core/tests)
cargo test -p relay-core --test phase02_trust_security
cargo test -p relay-core --test phase03_workflow_and_intent
cargo test -p relay-core --test phase04_candidate_lease_artifact
cargo test -p relay-core --test phase05_tracer_bullet
cargo test -p relay-core --test phase09_exchange_capabilities
cargo test -p relay-core --test phase10_delegation_budgets
cargo test -p relay-core --test phase11_multi_project_scheduler
cargo test -p relay-core --test phase12_daemon_clients
cargo test -p relay-core --test phase13_privileged_actions
cargo test -p relay-core --test phase14_semantic_decisions
cargo test -p relay-core --test phase15_integrated_recovery

# runtime/backplane suites (crates/relay-runtime/tests)
cargo test -p relay-runtime --test phase06_runtime_backplane
cargo test -p relay-runtime --test phase07_herdr_backplane
cargo test -p relay-runtime --test phase08_provider_adapters

# process-boundary integration tests (apps/relay-tui/tests) — spawn the real binary
cargo test -p relay-tui --test daemon_cli
cargo test -p relay-tui --test serve_mcp
```

On Windows you can also run the portability-friendly helper from the repository
root:

```bat
run_checks.bat
```

It uses its own directory, runs the focused v2 suites plus `fmt` and `clippy`,
and stops on the first failure. It relies on link.exe already being available,
so launch it from a Developer Command Prompt.

### Testing conventions

- Prefer the **fake runtime** (`runtimes/relay-runtime-fake`) and the fake
  backplane for scheduler/orchestration tests. They are deterministic and need
  no network or credentials.
- Use temporary-directory fixtures for store and registry tests.
- Process-boundary tests may spawn the real binary (see
  `apps/relay-tui/tests/`), but must still be hermetic and must set
  `RELAY_CONFIG_PATH` / `RELAY_DATABASE_PATH` into a temporary directory.
- Do not weaken tests to make a refactor pass. Move or update assertions
  deliberately.
- Do not commit tests that require a real provider account, a hosted service, or
  a paid quota. If a test would need one, isolate it behind an explicit,
  documented seam instead.

## Optional live integrations

Provider adapters (Codex, Antigravity) and the reference Herdr backplane are not
validated by CI. To try them you need the corresponding CLI installed and
authenticated locally. Treat any live run as experimental:

- use a disposable repository and a least-privilege account;
- never paste credentials, private logs, or private task handoffs into an issue;
- do not present an unverified integration as supported.

## Pull requests

Keep changes small and coherent. In the PR description, state the problem, the
affected layers, the invariants or security boundary involved, the checks you
ran (and any you skipped), and the docs you updated. See
[CONTRIBUTING.md](../CONTRIBUTING.md) and the PR template.

## Troubleshooting

| Symptom | Likely cause |
|---|---|
| `link.exe not found` on Windows | Not in a Developer Command Prompt, or C++ build tools/SDK missing |
| `cargo build` complains about the toolchain | Rust older than 1.85, or a non-Edition-2024 toolchain |
| TUI shows a config error | `RELAY_CONFIG_PATH` points at a missing/invalid file; project registration is disabled until fixed |
| `--locked` fails | `Cargo.lock` out of date relative to `Cargo.toml`; update it deliberately in the same change |
| Tests cannot find a project | Tests must register trusted projects themselves; remote/task input cannot supply a path |
| A run appears to do nothing | Task submission only marks a task `READY`; execution requires an explicit Run approval |
