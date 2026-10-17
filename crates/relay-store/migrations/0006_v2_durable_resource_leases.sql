-- Migration 0006: Relay Architecture v2 - Wave B durable resource leases (R12-08)
--
-- Persists durable, monotonically fenced write/read/verification leases for
-- checkouts, tasks, and workflow runs. Ensures crash-safe write admission and
-- fencing token monotonicity across daemon restarts.

CREATE TABLE resource_leases (
    id TEXT PRIMARY KEY NOT NULL,
    resource_type TEXT NOT NULL,
    resource_id TEXT NOT NULL,
    lease_kind TEXT NOT NULL,
    holder_id TEXT NOT NULL,
    holder_pid INTEGER NOT NULL,
    holder_host TEXT NOT NULL,
    fencing_token INTEGER NOT NULL,
    acquired_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    expires_at TEXT NOT NULL,
    state TEXT NOT NULL,
    CHECK (resource_type IN ('checkout', 'task', 'workflow_run')),
    CHECK (lease_kind IN ('exclusive_write', 'shared_read', 'verification')),
    CHECK (state IN ('active', 'expired', 'superseded', 'released'))
);

CREATE INDEX idx_resource_leases_resource ON resource_leases(resource_type, resource_id);
CREATE INDEX idx_resource_leases_state ON resource_leases(state);
CREATE INDEX idx_resource_leases_holder ON resource_leases(holder_id);
