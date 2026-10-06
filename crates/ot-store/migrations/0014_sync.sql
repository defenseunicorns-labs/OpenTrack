-- Multi-node: the replicated log of track-management decisions, every
-- node's, under global ids (<site>:<seq>) and hybrid logical clock stamps.
-- A node's own entries are written when its decision is made; other nodes'
-- as they arrive, then applied here (or kept pending until the tracks they
-- name arrive, or superseded by a later decision on the same tracks).
CREATE TABLE sync_log (
    site            TEXT    NOT NULL,
    seq             INTEGER NOT NULL CHECK (seq > 0),
    hlc             INTEGER NOT NULL,
    actor           TEXT    NOT NULL,
    role            TEXT    NOT NULL,
    command         TEXT    NOT NULL CHECK (json_valid(command)),
    received_at_ms  INTEGER NOT NULL,
    status          TEXT    NOT NULL CHECK (status IN
                        ('applied', 'pending', 'superseded', 'failed')),
    detail          TEXT,
    PRIMARY KEY (site, seq)
);
CREATE INDEX sync_log_hlc ON sync_log (hlc);
CREATE INDEX sync_log_pending ON sync_log (status) WHERE status = 'pending';

-- The local decisions an entry made: evidence.sync is its global id, and
-- evidence.sync_part the track, when one entry made several decisions.
ALTER TABLE decisions ADD COLUMN sync_id TEXT
    GENERATED ALWAYS AS (json_extract(evidence, '$.sync')) VIRTUAL;
CREATE INDEX decisions_sync ON decisions (sync_id) WHERE sync_id IS NOT NULL;
