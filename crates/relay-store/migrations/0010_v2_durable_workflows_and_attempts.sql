-- Migration 0010: Relay Architecture v2 - Wave B durable workflows and attempts (R12-02)
--
-- Persists durable workflow runs, role instances, and role attempts for crash-safe
-- v2 orchestration, restart reconciliation, and execution tracking.

CREATE TABLE workflow_runs (
    id TEXT PRIMARY KEY NOT NULL,
    task_id TEXT NOT NULL,
    execution_grant_id TEXT NOT NULL,
    workflow_definition_id TEXT NOT NULL,
    state TEXT NOT NULL,
    config_snapshot_id TEXT NOT NULL,
    started_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    completed_at TEXT,
    CHECK (state IN ('pending', 'running', 'waiting_decision', 'blocked', 'completed', 'failed', 'cancelled')),
    FOREIGN KEY (task_id) REFERENCES tasks(id) ON DELETE RESTRICT
);

CREATE INDEX idx_workflow_runs_task_id ON workflow_runs(task_id);
CREATE INDEX idx_workflow_runs_state ON workflow_runs(state);

CREATE TABLE role_instances (
    id TEXT PRIMARY KEY NOT NULL,
    workflow_run_id TEXT NOT NULL,
    role_definition_id TEXT NOT NULL,
    generation INTEGER NOT NULL,
    state TEXT NOT NULL,
    parent_role_instance_id TEXT,
    accepted_result_artifact TEXT,
    attempts_count INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (state IN (
        'created', 'waiting_inputs', 'ready', 'active', 'waiting_children',
        'result_pending', 'completed', 'blocked', 'failed', 'cancelled'
    )),
    FOREIGN KEY (workflow_run_id) REFERENCES workflow_runs(id) ON DELETE RESTRICT
);

CREATE INDEX idx_role_instances_workflow_run_id ON role_instances(workflow_run_id);
CREATE INDEX idx_role_instances_state ON role_instances(state);

CREATE TABLE role_attempts (
    id TEXT PRIMARY KEY NOT NULL,
    role_instance_id TEXT NOT NULL,
    attempt_no INTEGER NOT NULL,
    execution_target_json TEXT NOT NULL,
    state TEXT NOT NULL,
    lease_id TEXT,
    fencing_token INTEGER,
    runtime_outcome TEXT,
    result_artifact_id TEXT,
    started_at TEXT,
    ended_at TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (state IN (
        'queued', 'provisioning', 'starting', 'running', 'result_pending',
        'completed', 'interrupted', 'failed', 'cancelled', 'lost'
    )),
    FOREIGN KEY (role_instance_id) REFERENCES role_instances(id) ON DELETE RESTRICT
);

CREATE INDEX idx_role_attempts_role_instance_id ON role_attempts(role_instance_id);
CREATE INDEX idx_role_attempts_state ON role_attempts(state);
