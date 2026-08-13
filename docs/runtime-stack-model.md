# Runtime and Stack Model

## Why This Model Exists

Relay must support provider-native behavior without coupling the entire application to one provider.

There is one Relay engine.

The model separates:

```text
Planner Integration
Submission Transport
Ingress
External Binding
Provider
Runtime
Runtime Transport
Runtime Profile
Capabilities
Stack
```

---

## Planner Integration

Thin client behavior on the planning side.

Responsibilities:

- recognize explicit handoff approval;
- construct a Relay protocol object;
- submit it through a supported transport;
- submit Resolution Packets.

It does not replicate Relay Core.

---

## Submission and Ingress

How work reaches Relay.

Examples:

```text
github-issues
local-file
mcp
```

An ingress may publish status/results back to its source channel. Separately, Relay may maintain an optional external binding such as a GitHub Issue.

---

## Provider

An ecosystem or vendor identity.

Examples:

```text
openai
anthropic
```

Provider identity alone does not describe execution.

---

## Runtime

The coding agent/application being controlled.

Examples:

```text
codex
claude-code
```

---

## Runtime Transport

How Relay communicates with a runtime.

Examples:

```text
cli
sdk
api
```

The same runtime may expose different capabilities depending on transport.

---

## Runtime Profile

A configured local execution target.

Concept:

```toml
[runtime_profiles.codex_cli]
provider = "openai"
runtime = "codex"
transport = "cli"
```

A profile may include provider-native options.

Those options are validated by the runtime implementation.

---

## Capabilities

Examples:

```text
streaming_output
cancellation
resume
session_list
skills
structured_output
native_diff
approval_controls
usage_snapshot
usage_limit_metadata
clarification_packets
```

Capabilities should be explicit.

The TUI enables/hides operations based on capability, not provider identity.

---

## Stack

A stack is an opinionated integration preset.

A stack can combine:

```text
Planner integration
Submission preference
Optional external binding policy
Runtime profile
Task preparation strategy
Recovery policy
Provider-native extensions
Status mapping
```

### `chatgpt-codex`

Initial stack concept:

```text
Planner             ChatGPT normal chat
Planner spec        Relay Handoff Planner Specification
Planner adapter     ChatGPT Skill
Submission          GitHub compatibility transport (v0.1)
External binding    GitHub Issue optional
Runtime             Codex
Runtime transport   CLI
Project instruction AGENTS.md
Execution approval  local/manual
Implementation fix  automatic within scope
Planning conflict   escalate
Push policy         disabled
```

---

## Recovery Policy

Stacks may tune behavior, but core understands the categories.

Default concept:

```text
implementation_error     auto_repair
verification_failure     auto_repair
requirement_ambiguity    needs_clarification
architecture_change      needs_decision
scope_expansion          needs_decision
destructive_change       needs_decision
security_policy_change   needs_decision
```

Local policy should bound:

- number of repair attempts;
- execution duration;
- whether fresh runtime sessions are used for review/repair.

---

## Generic Mode

Relay should eventually support a `generic` stack.

Generic mode favors portability over native optimization.

It is not the standard by which optimized stacks are artificially limited.

---

## Mixed Stacks

A future stack may combine:

```text
ChatGPT -> Relay -> Claude Code
Claude   -> Relay -> Codex
```

Planner identity and runtime identity therefore remain separate.

---

## Provider-Specific Extensions

Task envelopes may contain namespaced extensions.

Core must not blindly convert remote extensions into command-line arguments.

The selected runtime validates supported keys against local policy.

---

## Community Extensions

Long-term:

```text
relay
  |
  +-- built-in runtime: codex
  +-- built-in ingress: github
  |
  `-- external community runtime/ingress processes
```

External extensions should declare protocol version and capabilities.

The first public extension protocol must come from a real second implementation, not speculation.


## Runtime Usage Awareness

Where a provider exposes usage/limit information through a supported local interface, a runtime may expose normalized usage metadata as a capability.

Examples might include:

- current usage;
- remaining included allowance;
- reset time/window;
- credit availability;
- throttled/unavailable status.

This information is observational.

Relay must not attempt to bypass a provider limit.

A runtime being unavailable because of provider usage policy should be surfaced clearly in the TUI.
