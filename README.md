# Relay

> **Planning is a conversation. Execution is a handoff.**

Relay is a local, developer-controlled handoff engine that moves **user-approved implementation intent** from a planning surface into a coding agent.

Relay is one implementation with multiple integration points around the same core.

The first experience is terminal-first, but Relay is **not architecturally a TUI application**. The TUI is the first client of Relay Core. Future desktop, local web, MCP, or API integrations must be able to use the same core without rewriting execution logic.

---

## Why Relay Exists

Coding agents are good at implementation, but planning and implementation are different activities.

A developer may want to:

- explore multiple designs;
- disagree with an AI suggestion;
- ask for counterarguments;
- change requirements during discussion;
- investigate trade-offs before committing;
- abandon an idea without starting an implementation agent;
- decide personally when implementation intent is ready.

Relay deliberately preserves that freedom.

Its default model is:

```text
You + Planner
      |
      | discuss, challenge, refine, decide
      v
Explicit user approval
      |
      | "Send to Relay"
      v
Planner Integration
      |
      | build HandoffTask v1
      v
Submission Transport
      | Relay Direct/MCP when available
      | provider-native remote compatibility
      | local file/paste fallback
      v
Relay Core
      |
      | validate, route, persist, execute, recover
      v
Coding Runtime
      |
      v
Target Repository
```

Relay should never infer that a conversation has gone on long enough and silently begin implementation.

### Agentic usage is a resource

Another original motivation for Relay is practical: coding-agent access may be limited by quotas, rate limits, credits, or workspace allowances.

Relay lets the developer keep exploratory reasoning in the planning surface and delay coding-runtime usage until there is approved implementation work.

For the first ChatGPT + Codex workflow:

```text
Normal ChatGPT
    |
    | architecture / debugging / requirements / disagreement
    v
"Send to Relay"
    |
    v
Relay
    |
    | Codex usage begins when implementation begins
    v
Codex
```

The goal is **not** to bypass or circumvent provider limits. Relay simply avoids invoking the coding runtime before it is useful.

Provider-specific quotas and billing models can change; Relay's portable principle is:

> **Use scarce agentic execution capacity for execution.**

---

## One Relay, Multiple Doors

There is not a separate "ChatGPT Relay" and "local Relay orchestrator."

There is one Relay system.

Planning surfaces and user interfaces are clients/integrations around it.

```text
                          RELAY

              +-------------------------+
              |       Relay Core        |
              |-------------------------|
              | Task lifecycle          |
              | Project registry        |
              | Security policy         |
              | Persistence             |
              | Runtime registry        |
              | Recovery policy         |
              | Execution events        |
              +------------+------------+
                           |
            +--------------+--------------+
            |                             |
            v                             v
        Relay TUI                    Future Clients
                                    Desktop / Web

Submission transports feed the same core:

ChatGPT -> Relay Direct/MCP ----\
Claude  -> Relay Direct/MCP -----+--> Relay Core
Provider GitHub compatibility ---/
Local file/paste ----------------/
```

After Relay accepts a task, local durable state is canonical. GitHub can remain an ingress and can also be an optional external binding, but it is not required for every Relay task.

---

## Product Definition

**Relay is a local control plane for moving user-approved implementation intent from conversational planning into coding-agent execution.**

Relay is responsible for:

- receiving handoff tasks;
- validating task envelopes;
- resolving trusted local projects;
- validating repositories;
- selecting configured stacks and runtimes;
- checking runtime capabilities;
- requiring local execution approval by default;
- spawning coding runtimes;
- streaming execution events;
- autonomously repairing implementation problems within approved scope;
- detecting when execution crosses back into planning territory;
- generating structured clarification/decision packets;
- preserving execution history;
- presenting results and repository changes.

Relay is not responsible for inventing new product intent without permission.

---

## Execution Autonomy

Relay may autonomously pursue the approved implementation intent after handoff.

That includes normal implementation recovery such as:

```text
implement
   |
   v
tests fail
   |
   v
inspect
   |
   v
repair
   |
   v
verify again
```

Examples that may be handled autonomously:

- compiler errors;
- type errors;
- lint failures;
- failing tests caused by the implementation;
- missing imports;
- incorrect queries;
- implementation mistakes;
- verification failures whose fix remains inside the approved task.

Relay should stop and request input when continuing would require a material change to approved intent.

Examples:

- unresolved product behavior;
- architecture choices not already decided;
- scope expansion;
- destructive data-model changes;
- security-policy changes;
- conflicting requirements.

This is the core boundary:

> **The human owns intent. Relay owns controlled execution.**

---

## Clarification and Decision Packets

A runtime must not return a vague message such as:

```text
NEEDS_INPUT: should I add a new table?
```

When Relay stops for planning input, it creates a structured packet explaining:

- what was being implemented;
- what was completed;
- what caused the stop;
- relevant implementation context;
- the exact decision/question;
- options discovered;
- executor assessment;
- current repository state;
- how execution can resume.

Relay distinguishes:

```text
NEEDS_CLARIFICATION
```

The approved intent exists, but something about it is unclear.

and:

```text
NEEDS_DECISION
```

Implementation uncovered a material choice that changes architecture, scope, behavior, policy, or another aspect of intent.

The packet can be:

- displayed in the TUI;
- persisted in SQLite;
- published to an optional bound channel such as a GitHub Issue;
- copied as Markdown for the planner;
- sent directly through Relay MCP/another Relay submission interface when available.

The matching planner response is a **Resolution Packet**.

See [`docs/clarification-protocol.md`](docs/clarification-protocol.md).

---

## First Reference Workflow

The first deeply supported stack is:

```text
Normal ChatGPT conversation
          |
          | explicit user approval
          | "Send to Relay"
          v
Relay Handoff planner adapter (ChatGPT)
          |
          | Relay Handoff Planner Specification + Task Protocol
          v
GitHub compatibility transport (v0.1 reference)
          |
          v
Relay Core
          |
          | local execution approval
          v
Codex CLI
          |
          | implement / test / repair
          v
Local Git repository
          |
          +---- success ------------------------> DONE
          |
          +---- implementation problem ---------> auto-repair
          |
          `---- intent boundary crossed --------> NEEDS_CLARIFICATION
                                                   or NEEDS_DECISION
```

The v0.1 reference workflow uses GitHub because it provides a practical durable bridge before Relay Direct is implemented. When this mailbox transport is used, each user/team configures its own repository; private visibility is the recommended default. Relay does not provide a maintainer-owned shared mailbox service.

When direct Relay MCP access is available, it becomes the preferred planner submission path:

```text
ChatGPT / Claude / OpenCode -> Relay MCP -> Relay Core
```

GitHub may then be omitted or configured as an optional external binding. The rest of Relay does not change.

---

## Core Philosophy

### Human-directed planning

Planning is genuine discussion, not merely a task-generation form.

### Explicit handoff

Execution begins only after the user deliberately crosses the planning-to-execution boundary.

### Controlled execution autonomy

After handoff, Relay may repair and retry implementation within the approved scope.

### Explain before escalating

When Relay needs input, it must explain why and provide enough context for the user and planner to make the next decision.

### Deterministic core

Relay should contain as little AI reasoning as possible. Routing, validation, policy, persistence, and execution control should be deterministic.

### Provider-agnostic core

Core domain logic must not contain provider-specific behavior.

### Provider-native edges

Stacks and runtimes may use native provider capabilities aggressively when doing so improves quality or reliability.

### Local-first

The default product runs locally and does not require a hosted Relay service.

### Lightweight

Relay itself should remain comfortable on ordinary developer hardware. The coding agent, not Relay, should normally be the meaningful compute consumer.

### UI-independent core

Ratatui is the first presentation layer, not the architecture.

---

## High-Level Architecture

```text
                   PLANNER SURFACES

            ChatGPT   Claude   Manual
                \       |       /
                 \      |      /
                  v     v     v

              PLANNER INTEGRATION
                       |
                       | Relay Task Protocol
                       v

              SUBMISSION TRANSPORT

         Relay Direct/MCP / Provider GitHub / Local
                       |
                       v

              +--------------------+
              |     RELAY CORE     |
              |--------------------|
              | Task lifecycle     |
              | Project registry   |
              | Security policy    |
              | Runtime registry   |
              | Capabilities       |
              | Persistence        |
              | Recovery policy    |
              | Clarification      |
              | Execution events   |
              +---------+----------+
                        |\
                        | \---- optional External Binding
                        |          GitHub Issue / future systems
                        |
                Runtime Profile
                        |
          +-------------+-------------+
          |                           |
          v                           v

        Codex                      Claude Code
      CLI runtime                  CLI runtime
          |                           |
          +-------------+-------------+
                        |
                        v
                 TARGET REPOSITORY


                 PRESENTATION CLIENTS

           +------------+------------+
           |            |            |
           v            v            v
       Relay TUI    Relay Desktop  Relay Server
        Ratatui        Future        Future
```

---

## Technology Direction

Relay Foundation v1.7 chooses:

```text
Core language       Rust, stable, Edition 2024
Workspace           Cargo workspace, resolver 3
Async runtime       Tokio
TUI                 Ratatui + Crossterm
Serialization       Serde
Local config        TOML
Task wire format    JSON
Persistence         SQLite through SQLx
CLI parsing         clap
Logging/events      tracing
Library errors      thiserror
App-boundary errors anyhow
Git integration     git executable first
GitHub integration  gh executable first
Runtime execution   child processes via Tokio
```

Potential future frontends:

```text
Desktop             Tauri 2
Local server/API    Axum
Browser UI          optional client of Relay server
```

---

## Repository Layout

Target direction:

```text
relay/
|
|-- apps/
|   `-- relay-tui/
|
|-- crates/
|   |-- relay-core/
|   |-- relay-protocol/
|   |-- relay-store/
|   |-- relay-runtime/
|   `-- relay-ingress/
|
|-- runtimes/
|   `-- relay-runtime-codex/
|
|-- ingresses/
|   `-- relay-ingress-github/
|
|-- stacks/
|   `-- chatgpt-codex/
|
|-- integrations/
|   `-- planners/relay-handoff/
|
|-- contexts/
|-- schemas/
|-- docs/
|   `-- adr/
|
|-- AGENTS.md
|-- README.md
`-- Cargo.toml
```

This is a target structure. Do not create empty crates merely to satisfy the diagram.

---

## Important Terms

**Planner**  
The conversational or manual surface where implementation intent is formed.

**Planner Integration**  
A thin client/integration that converts explicitly approved intent into the Relay protocol and selects a supported submission transport.

**Handoff Task**  
A user-approved implementation specification accepted by Relay.

**Submission Transport**  
How a planner delivers a HandoffTask to Relay, such as Relay Direct/MCP, provider GitHub submission, local file, or local paste.

**Ingress**  
A Relay-side receiving mechanism for tasks, such as GitHub ingestion, local import, or Relay Direct.

**External Binding**  
An optional remote record, such as a GitHub Issue, synchronized with a canonical local Relay task.

**Provider**  
The model/tool ecosystem behind a coding runtime.

**Runtime**  
The coding agent Relay executes.

**Runtime Transport**  
How Relay talks to that runtime: CLI, SDK, API, or another mechanism.

**Runtime Profile**  
Local configuration selecting a runtime, provider, transport, and native options.

**Capability**  
A feature a runtime profile can actually support.

**Stack**  
An opinionated composition of planner integration, submission preference, optional external binding policy, runtime profile, and provider-native behavior.

**Clarification Request**  
A structured request for explanation of already-approved intent.

**Decision Request**  
A structured request for a new material decision discovered during implementation.

**Resolution Packet**  
The approved answer that allows Relay to resume or restart execution.

---

## Planner Adapter Architecture

Relay's planner integration is provider-neutral at the semantic layer and provider-native at the host edge.

```text
Planner Specification
       |
       +-- ChatGPT Skill
       +-- Claude Code Skill
       +-- OpenCode Skill
       +-- GitHub Copilot Agent Skill
       `-- Generic instructions
              |
              v
       Handoff Task v1
              |
              v
            Relay
```

All first-party adapters must pass the same conformance cases. Provider-specific installation or tool use may differ; task semantics may not.

See `docs/provider-support.md`.

---

## First Stack: `chatgpt-codex`

This stack is intentionally first-class.

It may eventually contain:

```text
Planner specification   Relay Handoff Planner Specification
Planner adapter         ChatGPT Skill (first adapter)
Submission              GitHub compatibility for v0.1; Relay Direct preferred when available
Runtime                 Codex
Runtime transport       CLI
Project instructions    AGENTS.md
Native extensions       Codex-specific capabilities/options
Recovery                automatic within approved scope
Clarification           structured packets
Push policy             disabled by default
```

The core only understands the common lifecycle and security rules.

---

## Safety Baseline

A remote task may request:

```json
{
  "project": "practicore"
}
```

It may not request:

```json
{
  "cwd": "C:\\some\\arbitrary\\directory"
}
```

The trusted local project registry owns:

- filesystem paths;
- expected Git remotes;
- default stack/runtime;
- execution policies.

Before launching a runtime, Relay must verify the project and repository.

Unknown or invalid targets fail closed.

---

## v0.1 Definition of Done

Relay v0.1 is successful when a user can:

1. launch `relay`;
2. see configured local projects;
3. receive a structured HandoffTask through GitHub or local import;
4. review the task inside the TUI;
5. see preflight repository/runtime checks;
6. explicitly choose **Run**;
7. have Codex CLI start in the correct local repository;
8. see streaming execution output;
9. allow bounded automatic implementation repair;
10. cancel a running execution when technically supported;
11. see `DONE`, `FAILED`, `NEEDS_CLARIFICATION`, or `NEEDS_DECISION`;
12. inspect a Markdown clarification/decision packet;
13. copy that packet for a planner;
14. see changed-file and Git status information;
15. restart Relay without losing task/execution history.

v0.1 must **not** automatically push code.

---

## Documentation Map

Start here:

- [`docs/product-context.md`](docs/product-context.md)
- [`docs/design-principles.md`](docs/design-principles.md)
- [`docs/agentic-usage-efficiency.md`](docs/agentic-usage-efficiency.md)
- [`docs/architecture.md`](docs/architecture.md)
- [`docs/planner-integrations.md`](docs/planner-integrations.md)
- [`docs/provider-support.md`](docs/provider-support.md) — planner-host support and installation matrix.
- [`docs/planner-adapter-conformance.md`](docs/planner-adapter-conformance.md) — shared behavioral contract for every adapter.
- [`docs/technology-stack.md`](docs/technology-stack.md)
- [`docs/task-protocol.md`](docs/task-protocol.md)
- [`docs/local-ingress.md`](docs/local-ingress.md) — local file and paste formats.
- [`docs/installation.md`](docs/installation.md) — proposed future installation experience.
- [`docs/first-run-setup.md`](docs/first-run-setup.md) — proposed setup wizard and onboarding flow.
- [`docs/github-integration.md`](docs/github-integration.md) — local GitHub detection, compatibility ingress, and optional bindings.
- [`docs/github-mailbox-setup.md`](docs/github-mailbox-setup.md) — user-owned private bootstrap mailbox setup and routing behavior.
- [`docs/submission-and-bindings.md`](docs/submission-and-bindings.md) — submission transports, canonical local state, and optional external bindings.
- [`docs/clarification-protocol.md`](docs/clarification-protocol.md)
- [`docs/runtime-stack-model.md`](docs/runtime-stack-model.md)
- [`docs/security-model.md`](docs/security-model.md)
- [`docs/data-persistence.md`](docs/data-persistence.md)
- [`docs/tui-ux.md`](docs/tui-ux.md)
- [`docs/testing-quality.md`](docs/testing-quality.md)
- [`docs/future-frontends.md`](docs/future-frontends.md)
- [`docs/build-plan.md`](docs/build-plan.md)
- [`docs/roadmap.md`](docs/roadmap.md)
- [`docs/glossary.md`](docs/glossary.md)
- [`contexts/PROJECT_CONTEXT.md`](contexts/PROJECT_CONTEXT.md)
- [`integrations/planners/relay-handoff/`](integrations/planners/relay-handoff/) — provider-neutral planner specification and adapters.
- [`integrations/planners/relay-handoff/adapters/chatgpt/`](integrations/planners/relay-handoff/adapters/chatgpt/) — first-class ChatGPT adapter.
- [`integrations/planners/relay-handoff/adapters/claude-code/`](integrations/planners/relay-handoff/adapters/claude-code/) — experimental Claude Code adapter.
- [`integrations/planners/relay-handoff/adapters/opencode/`](integrations/planners/relay-handoff/adapters/opencode/) — experimental OpenCode adapter.
- [`integrations/planners/relay-handoff/adapters/github-copilot/`](integrations/planners/relay-handoff/adapters/github-copilot/) — experimental GitHub Copilot adapter.
- [`ACKNOWLEDGMENTS.md`](ACKNOWLEDGMENTS.md) — open-source inspirations and attribution boundary.
- [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md) — notices for any future copied/vendored third-party material.
- [`AGENTS.md`](AGENTS.md)
- [`docs/adr/`](docs/adr/) — architecture decision records, including ADR-0015 for canonical local state and optional external bindings.

---

## Open-Source Direction

Relay begins private so the real workflow can be exercised before a public API is promised.

The intention is to open-source it after the first end-to-end workflow, recovery model, security boundaries, and protocol have been proven in daily use.

No license is granted by this repository until a license file is intentionally added.

---

## One Rule to Remember

Before adding a feature, ask:

> **Does this help a developer deliberately move approved intent from planning into execution, or safely complete that execution without silently changing the intent?**

If not, it may belong in a stack, runtime extension, frontend, or another project.
