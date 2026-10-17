-- Migration 0004: Relay Architecture v2 - Phase 01 Durable Commands, Pending Effects, and Daemon Leases

CREATE TABLE commands (
    id TEXT PRIMARY KEY NOT NULL,
    command_type TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    status TEXT NOT NULL,
    response_json TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (status IN ('received', 'applied', 'failed'))
);

CREATE INDEX idx_commands_status ON commands(status);
CREATE INDEX idx_commands_created_at ON commands(created_at);

CREATE TABLE pending_effects (
    id TEXT PRIMARY KEY NOT NULL,
    effect_type TEXT NOT NULL,
    idempotency_key TEXT NOT NULL UNIQUE,
    payload_json TEXT NOT NULL,
    state TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (state IN ('pending', 'in_flight', 'completed', 'failed', 'cancelled'))
);

CREATE INDEX idx_pending_effects_state ON pending_effects(state);
CREATE INDEX idx_pending_effects_idempotency_key ON pending_effects(idempotency_key);

CREATE TABLE effect_results (
    id TEXT PRIMARY KEY NOT NULL,
    effect_id TEXT NOT NULL,
    outcome TEXT NOT NULL,
    result_json TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (outcome IN ('success', 'failure')),
    FOREIGN KEY (effect_id) REFERENCES pending_effects(id) ON DELETE RESTRICT
);

CREATE INDEX idx_effect_results_effect_id ON effect_results(effect_id);

CREATE TABLE daemon_leases (
    lease_id TEXT PRIMARY KEY NOT NULL,
    holder_pid INTEGER NOT NULL,
    holder_host TEXT NOT NULL,
    acquired_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    heartbeat_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    fencing_token INTEGER NOT NULL DEFAULT 1
);
