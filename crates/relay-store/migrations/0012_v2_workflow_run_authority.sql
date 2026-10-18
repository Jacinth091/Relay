-- Migration 0012: Relay Architecture v2 - Wave B workflow run authority (R12-02)
--
-- A Run approval is one durable fact: the exact ApprovedIntent the human
-- approved, the ExecutionGrant derived from it, and the ConfigSnapshot that
-- froze the execution budget. Storing the authority on the run row makes the
-- approved scope readable after a restart instead of being an in-memory
-- artifact of the command that approved it, and lets the start of a run be a
-- single atomic transaction (run + task transition + start event).
--
-- Existing rows predate authority capture; an empty string means "no stored
-- authority", never "approved with empty intent".

ALTER TABLE workflow_runs ADD COLUMN approved_intent_json TEXT NOT NULL DEFAULT '';
ALTER TABLE workflow_runs ADD COLUMN execution_grant_json TEXT NOT NULL DEFAULT '';
ALTER TABLE workflow_runs ADD COLUMN config_snapshot_json TEXT NOT NULL DEFAULT '';
