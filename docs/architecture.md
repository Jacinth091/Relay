# Architecture

> **Authority:** [ADR-0020: Relay Architecture v2](adr/0020-relay-architecture-v2.md).
> This page is a concise guide to the current architecture. Where it and
> ADR-0020 disagree, ADR-0020 wins. For the honest per-phase implementation
> status, see [`docs/v2_architecture_rebaseline/ROADMAP.md`](v2_architecture_rebaseline/ROADMAP.md) —
> module or test presence is not proof that a path is wired into production.
>
> The original v1 architecture note is preserved at
> [`docs/archive/legacy_v1/architecture-v1.md`](archive/legacy_v1/architecture-v1.md).

## Identity

Relay is a **local-first, policy-governed orchestration control plane for
user-approved software work**.

> *"The human owns intent. Relay owns controlled orchestration. Runtime
> backplanes host execution. Providers perform assigned roles."*

Relay coordinates multi-step, multi-role work under explicit human authority. It
does **not** own intent: it never decides what should be built, never expands
scope on its own, and never infers that a discussion is finished.

## Canonical ownership

```text
Human              owns intent, approves objectives and constraints,
                   grants execution, resolves scope-expanding decisions,
                   and approves privileged actions

Relay (Daemon)     canonical owner of durable tasks, approved intent, execution
                   grants, workflow definitions, role instances, role attempts,
                   candidates, leases, artifacts, exchange, policies, and recovery

Runtime backplane  hosts process/session resources and reports observations.
                   Does not own tasks, decide policy, or determine completion

Provider adapter   translates provider-specific launch/readiness/result behavior
                   (Codex, Claude, AGY, OpenCode)

Provider CLI       performs one assigned role attempt
                   (RoleAttempt)

Semantic engine    optional advisory recommendations after deterministic
(optional)         filtering; never an authority, never required for correctness
```

`Provider != Role != RoleAttempt != Skill != Policy != RuntimeProfile`.

## Domain primitives

The v2 domain separates concerns that v1 conflated (full detail in
[ADR-0020 §3](adr/0020-relay-architecture-v2.md)):

- `ProjectId` / `DeclaredProjectIdentity` — logical repository identity, distinct
  from filesystem paths.
- `TrustedProjectRegistration` / `CheckoutId` — trusted local root, remote
  fingerprint, and allowed worktree boundaries.
- `Task` — the user-visible objective and its durable state.
- `ApprovedIntent` — immutable approved objective, acceptance criteria,
  constraints, and non-goals. Scope changes create a new generation.
- `ExecutionGrant` — the explicit authorization created by a human Run.
- `WorkflowDefinition` / `WorkflowRun` — a validated graph of typed inputs,
  roles, transitions, and bounds.
- `RoleDefinition` / `RoleInstance` / `RoleAttempt` — responsibility, activation,
  and one concrete provider attempt.
- `Candidate` — a generation-bound repository state frozen with a full fingerprint.
- `Lease` — a monotonically fenced ownership token preventing conflicting writes.
- `Artifact` / `Exchange` — provenance-tracked, checksummed work products passed
  across role boundaries.

## Layer model

```text
Planner surface  (conversation, editor, your own tool)
      |
      | explicit human approval of intent
      v
HandoffTask v1  -->  Submission transport  -->  Relay Daemon
                     (serve-mcp stdio,        |
                      local file/paste)       | validate -> resolve trusted
                                              | project -> persist -> READY
                                              v
                                    explicit human Run approval
                                              |
                                              v
                                    ExecutionGrant + workflow
                                              |
                                              v
                                    RuntimeBackplane (provision/observe/
                                    send/interrupt/dispose/reconcile)
                                              |
                                              v
                                    Provider CLI role attempt
                                              |
                                              v
                                    Target repository (local)
```

The daemon is the single canonical writer of durable state, effects, and events.
Presentation clients (TUI today; Web UI planned) and submission transports are
clients of the daemon, not alternative engines.

## Runtime backplane seam

`RuntimeBackplane` is deliberately decoupled from provider and from Herdr
topology:

- `FakeBackplane` — deterministic test infrastructure.
- `LocalProcessBackplane` — direct process spawning with argument arrays and a
  trusted CWD; the minimal non-Herdr execution path.
- `HerdrBackplane` — the reference interactive backplane. Herdr's internal
  workspace/tab/pane topology is private to that adapter; Relay core has no
  dependency on it.

A backplane hosts processes. It does not own tasks, decide policy, or determine
completion.

## Preserved invariants

These are non-negotiable without a superseding ADR:

1. `HandoffTask v1` remains wire-compatible; it is adapted internally into
   `ApprovedIntent` (generation 1) plus a compatibility workflow run.
2. `relay serve-mcp` `submit_handoff` means *validate → resolve trusted project →
   persist → `READY`*. It never auto-executes.
3. No workflow executes without an explicit human Run creating an `ExecutionGrant`.
4. Relay never pushes to a remote repository automatically.
5. Tasks cannot inject filesystem paths; working directories derive solely from
   trusted local project registrations. Repository `.relay/` declarations may
   restrict, but never expand, what is permitted.
6. Provider authentication, billing, and subscription controls stay with the
   provider CLIs. Relay never bypasses them.
7. Unknown protocols, malformed outputs, and security mismatches fail closed.

## Where things live

```text
apps/relay-tui/            presentation + CLI entry + MCP service + daemon wiring
crates/relay-core/         deterministic domain engine (no LLM reasoning)
crates/relay-protocol/     HandoffTask v1 wire format
crates/relay-runtime/      runtime trait, backplane seam, profiles, router
crates/relay-store/        SQLite persistence
runtimes/*                 provider adapters and the fake runtime
integrations/planners/     planner specification and adapter documents
schemas/                   wire format schemas
```

Dependency direction: presentation → core; runtime/ingress/store
implementations → traits and core types. Core must not depend on Ratatui, a
specific provider, or Herdr.

## Relationship to v1

v1 was a single-turn handoff engine: human-approved intent in, one coding
runtime launched, results and structured escalations out. v2 keeps all of that
as its compatibility baseline (M0–M9 is frozen historical evidence) and adds
policy-governed multi-role workflows, candidates, leases, artifacts, and
Exchange on top — while keeping intent authority with the human.

See also:

- [docs/design-principles.md](design-principles.md)
- [docs/security-model.md](security-model.md)
- [docs/task-protocol.md](task-protocol.md)
- [docs/clarification-protocol.md](clarification-protocol.md)
- [docs/runtime-stack-model.md](runtime-stack-model.md)
