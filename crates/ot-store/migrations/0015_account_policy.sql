-- Account policy (DoD ASD STIG): password age and history, lockout after
-- failed sign-ins, inactive accounts turned off, and server-side sessions
-- with an idle timeout and a limit per account.

-- When the password was last set. Existing passwords count from now, so
-- they keep working until the maximum age runs out from the upgrade.
ALTER TABLE users ADD COLUMN password_changed_at_ms INTEGER;
UPDATE users SET password_changed_at_ms = CAST(strftime('%s', 'now') AS INTEGER) * 1000
    WHERE password_hash IS NOT NULL;
-- Set by an admin (a temporary password) or by the password's age: the
-- next sign-in must choose a new one before doing anything else.
ALTER TABLE users ADD COLUMN must_change_password INTEGER NOT NULL DEFAULT 0;
-- Consecutive failed sign-ins in the current window, and when it began.
ALTER TABLE users ADD COLUMN failed_logins INTEGER NOT NULL DEFAULT 0;
ALTER TABLE users ADD COLUMN failed_window_from_ms INTEGER;
-- Failed sign-ins since the last good one (shown after the next sign-in).
ALTER TABLE users ADD COLUMN failed_since_login INTEGER NOT NULL DEFAULT 0;
-- Locked until then (9223372036854775807: until an admin unlocks it).
ALTER TABLE users ADD COLUMN locked_until_ms INTEGER;
-- When the account was last turned on, so re-enabling an inactive account
-- restarts its inactivity clock; and why it was turned off, if automatically.
ALTER TABLE users ADD COLUMN active_since_ms INTEGER;
ALTER TABLE users ADD COLUMN disabled_reason TEXT;

-- Earlier password hashes, so a new password cannot repeat a recent one.
CREATE TABLE password_history (
    user_id   TEXT    NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    hash      TEXT    NOT NULL,
    set_at_ms INTEGER NOT NULL
);
CREATE INDEX password_history_user ON password_history(user_id, set_at_ms);

-- Browser sessions: one row per sign-in, named by the session token's id.
-- Per node: another node's sessions are not known here.
CREATE TABLE sessions (
    id               TEXT    PRIMARY KEY,                 -- the token's jti
    user_id          TEXT    NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at_ms    INTEGER NOT NULL,
    last_seen_ms     INTEGER NOT NULL,
    expires_at_ms    INTEGER NOT NULL,
    ip               TEXT,
    user_agent       TEXT,
    ended_at_ms      INTEGER,
    -- sign_out, idle, expired, limit, revoked, password, disabled, ended
    end_reason       TEXT,
    -- The account's previous good sign-in and the failed ones since, as
    -- they stood when this session began (the notice after signing in).
    prev_login_at_ms INTEGER,
    failed_before    INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX sessions_user ON sessions(user_id, ended_at_ms);
CREATE INDEX sessions_live ON sessions(ended_at_ms) WHERE ended_at_ms IS NULL;
