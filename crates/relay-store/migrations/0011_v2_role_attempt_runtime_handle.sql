-- Migration 0011: Relay Architecture v2 - Wave B role attempt runtime handle (R12-02)
--
-- Records which backplane handle and which backplane a role attempt was
-- launched as. Without this, a daemon that restarts mid-run cannot ask the
-- backplane about the process it actually launched, so restart reconciliation
-- could only guess at handle identity.

ALTER TABLE role_attempts ADD COLUMN runtime_handle_id TEXT;
ALTER TABLE role_attempts ADD COLUMN runtime_backplane_id TEXT;
