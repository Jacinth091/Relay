# Handoff Task Protocol

Status: **Draft v1 contract**

The protocol separates planner integrations/submission transports from local Relay execution and from optional external bindings.

The canonical wire representation is JSON.

---

## Handoff Envelope

Example:

```json
{
  "protocol_version": 1,
  "project": "practicore",
  "title": "Implement placement reassignment",
  "stack": "chatgpt-codex",
  "source": {
    "kind": "conversational_ai",
    "provider": "openai"
  },
  "task": {
    "goal": "Implement the approved placement reassignment behavior.",
    "requirements": [
      "Preserve historical placement data.",
      "Preserve previously credited records."
    ],
    "constraints": [
      "Do not redesign unrelated modules.",
      "Do not push automatically."
    ],
    "verification": [
      "Run relevant tests.",
      "Run type checking."
    ]
  },
  "ambiguity_policy": "needs_clarification",
  "extensions": {
    "openai": {
      "codex": {
        "use_project_skills": true
      }
    }
  }
}
```

---

## Core Fields

### `protocol_version`

Required integer.

Relay rejects unsupported major versions rather than guessing.

### `project`

Required logical project key.

Never a filesystem path.

### `title`

Required concise human-readable title.

### `stack`

Optional requested stack.

Local policy decides whether the requested stack is allowed.

### `source`

Optional provenance metadata.

Source metadata is informational and does not grant trust.

### `task.goal`

Required approved implementation objective.

### `task.requirements`

Optional but strongly recommended concrete behavior.

### `task.constraints`

Optional boundaries and preservation requirements.

### `task.verification`

Optional expected validation outcomes.

### `ambiguity_policy`

Initial value:

```text
needs_clarification
```

Important ambiguity should stop implementation rather than invent product behavior.

### `extensions`

Namespaced provider/stack-native data.

Core preserves supported extension payloads but does not interpret arbitrary provider configuration.

Local policy decides which extension keys can influence execution.

---

## Forbidden Remote Fields

A task must not directly choose:

- local working directory;
- executable path;
- arbitrary shell command;
- Git credential;
- auth token;
- arbitrary expected remote;
- unrestricted environment variables.

Those are local configuration concerns.

---

## GitHub Issue Representation (Future Optional Transport or Binding)

> Not implemented in v0.1 (ADR-0016 descope). Preserved as future design; the HandoffTask remains fully transport-neutral.

When GitHub carries a task into Relay, the GitHub ingress should render/read a human-readable issue while retaining a canonical machine block. When GitHub is only an external binding, Relay may mirror the task and lifecycle there without making the Issue canonical.

Concept:

````markdown
# Implement placement reassignment

Project: `practicore`
Stack: `chatgpt-codex`

## Goal
...

## Requirements
...

<!-- relay:task:start -->
```json
{ ...canonical task envelope... }
```
<!-- relay:task:end -->
````

Machine parsing uses the canonical block rather than inferring the task from arbitrary prose.

---


## Local File and Paste Representation

The canonical local representation is the same JSON object described above.

Preferred file extension:

```text
*.relay.json
```

Relay may also import a human-readable Markdown wrapper such as:

```text
*.relay.md
```

A Markdown artifact must contain one canonical JSON task block between Relay markers. The prose is presentation only; the embedded JSON is authoritative.

The TUI should support both:

- **Import file** — select JSON or Markdown;
- **Paste task** — paste canonical JSON or Markdown containing a Relay task block.

A planner integration must be able to generate either form when the user explicitly requests a local/offline Relay artifact instead of remote submission.

See `docs/local-ingress.md`.

---
## Related Protocol Objects

Execution may later produce:

- Clarification Request;
- Decision Request;
- Resolution Packet;
- Execution Result.

These are separate structured objects linked to the Relay task ID/execution ID.

See `docs/clarification-protocol.md`.

---

## Task Identity

Submission provenance, external binding identity, and Relay identity are separate.

Example:

```text
GitHub Issue #18
    ->
Relay task UUID
```

This prevents the domain model from assuming every submission transport or Relay task has a GitHub/numeric external identity.

---

## Trust

Valid JSON is not trusted JSON.

After protocol validation, Relay still applies:

- project allowlisting;
- stack policy;
- runtime capability checks;
- repository validation;
- local execution approval.
