# Relay Agent Instructions

This file is for coding agents working **on Relay itself**.

Read `contexts/PROJECT_CONTEXT.md` and relevant architecture documents before structural changes.

## Product Invariants

Do not violate these without an explicit ADR:

1. There is one Relay engine.
2. Planning is external and human-directed by default.
3. Execution crosses an explicit user-controlled handoff boundary.
4. After handoff, bounded implementation repair may be autonomous.
5. Material changes to approved intent require `NEEDS_CLARIFICATION` or `NEEDS_DECISION`.
6. Escalation must produce a structured packet, not a vague question.
7. Relay Core contains no LLM reasoning requirement.
8. Relay Core is provider-agnostic.
9. Provider-specific behavior belongs in stacks, runtime implementations, ingresses, or planner integrations.
10. Relay is not architecturally coupled to Ratatui.
11. Local project configuration is authoritative for filesystem paths and expected Git remotes.
12. Remote tasks cannot provide arbitrary execution paths or arbitrary shell commands.
13. v0.1 requires explicit local Run approval.
14. v0.1 never automatically pushes code.
15. Prefer capability checks over provider-name conditionals.
16. Prefer typed domain models over unstructured maps in core code.
17. Security failures fail closed.
18. Coding runtimes should not be invoked for exploratory planning by default.
19. Relay must never bypass provider quotas, billing, authentication, or usage controls.
20. First-party planner adapters must pass the shared planner-adapter conformance cases before being called first-class.
21. Treat planner host identity separately from model/provider identity when integration behavior differs by product surface.
22. After task acceptance, Relay local durable state is canonical; external bindings such as GitHub Issues are optional and must not replace Relay task identity.
23. Planner adapters prefer Relay Direct when available, then supported provider-native compatibility transports, then local artifact/paste fallback.

## Technology Baseline

Use:

- Rust stable, Edition 2024.
- Cargo workspace with resolver 3.
- Tokio for async/process orchestration.
- Ratatui + Crossterm for the first UI.
- Serde for serialization.
- JSON for canonical task/clarification wire formats.
- TOML for local configuration.
- SQLite + SQLx for durable local state.
- clap for command-line flags/subcommands.
- tracing for diagnostics.
- thiserror for library/domain errors.
- anyhow only at application boundaries.

## Architecture Rules

Presentation code must not own:

- task execution;
- ingress logic;
- project routing;
- runtime selection;
- repository validation;
- persistence policy;
- repair policy.

The TUI calls Relay Core.

Runtime implementations receive validated local execution context.

Ingress implementations move protocol data/status and do not decide implementation strategy.

Planner integrations are clients of Relay; they do not replicate core orchestration.

## Recovery / Clarification

Ordinary implementation failures should be repaired automatically within configured bounds.

When the runtime crosses the approved-intent boundary, create a typed Clarification or Decision Request containing enough context for a planner/user to decide.

Executor recommendations are not approvals.

Resolution Packets must be validated before resuming work.

## Process Execution

Never concatenate untrusted text into shell command strings.

Prefer direct process spawning with explicit executable and argument arrays.

Treat runtime output as untrusted display/log content.

Do not log secrets.

## Third-Party Inspiration

Architectural ideas may be independently implemented from public references.

Do not copy or substantially adapt third-party source code or documentation into Relay without first checking its license and preserving all required notices.

Record deliberate third-party material and required notices in `THIRD_PARTY_NOTICES.md`; use `ACKNOWLEDGMENTS.md` for inspirations and thanks.

## Changes

For non-trivial architectural changes:

1. identify affected invariant;
2. update/add an ADR;
3. update relevant docs;
4. add tests for the boundary;
5. preserve protocol compatibility unless a version change is intentional.

## Quality

Before considering implementation complete:

- `cargo fmt --check`
- `cargo clippy --workspace --all-targets --all-features`
- `cargo test --workspace`

Add focused integration tests for process boundaries, repository safety, recovery limits, and clarification/resolution flows.
