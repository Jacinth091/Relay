# Architecture (archived - Relay v1)

> **Archived historical document.** This is the original v1 architecture note
> ("one local handoff engine", execution-orchestration-only framing). It was
> superseded by [ADR-0020](../../adr/0020-relay-architecture-v2.md), which
> establishes Relay v2 as a local-first, policy-governed orchestration control
> plane and gives Relay ownership of workflows, roles, grants, candidates,
> leases, and recovery (while intent authority still remains with the human).
>
> Paths written as `docs/...` are relative to the repository root. For the
> current architecture see [docs/architecture.md](../../architecture.md).

---

## Architectural Statement

Relay is **one local handoff engine** with replaceable planner integrations, ingresses, runtimes, stacks, and presentation clients.

The first presentation client is a terminal UI.

GitHub is an integration option, not a second Relay implementation and not the canonical task store.

---

## What Relay Is / Is Not

"Orchestrator" is an overloaded word, so this is stated explicitly.

There are two activities people call orchestration:

**Intent orchestration** — deciding what should be done: planning, reasoning, strategy selection. Relay does **not** do this. It is reserved for the human and the planner surface (AGENTS.md invariants 3 and 7; "Relay should never infer that a conversation has gone on long enough and silently begin implementation").

**Execution orchestration** — deterministic lifecycle management: validate, route, persist, spawn a runtime process, stream events, apply bounded repair, escalate with structured packets. This **is** Relay's job. It is a state machine, not a brain.

Relay's identity in one sentence:

> **Relay is the border checkpoint between conversation and execution — it stamps, routes, and logs; it never decides where anyone is going.**

This also settles whether Relay is "an MCP server" or similar. Transport and presentation surfaces are doors around one core:

```text
TUI        = presentation door
MCP server = submission door
GitHub     = ingress door (optional)
paste/file = ingress door (universal fallback)
```

None of these is what Relay *is*. If a protocol disappears, its door is replaced; the engine survives. A new surface passes the identity test only by answering three questions:

| Question | Answer decides |
|---|---|
| Is it deterministic policy, lifecycle, persistence, validation? | belongs in Relay Core |
| Is it moving data to/from an external host or provider? | belongs in an edge crate |
| Does it decide intent or strategy? | does not belong in Relay at all |

Scope discipline follows from README's One Rule: features widen the *data* available to core (capabilities, metadata) rather than widening core's *authority*.

---

## Layer Model

```text
Planner Surface
      |
Planner Integration
      |
Relay Task Protocol
      |
Submission Transport / Ingress
      |
Relay Core
  |   |   |   |
  |   |   |   +-- Recovery / clarification
  |   |   +------ Persistence
  |   +---------- Project/Security Policy
  +-------------- Runtime Registry
      |
Runtime Profile
      |
Runtime Implementation
      |
Coding Agent Process
      |
Target Repository
```

Presentation clients observe and command the same Relay Core:

```text
                  Relay Core
                /     |      \
               /      |       \
            TUI    Desktop    Server
```

---

## Planner Integration

The provider-neutral Planner Specification defines handoff semantics. Provider adapters live outside core execution.

They may:

- recognize explicit user handoff approval;
- convert approved intent into the Relay task protocol;
- submit tasks through a supported submission transport;
- attach source metadata;
- attach stack-specific extensions;
- submit Resolution Packets.

They do not control local paths.

They are clients of Relay.

---

## Submission Transport, Ingress, and External Binding

A **submission transport** describes how a planner delivers a HandoffTask toward Relay.

A Relay-side **ingress** receives it.

Examples:

- Relay Direct/MCP;
- provider-created GitHub Issue consumed by Relay's GitHub ingress;
- local file import;
- local paste.

When a direct Relay interface is available, planner adapters should prefer it. A planner-host GitHub connector is a compatibility transport, not a universal dependency.

After acceptance, Relay's local durable task state is canonical.

An **external binding** is optional. For example, Relay may create or synchronize a GitHub Issue for collaboration/history even when the task arrived through MCP or local paste.

```text
submission provenance      external binding
---------------------      ----------------
relay-mcp                  none
github-direct              github-issue
local-paste                github-issue
local-file                 none
```

The local ingress accepts either canonical Relay JSON or Markdown containing an embedded canonical Relay JSON block. The TUI should support file selection and direct paste.

Relay's first GitHub implementation should use `gh` so authentication remains owned by GitHub CLI. Relay can use that integration both to consume GitHub-ingress tasks and to manage optional GitHub bindings.

For a bound GitHub Issue, Relay may publish:

- status;
- clarification/decision packets;
- resolution acknowledgments;
- final result.

Submission transport and external binding do not decide implementation strategy.

See `docs/submission-and-bindings.md`.

## Core Responsibilities

Relay Core owns:

- canonical task state;
- task transition validation;
- project registry;
- project lookup;
- execution policy;
- repository preflight orchestration;
- runtime-profile selection;
- capability checks;
- execution lifecycle;
- recovery/repair policy;
- clarification/decision state;
- durable execution metadata;
- domain events.

Relay Core must not depend on Ratatui.

---

## Project Registry

The local registry maps a logical project key to trusted configuration.

Example:

```toml
[projects.practicore]
name = "PractiCore"
path = "D:\\Projects\\PractiCore"
expected_remote = "..."
default_stack = "chatgpt-codex"
```

A handoff task contains only:

```json
{ "project": "practicore" }
```

The project registry resolves it locally.

---

## Runtime Registry

A runtime implementation describes:

- runtime identifier;
- provider identifier;
- supported runtime transports;
- capabilities;
- health/version detection;
- task preparation;
- process execution;
- output/event parsing;
- cancellation behavior;
- recovery/clarification support.

Example identity:

```text
provider          openai
runtime           codex
runtime transport cli
```

---

## Runtime Profile

A runtime profile is local configuration for a runtime instance.

Example concept:

```toml
[runtime_profiles.codex_default]
provider = "openai"
runtime = "codex"
transport = "cli"
```

Runtime-native options belong under namespaced configuration.

---

## Stack

A Stack is an opinionated composition.

It may choose:

- planner integration;
- default submission preference;
- optional external binding policy;
- runtime profile;
- native task preparation;
- native extension behavior;
- recovery policy;
- project instruction conventions.

Initial stack:

```text
chatgpt-codex
```

---

## Execution and Recovery Flow

```text
1. A submission transport/ingress delivers an approved task to Relay.
2. Protocol parser validates envelope/version.
3. Core records task.
4. User reviews task.
5. Core resolves project key.
6. Core runs repository/runtime preflight.
7. User selects Run.
8. Core starts execution.
9. Runtime receives validated project context + task.
10. Runtime implements and verifies.
11. Ordinary implementation failures may enter bounded repair.
12. If repair stays in scope, execution continues automatically.
13. If execution crosses the intent boundary:
      - NEEDS_CLARIFICATION, or
      - NEEDS_DECISION.
14. Relay creates a structured packet.
15. Packet is persisted and optionally published through a direct planner channel or external binding when supported.
16. User/planner provides a Resolution Packet.
17. Relay validates the resolution.
18. Execution resumes or restarts according to the packet/policy.
19. On success, Core refreshes repository state and records DONE.
```

---

## State Machine

Foundation state model:

```text
RECEIVED
   |
   v
READY
   |
   | user Run
   v
RUNNING
   |
   +-------------------------+
   |                         |
   v                         v
VERIFYING                 FAILED
   |
   +-----------+
   |           |
   v           v
PASS        REPAIRING
   |           |
   |           +----> RUNNING / VERIFYING
   |
   v
DONE
```

Intent-boundary states:

```text
RUNNING / REPAIRING / VERIFYING
            |
            +----> NEEDS_CLARIFICATION
            |
            `----> NEEDS_DECISION
```

Resolution:

```text
NEEDS_CLARIFICATION / NEEDS_DECISION
            |
            v
RESOLVED
            |
      +-----+------+
      |            |
      v            v
   RUNNING       FAILED/CANCELLED
```

Exact persisted state names may evolve, but the distinction between implementation recovery and planning escalation is architectural.

---

## Recovery Policy

Default stack behavior should distinguish:

```text
Implementation repair       AUTO
Verification failure repair AUTO
Requirement ambiguity       ESCALATE
Architecture change         ESCALATE
Scope expansion             ESCALATE
Destructive change          ESCALATE
Security-policy change      ESCALATE
```

Repair loops must be bounded by local policy such as attempt/time limits.

---

## Clarification / Decision Packets

Relay should model structured domain data, not parse arbitrary prose.

See `docs/clarification-protocol.md`.

Packets may be rendered as Markdown for:

- optional GitHub binding comments;
- clipboard;
- terminal display;
- future planner integrations.

The structured object remains authoritative.

---

## Domain Events

Relay should expose typed events such as:

```text
TaskReceived
TaskValidated
TaskReady
ExecutionPreflightStarted
ExecutionPreflightFailed
ExecutionStarted
RuntimeOutput
RuntimeWarning
VerificationStarted
RepairStarted
RepairAttempted
ClarificationRequested
DecisionRequested
ResolutionReceived
ExecutionResumed
RuntimeExited
TaskCompleted
TaskFailed
ExecutionCancelled
RepositoryStatusChanged
```

Presentation clients consume events without owning execution.

---

## Concurrency

v0.1 may allow only one running execution globally or one per project.

Do not build a scheduler until real usage requires one.

A project should eventually have an execution lock to prevent accidental simultaneous modification unless explicitly supported.

---

## Extension Model

Built-in runtimes/ingresses come first.

Long-term community extensions should be out-of-process rather than native dynamic libraries.

The exact external extension wire protocol is intentionally deferred until the first non-built-in extension is implemented.

---

## Presentation Independence

No core method should return Ratatui widgets, terminal coordinates, or keybindings.

Core APIs expose domain data/events.

The same operation should eventually be callable from:

```text
relay-tui
relay-desktop
relay-server
tests
```

---

## Dependency Direction

Preferred dependency direction:

```text
presentation -> core
ingress implementation -> ingress traits/core types
runtime implementation -> runtime traits/core types
store implementation -> core/domain types

core -X-> presentation
core -X-> Codex
core -X-> GitHub
core -X-> ChatGPT
```

`-X->` means must not depend directly on.
