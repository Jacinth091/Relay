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
    CHECK (submission_transport IN ('local-file', 'local-paste')),
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

CREATE INDEX idx_tasks_project_key ON tasks(project_key);
CREATE INDEX idx_tasks_state ON tasks(state);
CREATE INDEX idx_tasks_created_at ON tasks(created_at);

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

CREATE INDEX idx_external_bindings_task_id ON external_bindings(task_id);
CREATE UNIQUE INDEX idx_external_bindings_kind_identity
    ON external_bindings(binding_kind, remote_identity);

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
    FOREIGN KEY (task_id) REFERENCES tasks(id) ON DELETE RESTRICT
);

CREATE INDEX idx_executions_task_id ON executions(task_id);
CREATE INDEX idx_executions_phase ON executions(phase);

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

CREATE INDEX idx_clarification_requests_task_id ON clarification_requests(task_id);
CREATE INDEX idx_clarification_requests_execution_id ON clarification_requests(execution_id);

CREATE TABLE resolutions (
    id TEXT PRIMARY KEY NOT NULL,
    request_id TEXT NOT NULL,
    resolution_json TEXT NOT NULL,
    work_disposition TEXT NOT NULL,
    resume_outcome TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    FOREIGN KEY (request_id) REFERENCES clarification_requests(id) ON DELETE RESTRICT
);

CREATE INDEX idx_resolutions_request_id ON resolutions(request_id);

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

CREATE INDEX idx_events_task_id ON events(task_id);
CREATE INDEX idx_events_execution_id ON events(execution_id);
CREATE INDEX idx_events_kind ON events(event_kind);
