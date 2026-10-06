-- The secret an external plugin and OpenTrack prove themselves to each other
-- with (HMAC-SHA256 challenge and response each way), or a ${env:NAME}
-- reference to it. OpenTrack needs the secret itself to answer the plugin's
-- challenge, so it is kept as given, like a source's credentials. NULL: none
-- (only a unix socket under the data directory may go without).
ALTER TABLE plugins ADD COLUMN secret TEXT;
