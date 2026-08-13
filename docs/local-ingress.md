# Local File and Paste Ingress

## Purpose

Relay should remain usable without GitHub or MCP.

The local ingress accepts tasks directly from the user's machine through either:

```text
file import
```

or:

```text
direct paste in the Relay TUI
```

This is still the same Relay Task Protocol. Local ingress does not invent another task schema.

---

## Canonical Format: JSON

JSON is the authoritative machine representation.

Recommended extension:

```text
*.relay.json
```

Example:

```json
{
  "protocol_version": 1,
  "project": "practicore",
  "title": "Implement placement reassignment",
  "stack": "chatgpt-codex",
  "task": {
    "goal": "Implement the approved placement reassignment behavior.",
    "requirements": [
      "Preserve historical placement records."
    ],
    "constraints": [
      "Do not push automatically."
    ],
    "verification": [
      "Run relevant tests."
    ]
  },
  "ambiguity_policy": "needs_clarification"
}
```

Use JSON when:

- the user asks for a Relay task file;
- another tool is generating the artifact;
- the user wants to paste directly into Relay;
- deterministic parsing is more important than presentation.

---

## Human-Friendly Format: Markdown

Markdown is an optional readable wrapper around the exact same canonical JSON.

Recommended extension:

```text
*.relay.md
```

Example:

````markdown
# Relay Handoff — Implement placement reassignment

Project: `practicore`
Stack: `chatgpt-codex`

## Goal

Implement the approved placement reassignment behavior.

## Requirements

- Preserve historical placement records.

<!-- relay:task:start -->
```json
{
  "protocol_version": 1,
  "project": "practicore",
  "title": "Implement placement reassignment",
  "stack": "chatgpt-codex",
  "task": {
    "goal": "Implement the approved placement reassignment behavior.",
    "requirements": [
      "Preserve historical placement records."
    ]
  }
}
```
<!-- relay:task:end -->
````

Relay parses the JSON block, not the surrounding prose.

This keeps the file easy for humans and conversational AIs to read while preserving deterministic machine input.

---

## TUI UX

Relay should eventually expose:

```text
New Task
  > Import File
    Paste Task
```

### Import File

Accept:

```text
.relay.json
.json
.relay.md
.md
```

For generic `.json` and `.md`, Relay must validate that the content is actually a Relay protocol object before import.

### Paste Task

Open an editor/modal where the user can paste:

- canonical JSON; or
- Markdown containing a Relay task block.

Relay then shows a parsed preview:

```text
Project: PractiCore
Title: Implement placement reassignment
Stack: chatgpt-codex

[Import] [Cancel]
```

Do not execute immediately after paste/import.

The normal review and Run boundary still applies.

---

## Planner Behavior

A planner integration should respond to explicit requests such as:

```text
Make this a local Relay file.
Format this so I can paste it into Relay.
Don't send this to GitHub; give me the Relay task locally.
```

Default behavior:

- local **file** request -> canonical `.relay.json`;
- **readable Markdown file** request -> `.relay.md` with embedded JSON;
- **paste** request -> canonical JSON code block.

The planner may also offer both JSON and Markdown when the user wants a portable human-readable artifact.

---

## Why JSON Is Canonical

Markdown is easier to read, but it is intentionally not the semantic protocol.

Using JSON as the canonical payload gives Relay:

- deterministic parsing;
- JSON Schema validation;
- cross-language interoperability;
- stable machine-to-machine behavior;
- no need to infer meaning from headings or prose.

Markdown remains valuable as a presentation/transport wrapper.
