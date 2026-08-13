# Relay Handoff Task Protocol

Use JSON as the canonical machine representation.

```json
{
  "protocol_version": 1,
  "project": "example-project",
  "title": "Implement an approved change",
  "stack": "chatgpt-codex",
  "source": {
    "kind": "conversational_ai",
    "provider": "openai"
  },
  "task": {
    "goal": "Implement the behavior approved during planning.",
    "requirements": [
      "Preserve existing behavior outside the requested change."
    ],
    "constraints": [
      "Do not redesign unrelated modules.",
      "Do not push automatically."
    ],
    "verification": [
      "Run relevant tests."
    ]
  },
  "ambiguity_policy": "needs_clarification",
  "extensions": {}
}
```

Rules:

- `project` is a logical Relay project key, never a local path.
- Keep the goal concise but sufficient for implementation.
- Put concrete accepted behavior in `requirements`.
- Put explicit non-goals/preservation rules in `constraints`.
- Put expected checks/outcomes in `verification`.
- Use namespaced `extensions` only for supported stack/runtime-native behavior.
- Never place secrets, tokens, executable paths, arbitrary shell commands, or local cwd values in a remote task.
