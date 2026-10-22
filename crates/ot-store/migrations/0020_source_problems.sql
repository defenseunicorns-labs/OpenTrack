-- Why a source may not run yet, as a JSON array of problems ({kind, stage,
-- field?, message}), worked out by the server each time the source is saved.
-- Empty: complete. Otherwise the source is a draft, which cannot be enabled.
-- Every source saved before drafts was complete.
ALTER TABLE sources ADD COLUMN problems TEXT NOT NULL DEFAULT '[]';
