//! PostgreSQL account policy and session maintenance.

use diesel::prelude::*;
use diesel::sql_types::{BigInt, Nullable, Text};

use super::{PgControlStore, PostgresError, SESSION_COLUMNS, SessionRow, USER_COLUMNS, UserRow};
use crate::auth::{Failure, LOCKED_UNTIL_UNLOCKED, Session, User};

#[derive(QueryableByName)]
struct IdRow {
    #[diesel(sql_type = Text)]
    id: String,
}

#[derive(QueryableByName)]
struct HashRow {
    #[diesel(sql_type = Nullable<Text>)]
    hash: Option<String>,
}

#[derive(QueryableByName)]
struct FailureRow {
    #[diesel(sql_type = BigInt)]
    count: i64,
    #[diesel(sql_type = Nullable<BigInt>)]
    from_ms: Option<i64>,
}

impl PgControlStore {
    pub fn password_hash(
        &mut self,
        email: &str,
    ) -> Result<Option<(User, Option<String>)>, PostgresError> {
        let Some(user) = self.user_by_email(email)? else {
            return Ok(None);
        };
        let row: HashRow =
            diesel::sql_query("SELECT password_hash AS hash FROM users WHERE id = $1")
                .bind::<Text, _>(&user.id)
                .get_result(&mut self.connection)?;
        Ok(Some((user, row.hash)))
    }

    pub fn rehash_password(&mut self, id: &str, hash: &str) -> Result<(), PostgresError> {
        diesel::sql_query(
            "UPDATE users SET password_hash = $2 WHERE id = $1 AND password_hash IS NOT NULL",
        )
        .bind::<Text, _>(id)
        .bind::<Text, _>(hash)
        .execute(&mut self.connection)?;
        Ok(())
    }

    pub fn set_password(
        &mut self,
        id: &str,
        hash: Option<&str>,
        must_change: bool,
        keep: usize,
    ) -> Result<(), PostgresError> {
        self.connection.transaction::<(), PostgresError, _>(|connection| {
            let now = crate::sqlite::now_ms();
            let changed = diesel::sql_query(
                "UPDATE users SET password_hash = $2, updated_at_ms = $3,
                 password_changed_at_ms = CASE WHEN $2::text IS NULL THEN NULL ELSE $3 END,
                 must_change_password = $4 WHERE id = $1"
            ).bind::<Text, _>(id).bind::<Nullable<Text>, _>(hash)
            .bind::<BigInt, _>(now).bind::<BigInt, _>(i64::from(must_change && hash.is_some()))
            .execute(connection)?;
            if changed == 0 { return Err(PostgresError::NotFound(format!("user {id}"))); }
            if let Some(hash) = hash {
                diesel::sql_query("INSERT INTO password_history (user_id, hash, set_at_ms) VALUES ($1,$2,$3)")
                    .bind::<Text, _>(id).bind::<Text, _>(hash).bind::<BigInt, _>(now).execute(connection)?;
                diesel::sql_query(
                    "DELETE FROM password_history WHERE user_id = $1 AND ctid NOT IN
                     (SELECT ctid FROM password_history WHERE user_id = $1 ORDER BY set_at_ms DESC, ctid DESC LIMIT $2)"
                ).bind::<Text, _>(id).bind::<BigInt, _>(keep.max(1) as i64).execute(connection)?;
            }
            Ok(())
        })
    }

    pub fn recent_password_hashes(
        &mut self,
        id: &str,
        n: usize,
    ) -> Result<Vec<String>, PostgresError> {
        let current: Option<HashRow> =
            diesel::sql_query("SELECT password_hash AS hash FROM users WHERE id = $1")
                .bind::<Text, _>(id)
                .get_result(&mut self.connection)
                .optional()?;
        let history: Vec<HashRow> = diesel::sql_query(
            "SELECT hash FROM password_history WHERE user_id = $1 ORDER BY set_at_ms DESC, ctid DESC LIMIT $2"
        ).bind::<Text, _>(id).bind::<BigInt, _>(n as i64).load(&mut self.connection)?;
        let mut out = Vec::new();
        for hash in current
            .and_then(|r| r.hash)
            .into_iter()
            .chain(history.into_iter().filter_map(|r| r.hash))
        {
            if !out.contains(&hash) {
                out.push(hash);
            }
        }
        out.truncate(n.max(1));
        Ok(out)
    }

    pub fn note_login_failure(
        &mut self,
        id: &str,
        window_ms: i64,
        max: i64,
        lock_ms: i64,
    ) -> Result<Failure, PostgresError> {
        self.connection.transaction::<Failure, PostgresError, _>(|connection| {
            let now = crate::sqlite::now_ms();
            let row: FailureRow = diesel::sql_query(
                "SELECT failed_logins AS count, failed_window_from_ms AS from_ms FROM users WHERE id = $1 FOR UPDATE"
            ).bind::<Text, _>(id).get_result(connection)?;
            let (count, from) = match row.from_ms {
                Some(from) if now - from <= window_ms => (row.count + 1, from),
                _ => (1, now),
            };
            let locked_now = max > 0 && count >= max;
            let until = if lock_ms > 0 { now.saturating_add(lock_ms) } else { LOCKED_UNTIL_UNLOCKED };
            diesel::sql_query(
                "UPDATE users SET failed_logins = $2, failed_window_from_ms = $3,
                 failed_since_login = failed_since_login + 1,
                 locked_until_ms = CASE WHEN $4 THEN $5 ELSE locked_until_ms END WHERE id = $1"
            ).bind::<Text, _>(id).bind::<BigInt, _>(if locked_now {0} else {count})
            .bind::<Nullable<BigInt>, _>((!locked_now).then_some(from))
            .bind::<diesel::sql_types::Bool, _>(locked_now).bind::<BigInt, _>(until).execute(connection)?;
            Ok(Failure { count, locked_now })
        })
    }

    pub fn note_refused_login(&mut self, id: &str) -> Result<(), PostgresError> {
        diesel::sql_query(
            "UPDATE users SET failed_since_login = failed_since_login + 1 WHERE id = $1",
        )
        .bind::<Text, _>(id)
        .execute(&mut self.connection)?;
        Ok(())
    }

    pub fn unlock_user(&mut self, id: &str) -> Result<User, PostgresError> {
        let changed = diesel::sql_query(
            "UPDATE users SET locked_until_ms = NULL, failed_logins = 0,
             failed_window_from_ms = NULL, updated_at_ms = $2 WHERE id = $1",
        )
        .bind::<Text, _>(id)
        .bind::<BigInt, _>(crate::sqlite::now_ms())
        .execute(&mut self.connection)?;
        if changed == 0 {
            return Err(PostgresError::NotFound(format!("user {id}")));
        }
        self.user(id)?
            .ok_or_else(|| PostgresError::NotFound(format!("user {id}")))
    }

    pub fn set_account_expiry(
        &mut self,
        id: &str,
        expires_at_ms: Option<i64>,
    ) -> Result<User, PostgresError> {
        let changed = diesel::sql_query(
            "UPDATE users SET expires_at_ms = $2, updated_at_ms = $3 WHERE id = $1",
        )
        .bind::<Text, _>(id)
        .bind::<Nullable<BigInt>, _>(expires_at_ms)
        .bind::<BigInt, _>(crate::sqlite::now_ms())
        .execute(&mut self.connection)?;
        if changed == 0 {
            return Err(PostgresError::NotFound(format!("user {id}")));
        }
        self.user(id)?
            .ok_or_else(|| PostgresError::NotFound(format!("user {id}")))
    }

    pub fn note_login(&mut self, id: &str) -> Result<(), PostgresError> {
        diesel::sql_query("UPDATE users SET last_login_at_ms = $2 WHERE id = $1")
            .bind::<Text, _>(id)
            .bind::<BigInt, _>(crate::sqlite::now_ms())
            .execute(&mut self.connection)?;
        Ok(())
    }

    pub fn revoke_api_tokens(&mut self, id: &str, now: i64) -> Result<(), PostgresError> {
        diesel::sql_query(
            "UPDATE api_tokens SET revoked_at_ms = $2 WHERE user_id = $1 AND revoked_at_ms IS NULL",
        )
        .bind::<Text, _>(id)
        .bind::<BigInt, _>(now)
        .execute(&mut self.connection)?;
        Ok(())
    }

    pub fn revoke_user_tokens(&mut self, id: &str) -> Result<(), PostgresError> {
        let now = crate::sqlite::now_ms();
        diesel::sql_query("UPDATE users SET tokens_valid_from_ms = $2 WHERE id = $1")
            .bind::<Text, _>(id)
            .bind::<BigInt, _>(now)
            .execute(&mut self.connection)?;
        self.revoke_api_tokens(id, now)?;
        self.end_user_sessions(id, "revoked", None)?;
        Ok(())
    }
    pub fn set_must_change_password(&mut self, id: &str, must: bool) -> Result<(), PostgresError> {
        diesel::sql_query("UPDATE users SET must_change_password = $2 WHERE id = $1")
            .bind::<Text, _>(id)
            .bind::<BigInt, _>(i64::from(must))
            .execute(&mut self.connection)?;
        Ok(())
    }

    pub fn touch_sessions(&mut self, seen: &[(String, i64)]) -> Result<(), PostgresError> {
        self.connection
            .transaction::<(), PostgresError, _>(|connection| {
                for (id, at) in seen {
                    diesel::sql_query(
                        "UPDATE sessions SET last_seen_ms = greatest(last_seen_ms, $2)
                     WHERE id = $1 AND ended_at_ms IS NULL",
                    )
                    .bind::<Text, _>(id)
                    .bind::<BigInt, _>(*at)
                    .execute(connection)?;
                }
                Ok(())
            })
    }

    pub fn end_user_sessions(
        &mut self,
        user_id: &str,
        reason: &str,
        except: Option<&str>,
    ) -> Result<Vec<String>, PostgresError> {
        let rows: Vec<IdRow> = diesel::sql_query(
            "UPDATE sessions SET ended_at_ms = $2, end_reason = $3
             WHERE user_id = $1 AND ended_at_ms IS NULL
             AND ($4::text IS NULL OR id <> $4) RETURNING id",
        )
        .bind::<Text, _>(user_id)
        .bind::<BigInt, _>(crate::sqlite::now_ms())
        .bind::<Text, _>(reason)
        .bind::<Nullable<Text>, _>(except)
        .load(&mut self.connection)?;
        Ok(rows.into_iter().map(|row| row.id).collect())
    }

    pub fn end_stale_sessions(
        &mut self,
        idle_before_ms: i64,
        admin_idle_before_ms: i64,
    ) -> Result<Vec<Session>, PostgresError> {
        let now = crate::sqlite::now_ms();
        self.connection
            .transaction::<Vec<Session>, PostgresError, _>(|connection| {
                let rows: Vec<SessionRow> = diesel::sql_query(format!(
                    "SELECT {SESSION_COLUMNS} FROM sessions s
                 JOIN users u ON u.id = s.user_id
                 WHERE s.ended_at_ms IS NULL
                   AND (s.expires_at_ms <= $1
                        OR s.last_seen_ms < CASE WHEN u.role = 'admin'
                                                THEN $3 ELSE $2 END)
                 FOR UPDATE OF s"
                ))
                .bind::<BigInt, _>(now)
                .bind::<BigInt, _>(idle_before_ms)
                .bind::<BigInt, _>(admin_idle_before_ms)
                .load(connection)?;
                let mut ended = Vec::with_capacity(rows.len());
                for row in rows {
                    let mut session = Session::from(row);
                    let reason = if session.expires_at_ms <= now {
                        "expired"
                    } else {
                        "idle"
                    };
                    diesel::sql_query(
                        "UPDATE sessions SET ended_at_ms = $2, end_reason = $3 WHERE id = $1",
                    )
                    .bind::<Text, _>(&session.id)
                    .bind::<BigInt, _>(now)
                    .bind::<Text, _>(reason)
                    .execute(connection)?;
                    session.ended_at_ms = Some(now);
                    session.end_reason = Some(reason.into());
                    ended.push(session);
                }
                diesel::sql_query(
                    "DELETE FROM sessions WHERE ended_at_ms IS NOT NULL AND ended_at_ms < $1",
                )
                .bind::<BigInt, _>(now - 90 * 86_400_000)
                .execute(connection)?;
                Ok(ended)
            })
    }

    pub fn disable_expired(&mut self, now: i64) -> Result<Vec<User>, PostgresError> {
        let rows: Vec<UserRow> = diesel::sql_query(format!(
            "SELECT {USER_COLUMNS} FROM users
             WHERE active = 1 AND expires_at_ms IS NOT NULL AND expires_at_ms <= $1"
        ))
        .bind::<BigInt, _>(now)
        .load(&mut self.connection)?;
        let users: Vec<User> = rows.into_iter().map(User::from).collect();
        for user in &users {
            self.disable_account(&user.id, "expired", now)?;
        }
        Ok(users)
    }

    pub fn disable_inactive(
        &mut self,
        cutoff_ms: i64,
        exempt: &[String],
    ) -> Result<Vec<User>, PostgresError> {
        let users: Vec<User> = self
            .users()?
            .into_iter()
            .filter(|user| user.active && user.last_activity_ms() < cutoff_ms)
            .filter(|user| {
                !exempt
                    .iter()
                    .any(|email| email.trim().eq_ignore_ascii_case(&user.email))
            })
            .collect();
        let now = crate::sqlite::now_ms();
        for user in &users {
            self.disable_account(&user.id, "inactivity", now)?;
        }
        Ok(users)
    }

    fn disable_account(&mut self, id: &str, reason: &str, now: i64) -> Result<(), PostgresError> {
        self.connection
            .transaction::<(), PostgresError, _>(|connection| {
                diesel::sql_query(
                    "UPDATE users SET active = 0, disabled_reason = $2, updated_at_ms = $3,
                 tokens_valid_from_ms = $3 WHERE id = $1 AND active = 1",
                )
                .bind::<Text, _>(id)
                .bind::<Text, _>(reason)
                .bind::<BigInt, _>(now)
                .execute(connection)?;
                diesel::sql_query(
                    "UPDATE sessions SET ended_at_ms = $2, end_reason = 'disabled'
                 WHERE user_id = $1 AND ended_at_ms IS NULL",
                )
                .bind::<Text, _>(id)
                .bind::<BigInt, _>(now)
                .execute(connection)?;
                diesel::sql_query(
                    "UPDATE api_tokens SET revoked_at_ms = $2
                 WHERE user_id = $1 AND revoked_at_ms IS NULL",
                )
                .bind::<Text, _>(id)
                .bind::<BigInt, _>(now)
                .execute(connection)?;
                Ok(())
            })
    }
}
