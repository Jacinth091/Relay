# Planner Adapter Conformance

Every first-party planner adapter must preserve the Relay Handoff Planner Specification even when installation and tool capabilities differ by host.

## Required cases

### 1. Planning must not hand off

Input:

```text
Let's discuss whether we should add another service.
```

Expected: remain in planning. Do not create a task or invoke a submission transport.

### 2. Explicit handoff

Input:

```text
Send this to Relay.
```

Expected:

1. construct HandoffTask v1 from approved intent;
2. prefer Relay Direct when available;
3. otherwise use an authorized provider-native remote compatibility transport;
4. otherwise provide local/pasteable task material.

### 3. Truthful confirmation

If a receiver does not confirm acceptance/creation, the adapter must not say the task was sent.

### 4. GitHub is not mandatory

When Relay Direct confirms acceptance, the adapter must consider the handoff complete even if no GitHub Issue exists.

### 5. Local artifact

Input:

```text
Make this a local Relay file.
```

Expected: produce canonical `.relay.json` unless the user explicitly asks for readable Markdown.

### 6. Readable local artifact

Input:

```text
Give me a readable Relay file.
```

Expected: `.relay.md` whose machine meaning is the embedded canonical JSON block, not inferred prose.

### 7. Pasteable task

Input:

```text
Format this so I can paste it into Relay.
```

Expected: canonical JSON.

### 8. Decision packet

Input includes a Relay `NEEDS_DECISION` packet.

Expected: discuss the blocker; do not implement; produce a Resolution Packet only after the user chooses a direction.

### 9. No local path injection

The planner must never put a trusted local repository path into the remote task protocol.

### 10. Rejected alternatives stay rejected

Task construction must not accidentally promote options rejected during planning into requirements.

### 11. Binding remains optional

If the task arrives by Relay Direct or local import, a later GitHub binding must not change the recorded submission provenance.

## Support claim

A provider adapter should not be labeled first-class until its installation path, explicit handoff, submission/fallback behavior, clarification/resolution loop, and security boundaries have been exercised end to end.
