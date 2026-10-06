-- Accounts, sessions and API tokens. A user signs in with a password
-- (local) or through SAML single sign-on; either way a signed session
-- token names the user, and every request checks the user here, so a
-- role change or a deactivation takes effect at once.
CREATE TABLE users (
    id               TEXT    PRIMARY KEY,                  -- UUID
    email            TEXT    NOT NULL UNIQUE COLLATE NOCASE,
    name             TEXT    NOT NULL DEFAULT '',
    role             TEXT    NOT NULL CHECK (role IN ('viewer', 'track_manager', 'admin')),
    active           INTEGER NOT NULL DEFAULT 1,
    password_hash    TEXT,                                 -- Argon2id (PHC string); NULL: no password sign-in
    origin           TEXT    NOT NULL DEFAULT 'local' CHECK (origin IN ('local', 'saml')),
    created_at_ms    INTEGER NOT NULL,
    updated_at_ms    INTEGER NOT NULL,
    last_login_at_ms INTEGER,
    -- Tokens issued before this are no longer accepted ("sign out everywhere").
    tokens_valid_from_ms INTEGER NOT NULL DEFAULT 0
);

-- Signed-out session tokens, kept until they would have expired anyway.
CREATE TABLE revoked_tokens (
    jti           TEXT    PRIMARY KEY,
    expires_at_ms INTEGER NOT NULL
);

-- Long-lived tokens for machines (a script, another service), each acting as a user.
CREATE TABLE api_tokens (
    jti           TEXT    PRIMARY KEY,
    name          TEXT    NOT NULL,
    user_id       TEXT    NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_by    TEXT    NOT NULL,
    created_at_ms INTEGER NOT NULL,
    expires_at_ms INTEGER NOT NULL,
    revoked_at_ms INTEGER
);

-- Sign-in settings (SAML, trusting OpenStare, client certificates): one
-- row, apart from the instance settings because it holds keys.
CREATE TABLE auth_settings (
    id            INTEGER PRIMARY KEY CHECK (id = 1),
    settings      TEXT    NOT NULL CHECK (json_valid(settings)),
    updated_at_ms INTEGER NOT NULL
);
