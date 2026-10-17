-- Add runtime_metadata_json to executions table for historical runtime configuration tracking
ALTER TABLE executions ADD COLUMN runtime_metadata_json TEXT;
