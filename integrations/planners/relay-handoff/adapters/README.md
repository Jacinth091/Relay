# Provider / Planner Host Adapters

These directories package the same Relay Handoff Planner Specification for concrete AI planner hosts.

| Adapter | Status | Native format |
|---|---|---|
| `chatgpt/` | First-class reference | ChatGPT Skill |
| `claude-code/` | Experimental first-party | Claude Code Skill |
| `opencode/` | Experimental first-party | OpenCode Skill |
| `github-copilot/` | Experimental first-party | GitHub Copilot Agent Skill |
| `generic/` | Fallback | Plain planner instructions |

A host-specific adapter must document installation, invocation, available ingresses, and fallback behavior.

All adapters must conform to `../core/PLANNER_SPEC.md` and `docs/planner-adapter-conformance.md`.
