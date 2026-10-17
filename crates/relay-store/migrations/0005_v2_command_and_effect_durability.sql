-- Migration 0005: Relay Architecture v2 - Wave A command/effect durability
-- hardening (R12-03, R12-04).
--
-- SQLite cannot ALTER a CHECK constraint in place, so the commands table is
-- recreated with an additional 'needs_reconciliation' status, used when
-- startup reconciliation finds a non-terminal command whose payload cannot
-- be safely replayed (e.g. corrupted JSON).

CREATE TABLE commands_new (
    id TEXT PRIMARY KEY NOT NULL,
    command_type TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    status TEXT NOT NULL,
    response_json TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (status IN ('received', 'applied', 'failed', 'needs_reconciliation'))
);

INSERT INTO commands_new (id, command_type, payload_json, status, response_json, created_at, updated_at)
SELECT id, command_type, payload_json, status, response_json, created_at, updated_at FROM commands;

DROP TABLE commands;
ALTER TABLE commands_new RENAME TO commands;

CREATE INDEX idx_commands_status ON commands(status);
CREATE INDEX idx_commands_created_at ON commands(created_at);

-- Supports the effect worker's scan for pending_effects stuck in 'in_flight'
-- past a staleness threshold (crash recovery for effects, not just commands).
CREATE INDEX idx_pending_effects_updated_at ON pending_effects(updated_at);
