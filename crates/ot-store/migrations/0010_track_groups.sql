-- Groups a track manager forms from tracks (a battle group, a flight, a
-- convoy). A group is published as a track of its own, under its own UID;
-- membership is the track graph's MEMBER_OF edges to the group's node.
CREATE TABLE track_groups (
    uid             TEXT    PRIMARY KEY,
    -- {name, sidc, affiliation, domain, echelon, description, ...}
    spec            TEXT    NOT NULL CHECK (json_valid(spec)),
    created_at_ms   INTEGER NOT NULL,
    updated_at_ms   INTEGER NOT NULL,
    dissolved_at_ms INTEGER
);

-- An operator's GOLD PAIR between two system tracks: a new edge kind, so the
-- edge table is rebuilt with it allowed.
CREATE TABLE edges_new (
    id              INTEGER PRIMARY KEY,
    kind            TEXT    NOT NULL CHECK (kind IN
                        ('REPORTS_FOR', 'MERGED_INTO', 'CANDIDATE_OF', 'DO_NOT_PAIR',
                         'CARRIES', 'RESOLVES_TO', 'MEMBER_OF', 'PAIRED_WITH')),
    src             INTEGER NOT NULL REFERENCES nodes(id),
    dst             INTEGER NOT NULL REFERENCES nodes(id),
    attrs           TEXT    NOT NULL DEFAULT '{}' CHECK (json_valid(attrs)),
    valid_from_ms   INTEGER NOT NULL,
    valid_to_ms     INTEGER,
    decision_id     INTEGER NOT NULL REFERENCES decisions(id),
    ended_by        INTEGER REFERENCES decisions(id),
    CHECK (valid_to_ms IS NULL OR valid_to_ms >= valid_from_ms),
    CHECK ((valid_to_ms IS NULL) = (ended_by IS NULL))
);
INSERT INTO edges_new SELECT * FROM edges;
DROP TABLE edges;
ALTER TABLE edges_new RENAME TO edges;
CREATE INDEX edges_src_live ON edges(src, kind) WHERE valid_to_ms IS NULL;
CREATE INDEX edges_dst_live ON edges(dst, kind) WHERE valid_to_ms IS NULL;
CREATE INDEX edges_src_all  ON edges(src, kind, valid_from_ms);
CREATE INDEX edges_dst_all  ON edges(dst, kind, valid_from_ms);
CREATE UNIQUE INDEX edges_one_live_report ON edges(src)
    WHERE kind = 'REPORTS_FOR' AND valid_to_ms IS NULL;
CREATE UNIQUE INDEX edges_one_live_merge ON edges(src)
    WHERE kind = 'MERGED_INTO' AND valid_to_ms IS NULL;
