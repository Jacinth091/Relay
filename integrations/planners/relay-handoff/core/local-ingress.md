# Local Relay Ingress

Relay should accept the same task through files or direct paste.

## Preferred formats

### Canonical `.json`

Use JSON when creating a machine artifact, downloadable task, or direct terminal paste payload.

The file contains only the canonical Relay protocol object.

Example filename:

```text
placement-reassignment.relay.json
```

### Human-friendly `.md`

Use Markdown when the user wants a readable artifact that can also be imported by Relay.

The Markdown prose is for humans. The embedded JSON block is authoritative.

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
  "task": {
    "goal": "Implement the approved placement reassignment behavior."
  }
}
```
<!-- relay:task:end -->
````

## Terminal paste

When a user says "format this so I can paste it into Relay," output the canonical JSON object only unless they explicitly ask for the readable wrapper.

Relay TUI should eventually support:

- Import file -> choose `.relay.json`, `.json`, `.relay.md`, or `.md`.
- Paste task -> paste canonical JSON or Markdown containing a Relay JSON block.
- Preview parsed task before importing.

Do not define separate task semantics for JSON and Markdown. Markdown is only a wrapper around the same JSON protocol.
