//! Accounts, revoked session tokens, API tokens and the sign-in settings.
//! Password hashing and token signing are the server's; this stores what
//! they need.

use rusqlite::{OptionalExtension, Row, params};
use serde::Serialize;
use serde_json::Value;

use crate::sqlite::{Db, Result, StoreError, now_ms};

/// An account. The password hash never leaves the store in this.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct User {
    pub id: String,
    pub email: String,
    pub name: String,
    pub role: String,
    pub active: bool,
    /// `local` (made here) or `saml` (made at its first single sign-on).
    pub origin: String,
    /// Whether it can sign in with a password.
    pub has_password: bool,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub last_login_at_ms: Option<i64>,
    #[serde(skip)]
    pub tokens_valid_from_ms: i64,
    /// When the password was last set (`None`: no password, or never).
    pub password_changed_at_ms: Option<i64>,
    /// The next sign-in must choose a new password first.
    pub must_change_password: bool,
    /// Consecutive failed sign-ins in the current lockout window.
    pub failed_logins: i64,
    /// Locked until then ([`LOCKED_UNTIL_UNLOCKED`]: until an admin unlocks).
    pub locked_until_ms: Option<i64>,
    /// When the account was last turned on (restarts its inactivity clock).
    pub active_since_ms: Option<i64>,
    /// Why it was turned off automatically (`inactivity`, `expired`).
    pub disabled_reason: Option<String>,
    /// A temporary account: turned off when this passes.
    pub expires_at_ms: Option<i64>,
}

/// `locked_until_ms` of an account locked until an admin unlocks it.
pub const LOCKED_UNTIL_UNLOCKED: i64 = i64::MAX;

impl User {
    /// Whether a temporary account's time has run out at `now_ms`.
    pub fn expired(&self, now_ms: i64) -> bool {
        self.expires_at_ms.is_some_and(|t| t <= now_ms)
    }

    /// Whether sign-in is locked at `now_ms`.
    pub fn locked(&self, now_ms: i64) -> bool {
        self.locked_until_ms.is_some_and(|t| t > now_ms)
    }

    /// When the account last showed life: its last sign-in, when it was
    /// last turned on, or when it was made.
    pub fn last_activity_ms(&self) -> i64 {
        self.created_at_ms
            .max(self.last_login_at_ms.unwrap_or(0))
            .max(self.active_since_ms.unwrap_or(0))
    }
}

/// A browser session (one sign-in), named by its token's id.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Session {
    pub id: String,
    pub user_id: String,
    pub user_email: String,
    pub created_at_ms: i64,
    pub last_seen_ms: i64,
    pub expires_at_ms: i64,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    pub ended_at_ms: Option<i64>,
    pub end_reason: Option<String>,
    /// The account's previous good sign-in, when this one began.
    pub prev_login_at_ms: Option<i64>,
    /// Failed sign-ins between that one and this.
    pub failed_before: i64,
}

/// A session about to begin.
#[derive(Debug, Clone)]
pub struct NewSession<'a> {
    pub id: &'a str,
    pub user_id: &'a str,
    pub expires_at_ms: i64,
    pub ip: Option<&'a str>,
    pub user_agent: Option<&'a str>,
    pub prev_login_at_ms: Option<i64>,
    pub failed_before: i64,
}

/// What a failed sign-in did to the account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Failure {
    /// Consecutive failures in the window, this one included.
    pub count: i64,
    /// This failure locked the account.
    pub locked_now: bool,
}

/// A new account.
#[derive(Debug, Clone)]
pub struct NewUser<'a> {
    pub id: &'a str,
    pub email: &'a str,
    pub name: &'a str,
    pub role: &'a str,
    pub password_hash: Option<&'a str>,
    pub origin: &'a str,
}

/// An API token's record (the token itself is shown once, never stored).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ApiToken {
    pub jti: String,
    pub name: String,
    pub user_id: String,
    pub user_email: String,
    pub created_by: String,
    pub created_at_ms: i64,
    pub expires_at_ms: i64,
    pub revoked_at_ms: Option<i64>,
}

const USER_COLUMNS: &str = "id, email, name, role, active, origin, password_hash IS NOT NULL,
    created_at_ms, updated_at_ms, last_login_at_ms, tokens_valid_from_ms,
    password_changed_at_ms, must_change_password, failed_logins, locked_until_ms,
    active_since_ms, disabled_reason, expires_at_ms";

const SESSION_COLUMNS: &str = "s.id, s.user_id, u.email, s.created_at_ms, s.last_seen_ms,
    s.expires_at_ms, s.ip, s.user_agent, s.ended_at_ms, s.end_reason, s.prev_login_at_ms,
    s.failed_before";

fn session(r: &Row<'_>) -> rusqlite::Result<Session> {
    Ok(Session {
        id: r.get(0)?,
        user_id: r.get(1)?,
        user_email: r.get(2)?,
        created_at_ms: r.get(3)?,
        last_seen_ms: r.get(4)?,
        expires_at_ms: r.get(5)?,
        ip: r.get(6)?,
        user_agent: r.get(7)?,
        ended_at_ms: r.get(8)?,
        end_reason: r.get(9)?,
        prev_login_at_ms: r.get(10)?,
        failed_before: r.get(11)?,
    })
}

fn user(r: &Row<'_>) -> rusqlite::Result<User> {
    Ok(User {
        id: r.get(0)?,
        email: r.get(1)?,
        name: r.get(2)?,
        role: r.get(3)?,
        active: r.get(4)?,
        origin: r.get(5)?,
        has_password: r.get(6)?,
        created_at_ms: r.get(7)?,
        updated_at_ms: r.get(8)?,
        last_login_at_ms: r.get(9)?,
        tokens_valid_from_ms: r.get(10)?,
        password_changed_at_ms: r.get(11)?,
        must_change_password: r.get(12)?,
        failed_logins: r.get(13)?,
        locked_until_ms: r.get(14)?,
        active_since_ms: r.get(15)?,
        disabled_reason: r.get(16)?,
        expires_at_ms: r.get(17)?,
    })
}

impl Db {
    pub fn users(&self) -> Result<Vec<User>> {
        let mut st = self
            .connection()
            .prepare(&format!("SELECT {USER_COLUMNS} FROM users ORDER BY email"))?;
        let rows = st.query_map([], user)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn user_count(&self) -> Result<i64> {
        Ok(self
            .connection()
            .query_row("SELECT count(*) FROM users", [], |r| r.get(0))?)
    }

    pub fn user(&self, id: &str) -> Result<Option<User>> {
        Ok(self
            .connection()
            .query_row(
                &format!("SELECT {USER_COLUMNS} FROM users WHERE id = ?1"),
                [id],
                user,
            )
            .optional()?)
    }

    pub fn user_by_email(&self, email: &str) -> Result<Option<User>> {
        Ok(self
            .connection()
            .query_row(
                &format!("SELECT {USER_COLUMNS} FROM users WHERE email = ?1"),
                [email.trim()],
                user,
            )
            .optional()?)
    }

    /// The password hash of an active account, by email.
    pub fn password_hash(&self, email: &str) -> Result<Option<(User, Option<String>)>> {
        let Some(u) = self.user_by_email(email)? else {
            return Ok(None);
        };
        let hash: Option<String> = self.connection().query_row(
            "SELECT password_hash FROM users WHERE id = ?1",
            [&u.id],
            |r| r.get(0),
        )?;
        Ok(Some((u, hash)))
    }

    pub fn create_user(&mut self, u: &NewUser<'_>) -> Result<User> {
        let now = now_ms();
        let email = u.email.trim();
        if self.user_by_email(email)?.is_some() {
            return Err(StoreError::Conflict(format!(
                "an account for {email} exists"
            )));
        }
        self.connection().execute(
            "INSERT INTO users (id, email, name, role, password_hash, origin, created_at_ms, updated_at_ms,
                 password_changed_at_ms, active_since_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, CASE WHEN ?5 IS NULL THEN NULL ELSE ?7 END, ?7)",
            params![u.id, email, u.name.trim(), u.role, u.password_hash, u.origin, now],
        )?;
        if let Some(h) = u.password_hash {
            self.connection().execute(
                "INSERT INTO password_history (user_id, hash, set_at_ms) VALUES (?1, ?2, ?3)",
                params![u.id, h, now],
            )?;
        }
        self.user(u.id)?
            .ok_or_else(|| StoreError::NotFound(format!("user {}", u.id)))
    }

    /// Change an account's name, role or active flag; deactivating also
    /// ends its sessions.
    pub fn update_user(
        &mut self,
        id: &str,
        name: Option<&str>,
        role: Option<&str>,
        active: Option<bool>,
    ) -> Result<User> {
        let now = now_ms();
        let n = self.connection().execute(
            "UPDATE users SET name = coalesce(?2, name), role = coalesce(?3, role),
                 updated_at_ms = ?5,
                 tokens_valid_from_ms = CASE WHEN ?4 = 0 THEN ?5 ELSE tokens_valid_from_ms END,
                 active_since_ms = CASE WHEN ?4 = 1 AND active = 0 THEN ?5 ELSE active_since_ms END,
                 disabled_reason = CASE WHEN ?4 = 1 THEN NULL ELSE disabled_reason END,
                 active = coalesce(?4, active)
             WHERE id = ?1",
            params![id, name.map(str::trim), role, active, now],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound(format!("user {id}")));
        }
        if active == Some(false) {
            self.end_user_sessions(id, "disabled", None)?;
            self.revoke_api_tokens(id, now)?;
        }
        self.user(id)?
            .ok_or_else(|| StoreError::NotFound(format!("user {id}")))
    }

    /// Set (or, with `None`, remove) an account's password.
    pub fn set_password_hash(&mut self, id: &str, hash: Option<&str>) -> Result<()> {
        self.set_password(id, hash, false, 24)
    }

    /// Replace the stored hash of the same password (a stronger or FIPS
    /// algorithm): its age, history and must-change flag stay as they are.
    pub fn rehash_password(&mut self, id: &str, hash: &str) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "UPDATE users SET password_hash = ?2 WHERE id = ?1 AND password_hash IS NOT NULL",
                params![id, hash],
            )?;
            Ok(())
        })
    }

    /// Set (or remove) an account's password: `must_change` makes it a
    /// temporary one (an admin's), changed at the next sign-in. The hash
    /// joins the account's history, which keeps the newest `keep`.
    pub fn set_password(
        &mut self,
        id: &str,
        hash: Option<&str>,
        must_change: bool,
        keep: usize,
    ) -> Result<()> {
        let now = now_ms();
        self.write(|tx| {
            let n = tx.execute(
                "UPDATE users SET password_hash = ?2, updated_at_ms = ?3,
                     password_changed_at_ms = CASE WHEN ?2 IS NULL THEN NULL ELSE ?3 END,
                     must_change_password = ?4
                 WHERE id = ?1",
                params![id, hash, now, must_change && hash.is_some()],
            )?;
            if n == 0 {
                return Err(StoreError::NotFound(format!("user {id}")));
            }
            if let Some(h) = hash {
                tx.execute(
                    "INSERT INTO password_history (user_id, hash, set_at_ms) VALUES (?1, ?2, ?3)",
                    params![id, h, now],
                )?;
                tx.execute(
                    "DELETE FROM password_history WHERE user_id = ?1 AND rowid NOT IN
                         (SELECT rowid FROM password_history WHERE user_id = ?1
                          ORDER BY set_at_ms DESC, rowid DESC LIMIT ?2)",
                    params![id, keep.max(1) as i64],
                )?;
            }
            Ok(())
        })
    }

    /// The current password hash and the `n` before it, newest first (to
    /// refuse reusing one).
    pub fn recent_password_hashes(&self, id: &str, n: usize) -> Result<Vec<String>> {
        let current: Option<String> = self
            .connection()
            .query_row("SELECT password_hash FROM users WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .optional()?
            .flatten();
        let mut st = self.connection().prepare(
            "SELECT hash FROM password_history WHERE user_id = ?1
             ORDER BY set_at_ms DESC, rowid DESC LIMIT ?2",
        )?;
        let mut out: Vec<String> = current.into_iter().collect();
        for h in st.query_map(params![id, n as i64], |r| r.get::<_, String>(0))? {
            let h = h?;
            if !out.contains(&h) {
                out.push(h);
            }
        }
        out.truncate(n.max(1));
        Ok(out)
    }

    /// Require (or stop requiring) a new password at the next sign-in.
    pub fn set_must_change_password(&mut self, id: &str, must: bool) -> Result<()> {
        self.connection().execute(
            "UPDATE users SET must_change_password = ?2 WHERE id = ?1",
            params![id, must],
        )?;
        Ok(())
    }

    /// Count a failed sign-in: consecutive failures within `window_ms` of
    /// the first; at `max` (0: never) the account locks for `lock_ms` (0:
    /// until an admin unlocks it).
    pub fn note_login_failure(
        &mut self,
        id: &str,
        window_ms: i64,
        max: i64,
        lock_ms: i64,
    ) -> Result<Failure> {
        let now = now_ms();
        self.write(|tx| {
            let (count, from): (i64, Option<i64>) = tx.query_row(
                "SELECT failed_logins, failed_window_from_ms FROM users WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let (count, from) = match from {
                Some(f) if now - f <= window_ms => (count + 1, f),
                _ => (1, now),
            };
            let locked_now = max > 0 && count >= max;
            let until = if lock_ms > 0 {
                now.saturating_add(lock_ms)
            } else {
                LOCKED_UNTIL_UNLOCKED
            };
            tx.execute(
                "UPDATE users SET failed_logins = ?2, failed_window_from_ms = ?3,
                     failed_since_login = failed_since_login + 1,
                     locked_until_ms = CASE WHEN ?4 THEN ?5 ELSE locked_until_ms END
                 WHERE id = ?1",
                params![
                    id,
                    if locked_now { 0 } else { count },
                    if locked_now { None } else { Some(from) },
                    locked_now,
                    until
                ],
            )?;
            Ok(Failure { count, locked_now })
        })
    }

    /// Count a failed sign-in that was refused before the password was
    /// checked (a locked account): it shows in the notice after the next
    /// sign-in but does not extend the lock.
    pub fn note_refused_login(&mut self, id: &str) -> Result<()> {
        self.connection().execute(
            "UPDATE users SET failed_since_login = failed_since_login + 1 WHERE id = ?1",
            [id],
        )?;
        Ok(())
    }

    /// A good sign-in: returns the previous good one and the failures
    /// since, and clears them.
    pub fn note_login_success(&mut self, id: &str) -> Result<(Option<i64>, i64)> {
        let now = now_ms();
        self.write(|tx| {
            let prev: (Option<i64>, i64) = tx.query_row(
                "SELECT last_login_at_ms, failed_since_login FROM users WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            tx.execute(
                "UPDATE users SET last_login_at_ms = ?2, failed_logins = 0,
                     failed_window_from_ms = NULL, failed_since_login = 0, locked_until_ms = NULL
                 WHERE id = ?1",
                params![id, now],
            )?;
            Ok(prev)
        })
    }

    /// Unlock an account locked by failed sign-ins.
    pub fn unlock_user(&mut self, id: &str) -> Result<User> {
        let n = self.connection().execute(
            "UPDATE users SET locked_until_ms = NULL, failed_logins = 0,
                 failed_window_from_ms = NULL, updated_at_ms = ?2
             WHERE id = ?1",
            params![id, now_ms()],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound(format!("user {id}")));
        }
        self.user(id)?
            .ok_or_else(|| StoreError::NotFound(format!("user {id}")))
    }

    /// Turn off active accounts whose last activity is before `cutoff_ms`
    /// (except those listed by email), ending their sessions. Returns them
    /// as they were.
    pub fn disable_inactive(&mut self, cutoff_ms: i64, exempt: &[String]) -> Result<Vec<User>> {
        let stale: Vec<User> = self
            .users()?
            .into_iter()
            .filter(|u| u.active && u.last_activity_ms() < cutoff_ms)
            .filter(|u| {
                !exempt
                    .iter()
                    .any(|e| e.trim().eq_ignore_ascii_case(&u.email))
            })
            .collect();
        let now = now_ms();
        for u in &stale {
            self.connection().execute(
                "UPDATE users SET active = 0, disabled_reason = 'inactivity', updated_at_ms = ?2,
                     tokens_valid_from_ms = ?2
                 WHERE id = ?1 AND active = 1",
                params![u.id, now],
            )?;
            self.end_user_sessions(&u.id, "disabled", None)?;
            self.revoke_api_tokens(&u.id, now)?;
        }
        Ok(stale)
    }

    /// Make an account temporary (turned off at `expires_at_ms`), or not.
    pub fn set_account_expiry(&mut self, id: &str, expires_at_ms: Option<i64>) -> Result<User> {
        let n = self.connection().execute(
            "UPDATE users SET expires_at_ms = ?2, updated_at_ms = ?3 WHERE id = ?1",
            params![id, expires_at_ms, now_ms()],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound(format!("user {id}")));
        }
        self.user(id)?
            .ok_or_else(|| StoreError::NotFound(format!("user {id}")))
    }

    /// Turn off active temporary accounts whose time has run out at
    /// `now_ms`, ending their sessions and revoking their API tokens.
    /// Returns them as they were.
    pub fn disable_expired(&mut self, now_ms: i64) -> Result<Vec<User>> {
        let due: Vec<User> = self
            .users()?
            .into_iter()
            .filter(|u| u.active && u.expired(now_ms))
            .collect();
        for u in &due {
            self.connection().execute(
                "UPDATE users SET active = 0, disabled_reason = 'expired', updated_at_ms = ?2,
                     tokens_valid_from_ms = ?2
                 WHERE id = ?1 AND active = 1",
                params![u.id, now_ms],
            )?;
            self.end_user_sessions(&u.id, "disabled", None)?;
            self.revoke_api_tokens(&u.id, now_ms)?;
        }
        Ok(due)
    }

    /// Begin a session.
    pub fn create_session(&mut self, s: &NewSession<'_>) -> Result<()> {
        let now = now_ms();
        self.connection().execute(
            "INSERT INTO sessions (id, user_id, created_at_ms, last_seen_ms, expires_at_ms, ip,
                 user_agent, prev_login_at_ms, failed_before)
             VALUES (?1, ?2, ?3, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                s.id,
                s.user_id,
                now,
                s.expires_at_ms,
                s.ip,
                s.user_agent
                    .map(|a| a.chars().take(512).collect::<String>()),
                s.prev_login_at_ms,
                s.failed_before
            ],
        )?;
        Ok(())
    }

    pub fn session(&self, id: &str) -> Result<Option<Session>> {
        Ok(self
            .connection()
            .query_row(
                &format!(
                    "SELECT {SESSION_COLUMNS} FROM sessions s JOIN users u ON u.id = s.user_id
                     WHERE s.id = ?1"
                ),
                [id],
                session,
            )
            .optional()?)
    }

    /// Sessions, newest first: one account's or everyone's; live only
    /// (not ended, not expired) unless `ended` too.
    pub fn sessions(
        &self,
        user_id: Option<&str>,
        ended: bool,
        limit: usize,
    ) -> Result<Vec<Session>> {
        let mut st = self.connection().prepare(&format!(
            "SELECT {SESSION_COLUMNS} FROM sessions s JOIN users u ON u.id = s.user_id
             WHERE (?1 IS NULL OR s.user_id = ?1)
               AND (?2 OR (s.ended_at_ms IS NULL AND s.expires_at_ms > ?3))
             ORDER BY s.created_at_ms DESC LIMIT ?4"
        ))?;
        let rows = st.query_map(params![user_id, ended, now_ms(), limit as i64], session)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Save when sessions were last used (batched by the server).
    pub fn touch_sessions(&mut self, seen: &[(String, i64)]) -> Result<()> {
        self.write(|tx| {
            let mut st = tx.prepare(
                "UPDATE sessions SET last_seen_ms = max(last_seen_ms, ?2)
                 WHERE id = ?1 AND ended_at_ms IS NULL",
            )?;
            for (id, at) in seen {
                st.execute(params![id, at])?;
            }
            Ok(())
        })
    }

    /// End a session; false when it had ended already.
    pub fn end_session(&mut self, id: &str, reason: &str) -> Result<bool> {
        Ok(self.connection().execute(
            "UPDATE sessions SET ended_at_ms = ?2, end_reason = ?3 WHERE id = ?1 AND ended_at_ms IS NULL",
            params![id, now_ms(), reason],
        )? > 0)
    }

    /// End an account's live sessions (but `except`); returns their ids.
    pub fn end_user_sessions(
        &mut self,
        user_id: &str,
        reason: &str,
        except: Option<&str>,
    ) -> Result<Vec<String>> {
        let mut st = self.connection().prepare(
            "UPDATE sessions SET ended_at_ms = ?2, end_reason = ?3
             WHERE user_id = ?1 AND ended_at_ms IS NULL AND (?4 IS NULL OR id <> ?4)
             RETURNING id",
        )?;
        let rows = st.query_map(params![user_id, now_ms(), reason, except], |r| {
            r.get::<_, String>(0)
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// End sessions idle since before `idle_before_ms` (admins' since
    /// before `admin_idle_before_ms`) or past their lifetime. Returns the
    /// ended sessions with their reason (`idle` or `expired`).
    pub fn end_stale_sessions(
        &mut self,
        idle_before_ms: i64,
        admin_idle_before_ms: i64,
    ) -> Result<Vec<Session>> {
        let now = now_ms();
        self.write(|tx| {
            let mut st = tx.prepare(&format!(
                "SELECT {SESSION_COLUMNS}, u.role FROM sessions s JOIN users u ON u.id = s.user_id
                 WHERE s.ended_at_ms IS NULL
                   AND (s.expires_at_ms <= ?1
                        OR s.last_seen_ms < CASE WHEN u.role = 'admin' THEN ?3 ELSE ?2 END)"
            ))?;
            let stale: Vec<Session> = st
                .query_map(params![now, idle_before_ms, admin_idle_before_ms], session)?
                .collect::<rusqlite::Result<_>>()?;
            drop(st);
            let mut out = Vec::new();
            for mut s in stale {
                let reason = if s.expires_at_ms <= now {
                    "expired"
                } else {
                    "idle"
                };
                tx.execute(
                    "UPDATE sessions SET ended_at_ms = ?2, end_reason = ?3 WHERE id = ?1",
                    params![s.id, now, reason],
                )?;
                s.ended_at_ms = Some(now);
                s.end_reason = Some(reason.into());
                out.push(s);
            }
            // Keep ended sessions for 90 days.
            tx.execute(
                "DELETE FROM sessions WHERE ended_at_ms IS NOT NULL AND ended_at_ms < ?1",
                [now - 90 * 86_400_000],
            )?;
            Ok(out)
        })
    }

    pub fn delete_user(&mut self, id: &str) -> Result<()> {
        let n = self
            .connection()
            .execute("DELETE FROM users WHERE id = ?1", [id])?;
        if n == 0 {
            return Err(StoreError::NotFound(format!("user {id}")));
        }
        Ok(())
    }

    pub fn note_login(&mut self, id: &str) -> Result<()> {
        self.connection().execute(
            "UPDATE users SET last_login_at_ms = ?2 WHERE id = ?1",
            params![id, now_ms()],
        )?;
        Ok(())
    }

    /// Revoke every API token an account holds, for good: turning the
    /// account on again does not bring them back.
    fn revoke_api_tokens(&mut self, id: &str, now: i64) -> Result<()> {
        self.connection().execute(
            "UPDATE api_tokens SET revoked_at_ms = ?2 WHERE user_id = ?1 AND revoked_at_ms IS NULL",
            params![id, now],
        )?;
        Ok(())
    }

    /// End every session and API token of an account issued before now.
    pub fn revoke_user_tokens(&mut self, id: &str) -> Result<()> {
        let now = now_ms();
        self.connection().execute(
            "UPDATE users SET tokens_valid_from_ms = ?2 WHERE id = ?1",
            params![id, now],
        )?;
        self.revoke_api_tokens(id, now)?;
        self.end_user_sessions(id, "revoked", None)?;
        Ok(())
    }

    /// Revoke one session token (sign out), and forget expired revocations.
    pub fn revoke_token(&mut self, jti: &str, expires_at_ms: i64) -> Result<()> {
        let now = now_ms();
        self.connection().execute(
            "INSERT OR IGNORE INTO revoked_tokens (jti, expires_at_ms) VALUES (?1, ?2)",
            params![jti, expires_at_ms],
        )?;
        self.connection()
            .execute("DELETE FROM revoked_tokens WHERE expires_at_ms < ?1", [now])?;
        Ok(())
    }

    /// Whether a token id was revoked: signed out, or an API token revoked.
    pub fn token_revoked(&self, jti: &str) -> Result<bool> {
        Ok(self.connection().query_row(
            "SELECT EXISTS (SELECT 1 FROM revoked_tokens WHERE jti = ?1)
                 OR EXISTS (SELECT 1 FROM api_tokens WHERE jti = ?1 AND revoked_at_ms IS NOT NULL)",
            [jti],
            |r| r.get(0),
        )?)
    }

    pub fn add_api_token(
        &mut self,
        jti: &str,
        name: &str,
        user_id: &str,
        created_by: &str,
        expires_at_ms: i64,
    ) -> Result<()> {
        self.connection().execute(
            "INSERT INTO api_tokens (jti, name, user_id, created_by, created_at_ms, expires_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                jti,
                name.trim(),
                user_id,
                created_by,
                now_ms(),
                expires_at_ms
            ],
        )?;
        Ok(())
    }

    pub fn api_tokens(&self) -> Result<Vec<ApiToken>> {
        let mut st = self.connection().prepare(
            "SELECT t.jti, t.name, t.user_id, u.email, t.created_by, t.created_at_ms,
                    t.expires_at_ms, t.revoked_at_ms
             FROM api_tokens t JOIN users u ON u.id = t.user_id
             ORDER BY t.created_at_ms DESC",
        )?;
        let rows = st.query_map([], |r| {
            Ok(ApiToken {
                jti: r.get(0)?,
                name: r.get(1)?,
                user_id: r.get(2)?,
                user_email: r.get(3)?,
                created_by: r.get(4)?,
                created_at_ms: r.get(5)?,
                expires_at_ms: r.get(6)?,
                revoked_at_ms: r.get(7)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn revoke_api_token(&mut self, jti: &str) -> Result<()> {
        let n = self.connection().execute(
            "UPDATE api_tokens SET revoked_at_ms = ?2 WHERE jti = ?1 AND revoked_at_ms IS NULL",
            params![jti, now_ms()],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound(format!("live API token {jti}")));
        }
        Ok(())
    }

    /// The sign-in settings (an empty object before any save).
    pub fn auth_settings(&self) -> Result<Value> {
        let text: Option<String> = self
            .connection()
            .query_row("SELECT settings FROM auth_settings WHERE id = 1", [], |r| {
                r.get(0)
            })
            .optional()?;
        Ok(match text {
            Some(t) => serde_json::from_str(&t)?,
            None => Value::Object(Default::default()),
        })
    }

    pub fn put_auth_settings(&mut self, settings: &Value) -> Result<()> {
        self.connection().execute(
            "INSERT INTO auth_settings (id, settings, updated_at_ms) VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET settings = excluded.settings, updated_at_ms = excluded.updated_at_ms",
            params![settings.to_string(), now_ms()],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new<'a>(id: &'a str, email: &'a str) -> NewUser<'a> {
        NewUser {
            id,
            email,
            name: "",
            role: "viewer",
            password_hash: Some("hash"),
            origin: "local",
        }
    }

    #[test]
    fn accounts_tokens_and_settings() {
        let mut db = Db::open_in_memory().unwrap();
        assert_eq!(db.user_count().unwrap(), 0);
        let u = db.create_user(&new("u1", "Ann@Example.org ")).unwrap();
        assert_eq!(u.email, "Ann@Example.org");
        assert!(u.has_password && u.active);
        // Emails are unique whatever their case.
        assert!(matches!(
            db.create_user(&new("u2", "ann@example.org")),
            Err(StoreError::Conflict(_))
        ));
        assert_eq!(
            db.password_hash("ANN@example.org")
                .unwrap()
                .unwrap()
                .1
                .as_deref(),
            Some("hash")
        );

        let u = db
            .update_user("u1", None, Some("admin"), Some(false))
            .unwrap();
        assert_eq!(u.role, "admin");
        assert!(!u.active);
        assert!(u.tokens_valid_from_ms > 0, "deactivating ends sessions");

        db.revoke_token("s1", now_ms() + 60_000).unwrap();
        assert!(db.token_revoked("s1").unwrap());
        db.add_api_token("t1", "feed script", "u1", "admin@x", now_ms() + 60_000)
            .unwrap();
        assert!(!db.token_revoked("t1").unwrap());
        db.revoke_api_token("t1").unwrap();
        assert!(db.token_revoked("t1").unwrap());
        assert_eq!(db.api_tokens().unwrap()[0].user_email, "Ann@Example.org");
        // Turning an account off revokes its API tokens for good: turning it
        // on again does not bring them back.
        db.update_user("u1", None, None, Some(true)).unwrap();
        db.add_api_token("t2", "another", "u1", "admin@x", now_ms() + 60_000)
            .unwrap();
        db.update_user("u1", None, None, Some(false)).unwrap();
        db.update_user("u1", None, None, Some(true)).unwrap();
        assert!(db.token_revoked("t2").unwrap());

        assert_eq!(db.auth_settings().unwrap(), serde_json::json!({}));
        db.put_auth_settings(&serde_json::json!({"saml": {"enabled": false}}))
            .unwrap();
        assert_eq!(db.auth_settings().unwrap()["saml"]["enabled"], false);

        db.delete_user("u1").unwrap();
        assert!(
            db.api_tokens().unwrap().is_empty(),
            "tokens go with the user"
        );
    }

    #[test]
    fn failed_sign_ins_lock_and_a_good_one_resets() {
        let mut db = Db::open_in_memory().unwrap();
        db.create_user(&new("u1", "a@x")).unwrap();
        let fail = |db: &mut Db| db.note_login_failure("u1", 900_000, 3, 900_000).unwrap();
        assert_eq!(fail(&mut db).count, 1);
        assert!(!fail(&mut db).locked_now);
        let third = fail(&mut db);
        assert!(third.locked_now && third.count == 3);
        let u = db.user("u1").unwrap().unwrap();
        assert!(u.locked(now_ms()));
        assert!(!u.locked(now_ms() + 900_001), "the lock runs out");
        db.note_refused_login("u1").unwrap();
        // Unlocked by an admin; the failures still show at the next sign-in.
        assert!(!db.unlock_user("u1").unwrap().locked(now_ms()));
        let (prev, failed) = db.note_login_success("u1").unwrap();
        assert_eq!((prev, failed), (None, 4));
        let (prev, failed) = db.note_login_success("u1").unwrap();
        assert!(prev.is_some());
        assert_eq!(failed, 0);

        // 0 minutes: locked until an admin unlocks it.
        for _ in 0..3 {
            db.note_login_failure("u1", 900_000, 3, 0).unwrap();
        }
        let u = db.user("u1").unwrap().unwrap();
        assert_eq!(u.locked_until_ms, Some(LOCKED_UNTIL_UNLOCKED));
        // Failures outside the window start a new count.
        db.unlock_user("u1").unwrap();
        db.note_login_failure("u1", 0, 3, 0).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(3));
        assert_eq!(db.note_login_failure("u1", 0, 3, 0).unwrap().count, 1);
    }

    #[test]
    fn password_history_keeps_the_newest() {
        let mut db = Db::open_in_memory().unwrap();
        db.create_user(&new("u1", "a@x")).unwrap();
        for i in 0..8 {
            db.set_password("u1", Some(&format!("h{i}")), false, 5)
                .unwrap();
        }
        assert_eq!(
            db.recent_password_hashes("u1", 5).unwrap(),
            ["h7", "h6", "h5", "h4", "h3"]
        );
        db.set_password("u1", Some("t"), true, 5).unwrap();
        let u = db.user("u1").unwrap().unwrap();
        assert!(u.must_change_password && u.password_changed_at_ms.is_some());
        db.set_password("u1", None, true, 5).unwrap();
        assert!(!db.user("u1").unwrap().unwrap().must_change_password);
    }

    #[test]
    fn a_temporary_account_is_turned_off_when_its_time_runs_out() {
        let mut db = Db::open_in_memory().unwrap();
        db.create_user(&new("u1", "temp@x")).unwrap();
        db.create_user(&new("u2", "perm@x")).unwrap();
        let now = now_ms();
        let u = db.set_account_expiry("u1", Some(now + 1_000)).unwrap();
        assert_eq!(u.expires_at_ms, Some(now + 1_000));
        assert!(!u.expired(now) && u.expired(now + 1_000));
        assert!(db.disable_expired(now).unwrap().is_empty(), "not yet");
        let off = db.disable_expired(now + 2_000).unwrap();
        assert_eq!(
            off.iter().map(|u| u.email.as_str()).collect::<Vec<_>>(),
            ["temp@x"]
        );
        let u = db.user("u1").unwrap().unwrap();
        assert!(!u.active && u.disabled_reason.as_deref() == Some("expired"));
        assert!(db.user("u2").unwrap().unwrap().active);
        // Permanent again.
        assert_eq!(
            db.set_account_expiry("u1", None).unwrap().expires_at_ms,
            None
        );
    }

    #[test]
    fn sessions_begin_are_seen_and_end() {
        let mut db = Db::open_in_memory().unwrap();
        db.create_user(&new("u1", "a@x")).unwrap();
        let s = |id: &'static str, exp: i64| NewSession {
            id,
            user_id: "u1",
            expires_at_ms: exp,
            ip: Some("10.0.0.1"),
            user_agent: Some("test"),
            prev_login_at_ms: None,
            failed_before: 2,
        };
        let far = now_ms() + 3_600_000;
        db.create_session(&s("s1", far)).unwrap();
        db.create_session(&s("s2", far)).unwrap();
        db.create_session(&s("s3", now_ms() - 1)).unwrap();
        assert_eq!(db.sessions(Some("u1"), false, 10).unwrap().len(), 2);
        assert_eq!(db.session("s1").unwrap().unwrap().failed_before, 2);
        // s2 was used a moment ago; s1 has been idle since it began.
        let now = now_ms();
        db.touch_sessions(&[("s2".into(), now + 1_000)]).unwrap();
        let ended = db.end_stale_sessions(now + 500, now + 500).unwrap();
        let mut reasons: Vec<(String, String)> = ended
            .into_iter()
            .map(|s| (s.id, s.end_reason.unwrap()))
            .collect();
        reasons.sort();
        assert_eq!(
            reasons,
            [
                ("s1".into(), "idle".into()),
                ("s3".into(), "expired".into())
            ]
        );
        assert!(!db.end_session("s1", "sign_out").unwrap(), "ended already");
        db.revoke_user_tokens("u1").unwrap();
        assert_eq!(
            db.session("s2").unwrap().unwrap().end_reason.as_deref(),
            Some("revoked")
        );
    }

    #[test]
    fn inactive_accounts_are_turned_off_unless_exempt() {
        let mut db = Db::open_in_memory().unwrap();
        db.create_user(&new("u1", "a@x")).unwrap();
        db.create_user(&new("u2", "breakglass@x")).unwrap();
        let later = now_ms() + 1_000;
        let off = db
            .disable_inactive(later, &["BreakGlass@x".into()])
            .unwrap();
        assert_eq!(off.len(), 1);
        let u = db.user("u1").unwrap().unwrap();
        assert!(!u.active);
        assert_eq!(u.disabled_reason.as_deref(), Some("inactivity"));
        // Turned on again: its clock restarts.
        std::thread::sleep(std::time::Duration::from_millis(2));
        let u = db.update_user("u1", None, None, Some(true)).unwrap();
        assert!(u.active && u.disabled_reason.is_none());
        assert!(u.last_activity_ms() > later - 1_000);
    }
}
