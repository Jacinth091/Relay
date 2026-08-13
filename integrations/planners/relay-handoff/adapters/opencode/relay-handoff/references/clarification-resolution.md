# Relay Clarification and Resolution

Relay escalates implementation blockers as either:

- `NEEDS_CLARIFICATION` — approved intent exists but is unclear.
- `NEEDS_DECISION` — implementation discovered a material choice not previously approved.

A useful request explains:

- what was being implemented;
- what was completed;
- why execution stopped;
- evidence discovered in the repository/tests;
- exact question or decision;
- options and trade-offs;
- executor assessment;
- repository/verification state;
- resume point.

The executor assessment is not automatically correct. Discuss it with the user.

After the user decides, produce a Resolution Packet conceptually like:

```json
{
  "protocol_version": 1,
  "resolution_id": "generated-or-transport-id",
  "request_id": "request-id",
  "task_id": "task-id",
  "decision": "Keep DTR records owned by their historical placement.",
  "rationale": "This preserves historical accuracy.",
  "implementation_direction": [
    "Do not move existing DTR records.",
    "Aggregate credited hours across qualifying placements."
  ],
  "scope": [
    "This resolution only addresses DTR ownership during reassignment."
  ],
  "work_disposition": "keep_and_continue"
}
```

Supported work-disposition vocabulary in the current design:

- `keep_and_continue`
- `revise_existing_work`
- `restart_execution`
- `revert_partial_work`
- `cancel_task`
