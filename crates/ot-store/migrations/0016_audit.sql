-- The audit record: an append-only, hash-chained log of every decision
-- (mirrored as it is recorded) and every sign-in event. Each row's hash is
-- SHA-256 over the previous row's hash and this row's content, so a row
-- changed, removed or inserted out of place breaks the chain
-- (GET /api/v1/audit/verify). The decisions table stays as it is: undo
-- still marks a decision undone in place; its audit copy never changes.
CREATE TABLE audit (
    seq         INTEGER PRIMARY KEY,         -- 1, 2, 3, ... never reused
    at_ms       INTEGER NOT NULL,
    actor       TEXT    NOT NULL,
    op          TEXT    NOT NULL,
    outcome     TEXT    NOT NULL CHECK (outcome IN ('success', 'failure')),
    ip          TEXT,
    detail      TEXT    NOT NULL CHECK (json_valid(detail)),
    decision_id INTEGER,
    prev_hash   TEXT    NOT NULL,
    hash        TEXT    NOT NULL
);
CREATE INDEX audit_at ON audit(at_ms);
CREATE INDEX audit_actor ON audit(actor, at_ms);
CREATE INDEX audit_op ON audit(op, at_ms);

-- Where the chain continues after a retention purge: the last purged
-- row's sequence number and hash (also written in the purge's own row).
CREATE TABLE audit_anchor (
    id   INTEGER PRIMARY KEY CHECK (id = 1),
    seq  INTEGER NOT NULL,
    hash TEXT    NOT NULL
);

-- Rows never change; they are deleted only by a retention purge, which
-- opens this gate for its own transaction.
CREATE TABLE audit_purge_gate (id INTEGER PRIMARY KEY CHECK (id = 1));
CREATE TRIGGER audit_no_update BEFORE UPDATE ON audit
BEGIN
    SELECT RAISE(ABORT, 'the audit record is append-only');
END;
CREATE TRIGGER audit_no_delete BEFORE DELETE ON audit
    WHEN NOT EXISTS (SELECT 1 FROM audit_purge_gate)
BEGIN
    SELECT RAISE(ABORT, 'the audit record is append-only');
END;
