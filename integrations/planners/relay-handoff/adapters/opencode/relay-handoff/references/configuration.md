# Relay Planner Integration Configuration

This reference is provider-facing and must remain independent of local project filesystem paths.

## Submission resolution

For an explicit **Send this to Relay** request, resolve submission capability in this order unless the user requested a specific mode:

1. Authorized direct Relay submission configured for the planner host, such as Relay MCP.
2. Authorized provider-native compatibility transport, such as GitHub Issue creation in the configured Relay mailbox.
3. Local Relay JSON/Markdown or pasteable canonical JSON.

Never invent a transport or claim success without confirmation from the receiver.

## Direct Relay configuration

Conceptual host configuration may identify a Relay tool/server rather than a GitHub repository:

```text
submission: relay-direct
transport: mcp
server: relay
```

Exact provider configuration is host-specific and may change. Keep it in the host adapter, not in the canonical task protocol.

## Reference GitHub compatibility configuration

```text
submission: provider-github
repository: <user-configured private handoff repository>
default_stack: <user-configured stack>
ready_label: relay-ready (optional)
```

The repository is a task mailbox. It is not the target project repository and does not provide a local execution path.

GitHub submission is a compatibility transport when a planner cannot reach Relay directly. It is not required when Relay Direct is available.

## Project resolution

Prefer a stable Relay project key supplied by planner/project context, such as:

```text
practicore
jci-backend
```

Never infer a local path from a GitHub repository name.

## Host capability

A planner host may submit automatically only through an authorized supported mechanism.

Examples include:

- a configured Relay MCP/direct tool;
- a connected GitHub app/connector with issue creation;
- an authorized GitHub MCP server with issue creation;
- an authenticated local `gh` CLI when the host is allowed to invoke it.

If no automatic transport is available:

- prepare the complete Relay task artifact/payload;
- say submission was not completed;
- do not claim a Relay task ID, GitHub Issue number, or successful handoff.

## External binding configuration is local to Relay

Whether Relay creates an optional GitHub Issue after direct receipt is Relay-side configuration and does not belong in the planner's HandoffTask.

The planner may know that a stack usually uses a binding for display purposes, but it must not require or fabricate one.
