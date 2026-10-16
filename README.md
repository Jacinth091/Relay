# Relay

> **Planning is a conversation. Execution is a handoff.**

Relay is a **local-first, policy-governed orchestration control plane for
user-approved software work**.

The human owns intent and privileged decisions. Relay owns controlled
orchestration: durable tasks, approved intent, execution grants, workflows,
roles, policy, provider resolution, scheduling, recovery, and the state you
actually see. Runtime backplanes host processes. Provider CLIs perform assigned
role attempts.

> *"The human owns intent. Relay owns controlled orchestration. Runtime
> backplanes host execution. Providers perform assigned roles."*
> — [ADR-0020](docs/adr/0020-relay-architecture-v2.md)

Relay begins *after* you decide a piece of work is ready. It never infers that a
conversation has gone on long enough and starts implementing on its own, and it
never pushes to a remote repository automatically.

This repository tracks **Relay Architecture v2**, which supersedes the original
v1 handoff engine framing (ADR-0020). The M0–M9 lineage is preserved as
historical evidence.

---

## ⚠️ Experimental hobby project

Relay is a **personal, experimental hobby and development project**, published
as source for learning, testing, and contributions.

- It is **not** a supported product, and there is **no** stability, uptime,
  compatibility, or response-time commitment.
- It is **not** audited or production-safe. It can start coding-agent processes
  that modify files and run commands on your machine.
- It **never** bypasses provider quotas, billing, authentication, or usage
  controls. Any model or CLI you connect stays separately authenticated and
  separately charged by its provider.
- Start with a **disposable Git repository**, a least-privilege account, and
  reviewed permissions.

See [SECURITY.md](SECURITY.md) and [docs/security-model.md](docs/security-model.md).

---

## What works today

Relay is under active v2 development. Modules and tests existing in
the source tree is **not** the same as a capability being proven end to end on
your OS. The authoritative per-phase status table lives in
[`docs/v2_architecture_rebaseline/ROADMAP.md`](docs/v2_architecture_rebaseline/ROADMAP.md);
read it before trusting any claim below.

| | Capability | Notes |
|---|---|---|
| ✅ **Today** | Build and test the whole workspace from source | Stable Rust, no provider account needed |
| ✅ **Today** | `relay` TUI: tasks, projects, runtimes, diffs, logs, command palette | First presentation client of Relay Core |
| ✅ **Today** | Local handoff intake: JSON file / paste → validate → preview → persist `READY` | `HandoffTask v1` wire format |
| ✅ **Today** | Explicit human **Run** approval before any execution | Execution grants are never automatic |
| ✅ **Today** | `relay serve-mcp`: MCP over **stdio** exposing `submit_handoff` | Validates, resolves a trusted project, persists `READY`, **never auto-runs** |
| ✅ **Today** | `relay daemon`: store-lease ownership with a fencing token and heartbeat renewal, serving clients over a Windows named pipe | The TUI and `relay serve-mcp` submit task creation through the daemon instead of writing the store directly. The transport is Windows-only for now (see [Supported platforms](CONTRIBUTING.md#supported-platforms)) |
| ✅ **Today** | Trusted local project registry + repository safety preflight | Tasks carry a project *key*, never filesystem paths |
| ✅ **Today** | Deterministic fake runtime + fake backplane for tests | Hermetic: no network, no credentials, no provider quota |
| ✅ **Today** | `NEEDS_CLARIFICATION` / `NEEDS_DECISION` escalation packets | Structured, not a vague question |
| 🟡 **Partial** | v2 orchestration domain: workflows, roles, execution grants, candidates, leases, artifacts, Exchange, delegation | Types and tests exist; most are **not yet on the production Run path** (ROADMAP phases 02–11, 13–14 are `PARTIAL`) |
| 🟡 **Partial** | Provider adapters: Codex (`app-server` stdio default, `exec` fallback), Antigravity | Require separately installed/authenticated provider CLIs; not exercised in CI |
| 🟡 **Partial** | Runtime backplanes: `LocalProcessBackplane` (minimal non-Herdr path), `FakeBackplane` | Containment/idempotency semantics still being hardened (ROADMAP phase 06) |
| 🟡 **Partial** | `HerdrBackplane` | Reference/test shape only; no durable production transport yet (ROADMAP phase 07) |
| 🟡 **Partial** | The user **Run** path | Still executes the v1 `ExecutionScheduler`; the cutover to the v2 orchestrator is not implemented at this revision |
| ⛔ **Planned** | Web UI (ROADMAP phase 12) | Deliberately not started |
| ⛔ **Planned** | Durable Herdr production backplane, Exchange/delegation durability, privilege-binding corrections | Not started |
| ⛔ **Planned** | GitHub ingress | Descoped from v0.1 by [ADR-0016](docs/adr/0016-mcp-first-submission-and-v0-1-local-descope.md) |
| ⛔ **Planned** | Release binaries, installers, package managers, auto-update | Not published |
| ⛔ **Planned** | Cross-provider fan-out / health-aware routing | Unimplemented portion of ADR-0018 superseded by ADR-0020 |

**On `relay serve-mcp`:** it is a **local stdio** server. A cloud-hosted planner
(for example, a ChatGPT deployment that cannot spawn local processes) cannot
reach your local Relay instance through it. Hosts without compatible local-MCP
access use the same canonical `HandoffTask v1` payload through local file/paste
import instead.

**On optional components:** Herdr is the reference interactive backplane, not a
requirement for Relay core correctness. Semantic decision engines such as Jev
are optional advisers that never create authority. Neither is required to build,
test, or run the local source.

---

## Prerequisites

To build and test from source you need:

- **Git**
- **Rust stable 1.85 or newer** with Cargo (the workspace is Edition 2024;
  `rust-version = "1.85"` in `Cargo.toml`)
- A normal interactive terminal (for the TUI)
- On Windows with the MSVC toolchain: the Microsoft C++ build tools and Windows
  SDK, used from a **Developer Command Prompt / PowerShell**

**Windows with the MSVC toolchain is the only supported host today.** The build
is not yet portable: the local Relay Daemon transport is a Windows named pipe,
so the `ubuntu-latest` CI leg is advisory and expected to fail. See
[CONTRIBUTING.md](CONTRIBUTING.md#supported-platforms) if you would like to help
change that.

You do **not** need a coding-agent login, API key, hosted service, Herdr server,
or a GitHub mailbox to compile and test the source. Real provider execution
additionally requires the corresponding provider CLI, installed and
authenticated by you.

---

## Build and run from source

From a fresh clone:

```sh
cargo build --workspace --locked
cargo test --workspace --locked
cargo run --locked -p relay-tui -- --help
```

`relay-tui` produces the `relay` executable. To start the interactive TUI:

```sh
cargo run --locked -p relay-tui
```

> Run the TUI against a **disposable** test repository with isolated local
> config/state, not a repository you care about. Relay writes local SQLite state
> and can launch coding-agent processes.

Other entry points:

```sh
cargo run --locked -p relay-tui -- serve-mcp    # MCP stdio submission server
cargo run --locked -p relay-tui -- daemon        # durable daemon
```

See [docs/cli.md](docs/cli.md) for the full command and TUI palette reference.

---

## How a handoff flows today

```text
planner surface (or your editor / your own tool)
        |
        | explicit human approval of the intent
        v
HandoffTask v1  ---->  relay serve-mcp (local stdio)  --\
                        local JSON file / paste         |--> protocol validate
                                                        |    -> preview
                                                        |    -> human confirms import
                                                        v
                                              task READY (SQLite, canonical)
                                                        |
                                                        | explicit human Run approval
                                                        v
                                      execution scheduler -> runtime (e.g. Codex)
                                                        |
                    +-----------------------------------+-----------------------------------+
                    |                                   |                                   |
                    v                                   v                                   v
                completed                        ordinary failure                    intent boundary crossed
                 (DONE)                    -> bounded auto-repair                  -> NEEDS_CLARIFICATION
                                                                                       or NEEDS_DECISION
```

Two things are load-bearing:

1. **Submission never executes.** `submit_handoff` (and file/paste import) only
   validates, resolves a trusted project, persists, and marks the task `READY`.
2. **Execution requires an explicit human Run.** Nothing is spawned, and nothing
   is pushed to a remote, without that approval.

Escalations produce a structured packet rather than a vague question — see
[docs/clarification-protocol.md](docs/clarification-protocol.md).

---

## Architecture at a glance

```text
Human              owns intent and privileged decisions
Relay (Daemon)     owns tasks, grants, workflows, roles, policy, state, recovery
Runtime backplane  hosts processes and sessions; does not own tasks
Provider adapter   translates provider-specific launch/readiness/result behavior
Provider CLI       performs one assigned role attempt
```

Relay Core is deterministic, provider-agnostic, and contains no LLM reasoning
requirement. Ratatui is the first presentation layer, not the architecture.

Read [ADR-0020](docs/adr/0020-relay-architecture-v2.md) for the authority and
[docs/architecture.md](docs/architecture.md) for the current guide.

---

## Repository layout

```text
apps/relay-tui/            Terminal UI (first presentation client)
crates/
  relay-core/              Deterministic domain engine
  relay-protocol/          HandoffTask v1 wire format and validation
  relay-runtime/           Runtime/backplane seam, profiles, router
  relay-store/             SQLite persistence
runtimes/
  relay-runtime-codex/     Codex adapter (app-server stdio default)
  relay-runtime-antigravity/
  relay-runtime-fake/      Scriptable runtime for hermetic tests
  relay-decision-jev/      Optional advisory decision adapter
integrations/planners/     Planner specification + host adapter documents
schemas/                   Wire format schemas
docs/                      Architecture docs, ADRs, implementation packs
contexts/                  Project context documents
```

---

## Documentation map

- [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) — **build, test, and source layout for contributors**
- [docs/v2_architecture_rebaseline/ROADMAP.md](docs/v2_architecture_rebaseline/ROADMAP.md) — **authoritative phase status**
- [docs/adr/0020-relay-architecture-v2.md](docs/adr/0020-relay-architecture-v2.md) — architecture authority
- [docs/architecture.md](docs/architecture.md) — current architecture guide
- [docs/cli.md](docs/cli.md) — binary commands and TUI command palette
- [docs/installation.md](docs/installation.md) — source build vs. unpublished installers
- [docs/roadmap.md](docs/roadmap.md) — roadmap pointers (v1 ladder is historical)
- [docs/task-protocol.md](docs/task-protocol.md) — `HandoffTask v1`
- [docs/local-ingress.md](docs/local-ingress.md) — local file and paste formats
- [docs/planner-hosts/](docs/planner-hosts/) — registering `relay serve-mcp` per host
- [docs/security-model.md](docs/security-model.md) — security posture and boundaries
- [docs/data-persistence.md](docs/data-persistence.md) — local state and SQLite
- [docs/glossary.md](docs/glossary.md) — terminology
- [AGENTS.md](AGENTS.md) — invariants for coding agents working on Relay

---

## Contributing

Contributions, issues, and experiments are welcome — with no guarantee of
review, acceptance, or support. Read [CONTRIBUTING.md](CONTRIBUTING.md) and
[AGENTS.md](AGENTS.md) first.

Report security problems privately per [SECURITY.md](SECURITY.md); do not open a
public issue containing exploit details, credentials, or private data.

---

## License

[MIT](LICENSE).

Relay is inspired by ideas from other open-source projects; see
[ACKNOWLEDGMENTS.md](ACKNOWLEDGMENTS.md) and
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

---

## One rule to remember

Before adding a feature, ask:

> **Does this help a developer deliberately move approved intent from planning
> into execution, or safely complete that execution without silently changing
> the intent?**
