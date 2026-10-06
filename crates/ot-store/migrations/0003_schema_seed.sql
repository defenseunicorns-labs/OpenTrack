-- Schema version 1: the fixed core with no extension fields. Admins add
-- extension fields in later versions; a published version never changes.
INSERT INTO schema_versions (version, status, published_at_ms, notes)
VALUES (1, 'published', 0, 'Core schema, no extension fields');
