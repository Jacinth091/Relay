# Clarification and Resolution Protocol

Status: **Foundation v1.6 draft**

This protocol defines how Relay explains an implementation blocker and how a planner/user resolves it.

The goal is to avoid vague escalation.

---

## Why This Exists

A coding runtime may discover something during implementation that cannot be safely resolved through normal repair.

Examples:

- two architecture choices satisfy the visible requirement differently;
- an existing data model conflicts with the approved intent;
- a requirement is ambiguous;
- continuing would expand scope;
- a destructive change appears necessary.

Relay should not merely say:

```text
NEEDS_INPUT
```

Relay should explain enough context that the user can return to a planner and make a grounded decision.

---

## Request Types

### `NEEDS_CLARIFICATION`

Use when approved intent exists but is unclear.

Example:

```text
The task says "admin", but the repository has both system administrators
and practicum coordinators.
```

### `NEEDS_DECISION`

Use when implementation uncovered a material choice that was not previously approved.

Example:

```text
Preserving DTR history requires choosing whether DTR ownership remains
placement-based or moves to a practicum-level record.
```

---

## Clarification / Decision Request Object

Canonical conceptual fields:

```json
{
  "protocol_version": 1,
  "request_id": "uuid",
  "task_id": "uuid",
  "execution_id": "uuid",
  "kind": "needs_decision",
  "category": "architecture",
  "summary": "DTR ownership requires a design decision.",
  "work_attempted": "...",
  "work_completed": [
    "Added reassignment service flow.",
    "Preserved historical placement records."
  ],
  "blocking_reason": "...",
  "evidence": [
    "Current relationship is DTR -> Placement.",
    "Moving ownership affects reporting queries."
  ],
  "question": "Should historical DTR records remain attached to the placement where they were recorded?",
  "options": [
    {
      "id": "A",
      "label": "Keep placement ownership",
      "benefits": ["Preserves historical accuracy."],
      "costs": ["Requires aggregation across placements."]
    },
    {
      "id": "B",
      "label": "Move to practicum-level ownership",
      "benefits": ["Simplifies total-hours queries."],
      "costs": ["Broad schema and service changes."]
    }
  ],
  "executor_assessment": "Option A appears more consistent with the approved historical-record requirement.",
  "repository_state": {
    "changed_files": [
      "src/services/placement.service.ts",
      "src/services/dtr.service.ts"
    ]
  },
  "verification_state": [
    "typecheck: pass",
    "existing placement tests: pass",
    "reassignment tests: blocked"
  ],
  "resume_point": "Resume DTR reassignment implementation after ownership decision."
}
```

Exact field naming may evolve before schema stabilization.

---

## Markdown Rendering

Relay should be able to render the structured request as Markdown.

Example:

```markdown
## Relay needs a decision — Task #42

**Project:** PractiCore
**State:** NEEDS_DECISION
**Runtime:** Codex
**Reason:** Architecture decision required

### What I was implementing

...

### What has been implemented so far

- ...

### What caused the stop

...

### Relevant context discovered

...

### Decision needed

...

### Options identified

**A — ...**
- ...

**B — ...**
- ...

### Executor assessment

...

### Current repository state

...

### Resume point

...
```

The Markdown is a presentation.

The structured request object is authoritative.

---

## Executor Assessment Is Not Authority

The runtime may recommend an option.

That recommendation is evidence, not approval.

The planner/user may disagree.

Relay must preserve the distinction between:

```text
executor recommendation
```

and:

```text
approved resolution
```

---

## Resolution Packet

After discussion, the planner/user returns a structured resolution.

Concept:

```json
{
  "protocol_version": 1,
  "resolution_id": "uuid",
  "request_id": "uuid",
  "task_id": "uuid",
  "decision": "Keep DTR records owned by their historical placement.",
  "rationale": "This preserves historical placement accuracy and matches the approved requirement.",
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

Possible `work_disposition` values may include:

```text
keep_and_continue
revise_existing_work
restart_execution
revert_partial_work
cancel_task
```

These values should be finalized after implementation experience.

---

## GitHub Bridge

For the first stack:

```text
Relay
  ->
optional GitHub binding comment containing Markdown packet
  ->
user/planner discussion
  ->
Resolution Packet
  ->
same Issue / Relay ingress
  ->
Relay resumes
```

The issue becomes durable history of:

- original intent;
- implementation state;
- discovered decision;
- resolution;
- final result.

---

## Clipboard Workflow

The TUI should offer an action such as:

```text
Copy for Planner
```

It produces:

```markdown
Relay task #42 needs a decision.

Please help me reason through this blocker. Do not implement anything yet.

[rendered Relay packet]
```

This keeps compatibility and local workflows usable when direct Relay submission is unavailable.

---

## Future Direct Planner Integration

When MCP or another callable planner mechanism exists:

```text
Relay -> planner integration -> planner -> resolution -> Relay
```

The same request/resolution protocol remains valid.

Only the ingress/communication path changes.

---

## Autonomy Boundary

Relay may continue without escalation when the fix is clearly required by the approved intent.

Relay escalates when:

- multiple materially different behaviors remain possible;
- a new architecture decision is required;
- scope must expand;
- destructive behavior is introduced;
- security/policy assumptions change;
- the runtime is uncertain whether a repair remains inside approved intent.

When uncertain about whether a decision changes intent, prefer escalation.
