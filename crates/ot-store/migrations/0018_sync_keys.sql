-- The public key an admin pinned for each peer site (Settings -> Nodes).
-- A sync message is accepted only from a site with a key here, and only if
-- its signature verifies with it (SC-8, IA-3). Text is `ed25519:<base64>`.
CREATE TABLE sync_peer_keys (
    site         TEXT PRIMARY KEY,
    public_key   TEXT NOT NULL,
    pinned_by    TEXT NOT NULL,
    pinned_at_ms INTEGER NOT NULL
);
