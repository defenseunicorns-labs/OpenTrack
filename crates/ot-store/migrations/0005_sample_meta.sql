-- Transport metadata captured with each probe sample (e.g. the MQTT topic),
-- so mapping previews see the same `_frame` fields the live source does.
ALTER TABLE probe_samples ADD COLUMN meta TEXT CHECK (meta IS NULL OR json_valid(meta));
