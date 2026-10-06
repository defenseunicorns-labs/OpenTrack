-- Plugins an operator added (Settings -> Plugins): WebAssembly components
-- stored here, or external plugins by address. Built-in plugins are not
-- listed. Every change is recorded in the decision log. Replaces the unused
-- placeholder table of 0001.
DROP TABLE plugins;
CREATE TABLE plugins (
    name            TEXT PRIMARY KEY,
    runtime         TEXT NOT NULL CHECK (runtime IN ('wasm', 'external')),
    version         TEXT NOT NULL,
    manifest        TEXT NOT NULL,
    -- The component (wasm), and its SHA-256.
    wasm            BLOB,
    sha256          TEXT,
    -- host:port, tcp://host:port or unix:/path (external).
    address         TEXT,
    grants          TEXT NOT NULL DEFAULT '{}',
    enabled         INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    created_at_ms   INTEGER NOT NULL,
    updated_at_ms   INTEGER NOT NULL
);
