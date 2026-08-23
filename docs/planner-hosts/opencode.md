# OpenCode as a Planner Host for Relay

Status: **implemented transport, no Relay-side host code** (M9). OpenCode
speaks to Relay through the standard MCP stdio protocol; everything here is
host configuration plus agent instructions, never Relay-side glue
([ADR-0016](../adr/0016-mcp-first-submission-and-v0-1-local-descope.md)).

## 1. Register `relay serve-mcp` as a local stdio MCP server

OpenCode registers local MCP servers in its JSON configuration
(`opencode.json` / `opencode.jsonc`):

```json
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "relay": {
      "type": "local",
      "command": ["relay", "serve-mcp"]
    }
  }
}
```

> Verify exact key names/syntax against current OpenCode documentation;
> MCP server configuration formats evolve. The invariant is: **local/stdio**
> server type running `relay serve-mcp`.

`relay` must be on the PATH OpenCode uses, or use an absolute path to the
binary. Relay reads its own configuration from the platform default location
(overridable with `RELAY_CONFIG_PATH`) — projects are registered there.

## 2. Agent usage instructions

When the user says **Send this to Relay** (or equivalent), the agent should:

1. Build ONE canonical HandoffTask v1 JSON object (no wrapper keys, no
   templates) — schema in [docs/task-protocol.md](../task-protocol.md).
2. Call the MCP tool `submit_handoff`; the tool arguments ARE the task JSON
   object itself.
3. On success the structured result contains:

   ```json
   { "task_id": "<Relay TaskId>", "state": "Ready" }
   ```

   A returned `task_id` means the task was durably persisted by Relay and has
   landed in **Ready** state. It does NOT mean execution started or will start.
4. Tell the user the truth about state: *the handoff is queued for review;
   execution requires explicit Run approval inside the Relay TUI* (invariant
   13). Never claim implementation began.
5. Treat structured JSON-RPC errors as failures of submission, not of intent.
   Notable codes: unknown/unregistered `project` key (-32001) fails closed —
   the project must exist in Relay's trusted local registry first; invalid
   payload fields (-32005); oversized payload (-32003).
6. Do not attempt to trigger runs, approve executions, or read execution
   results back over this transport. Submission is the entire contract.

## 3. Fallback when MCP is unavailable

Per the ADR-0016 layering (local paste/file first, MCP as primary automation,
optional GitHub bridge last): if the `relay` MCP server cannot be reached,
produce the canonical task as a `.relay.json` file, a readable `.relay.md`
wrapper, or pasteable JSON for local import instead of claiming success.

See [docs/local-ingress.md](../local-ingress.md).
