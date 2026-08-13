# Roadmap

## v0.1 — Reliable Human-Controlled Handoff

Primary stack:

```text
ChatGPT -> GitHub compatibility transport -> Relay -> Codex CLI
```

Must include:

- one Relay engine;
- project registry;
- repository safety;
- task protocol;
- local JSON/Markdown file and paste ingress;
- durable state;
- GitHub ingress and optional GitHub binding;
- Codex runtime;
- live execution;
- bounded automatic repair;
- `NEEDS_CLARIFICATION`;
- `NEEDS_DECISION`;
- structured Markdown packets;
- Resolution Packets;
- no automatic push.

## v0.2 — Daily Driver

- stronger task history;
- project locks;
- better diff experience;
- clarification/resolution history;
- retry/restart controls;
- validation profiles;
- runtime diagnostics;
- `relay doctor`;
- bounded log retention.

## v0.3 — Native Stack Quality

- provider-neutral Relay Handoff Planner Specification;
- ChatGPT Skill as the first provider adapter;
- Codex-native capability improvements;
- project Skills/instruction awareness;
- optional provider-reported usage/limit display where reliable;
- better GitHub status synchronization.

## v0.4 — Second Ecosystem

- run planner-adapter conformance against Claude Code/OpenCode/Copilot as applicable;
- promote only adapters that pass real end-to-end validation;

Support a meaningfully different planner/runtime/ingress combination.

Use it to validate:

- runtime contracts;
- ingress contracts;
- stack composition;
- capability model;
- provider-native extensions;
- clarification/resolution portability.

## v0.5 — Extension Boundary

Design public out-of-process extension protocol from real integration needs.

## v1.0 — Open-Source Stable Core

Before v1.0:

- public license chosen;
- stable task/clarification protocol;
- stable config migration policy;
- runtime/ingress contributor docs;
- Windows/Linux/macOS release story;
- security documentation reviewed;
- upgrade path documented.

## Possible Later Directions

Optional, not promises:

- Tauri desktop client;
- local Axum server/browser client;
- Relay Direct/MCP submission;
- optional external binding configuration independent of submission transport;
- multiple execution lanes;
- remote Relay node;
- team task sources;
- richer source-control workflows;
- optional autonomous planning stacks.

Autonomous planning may be added as an option, but must not replace human-directed planning as Relay's default philosophy.
