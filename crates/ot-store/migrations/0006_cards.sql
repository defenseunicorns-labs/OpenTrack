-- The output schema can link a field to an OpenTrack built-in value.
ALTER TABLE extension_fields ADD COLUMN builtin TEXT;

-- Baseball cards: admin-entered values for an entity's output schema fields.
-- The card is the authority for its fields; every save is a revision.
CREATE TABLE entity_cards (
    entity_id       TEXT    PRIMARY KEY REFERENCES registry_entities(id) ON DELETE CASCADE,
    card_values     TEXT    NOT NULL DEFAULT '{}' CHECK (json_valid(card_values)),
    schema_version  INTEGER NOT NULL,
    updated_at_ms   INTEGER NOT NULL,
    decision_id     INTEGER REFERENCES decisions(id)
);

CREATE TABLE entity_card_revisions (
    id              INTEGER PRIMARY KEY,
    entity_id       TEXT    NOT NULL,
    card_values     TEXT    NOT NULL CHECK (json_valid(card_values)),
    schema_version  INTEGER NOT NULL,
    saved_at_ms     INTEGER NOT NULL,
    decision_id     INTEGER REFERENCES decisions(id)
);
CREATE INDEX entity_card_revisions_entity ON entity_card_revisions(entity_id, id);
