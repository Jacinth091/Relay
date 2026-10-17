-- M9 Phase 03: allow the Relay Direct (MCP) submission transport.
--
-- SQLite cannot alter a CHECK constraint in place, so the tasks table is
-- rebuilt with 'relay-mcp' added to its submission_transport CHECK (see
-- docs/submission-and-bindings.md).
--
-- The rebuild runs while foreign_keys is ON, so it uses the safe rename-aside
-- recipe: every old table is renamed to *_old first (renames rewrite the
-- REFERENCES clauses in remaining old tables), the final-schema tables are
-- created fresh, data is copied parent-first so immediate FK checks pass, and
-- only then are the emptied old tables dropped child-first. No DROP of a
-- parent ever happens while a table holding rows still references it.

ALTER TABLE tasks RENAME TO tasks_old;
ALTER TABLE external_bindings RENAME TO external_bindings_old;
ALTER TABLE executions RENAME TO executions_old;
ALTER TABLE clarification_requests RENAME TO clarification_requests_old;
ALTER TABLE resolutions RENAME TO resolutions_old;
ALTER TABLE events RENAME TO events_old;

CREATE TABLE tasks (
    id TEXT PRIMARY KEY NOT NULL,
    project_key TEXT NOT NULL,
    title TEXT NOT NULL,
    requested_stack TEXT,
    submission_transport TEXT NOT NULL,
    source_kind TEXT,
    source_provider TEXT,
    state TEXT NOT NULL,
    protocol_version INTEGER NOT NULL,
    handoff_json TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (submission_transport IN ('local-file', 'local-paste', 'relay-mcp')),
    CHECK (state IN (
        'received',
        'ready',
        'running',
        'verifying',
        'repairing',
        'needs_clarification',
        'needs_decision',
        'resolved',
        'done',
        'failed',
        'cancelled'
    )),
    CHECK (protocol_version >= 1)
);

CREATE TABLE external_bindings (
    id TEXT PRIMARY KEY NOT NULL,
    task_id TEXT NOT NULL,
    binding_kind TEXT NOT NULL,
    remote_identity TEXT NOT NULL,
    sync_status TEXT NOT NULL,
    last_sync_error TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    FOREIGN KEY (task_id) REFERENCES tasks(id) ON DELETE RESTRICT
);

CREATE TABLE executions (
    id TEXT PRIMARY KEY NOT NULL,
    task_id TEXT NOT NULL,
    runtime_profile TEXT NOT NULL,
    phase TEXT NOT NULL,
    repair_attempt_count INTEGER NOT NULL DEFAULT 0,
    started_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    ended_at TEXT,
    exit_outcome TEXT,
    failure_category TEXT,
    repository_snapshot_json TEXT,
    runtime_metadata_json TEXT,
    FOREIGN KEY (task_id) REFERENCES tasks(id) ON DELETE RESTRICT
);

CREATE TABLE clarification_requests (
    id TEXT PRIMARY KEY NOT NULL,
    task_id TEXT NOT NULL,
    execution_id TEXT,
    request_kind TEXT NOT NULL,
    category TEXT NOT NULL,
    packet_json TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    resolved_at TEXT,
    publication_json TEXT,
    FOREIGN KEY (task_id) REFERENCES tasks(id) ON DELETE RESTRICT,
    FOREIGN KEY (execution_id) REFERENCES executions(id) ON DELETE SET NULL
);

CREATE TABLE resolutions (
    id TEXT PRIMARY KEY NOT NULL,
    request_id TEXT NOT NULL,
    resolution_json TEXT NOT NULL,
    work_disposition TEXT NOT NULL,
    resume_outcome TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    FOREIGN KEY (request_id) REFERENCES clarification_requests(id) ON DELETE RESTRICT
);

CREATE TABLE events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT,
    execution_id TEXT,
    event_kind TEXT NOT NULL,
    event_json TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    FOREIGN KEY (task_id) REFERENCES tasks(id) ON DELETE RESTRICT,
    FOREIGN KEY (execution_id) REFERENCES executions(id) ON DELETE SET NULL
);

INSERT INTO tasks (
    id, project_key, title, requested_stack, submission_transport,
    source_kind, source_provider, state, protocol_version, handoff_json,
    created_at, updated_at
)
SELECT
    id, project_key, title, requested_stack, submission_transport,
    source_kind, source_provider, state, protocol_version, handoff_json,
    created_at, updated_at
FROM tasks_old;

INSERT INTO executions (
    id, task_id, runtime_profile, phase, repair_attempt_count, started_at,
    ended_at, exit_outcome, failure_category, repository_snapshot_json,
    runtime_metadata_json
)
SELECT
    id, task_id, runtime_profile, phase, repair_attempt_count, started_at,
    ended_at, exit_outcome, failure_category, repository_snapshot_json,
    runtime_metadata_json
FROM executions_old;

INSERT INTO external_bindings (
    id, task_id, binding_kind, remote_identity, sync_status,
    last_sync_error, created_at, updated_at
)
SELECT
    id, task_id, binding_kind, remote_identity, sync_status,
    last_sync_error, created_at, updated_at
FROM external_bindings_old;

INSERT INTO clarification_requests (
    id, task_id, execution_id, request_kind, category, packet_json,
    created_at, resolved_at, publication_json
)
SELECT
    id, task_id, execution_id, request_kind, category, packet_json,
    created_at, resolved_at, publication_json
FROM clarification_requests_old;

INSERT INTO resolutions (
    id, request_id, resolution_json, work_disposition, resume_outcome, created_at
)
SELECT
    id, request_id, resolution_json, work_disposition, resume_outcome, created_at
FROM resolutions_old;

INSERT INTO events (
    id, task_id, execution_id, event_kind, event_json, created_at
)
SELECT
    id, task_id, execution_id, event_kind, event_json, created_at
FROM events_old;

DROP TABLE external_bindings_old;
DROP TABLE events_old;
DROP TABLE resolutions_old;
DROP TABLE clarification_requests_old;
DROP TABLE executions_old;
DROP TABLE tasks_old;

CREATE INDEX idx_tasks_project_key ON tasks(project_key);
CREATE INDEX idx_tasks_state ON tasks(state);
CREATE INDEX idx_tasks_created_at ON tasks(created_at);

CREATE INDEX idx_external_bindings_task_id ON external_bindings(task_id);
CREATE UNIQUE INDEX idx_external_bindings_kind_identity
    ON external_bindings(binding_kind, remote_identity);

CREATE INDEX idx_executions_task_id ON executions(task_id);
CREATE INDEX idx_executions_phase ON executions(phase);

CREATE INDEX idx_clarification_requests_task_id ON clarification_requests(task_id);
CREATE INDEX idx_clarification_requests_execution_id ON clarification_requests(execution_id);

CREATE INDEX idx_resolutions_request_id ON resolutions(request_id);

CREATE INDEX idx_events_task_id ON events(task_id);
CREATE INDEX idx_events_execution_id ON events(execution_id);
CREATE INDEX idx_events_kind ON events(event_kind);
