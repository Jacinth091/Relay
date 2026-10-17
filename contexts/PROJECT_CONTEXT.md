# Relay — Canonical Project Context

Use this document to recontextualize a new human or AI conversation.

**Authority:** [ADR-0020: Relay Architecture v2](../docs/adr/0020-relay-architecture-v2.md)
supersedes [ADR-0017](../docs/adr/0017-relay-identity-deterministic-handoff-engine.md)
and the incompatible unimplemented portions of
[ADR-0018](../docs/adr/0018-provider-distribution-model.md). The original
M0–M9 lineage remains frozen historical evidence. For what actually works today,
read [`docs/v2_architecture_rebaseline/ROADMAP.md`](../docs/v2_architecture_rebaseline/ROADMAP.md).

## What Relay Is

Relay is a **local-first, policy-governed orchestration control plane for
user-approved software work**.

- **The human** owns intent, approves objectives and constraints, grants
  execution, resolves scope-expanding decisions, and approves privileged actions.
- **Relay (the daemon)** is the canonical owner of durable tasks, approved intent,
  execution grants, workflow definitions, role instances, role attempts,
  candidates, leases, artifacts, exchange, policies, recovery, and user-facing
  semantic state.
- **Runtime backplanes** host processes and sessions and report observations,
  but do not own tasks or decide policy. Herdr is the reference interactive
  backplane; `LocalProcessBackplane` is the minimal non-Herdr path;
  `FakeBackplane` is for deterministic tests.
- **Provider adapters** translate provider-specific launch, readiness, and result
  behavior (Codex, Claude, AGY, OpenCode).
- **Provider CLIs** perform one assigned role attempt.
- **Semantic decision engines** (e.g., Jev) are optional advisers after
  deterministic filtering. They never create authority.

One-line anchor:

> The human owns intent. Relay owns controlled orchestration. Runtime backplanes host execution. Providers perform assigned roles.

### Role mapping

Relay's roles map onto a manager-and-sub-workers model applied to coding
runtimes rather than to LLM subagents:

```text
manager                  -> Relay daemon (deterministic policy + lifecycle)
roles                    -> Implementer, Verifier, Reviewer, Remediator, ...
work assignment          -> approved intent + execution grant + workflow
consolidation            -> candidates, artifacts, exchange, activity records
verification relay-back  -> COMPLETED / FAILED or NEEDS_* packets
```

Relay never becomes a reasoning manager: it does not invent intent, expand scope,
or judge beyond configured verification.

Core flow:

```text
Human + Planner
    -> explicit approval
    -> planner integration / submission transport
    -> Relay (validate, resolve trusted project, persist READY)
    -> explicit local Run approval (ExecutionGrant)
    -> workflow + roles + runtime backplane + provider CLI
    -> target repository (local)
    -> COMPLETED | FAILED | NEEDS_CLARIFICATION | NEEDS_DECISION
```

The supported submission paths today (ADR-0016):

```text
Planner host with local stdio MCP support -> relay serve-mcp -> Relay
Planner host without usable MCP access    -> local file/paste -> Relay
```

`relay serve-mcp` exposes `submit_handoff`: validate → resolve trusted project →
persist → `READY`. It never auto-executes. Local file/paste is the hard fallback
that always works. GitHub ingress was descoped from v0.1 and remains a future
optional compatibility path / external binding, not current behavior.

## Why It Exists

Relay rejects the assumption that planning should be performed autonomously by
the same agent that implements the code. The developer should be free to
discuss, disagree, ask for counterarguments, revise intent, stop, and decide
when implementation is ready. Relay begins at the explicit handoff boundary.

## Agentic Usage / Quota Motivation

Coding-agent execution may be subject to finite quotas, rate limits, credits, or
workspace allowances. Relay lets exploratory reasoning stay in the planning
surface and starts the coding runtime only after explicit approval. This is
usage optimization, **not** limit circumvention. Relay never bypasses provider
billing, usage controls, or authentication.

## Execution Autonomy

After handoff, Relay may autonomously pursue the approved intent within an
execution grant. It may repair compiler/type errors, failed tests, lint failures,
implementation mistakes, and verification failures **when the repair stays inside
approved scope**. It must stop when continuing would materially change intent.

> The human owns intent. Relay owns controlled execution.

## Clarification Boundary

When execution crosses back into planning territory, Relay creates a structured
packet in `NEEDS_CLARIFICATION` or `NEEDS_DECISION` state, explaining the work
attempted, what completed, the blocking reason, the evidence, the explicit
question, discovered options, executor assessment, repository/verification state,
and the resume point. The planner returns a Resolution Packet. Today, packets are
copied as Markdown into a normal planning conversation via local file/paste; the
v2 Exchange vocabulary exists in source but is not yet durable end to end.

## Hard Invariants

- There is one Relay engine.
- Relay Core contains no LLM reasoning requirement and is provider-agnostic.
- Human approval of intent is explicit; execution requires an explicit Run grant.
- Execution autonomy is bounded by the approved intent and the execution grant.
- Local configuration owns repository paths; remote tasks cannot supply paths.
- Runtime execution is validated before spawn.
- Ratatui is the first presentation layer, not the architecture.
- No automatic Git push.
- Unknown or unsafe conditions fail closed.
- Escalation states must explain why execution stopped.
- Relay does not invoke coding runtimes for exploratory planning by default.
- Relay never bypasses provider quotas, billing, or usage controls.

## Foundation Technology

- Rust stable / Edition 2024, Cargo workspace (resolver 3).
- Tokio for async/process orchestration.
- Ratatui + Crossterm for the TUI.
- Serde; JSON for task/clarification payloads; TOML for local config.
- SQLite + SQLx for durable local state.
- clap for the CLI; tracing for diagnostics; thiserror/anyhow for errors.
- `git` executable first for Git; `gh` executable first for GitHub.
- Provider CLIs (Codex first) as coding runtimes.

Future frontends may use Tauri 2 or an Axum-based local server.

## Architectural Vocabulary

**Planner** — where intent is formed.
**Planner Specification** — provider-neutral rules for converting approved intent into Relay protocol.
**Planner Adapter** — provider-native packaging of the planner specification.
**Handoff Task (`HandoffTask v1`)** — approved implementation specification.
**Ingress / submission transport** — how work reaches Relay.
**Provider** — the ecosystem behind a runtime.
**Runtime** — the coding agent being executed.
**Runtime backplane** — hosts the process/session; does not own tasks.
**Runtime Profile** — locally configured runtime instance and options.
**Capabilities** — runtime features that can be safely used.
**Stack** — opinionated composition of planner adapter/specification, transport, runtime, and provider-native behavior.
**ApprovedIntent / ExecutionGrant** — the approved objective and the explicit authorization to execute it.
**Role / RoleAttempt** — a semantic responsibility and one concrete provider attempt.
**Candidate / Lease / Artifact / Exchange** — frozen repository state, fenced write ownership, and provenance-tracked work products.
**Clarification Request** — asks for explanation of existing approved intent.
**Decision Request** — asks for a new material choice.
**Resolution Packet** — the approved answer used to resume/restart execution.

## First Stack

`chatgpt-codex`

- planner: normal ChatGPT conversation;
- planner specification: Relay Handoff Planner Specification;
- planner adapter: installable ChatGPT `relay-handoff` skill;
- submission: `relay serve-mcp` (local stdio MCP) preferred when the host supports
  it; local file/paste is the hard fallback;
- GitHub ingress: descoped from v0.1; future optional compatibility path;
- external binding: optional GitHub Issue;
- runtime: Codex;
- runtime transport: CLI (`app-server` stdio default, `exec` fallback);
- local execution approval: required;
- implementation repair: automatic within policy;
- planning ambiguity: escalate;
- code push: manual.

## Positioning

Relay is not trying to outperform autonomous orchestration systems at autonomous
planning. Its differentiated default is:

> Planning is a conversation. Execution is a handoff.

Its execution principle is:

> After handoff, Relay may autonomously pursue approved intent without inventing new intent.

## Planner-host adapters

Relay Handoff has one provider-neutral planner specification with host-native
adapters.

```text
ChatGPT          first-class reference
Claude Code      experimental first-party
OpenCode         experimental first-party
GitHub Copilot   experimental first-party
Generic          fallback
```

Provider-native installation and tool behavior may differ, but all adapters must
emit the same Relay task and clarification/resolution semantics. Adapters are
documentation/skill documents today; there is no executable conformance harness yet.
