# First-Run Setup

> [!NOTE]
> This is the intended onboarding experience for a future Relay build. Exact screens and fields may change during implementation.

## Setup flow

The first launch should guide the user through:

```text
Environment
    ->
Projects
    ->
Planner
    ->
Submission
    ->
Optional External Binding
    ->
Runtime
    ->
Safety
    ->
Ready
```

Missing providers that are not used by the selected stack are not errors.

## Environment

Relay should detect only the tools relevant to the selected workflow, for example:

```text
Git                  found / missing
GitHub CLI           found / missing
Codex CLI            found / missing
Claude Code          found / missing
OpenCode             found / missing
```

A missing runtime or planner that the user did not select should not block setup.

## Projects

Relay executes only in locally registered projects.

Project setup should:

1. let the user choose a local repository directory;
2. verify that it is a Git worktree/repository;
3. read the Git remote when available;
4. ask for a stable Relay project key;
5. store the local path and expected remote in trusted local configuration.

A remote HandoffTask contains the logical project key, never the local path.

## Planner

The user selects a planner host such as ChatGPT, Claude Code, OpenCode, GitHub Copilot, or Generic/Manual.

Relay should then show provider-native installation/export instructions for the `relay-handoff` adapter.

For cloud/UI-managed hosts, export the supported package and show installation instructions rather than automating unsupported private interfaces.

For documented local Skill directories, Relay may offer installation after explicit user approval.

## Submission

The planner-to-Relay submission choice is separate from optional external bindings.

Conceptual choices:

```text
Relay Direct / MCP          preferred when available
GitHub compatibility        useful for the v0.1 reference workflow
Local file / paste          universal fallback
```

If Relay Direct is unavailable, setup should not fail. Local file/paste remains usable.

## Optional External Binding

After direct/local receipt, the user may choose whether Relay mirrors task lifecycle to an external system.

Initial concept:

```text
None
GitHub Issue
```

A binding is optional. Relay's local durable task record remains canonical.

## Runtime

The user chooses the coding runtime, for example Codex.

A setup test should prefer cheap/non-agentic health checks such as executable discovery and version/authentication status before asking a coding agent to perform work.

## Safety

The setup flow should clearly teach the default security model:

- registered projects only;
- expected Git remote validation where configured;
- task review before execution;
- explicit local Run approval for v0.1;
- no automatic push;
- bounded repair attempts;
- material intent changes escalate to the user.

## Completion

The final setup screen should summarize the selected stack and show a simple first-handoff example:

```text
Send this to Relay.
```

For a local-only test:

```text
Make this a local Relay file.
```
