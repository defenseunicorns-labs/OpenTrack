//! PostgreSQL account policy and session maintenance.

use diesel::prelude::*;
use diesel::sql_types::{BigInt, Nullable, Text};

use super::{PgControlStore, PostgresError, SESSION_COLUMNS, SessionRow, USER_COLUMNS, UserRow};
use crate::auth::{Session, User};

#[derive(QueryableByName)]
struct IdRow {
    #[diesel(sql_type = Text)]
    id: String,
}

impl PgControlStore {
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
