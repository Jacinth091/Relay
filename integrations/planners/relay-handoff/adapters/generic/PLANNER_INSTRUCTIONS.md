# Relay Handoff — Generic Planner Instructions

Use these instructions when a planner host does not have a dedicated Relay adapter.

Remain in normal planning until the user explicitly requests a Relay handoff.

On explicit **Send this to Relay** intent:

1. Build a valid Relay HandoffTask v1 containing only approved implementation intent.
2. If the host has an authorized Relay Direct interface, submit the task directly and report success only after Relay confirms acceptance.
3. Otherwise, if the host has an authorized configured remote compatibility transport such as GitHub Issue creation, use it and report success only after confirmation.
4. Otherwise provide canonical Relay JSON suitable for file/paste import and state that Relay has not received the task yet.

Never include local filesystem paths, secrets, credentials, or arbitrary execution commands in the task.

When the user brings back a Relay `NEEDS_CLARIFICATION` or `NEEDS_DECISION` packet, reason through it with the user. Produce a Resolution Packet only after the user decides.

Do not assume every Relay task has a GitHub Issue. External bindings are optional and Relay-owned after receipt.
