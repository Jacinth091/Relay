-- Migration 0008: Relay Architecture v2 - serialized command resolution (PR #16 review R16-05)
--
-- Concurrent replays of the same command id could previously both observe a
-- non-terminal status and both apply the command, racing the terminal result.
-- `claimed_at` marks the single applier: claiming is one atomic UPDATE on this
-- column, and a claim left behind by a crashed daemon is cleared during startup
-- reconciliation, before any request listener accepts connections.

ALTER TABLE commands ADD COLUMN claimed_at TEXT;
