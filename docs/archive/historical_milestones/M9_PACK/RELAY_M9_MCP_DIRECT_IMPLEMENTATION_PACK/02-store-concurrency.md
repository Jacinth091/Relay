# M9 Phase 02 — Store Concurrency Hardening

## Goal

Two Relay processes (TUI + `serve-mcp`) open the same SQLite database concurrently without corruption, lost updates, or spurious failures.

## Current state

- `crates/relay-store` opens the database assuming a single owner.
- WAL mode, busy timeout, and retry-on-locked behavior are unverified.

## Requirements

1. Every connection sets:
   - journal mode WAL;
   - `busy_timeout` (recommend 5000 ms, configurable later);
   - foreign keys ON if the schema uses them.
2. Transient `SQLITE_BUSY`/`SQLITE_LOCKED` on write paths is retried with bounded backoff before surfacing as `TaskPersistenceError`.
3. Integration tests (`relay-store`, tokio):
   - two store instances against one temp file;
   - interleaved task inserts from both; all tasks readable from both afterward;
   - concurrent write to the same task row: one wins deterministically, no corruption, loser gets a typed error — never a hang;
   - reader active while writer commits (TUI listing during MCP submit).
4. Document in `docs/data-persistence.md`: multi-process access is now supported for exactly two writer classes (TUI, serve-mcp); anything more is out of scope until re-examined.

## Non-goals

- Networked/shared databases.
- Migration-under-live-reader automation beyond "second process must not crash on schema version mismatch" — detect and fail closed with a clear error.

## Done when

- The concurrency test suite passes reliably (run it several times locally).
- Full workspace gates pass.
- Phase 03 may begin only after this phase is merged.
