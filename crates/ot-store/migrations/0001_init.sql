-- OpenTrack SQLite schema, version 1.
-- All times are integer milliseconds since the Unix epoch (UTC).

-- The audit log. Every operator or engine decision, with its evidence and
-- before/after state. Graph edges point here, which is what makes every
-- link explainable and every operation undoable.
CREATE TABLE decisions (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    at_ms       INTEGER NOT NULL,
    actor       TEXT    NOT NULL,            -- 'engine', 'system' or an operator id
    op          TEXT    NOT NULL,            -- create_system_track, pair, merge, ...
    reason      TEXT,
    evidence    TEXT    NOT NULL DEFAULT '{}' CHECK (json_valid(evidence)),
    before      TEXT             CHECK (before IS NULL OR json_valid(before)),
    after       TEXT             CHECK (after  IS NULL OR json_valid(after)),
    undoes      INTEGER REFERENCES decisions(id),
    undone_by   INTEGER REFERENCES decisions(id)
);
CREATE INDEX decisions_at ON decisions(at_ms);
CREATE INDEX decisions_op ON decisions(op, at_ms);

-- Sources: transport + codec + mapping, one row per configured feed.
CREATE TABLE sources (
    id              TEXT    PRIMARY KEY,
    name            TEXT    NOT NULL,
    transport       TEXT    NOT NULL,        -- tcp, udp, http_poll, websocket, mqtt, ...
    framing         TEXT,
    codec           TEXT    NOT NULL,        -- json, cot_xml, protobuf, plugin:<name>
    settings        TEXT    NOT NULL DEFAULT '{}' CHECK (json_valid(settings)),
    enabled         INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0, 1)),
    priority        INTEGER NOT NULL DEFAULT 100,
    raw_collection  TEXT,                    -- opt-in raw output destination (renamed raw_subject in 0004)
    raw_consent     INTEGER REFERENCES decisions(id),
    revision        INTEGER NOT NULL DEFAULT 1,
    created_at_ms   INTEGER NOT NULL,
    updated_at_ms   INTEGER NOT NULL,
    CHECK (raw_collection IS NULL OR raw_consent IS NOT NULL)
);

CREATE TABLE source_revisions (
    source_id       TEXT    NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    revision        INTEGER NOT NULL,
    config          TEXT    NOT NULL CHECK (json_valid(config)),
    decision_id     INTEGER REFERENCES decisions(id),
    created_at_ms   INTEGER NOT NULL,
    PRIMARY KEY (source_id, revision)
);

CREATE TABLE plugins (
    name            TEXT    NOT NULL,
    version         TEXT    NOT NULL,
    kind            TEXT    NOT NULL CHECK (kind IN ('builtin', 'wasm')),
    sha256          TEXT,
    installed_at_ms INTEGER NOT NULL,
    PRIMARY KEY (name, version)
);

-- Captured frames per source, for mapping preview and plugin conformance.
CREATE TABLE probe_samples (
    id              INTEGER PRIMARY KEY,
    source_id       TEXT    NOT NULL,
    captured_at_ms  INTEGER NOT NULL,
    frame           BLOB    NOT NULL,
    decoded         TEXT             CHECK (decoded IS NULL OR json_valid(decoded))
);
CREATE INDEX probe_samples_source ON probe_samples(source_id, captured_at_ms);

-- Admin-defined extension schema. A published version is immutable.
CREATE TABLE schema_versions (
    version         INTEGER PRIMARY KEY,
    status          TEXT    NOT NULL CHECK (status IN ('draft', 'published')),
    published_at_ms INTEGER,
    notes           TEXT
);

CREATE TABLE extension_fields (
    schema_version  INTEGER NOT NULL REFERENCES schema_versions(version),
    key             TEXT    NOT NULL,
    type            TEXT    NOT NULL CHECK (type IN
                        ('string', 'integer', 'number', 'boolean', 'enum', 'timestamp', 'position', 'json')),
    unit            TEXT,
    required        INTEGER NOT NULL DEFAULT 0 CHECK (required IN (0, 1)),
    default_json    TEXT             CHECK (default_json IS NULL OR json_valid(default_json)),
    enum_values     TEXT             CHECK (enum_values IS NULL OR json_valid(enum_values)),
    description     TEXT,
    PRIMARY KEY (schema_version, key)
);

CREATE TABLE mappings (
    source_id       TEXT    NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    schema_version  INTEGER NOT NULL,
    spec            TEXT    NOT NULL CHECK (json_valid(spec)),
    updated_at_ms   INTEGER NOT NULL,
    PRIMARY KEY (source_id, schema_version)
);

-- System track UID allocation, one counter per GOLD site code.
CREATE TABLE uid_sequences (
    site            TEXT    PRIMARY KEY CHECK (length(site) = 3),
    next_sequence   INTEGER NOT NULL CHECK (next_sequence >= 1)
);

-- The temporal track graph (OpenTrack's replacement for BGDBM).
-- Nodes: source tracks, system tracks, identifiers, registry entities, groups.
CREATE TABLE nodes (
    id              INTEGER PRIMARY KEY,
    kind            TEXT    NOT NULL CHECK (kind IN
                        ('source_track', 'system_track', 'identifier', 'entity', 'group')),
    key             TEXT    NOT NULL,        -- '<source>/<track key>', UID, 'scheme:value', ...
    attrs           TEXT    NOT NULL DEFAULT '{}' CHECK (json_valid(attrs)),
    created_at_ms   INTEGER NOT NULL,
    retired_at_ms   INTEGER,
    UNIQUE (kind, key)
);

-- Edges carry a validity interval and the decisions that opened and closed
-- them, so the picture at any instant can be rebuilt and every link explained.
CREATE TABLE edges (
    id              INTEGER PRIMARY KEY,
    kind            TEXT    NOT NULL CHECK (kind IN
                        ('REPORTS_FOR', 'MERGED_INTO', 'CANDIDATE_OF', 'DO_NOT_PAIR',
                         'CARRIES', 'RESOLVES_TO', 'MEMBER_OF')),
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
CREATE INDEX edges_src_live ON edges(src, kind) WHERE valid_to_ms IS NULL;
CREATE INDEX edges_dst_live ON edges(dst, kind) WHERE valid_to_ms IS NULL;
CREATE INDEX edges_src_all  ON edges(src, kind, valid_from_ms);
CREATE INDEX edges_dst_all  ON edges(dst, kind, valid_from_ms);
-- A source track reports for at most one system track at a time, and a
-- system track is merged into at most one destination.
CREATE UNIQUE INDEX edges_one_live_report ON edges(src)
    WHERE kind = 'REPORTS_FOR' AND valid_to_ms IS NULL;
CREATE UNIQUE INDEX edges_one_live_merge ON edges(src)
    WHERE kind = 'MERGED_INTO' AND valid_to_ms IS NULL;
