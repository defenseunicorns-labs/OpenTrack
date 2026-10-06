-- The identity registry (moved from data-services' Redis reg:*).
-- An entity is a real-world platform; identifiers (scheme + value, e.g.
-- mmsi:338924210) resolve to it. Changes are recorded in `decisions`.

CREATE TABLE registry_entities (
    id              TEXT    PRIMARY KEY,
    name            TEXT,
    status          TEXT    NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'retired')),
    -- Everything else (cot, flag, hull_code, ship_class, imo, note...).
    fields          TEXT    NOT NULL DEFAULT '{}' CHECK (json_valid(fields)),
    source          TEXT,
    created_at_ms   INTEGER NOT NULL,
    updated_at_ms   INTEGER NOT NULL
);

CREATE TABLE registry_identifiers (
    scheme          TEXT    NOT NULL,
    value           TEXT    NOT NULL,
    entity_id       TEXT    NOT NULL REFERENCES registry_entities(id) ON DELETE CASCADE,
    -- Name the platform is expected to broadcast under this identifier.
    expected_name   TEXT,
    source          TEXT,
    added_at_ms     INTEGER NOT NULL,
    PRIMARY KEY (scheme, value)
);
CREATE INDEX registry_identifiers_entity ON registry_identifiers(entity_id);
