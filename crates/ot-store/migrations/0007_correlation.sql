-- Correlation settings an operator can change at runtime (one row), and the
-- engine's suggestions: pairs of system tracks it believes are one object
-- (suggest mode), and source tracks it believes left their system track.
CREATE TABLE correlation_settings (
    id              INTEGER PRIMARY KEY CHECK (id = 1),
    settings        TEXT    NOT NULL CHECK (json_valid(settings)),
    updated_at_ms   INTEGER NOT NULL,
    decision_id     INTEGER REFERENCES decisions(id)
);

CREATE TABLE correlation_suggestions (
    id              INTEGER PRIMARY KEY,
    kind            TEXT    NOT NULL CHECK (kind IN ('pair', 'split')),
    -- pair: the two system tracks; split: the system track and the source
    -- track ('<source>/<key>') that should leave it.
    track_a         TEXT    NOT NULL,
    track_b         TEXT,
    source_track    TEXT,
    evidence        TEXT    NOT NULL DEFAULT '{}' CHECK (json_valid(evidence)),
    status          TEXT    NOT NULL DEFAULT 'open'
                            CHECK (status IN ('open', 'accepted', 'rejected', 'expired')),
    created_at_ms   INTEGER NOT NULL,
    updated_at_ms   INTEGER NOT NULL,
    decision_id     INTEGER REFERENCES decisions(id)
);
CREATE UNIQUE INDEX correlation_suggestions_open
    ON correlation_suggestions (kind, track_a, coalesce(track_b, ''), coalesce(source_track, ''))
    WHERE status = 'open';
CREATE INDEX correlation_suggestions_status ON correlation_suggestions (status, updated_at_ms);
