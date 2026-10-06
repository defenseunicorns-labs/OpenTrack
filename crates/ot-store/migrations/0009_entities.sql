-- One Entity model: the registry entity, its registry fields and its card
-- become one record. An entity has identifiers (one to many), a status, the
-- OTH-GOLD minimum a track publishes (name, class name, domain, affiliation,
-- track type, symbol) and free-form typed attributes.

ALTER TABLE registry_entities ADD COLUMN class_name  TEXT;
ALTER TABLE registry_entities ADD COLUMN domain      TEXT;
ALTER TABLE registry_entities ADD COLUMN affiliation TEXT;
ALTER TABLE registry_entities ADD COLUMN track_type  TEXT;
ALTER TABLE registry_entities ADD COLUMN cot_type    TEXT;
ALTER TABLE registry_entities ADD COLUMN sidc        TEXT;
-- [{"key": "flag", "type": "text", "value": "USA"}, ...], in the admin's order.
ALTER TABLE registry_entities ADD COLUMN attributes  TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(attributes));

-- Registry fields `cot` and `ship_class` are the symbol and class name; every
-- other registry field and every card value becomes an attribute (the card
-- wins where both have a key, as it did when published).
UPDATE registry_entities SET
    cot_type   = nullif(trim(json_extract(fields, '$.cot')), ''),
    class_name = nullif(trim(json_extract(fields, '$.ship_class')), ''),
    attributes = coalesce((
        SELECT json_group_array(json_object(
            'key', a.key,
            'type', CASE a.type WHEN 'integer' THEN 'number' WHEN 'real' THEN 'number'
                                WHEN 'true' THEN 'boolean' WHEN 'false' THEN 'boolean'
                                WHEN 'text' THEN 'text' ELSE 'json' END,
            'value', CASE a.type WHEN 'true' THEN json('true') WHEN 'false' THEN json('false')
                                 WHEN 'array' THEN json(a.value) WHEN 'object' THEN json(a.value)
                                 ELSE a.value END))
        FROM (
            SELECT f.key, f.type, f.value, 0 AS part, f.id AS ord
            FROM json_each(registry_entities.fields) f
            WHERE f.key NOT IN ('cot', 'ship_class') AND f.type <> 'null'
              AND f.key NOT IN (SELECT c.key FROM entity_cards ec, json_each(ec.card_values) c
                                WHERE ec.entity_id = registry_entities.id)
            UNION ALL
            SELECT c.key, c.type, c.value, 1 AS part, c.id AS ord
            FROM entity_cards ec, json_each(ec.card_values) c
            WHERE ec.entity_id = registry_entities.id AND c.type <> 'null'
            ORDER BY part, ord
        ) a
    ), '[]');

ALTER TABLE registry_entities DROP COLUMN fields;
DROP TABLE entity_card_revisions;
DROP TABLE entity_cards;

-- Every save of an entity, as the whole entity after it.
CREATE TABLE entity_revisions (
    id              INTEGER PRIMARY KEY,
    entity_id       TEXT    NOT NULL,
    entity          TEXT    NOT NULL CHECK (json_valid(entity)),
    saved_at_ms     INTEGER NOT NULL,
    decision_id     INTEGER REFERENCES decisions(id)
);
CREATE INDEX entity_revisions_entity ON entity_revisions(entity_id, id);
