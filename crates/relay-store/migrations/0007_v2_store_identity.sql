-- Migration 0007: Relay Architecture v2 - durable store identity (PR #16 review R16-02)
--
-- Binds a daemon client to the exact durable state it was opened against. The
-- identity is generated once, inside the database, so two databases never share
-- it and a deleted/recreated database gets a fresh one. Daemon and client
-- compare it during the transport handshake and fail closed on mismatch, so a
-- reachable pipe is never treated as proof of matching durable-state identity.

CREATE TABLE store_identity (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    identity TEXT NOT NULL
);

INSERT INTO store_identity (id, identity)
VALUES (1, lower(hex(randomblob(16))));
