-- PostgreSQL control-store schema for fresh OpenTrack installations.
-- This is the logical schema at SQLite version 20. PostgreSQL migration
-- versions are separate from SQLite's PRAGMA user_version.

CREATE FUNCTION json_valid(input TEXT) RETURNS BOOLEAN LANGUAGE plpgsql IMMUTABLE AS $$
BEGIN
    PERFORM input::jsonb;
    RETURN TRUE;
EXCEPTION WHEN invalid_text_representation THEN
    RETURN FALSE;
END $$;

CREATE FUNCTION json_extract(input TEXT, path TEXT) RETURNS TEXT LANGUAGE sql IMMUTABLE AS $$
    SELECT input::jsonb #>> string_to_array(trim(leading '$.' from path), '.')
$$;

CREATE TABLE decisions (
    id          BIGSERIAL PRIMARY KEY,
    at_ms       BIGINT NOT NULL,
    actor       TEXT    NOT NULL,            -- 'engine', 'system' or an operator id
    op          TEXT    NOT NULL,            -- create_system_track, pair, merge, ...
    reason      TEXT,
    evidence    TEXT    NOT NULL DEFAULT '{}' CHECK (json_valid(evidence)),
    before      TEXT             CHECK (before IS NULL OR json_valid(before)),
    after       TEXT             CHECK (after  IS NULL OR json_valid(after)),
    undoes      BIGINT REFERENCES decisions(id),
    undone_by   BIGINT REFERENCES decisions(id),
    sync_id     TEXT GENERATED ALWAYS AS (json_extract(evidence, '$.sync')) STORED
);

CREATE INDEX decisions_at ON decisions(at_ms);

CREATE INDEX decisions_op ON decisions(op, at_ms);

CREATE TABLE sources (
    id              TEXT    PRIMARY KEY,
    name            TEXT    NOT NULL,
    transport       TEXT    NOT NULL,        -- tcp, udp, http_poll, websocket, mqtt, ...
    framing         TEXT,
    codec           TEXT    NOT NULL,        -- json, cot_xml, protobuf, plugin:<name>
    settings        TEXT    NOT NULL DEFAULT '{}' CHECK (json_valid(settings)),
    enabled         BIGINT NOT NULL DEFAULT 0 CHECK (enabled IN (0, 1)),
    priority        BIGINT NOT NULL DEFAULT 100,
    raw_subject     TEXT,
    raw_consent     BIGINT REFERENCES decisions(id),
    revision        BIGINT NOT NULL DEFAULT 1,
    created_at_ms   BIGINT NOT NULL,
    updated_at_ms   BIGINT NOT NULL,
    problems        TEXT NOT NULL DEFAULT '[]',
    CHECK (raw_subject IS NULL OR raw_consent IS NOT NULL)
);

CREATE TABLE source_revisions (
    source_id       TEXT    NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    revision        BIGINT NOT NULL,
    config          TEXT    NOT NULL CHECK (json_valid(config)),
    decision_id     BIGINT REFERENCES decisions(id),
    created_at_ms   BIGINT NOT NULL,
    PRIMARY KEY (source_id, revision)
);

CREATE TABLE probe_samples (
    id              BIGSERIAL PRIMARY KEY,
    source_id       TEXT    NOT NULL,
    captured_at_ms  BIGINT NOT NULL,
    frame           BYTEA    NOT NULL,
    decoded         TEXT CHECK (decoded IS NULL OR json_valid(decoded)),
    meta            TEXT CHECK (meta IS NULL OR json_valid(meta))
);

CREATE INDEX probe_samples_source ON probe_samples(source_id, captured_at_ms);

CREATE TABLE schema_versions (
    version         BIGINT PRIMARY KEY,
    status          TEXT    NOT NULL CHECK (status IN ('draft', 'published')),
    published_at_ms BIGINT,
    notes           TEXT
);

CREATE TABLE extension_fields (
    schema_version  BIGINT NOT NULL REFERENCES schema_versions(version),
    key             TEXT    NOT NULL,
    type            TEXT    NOT NULL CHECK (type IN
                        ('string', 'integer', 'number', 'boolean', 'enum', 'timestamp', 'position', 'json')),
    unit            TEXT,
    required        BIGINT NOT NULL DEFAULT 0 CHECK (required IN (0, 1)),
    default_json    TEXT             CHECK (default_json IS NULL OR json_valid(default_json)),
    enum_values     TEXT             CHECK (enum_values IS NULL OR json_valid(enum_values)),
    description     TEXT,
    builtin         TEXT,
    PRIMARY KEY (schema_version, key)
);

CREATE TABLE mappings (
    source_id       TEXT    NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    schema_version  BIGINT NOT NULL,
    spec            TEXT    NOT NULL CHECK (json_valid(spec)),
    updated_at_ms   BIGINT NOT NULL,
    PRIMARY KEY (source_id, schema_version)
);

CREATE TABLE uid_sequences (
    site            TEXT    PRIMARY KEY CHECK (length(site) = 3),
    next_sequence   BIGINT NOT NULL CHECK (next_sequence >= 1)
);

CREATE TABLE nodes (
    id              BIGSERIAL PRIMARY KEY,
    kind            TEXT    NOT NULL CHECK (kind IN
                        ('source_track', 'system_track', 'identifier', 'entity', 'group')),
    key             TEXT    NOT NULL,        -- '<source>/<track key>', UID, 'scheme:value', ...
    attrs           TEXT    NOT NULL DEFAULT '{}' CHECK (json_valid(attrs)),
    created_at_ms   BIGINT NOT NULL,
    retired_at_ms   BIGINT,
    UNIQUE (kind, key)
);

CREATE TABLE registry_entities (
    id              TEXT    PRIMARY KEY,
    name            TEXT,
    status          TEXT    NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'retired')),
    -- Everything else (cot, flag, hull_code, ship_class, imo, note...).
    source          TEXT,
    created_at_ms   BIGINT NOT NULL,
    updated_at_ms   BIGINT NOT NULL,
    class_name      TEXT,
    domain          TEXT,
    affiliation     TEXT,
    track_type      TEXT,
    cot_type        TEXT,
    sidc            TEXT,
    attributes      TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(attributes)),
    publish         TEXT CHECK (publish IS NULL OR publish IN ('always', 'never'))
);

CREATE TABLE registry_identifiers (
    scheme          TEXT    NOT NULL,
    value           TEXT    NOT NULL,
    entity_id       TEXT    NOT NULL REFERENCES registry_entities(id) ON DELETE CASCADE,
    -- Name the platform is expected to broadcast under this identifier.
    expected_name   TEXT,
    source          TEXT,
    added_at_ms     BIGINT NOT NULL,
    PRIMARY KEY (scheme, value)
);

CREATE INDEX registry_identifiers_entity ON registry_identifiers(entity_id);

CREATE TABLE correlation_settings (
    id              BIGINT PRIMARY KEY CHECK (id = 1),
    settings        TEXT    NOT NULL CHECK (json_valid(settings)),
    updated_at_ms   BIGINT NOT NULL,
    decision_id     BIGINT REFERENCES decisions(id)
);

CREATE TABLE correlation_suggestions (
    id              BIGSERIAL PRIMARY KEY,
    kind            TEXT    NOT NULL CHECK (kind IN ('pair', 'split')),
    -- pair: the two system tracks; split: the system track and the source
    -- track ('<source>/<key>') that should leave it.
    track_a         TEXT    NOT NULL,
    track_b         TEXT,
    source_track    TEXT,
    evidence        TEXT    NOT NULL DEFAULT '{}' CHECK (json_valid(evidence)),
    status          TEXT    NOT NULL DEFAULT 'open'
                            CHECK (status IN ('open', 'accepted', 'rejected', 'expired')),
    created_at_ms   BIGINT NOT NULL,
    updated_at_ms   BIGINT NOT NULL,
    decision_id     BIGINT REFERENCES decisions(id)
);

CREATE UNIQUE INDEX correlation_suggestions_open
    ON correlation_suggestions (kind, track_a, coalesce(track_b, ''), coalesce(source_track, ''))
    WHERE status = 'open';

CREATE INDEX correlation_suggestions_status ON correlation_suggestions (status, updated_at_ms);

CREATE TABLE app_settings (
    id              BIGINT PRIMARY KEY CHECK (id = 1),
    settings        TEXT    NOT NULL CHECK (json_valid(settings)),
    updated_at_ms   BIGINT NOT NULL,
    decision_id     BIGINT REFERENCES decisions(id)
);

CREATE TABLE entity_revisions (
    id              BIGSERIAL PRIMARY KEY,
    entity_id       TEXT    NOT NULL,
    entity          TEXT    NOT NULL CHECK (json_valid(entity)),
    saved_at_ms     BIGINT NOT NULL,
    decision_id     BIGINT REFERENCES decisions(id)
);

CREATE INDEX entity_revisions_entity ON entity_revisions(entity_id, id);

CREATE TABLE track_groups (
    uid             TEXT    PRIMARY KEY,
    -- {name, sidc, affiliation, domain, echelon, description, ...}
    spec            TEXT    NOT NULL CHECK (json_valid(spec)),
    created_at_ms   BIGINT NOT NULL,
    updated_at_ms   BIGINT NOT NULL,
    dissolved_at_ms BIGINT
);

CREATE TABLE "edges" (
    id              BIGSERIAL PRIMARY KEY,
    kind            TEXT    NOT NULL CHECK (kind IN
                        ('REPORTS_FOR', 'MERGED_INTO', 'CANDIDATE_OF', 'DO_NOT_PAIR',
                         'CARRIES', 'RESOLVES_TO', 'MEMBER_OF', 'PAIRED_WITH')),
    src             BIGINT NOT NULL REFERENCES nodes(id),
    dst             BIGINT NOT NULL REFERENCES nodes(id),
    attrs           TEXT    NOT NULL DEFAULT '{}' CHECK (json_valid(attrs)),
    valid_from_ms   BIGINT NOT NULL,
    valid_to_ms     BIGINT,
    decision_id     BIGINT NOT NULL REFERENCES decisions(id),
    ended_by        BIGINT REFERENCES decisions(id),
    CHECK (valid_to_ms IS NULL OR valid_to_ms >= valid_from_ms),
    CHECK ((valid_to_ms IS NULL) = (ended_by IS NULL))
);

CREATE INDEX edges_src_live ON edges(src, kind) WHERE valid_to_ms IS NULL;

CREATE INDEX edges_dst_live ON edges(dst, kind) WHERE valid_to_ms IS NULL;

CREATE INDEX edges_src_all  ON edges(src, kind, valid_from_ms);

CREATE INDEX edges_dst_all  ON edges(dst, kind, valid_from_ms);

CREATE UNIQUE INDEX edges_one_live_report ON edges(src)
    WHERE kind = 'REPORTS_FOR' AND valid_to_ms IS NULL;

CREATE UNIQUE INDEX edges_one_live_merge ON edges(src)
    WHERE kind = 'MERGED_INTO' AND valid_to_ms IS NULL;

CREATE TABLE plugins (
    name            TEXT PRIMARY KEY,
    runtime         TEXT NOT NULL CHECK (runtime IN ('wasm', 'external')),
    version         TEXT NOT NULL,
    manifest        TEXT NOT NULL,
    -- The component (wasm), and its SHA-256.
    wasm            BYTEA,
    sha256          TEXT,
    -- host:port, tcp://host:port or unix:/path (external).
    address         TEXT,
    grants          TEXT NOT NULL DEFAULT '{}',
    enabled         BIGINT NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    created_at_ms   BIGINT NOT NULL,
    updated_at_ms   BIGINT NOT NULL,
    secret          TEXT
);

CREATE TABLE users (
    id               TEXT    PRIMARY KEY,                  -- UUID
    email            TEXT    NOT NULL UNIQUE,
    name             TEXT    NOT NULL DEFAULT '',
    role             TEXT    NOT NULL CHECK (role IN ('viewer', 'track_manager', 'admin')),
    active           BIGINT NOT NULL DEFAULT 1,
    password_hash    TEXT,                                 -- Argon2id (PHC string); NULL: no password sign-in
    origin           TEXT    NOT NULL DEFAULT 'local' CHECK (origin IN ('local', 'saml')),
    created_at_ms    BIGINT NOT NULL,
    updated_at_ms    BIGINT NOT NULL,
    last_login_at_ms BIGINT,
    -- Tokens issued before this are no longer accepted ("sign out everywhere").
    tokens_valid_from_ms BIGINT NOT NULL DEFAULT 0,
    password_changed_at_ms BIGINT,
    must_change_password BIGINT NOT NULL DEFAULT 0,
    failed_logins BIGINT NOT NULL DEFAULT 0,
    failed_window_from_ms BIGINT,
    failed_since_login BIGINT NOT NULL DEFAULT 0,
    locked_until_ms BIGINT,
    active_since_ms BIGINT,
    disabled_reason TEXT,
    expires_at_ms BIGINT
);

CREATE TABLE revoked_tokens (
    jti           TEXT    PRIMARY KEY,
    expires_at_ms BIGINT NOT NULL
);

CREATE TABLE api_tokens (
    jti           TEXT    PRIMARY KEY,
    name          TEXT    NOT NULL,
    user_id       TEXT    NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_by    TEXT    NOT NULL,
    created_at_ms BIGINT NOT NULL,
    expires_at_ms BIGINT NOT NULL,
    revoked_at_ms BIGINT
);

CREATE TABLE auth_settings (
    id            BIGINT PRIMARY KEY CHECK (id = 1),
    settings      TEXT    NOT NULL CHECK (json_valid(settings)),
    updated_at_ms BIGINT NOT NULL
);

CREATE TABLE sync_log (
    site            TEXT    NOT NULL,
    seq             BIGINT NOT NULL CHECK (seq > 0),
    hlc             BIGINT NOT NULL,
    actor           TEXT    NOT NULL,
    role            TEXT    NOT NULL,
    command         TEXT    NOT NULL CHECK (json_valid(command)),
    received_at_ms  BIGINT NOT NULL,
    status          TEXT    NOT NULL CHECK (status IN
                        ('applied', 'pending', 'superseded', 'failed')),
    detail          TEXT,
    PRIMARY KEY (site, seq)
);

CREATE INDEX sync_log_hlc ON sync_log (hlc);

CREATE INDEX sync_log_pending ON sync_log (status) WHERE status = 'pending';

CREATE INDEX decisions_sync ON decisions (sync_id) WHERE sync_id IS NOT NULL;

CREATE TABLE password_history (
    user_id   TEXT    NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    hash      TEXT    NOT NULL,
    set_at_ms BIGINT NOT NULL
);

CREATE INDEX password_history_user ON password_history(user_id, set_at_ms);

CREATE TABLE sessions (
    id               TEXT    PRIMARY KEY,                 -- the token's jti
    user_id          TEXT    NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at_ms    BIGINT NOT NULL,
    last_seen_ms     BIGINT NOT NULL,
    expires_at_ms    BIGINT NOT NULL,
    ip               TEXT,
    user_agent       TEXT,
    ended_at_ms      BIGINT,
    -- sign_out, idle, expired, limit, revoked, password, disabled, ended
    end_reason       TEXT,
    -- The account's previous good sign-in and the failed ones since, as
    -- they stood when this session began (the notice after signing in).
    prev_login_at_ms BIGINT,
    failed_before    BIGINT NOT NULL DEFAULT 0
);

CREATE INDEX sessions_user ON sessions(user_id, ended_at_ms);

CREATE INDEX sessions_live ON sessions(ended_at_ms) WHERE ended_at_ms IS NULL;

CREATE TABLE audit (
    seq         BIGINT PRIMARY KEY,         -- 1, 2, 3, ... never reused
    at_ms       BIGINT NOT NULL,
    actor       TEXT    NOT NULL,
    op          TEXT    NOT NULL,
    outcome     TEXT    NOT NULL CHECK (outcome IN ('success', 'failure')),
    ip          TEXT,
    detail      TEXT    NOT NULL CHECK (json_valid(detail)),
    decision_id BIGINT,
    prev_hash   TEXT    NOT NULL,
    hash        TEXT    NOT NULL
);

CREATE INDEX audit_at ON audit(at_ms);

CREATE INDEX audit_actor ON audit(actor, at_ms);

CREATE INDEX audit_op ON audit(op, at_ms);

CREATE TABLE audit_anchor (
    id   BIGINT PRIMARY KEY CHECK (id = 1),
    seq  BIGINT NOT NULL,
    hash TEXT    NOT NULL
);

CREATE TABLE audit_purge_gate (id BIGINT PRIMARY KEY CHECK (id = 1));

CREATE TABLE sync_peer_keys (
    site         TEXT PRIMARY KEY,
    public_key   TEXT NOT NULL,
    pinned_by    TEXT NOT NULL,
    pinned_at_ms BIGINT NOT NULL
);

-- The published core schema is present before an administrator adds fields.
INSERT INTO schema_versions (version, status, published_at_ms, notes)
VALUES (1, 'published', 0, 'Core schema, no extension fields');

-- SQLite uses COLLATE NOCASE for email uniqueness. Preserve that rule without
-- requiring a PostgreSQL extension in an application-owned database.
CREATE UNIQUE INDEX users_email_nocase ON users (lower(email));

-- The application opens this gate only within its audit-retention transaction.
CREATE FUNCTION audit_guard_update() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'the audit record is append-only';
END $$;
CREATE TRIGGER audit_no_update BEFORE UPDATE ON audit
FOR EACH ROW EXECUTE FUNCTION audit_guard_update();
-- TRUNCATE does not fire row-level DELETE triggers in PostgreSQL.
CREATE TRIGGER audit_no_truncate BEFORE TRUNCATE ON audit
FOR EACH STATEMENT EXECUTE FUNCTION audit_guard_update();

CREATE FUNCTION audit_guard_delete() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM audit_purge_gate) THEN
        RAISE EXCEPTION 'the audit record is append-only';
    END IF;
    RETURN OLD;
END $$;
CREATE TRIGGER audit_no_delete BEFORE DELETE ON audit
FOR EACH ROW EXECUTE FUNCTION audit_guard_delete();

CREATE TABLE control_schema_version (version BIGINT PRIMARY KEY);
INSERT INTO control_schema_version (version) VALUES (1);
