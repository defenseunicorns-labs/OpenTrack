-- A track manager's override of whether tracks resolving to an entity are
-- published: 'always', 'never', or NULL for the engine's rules.
ALTER TABLE registry_entities ADD COLUMN publish TEXT CHECK (publish IS NULL OR publish IN ('always', 'never'));
