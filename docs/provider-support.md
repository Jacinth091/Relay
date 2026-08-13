# Planner Host Support

Provider integration mechanisms change over time. Re-verify host installation/tool details before implementing installation automation.

Relay distinguishes a model/provider from the product surface that hosts the planner integration.

The relevant compatibility unit is the **planner host**.

| Planner host | Adapter status | Native mechanism | Local handoff | Relay Direct | Provider GitHub compatibility |
|---|---|---|---|---|---|
| ChatGPT | First-class reference | ChatGPT Skill | Yes | Preferred when a supported Relay direct tool/MCP connection is available | Supported when a write-capable GitHub integration is available |
| Claude Code | Experimental first-party | Claude Code Skill | Yes | Preferred when Relay MCP/direct tooling is configured | Supported when authorized GitHub tooling/MCP/`gh` is available |
| OpenCode | Experimental first-party | OpenCode Skill | Yes | Preferred when Relay MCP/direct tooling is configured | Supported when authorized GitHub tooling/MCP/`gh` is available |
| GitHub Copilot | Experimental first-party | Copilot Agent Skill | Yes | Preferred when the active surface can call Relay directly | Supported when the active surface has authorized issue-write capability |
| Generic | Fallback | Plain instructions | Yes | Host-dependent | Host-dependent |

## Status meanings

### First-class reference

- provider-native packaging exists;
- installation is documented;
- intended to be exercised in Relay's reference stack;
- adapter behavior is part of v0.1 acceptance testing.

### Experimental first-party

- provider-native integration mechanism is documented;
- a concrete adapter is included;
- protocol semantics are defined;
- end-to-end Relay testing is still required before claiming first-class support.

### Generic

A portable instruction document for hosts without a dedicated adapter.

## Submission capability vs external binding

Planner-host capability answers:

> How can this planner deliver a HandoffTask to Relay?

Relay-side binding configuration answers:

> After Relay receives it, should Relay also mirror/synchronize it somewhere such as a GitHub Issue?

Those are separate questions.

A host can therefore support Relay Direct even if it has no GitHub access at all.

## Why support is host-specific

`OpenAI`, `Anthropic`, or another vendor name is not enough to describe integration behavior.

A provider may expose multiple hosts with different installation and tool capabilities.

Relay configuration should therefore prefer fields such as:

```toml
planner_host = "chatgpt"
planner_adapter = "relay-handoff"
```

rather than assuming provider identity determines installation behavior.

## Alignment guarantee

Every first-party adapter must pass the same behavioral conformance cases.

Provider-specific code may change:

- installation;
- skill metadata supported by the host;
- tool/connector invocation;
- automatic submission transports.

It may not change:

- explicit user handoff semantics;
- canonical task JSON;
- local `.relay.json` / `.relay.md` meaning;
- clarification/decision semantics;
- security boundaries;
- truthful submission reporting;
- the rule that GitHub binding is optional after direct Relay receipt.
