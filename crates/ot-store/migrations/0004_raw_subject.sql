-- Raw output now goes to a NATS subject rather than a named collection.
-- Existing destinations are cleared: they named collections that no longer
-- exist, and consent was given for a different destination. The consent
-- decisions themselves stay in the decision log.
ALTER TABLE sources RENAME COLUMN raw_collection TO raw_subject;
UPDATE sources SET raw_subject = NULL, raw_consent = NULL WHERE raw_subject IS NOT NULL;
