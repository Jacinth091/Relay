# Architecture Decision Records

ADRs capture decisions that should not be casually reversed without understanding their rationale.

Current decisions:

- [0001 — Rust Core](0001-rust-core.md)
- [0002 — TUI First, Not TUI Locked](0002-tui-first-not-tui-locked.md)
- [0003 — Human-Directed Planning](0003-human-directed-planning.md)
- [0004 — Provider-Agnostic Core, Native Stacks](0004-provider-agnostic-core-native-stacks.md)
- [0005 — External-Process Extension Direction](0005-external-process-extension-direction.md)
- [0006 — SQLite Durable Local State](0006-sqlite-durable-local-state.md)
- [0007 — CLI-First Git and GitHub Integration](0007-cli-first-git-github.md)
- [0008 — JSON Task Protocol and TOML Config](0008-json-protocol-toml-config.md)

- [0009 — Single Relay Engine and Ingress Model](0009-single-relay-ingress-model.md)
- [0010 — Controlled Execution Autonomy and Structured Escalation](0010-controlled-autonomy-structured-escalation.md)
- [0011 — Agentic Usage Efficiency Without Circumvention](0011-agentic-usage-efficiency.md)
- [0012 — Local Ingress Uses Canonical JSON with Optional Markdown Wrapper](0012-local-ingress-format.md)
- [0013 — Provider-Neutral Planner Specification](0013-provider-neutral-planner-specification.md)
- [0014 — Host-Native Planner Adapters with Shared Conformance](0014-host-native-planner-adapters.md)

- `0015-canonical-local-state-optional-external-bindings.md` — keep Relay local state canonical and external records optional.
- `0016-mcp-first-submission-and-v0-1-local-descope.md` — Relay Direct/MCP is the first automatic submission transport; GitHub ingress descoped from v0.1.
- `0017-relay-identity-deterministic-handoff-engine.md` — Relay is a deterministic handoff engine (superseded by ADR-0020 for Relay Architecture v2; historical record preserved).
- `0018-provider-distribution-model.md` — deterministic provider distribution posture (unimplemented fan-out/slot direction superseded by ADR-0020; historical record preserved).
- `0019-multi-runtime-orchestration.md` — multi-runtime orchestration architecture. Renumbered from a duplicate `0009` file name; decision content unchanged.
- [0020 — Relay Architecture v2](0020-relay-architecture-v2.md) — Policy-governed workflow orchestration control plane, provider-neutral roles, execution grants, runtime backplanes, and optional semantic decisions.
