//! PostgreSQL control store connection and transactional schema migration.
//! The application uses Diesel's safe libpq wrapper, so password authentication
//! and TLS use the libpq/OpenSSL stack rather than a second Rust crypto stack.

use diesel::connection::SimpleConnection;
use diesel::pg::PgConnection;
use diesel::prelude::*;
use diesel::sql_types::{BigInt, Nullable, Text};
use serde_json::Value;

use crate::Decision;

#[path = "postgres_audit.rs"]
mod audit_queries;
#[path = "postgres_auth_maintenance.rs"]
mod auth_maintenance;
#[path = "postgres_correlation.rs"]
mod correlation;
#[path = "postgres_graph.rs"]
mod graph;
#[path = "postgres_groups.rs"]
mod groups;
#[path = "postgres_plugins.rs"]
mod plugins;
#[path = "postgres_registry.rs"]
mod registry;
#[path = "postgres_schema.rs"]
mod schema;
#[path = "postgres_sources.rs"]
mod sources;
#[path = "postgres_sync.rs"]
mod sync;
#[path = "postgres_uid.rs"]
mod uid;
use crate::audit::{AuditEvent, GENESIS, row_hash};
use crate::auth::{ApiToken, NewSession, NewUser, Session, User};

const INITIAL_SCHEMA: &str = include_str!("../postgres/0001_init.sql");
const SUPPORTED_SCHEMA: i64 = 1;

#[derive(Debug, thiserror::Error)]
pub enum PostgresError {
    #[error("PostgreSQL connection failed: {0}")]
    Connection(#[from] diesel::ConnectionError),
    #[error("PostgreSQL query failed: {0}")]
    Query(#[from] diesel::result::Error),
    #[error("invalid stored JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Uid(#[from] ot_core::UidError),
    #[error("PostgreSQL schema migration failed: {0}")]
    Migration(String),
    #[error("PostgreSQL schema version {found} is newer than this build supports ({supported})")]
    TooNew { found: i64, supported: i64 },
    #[error("PostgreSQL control schema version is missing")]
    MissingVersion,
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("not found: {0}")]
    NotFound(String),
}

#[derive(QueryableByName)]
struct TableName {
    #[diesel(sql_type = Nullable<Text>)]
    name: Option<String>,
}

#[derive(QueryableByName)]
struct SchemaVersion {
    #[diesel(sql_type = BigInt)]
    version: i64,
}

#[derive(QueryableByName)]
struct SettingsRow {
    #[diesel(sql_type = Text)]
    settings: String,
}

#[derive(QueryableByName)]
struct DecisionId {
    #[diesel(sql_type = BigInt)]
    id: i64,
}

#[derive(QueryableByName)]
struct AuditHead {
    #[diesel(sql_type = BigInt)]
    seq: i64,
    #[diesel(sql_type = Text)]
    hash: String,
}

#[derive(QueryableByName)]
struct ApiTokenRow {
    #[diesel(sql_type = Text)]
    jti: String,
    #[diesel(sql_type = Text)]
    name: String,
    #[diesel(sql_type = Text)]
    user_id: String,
    #[diesel(sql_type = Text)]
    user_email: String,
    #[diesel(sql_type = Text)]
    created_by: String,
    #[diesel(sql_type = BigInt)]
    created_at_ms: i64,
    #[diesel(sql_type = BigInt)]
    expires_at_ms: i64,
    #[diesel(sql_type = Nullable<BigInt>)]
    revoked_at_ms: Option<i64>,
}

impl From<ApiTokenRow> for ApiToken {
    fn from(row: ApiTokenRow) -> Self {
        Self {
            jti: row.jti,
            name: row.name,
            user_id: row.user_id,
            user_email: row.user_email,
            created_by: row.created_by,
            created_at_ms: row.created_at_ms,
            expires_at_ms: row.expires_at_ms,
            revoked_at_ms: row.revoked_at_ms,
        }
    }
}

fn append_audit(
    connection: &mut PgConnection,
    event: &AuditEvent,
    at_ms: i64,
    decision_id: Option<i64>,
) -> Result<i64, PostgresError> {
    // One advisory transaction lock serializes audit appends from every pod.
    let _: SchemaVersion =
        diesel::sql_query("SELECT 1::bigint AS version FROM pg_advisory_xact_lock(6869400123456)")
            .get_result(connection)?;
    let head: AuditHead = diesel::sql_query(
        "SELECT coalesce((SELECT seq FROM audit ORDER BY seq DESC LIMIT 1),
                         (SELECT seq FROM audit_anchor WHERE id = 1), 0)::bigint AS seq,
                coalesce((SELECT hash FROM audit ORDER BY seq DESC LIMIT 1),
                         (SELECT hash FROM audit_anchor WHERE id = 1), $1)::text AS hash",
    )
    .bind::<Text, _>(GENESIS)
    .get_result(connection)?;
    let seq = head.seq + 1;
    let outcome = if event.success { "success" } else { "failure" };
    let detail = event.detail.to_string();
    let hash = row_hash(
        &head.hash,
        seq,
        at_ms,
        &event.actor,
        &event.op,
        outcome,
        event.ip.as_deref(),
        &detail,
        decision_id,
    );
    diesel::sql_query(
        "INSERT INTO audit (seq, at_ms, actor, op, outcome, ip, detail,
         decision_id, prev_hash, hash) VALUES
         ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind::<BigInt, _>(seq)
    .bind::<BigInt, _>(at_ms)
    .bind::<Text, _>(&event.actor)
    .bind::<Text, _>(&event.op)
    .bind::<Text, _>(outcome)
    .bind::<Nullable<Text>, _>(event.ip.as_deref())
    .bind::<Text, _>(detail)
    .bind::<Nullable<BigInt>, _>(decision_id)
    .bind::<Text, _>(head.hash)
    .bind::<Text, _>(hash)
    .execute(connection)?;
    Ok(seq)
}

#[derive(QueryableByName)]
struct VersionText {
    #[diesel(sql_type = Text)]
    version: String,
}

#[derive(QueryableByName)]
struct UserRow {
    #[diesel(sql_type = Text)]
    id: String,
    #[diesel(sql_type = Text)]
    email: String,
    #[diesel(sql_type = Text)]
    name: String,
    #[diesel(sql_type = Text)]
    role: String,
    #[diesel(sql_type = BigInt)]
    active: i64,
    #[diesel(sql_type = Text)]
    origin: String,
    #[diesel(sql_type = BigInt)]
    has_password: i64,
    #[diesel(sql_type = BigInt)]
    created_at_ms: i64,
    #[diesel(sql_type = BigInt)]
    updated_at_ms: i64,
    #[diesel(sql_type = Nullable<BigInt>)]
    last_login_at_ms: Option<i64>,
    #[diesel(sql_type = BigInt)]
    tokens_valid_from_ms: i64,
    #[diesel(sql_type = Nullable<BigInt>)]
    password_changed_at_ms: Option<i64>,
    #[diesel(sql_type = BigInt)]
    must_change_password: i64,
    #[diesel(sql_type = BigInt)]
    failed_logins: i64,
    #[diesel(sql_type = Nullable<BigInt>)]
    locked_until_ms: Option<i64>,
    #[diesel(sql_type = Nullable<BigInt>)]
    active_since_ms: Option<i64>,
    #[diesel(sql_type = Nullable<Text>)]
    disabled_reason: Option<String>,
    #[diesel(sql_type = Nullable<BigInt>)]
    expires_at_ms: Option<i64>,
}

#[derive(QueryableByName)]
struct SessionRow {
    #[diesel(sql_type = Text)]
    id: String,
    #[diesel(sql_type = Text)]
    user_id: String,
    #[diesel(sql_type = Text)]
    user_email: String,
    #[diesel(sql_type = BigInt)]
    created_at_ms: i64,
    #[diesel(sql_type = BigInt)]
    last_seen_ms: i64,
    #[diesel(sql_type = BigInt)]
    expires_at_ms: i64,
    #[diesel(sql_type = Nullable<Text>)]
    ip: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    user_agent: Option<String>,
    #[diesel(sql_type = Nullable<BigInt>)]
    ended_at_ms: Option<i64>,
    #[diesel(sql_type = Nullable<Text>)]
    end_reason: Option<String>,
    #[diesel(sql_type = Nullable<BigInt>)]
    prev_login_at_ms: Option<i64>,
    #[diesel(sql_type = BigInt)]
    failed_before: i64,
}

impl From<SessionRow> for Session {
    fn from(row: SessionRow) -> Self {
        Self {
            id: row.id,
            user_id: row.user_id,
            user_email: row.user_email,
            created_at_ms: row.created_at_ms,
            last_seen_ms: row.last_seen_ms,
            expires_at_ms: row.expires_at_ms,
            ip: row.ip,
            user_agent: row.user_agent,
            ended_at_ms: row.ended_at_ms,
            end_reason: row.end_reason,
            prev_login_at_ms: row.prev_login_at_ms,
            failed_before: row.failed_before,
        }
    }
}

impl From<UserRow> for User {
    fn from(row: UserRow) -> Self {
        Self {
            id: row.id,
            email: row.email,
            name: row.name,
            role: row.role,
            active: row.active != 0,
            origin: row.origin,
            has_password: row.has_password != 0,
            created_at_ms: row.created_at_ms,
            updated_at_ms: row.updated_at_ms,
            last_login_at_ms: row.last_login_at_ms,
            tokens_valid_from_ms: row.tokens_valid_from_ms,
            password_changed_at_ms: row.password_changed_at_ms,
            must_change_password: row.must_change_password != 0,
            failed_logins: row.failed_logins,
            locked_until_ms: row.locked_until_ms,
            active_since_ms: row.active_since_ms,
            disabled_reason: row.disabled_reason,
            expires_at_ms: row.expires_at_ms,
        }
    }
}

const USER_COLUMNS: &str = "id, email, name, role, active, origin,
    (CASE WHEN password_hash IS NULL THEN 0 ELSE 1 END)::bigint AS has_password,
    created_at_ms, updated_at_ms, last_login_at_ms, tokens_valid_from_ms,
    password_changed_at_ms, must_change_password, failed_logins, locked_until_ms,
    active_since_ms, disabled_reason, expires_at_ms";
const SESSION_COLUMNS: &str = "s.id, s.user_id, u.email AS user_email,
    s.created_at_ms, s.last_seen_ms, s.expires_at_ms, s.ip, s.user_agent,
    s.ended_at_ms, s.end_reason, s.prev_login_at_ms, s.failed_before";

fn record_decision(
    connection: &mut PgConnection,
    decision: &Decision,
) -> Result<i64, PostgresError> {
    let now = crate::sqlite::now_ms();
    let row: DecisionId = diesel::sql_query(
        "INSERT INTO decisions (at_ms, actor, op, reason, evidence, before, after)
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id",
    )
    .bind::<BigInt, _>(now)
    .bind::<Text, _>(&decision.actor)
    .bind::<Text, _>(&decision.op)
    .bind::<Nullable<Text>, _>(decision.reason.as_deref())
    .bind::<Text, _>(decision.evidence.to_string())
    .bind::<Nullable<Text>, _>(decision.before.as_ref().map(Value::to_string))
    .bind::<Nullable<Text>, _>(decision.after.as_ref().map(Value::to_string))
    .get_result(connection)?;
    let event = AuditEvent::new(&decision.actor, &decision.op).detail(serde_json::json!({
        "reason": decision.reason,
        "evidence": decision.evidence,
        "before": decision.before,
        "after": decision.after,
    }));
    append_audit(connection, &event, now, Some(row.id))?;
    Ok(row.id)
}

pub struct PgControlStore {
    connection: PgConnection,
}

fn count_rows(connection: &mut PgConnection, sql: &'static str) -> Result<i64, PostgresError> {
    let row: SchemaVersion = diesel::sql_query(sql).get_result(connection)?;
    Ok(row.version)
}

fn note_count(
    out: &mut Vec<String>,
    connection: &mut PgConnection,
    sql: &'static str,
    label: &'static str,
) -> Result<(), PostgresError> {
    let n = count_rows(connection, sql)?;
    if n > 0 {
        out.push(format!("{n} {label}"));
    }
    Ok(())
}

impl PgControlStore {
    /// Connect and migrate a fresh database. The URL must be assembled from
    /// trusted configuration and never logged because it contains a password.
    pub fn connect(database_url: &str) -> Result<Self, PostgresError> {
        let connection = PgConnection::establish(database_url)?;
        let mut store = Self { connection };
        store.migrate()?;
        Ok(store)
    }

    pub fn schema_version(&mut self) -> Result<i64, PostgresError> {
        let versions: Vec<SchemaVersion> = diesel::sql_query(
            "SELECT version FROM control_schema_version ORDER BY version DESC LIMIT 1",
        )
        .load(&mut self.connection)?;
        versions
            .first()
            .map(|v| v.version)
            .ok_or(PostgresError::MissingVersion)
    }

    pub fn users(&mut self) -> Result<Vec<User>, PostgresError> {
        let rows: Vec<UserRow> =
            diesel::sql_query(format!("SELECT {USER_COLUMNS} FROM users ORDER BY email"))
                .load(&mut self.connection)?;
        Ok(rows.into_iter().map(User::from).collect())
    }

    pub fn user_count(&mut self) -> Result<i64, PostgresError> {
        let row: SchemaVersion = diesel::sql_query("SELECT count(*) AS version FROM users")
            .get_result(&mut self.connection)?;
        Ok(row.version)
    }

    pub fn config_present(&mut self) -> Result<Vec<String>, PostgresError> {
        let mut out = Vec::new();
        note_count(
            &mut out,
            &mut self.connection,
            "SELECT count(*) AS version FROM sources",
            "source(s)",
        )?;
        note_count(
            &mut out,
            &mut self.connection,
            "SELECT count(*) AS version FROM schema_versions WHERE version > 1",
            "output schema version(s) beyond the built-in one",
        )?;
        note_count(
            &mut out,
            &mut self.connection,
            "SELECT count(*) AS version FROM correlation_settings",
            "saved correlation settings",
        )?;
        note_count(
            &mut out,
            &mut self.connection,
            "SELECT count(*) AS version FROM app_settings",
            "saved instance settings (Settings → General)",
        )?;
        note_count(
            &mut out,
            &mut self.connection,
            "SELECT count(*) AS version FROM auth_settings",
            "saved sign-in settings (Settings → Security)",
        )?;
        let users = count_rows(
            &mut self.connection,
            "SELECT count(*) AS version FROM users",
        )?;
        let admins = count_rows(
            &mut self.connection,
            "SELECT count(*) AS version FROM users WHERE role = 'admin'",
        )?;
        if users > 1 || users == 1 && admins == 0 {
            out.push(format!(
                "{users} account(s) (an empty node has at most its first admin)"
            ));
        }
        note_count(
            &mut out,
            &mut self.connection,
            "SELECT count(*) AS version FROM api_tokens",
            "API token(s)",
        )?;
        note_count(
            &mut out,
            &mut self.connection,
            "SELECT count(*) AS version FROM registry_entities",
            "registry entit(ies)",
        )?;
        note_count(
            &mut out,
            &mut self.connection,
            "SELECT count(*) AS version FROM plugins",
            "plugin(s)",
        )?;
        if count_rows(
            &mut self.connection,
            "SELECT count(*) AS version FROM decisions WHERE op = 'import_config'",
        )? > 0
        {
            out.push("an imported configuration".into());
        }
        Ok(out)
    }

    pub fn user(&mut self, id: &str) -> Result<Option<User>, PostgresError> {
        let rows: Vec<UserRow> =
            diesel::sql_query(format!("SELECT {USER_COLUMNS} FROM users WHERE id = $1"))
                .bind::<Text, _>(id)
                .load(&mut self.connection)?;
        Ok(rows.into_iter().next().map(User::from))
    }

    pub fn user_by_email(&mut self, email: &str) -> Result<Option<User>, PostgresError> {
        let rows: Vec<UserRow> = diesel::sql_query(format!(
            "SELECT {USER_COLUMNS} FROM users WHERE lower(email) = lower($1)"
        ))
        .bind::<Text, _>(email)
        .load(&mut self.connection)?;
        Ok(rows.into_iter().next().map(User::from))
    }

    pub fn create_user(&mut self, user: &NewUser<'_>) -> Result<User, PostgresError> {
        let email = user.email.trim();
        if self.user_by_email(email)?.is_some() {
            return Err(PostgresError::Conflict(email.into()));
        }
        let now = crate::sqlite::now_ms();
        self.connection
            .transaction::<(), PostgresError, _>(|connection| {
                diesel::sql_query(
                    "INSERT INTO users (id, email, name, role, password_hash, origin,
                 created_at_ms, updated_at_ms, password_changed_at_ms, active_since_ms)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $7,
                         CASE WHEN $5 IS NULL THEN NULL ELSE $7 END, $7)",
                )
                .bind::<Text, _>(user.id)
                .bind::<Text, _>(email)
                .bind::<Text, _>(user.name.trim())
                .bind::<Text, _>(user.role)
                .bind::<Nullable<Text>, _>(user.password_hash)
                .bind::<Text, _>(user.origin)
                .bind::<BigInt, _>(now)
                .execute(connection)?;
                if let Some(hash) = user.password_hash {
                    diesel::sql_query(
                    "INSERT INTO password_history (user_id, hash, set_at_ms) VALUES ($1, $2, $3)",
                )
                .bind::<Text, _>(user.id)
                .bind::<Text, _>(hash)
                .bind::<BigInt, _>(now)
                .execute(connection)?;
                }
                Ok(())
            })?;
        self.user(user.id)?
            .ok_or_else(|| PostgresError::NotFound(user.id.into()))
    }

    pub fn update_user(
        &mut self,
        id: &str,
        name: Option<&str>,
        role: Option<&str>,
        active: Option<bool>,
    ) -> Result<User, PostgresError> {
        let now = crate::sqlite::now_ms();
        let active = active.map(i64::from);
        let changed = diesel::sql_query(
            "UPDATE users SET name = coalesce($2, name), role = coalesce($3, role),
             updated_at_ms = $5,
             tokens_valid_from_ms = CASE WHEN $4 = 0 THEN $5 ELSE tokens_valid_from_ms END,
             active_since_ms = CASE WHEN $4 = 1 AND active = 0 THEN $5 ELSE active_since_ms END,
             disabled_reason = CASE WHEN $4 = 1 THEN NULL ELSE disabled_reason END,
             active = coalesce($4, active) WHERE id = $1",
        )
        .bind::<Text, _>(id)
        .bind::<Nullable<Text>, _>(name.map(str::trim))
        .bind::<Nullable<Text>, _>(role)
        .bind::<Nullable<BigInt>, _>(active)
        .bind::<BigInt, _>(now)
        .execute(&mut self.connection)?;
        if changed == 0 {
            return Err(PostgresError::NotFound(id.into()));
        }
        if active == Some(0) {
            diesel::sql_query(
                "UPDATE sessions SET ended_at_ms = $2, end_reason = 'disabled'
                 WHERE user_id = $1 AND ended_at_ms IS NULL",
            )
            .bind::<Text, _>(id)
            .bind::<BigInt, _>(now)
            .execute(&mut self.connection)?;
            diesel::sql_query(
                "UPDATE api_tokens SET revoked_at_ms = $2
                 WHERE user_id = $1 AND revoked_at_ms IS NULL",
            )
            .bind::<Text, _>(id)
            .bind::<BigInt, _>(now)
            .execute(&mut self.connection)?;
        }
        self.user(id)?
            .ok_or_else(|| PostgresError::NotFound(id.into()))
    }

    pub fn record(&mut self, decision: &Decision) -> Result<i64, PostgresError> {
        self.connection
            .transaction::<i64, PostgresError, _>(|connection| {
                record_decision(connection, decision)
            })
    }

    pub fn audit(&mut self, event: &AuditEvent) -> Result<i64, PostgresError> {
        let at_ms = crate::sqlite::now_ms();
        self.connection
            .transaction::<i64, PostgresError, _>(|connection| {
                append_audit(connection, event, at_ms, None)
            })
    }

    pub fn audit_head(&mut self) -> Result<(i64, String), PostgresError> {
        let head: AuditHead = diesel::sql_query(
            "SELECT coalesce((SELECT seq FROM audit ORDER BY seq DESC LIMIT 1),
                             (SELECT seq FROM audit_anchor WHERE id = 1), 0)::bigint AS seq,
                    coalesce((SELECT hash FROM audit ORDER BY seq DESC LIMIT 1),
                             (SELECT hash FROM audit_anchor WHERE id = 1), $1)::text AS hash",
        )
        .bind::<Text, _>(GENESIS)
        .get_result(&mut self.connection)?;
        Ok((head.seq, head.hash))
    }

    pub fn note_login_success(&mut self, id: &str) -> Result<(Option<i64>, i64), PostgresError> {
        #[derive(QueryableByName)]
        struct PreviousLogin {
            #[diesel(sql_type = Nullable<BigInt>)]
            last_login_at_ms: Option<i64>,
            #[diesel(sql_type = BigInt)]
            failed_since_login: i64,
        }
        let now = crate::sqlite::now_ms();
        self.connection
            .transaction::<(Option<i64>, i64), PostgresError, _>(|connection| {
                let previous: PreviousLogin = diesel::sql_query(
                    "SELECT last_login_at_ms, failed_since_login FROM users
                     WHERE id = $1 FOR UPDATE",
                )
                .bind::<Text, _>(id)
                .get_result(connection)?;
                diesel::sql_query(
                    "UPDATE users SET last_login_at_ms = $2, failed_logins = 0,
                     failed_window_from_ms = NULL, failed_since_login = 0,
                     locked_until_ms = NULL WHERE id = $1",
                )
                .bind::<Text, _>(id)
                .bind::<BigInt, _>(now)
                .execute(connection)?;
                Ok((previous.last_login_at_ms, previous.failed_since_login))
            })
    }

    pub fn create_session(&mut self, session: &NewSession<'_>) -> Result<(), PostgresError> {
        let now = crate::sqlite::now_ms();
        let user_agent = session
            .user_agent
            .map(|agent| agent.chars().take(512).collect::<String>());
        diesel::sql_query(
            "INSERT INTO sessions (id, user_id, created_at_ms, last_seen_ms,
             expires_at_ms, ip, user_agent, prev_login_at_ms, failed_before)
             VALUES ($1, $2, $3, $3, $4, $5, $6, $7, $8)",
        )
        .bind::<Text, _>(session.id)
        .bind::<Text, _>(session.user_id)
        .bind::<BigInt, _>(now)
        .bind::<BigInt, _>(session.expires_at_ms)
        .bind::<Nullable<Text>, _>(session.ip)
        .bind::<Nullable<Text>, _>(user_agent)
        .bind::<Nullable<BigInt>, _>(session.prev_login_at_ms)
        .bind::<BigInt, _>(session.failed_before)
        .execute(&mut self.connection)?;
        Ok(())
    }

    pub fn session(&mut self, id: &str) -> Result<Option<Session>, PostgresError> {
        let rows: Vec<SessionRow> = diesel::sql_query(format!(
            "SELECT {SESSION_COLUMNS} FROM sessions s JOIN users u ON u.id = s.user_id
             WHERE s.id = $1"
        ))
        .bind::<Text, _>(id)
        .load(&mut self.connection)?;
        Ok(rows.into_iter().next().map(Session::from))
    }

    pub fn sessions(
        &mut self,
        user_id: Option<&str>,
        ended: bool,
        limit: usize,
    ) -> Result<Vec<Session>, PostgresError> {
        let rows: Vec<SessionRow> = diesel::sql_query(format!(
            "SELECT {SESSION_COLUMNS} FROM sessions s JOIN users u ON u.id = s.user_id
             WHERE ($1::text IS NULL OR s.user_id = $1)
               AND ($2::bigint = 1 OR (s.ended_at_ms IS NULL AND s.expires_at_ms > $3))
             ORDER BY s.created_at_ms DESC LIMIT $4"
        ))
        .bind::<Nullable<Text>, _>(user_id)
        .bind::<BigInt, _>(i64::from(ended))
        .bind::<BigInt, _>(crate::sqlite::now_ms())
        .bind::<BigInt, _>(limit as i64)
        .load(&mut self.connection)?;
        Ok(rows.into_iter().map(Session::from).collect())
    }

    pub fn end_session(&mut self, id: &str, reason: &str) -> Result<bool, PostgresError> {
        let changed = diesel::sql_query(
            "UPDATE sessions SET ended_at_ms = $2, end_reason = $3
             WHERE id = $1 AND ended_at_ms IS NULL",
        )
        .bind::<Text, _>(id)
        .bind::<BigInt, _>(crate::sqlite::now_ms())
        .bind::<Text, _>(reason)
        .execute(&mut self.connection)?;
        Ok(changed != 0)
    }

    pub fn revoke_token(&mut self, jti: &str, expires_at_ms: i64) -> Result<(), PostgresError> {
        diesel::sql_query(
            "INSERT INTO revoked_tokens (jti, expires_at_ms) VALUES ($1, $2)
             ON CONFLICT (jti) DO NOTHING",
        )
        .bind::<Text, _>(jti)
        .bind::<BigInt, _>(expires_at_ms)
        .execute(&mut self.connection)?;
        diesel::sql_query("DELETE FROM revoked_tokens WHERE expires_at_ms < $1")
            .bind::<BigInt, _>(crate::sqlite::now_ms())
            .execute(&mut self.connection)?;
        Ok(())
    }

    pub fn token_revoked(&mut self, jti: &str) -> Result<bool, PostgresError> {
        #[derive(QueryableByName)]
        struct Revocation {
            #[diesel(sql_type = diesel::sql_types::Bool)]
            revoked: bool,
        }
        let row: Revocation = diesel::sql_query(
            "SELECT (EXISTS (SELECT 1 FROM revoked_tokens WHERE jti = $1)
                    OR EXISTS (SELECT 1 FROM api_tokens
                               WHERE jti = $1 AND revoked_at_ms IS NOT NULL)) AS revoked",
        )
        .bind::<Text, _>(jti)
        .get_result(&mut self.connection)?;
        Ok(row.revoked)
    }

    pub fn add_api_token(
        &mut self,
        jti: &str,
        name: &str,
        user_id: &str,
        created_by: &str,
        expires_at_ms: i64,
    ) -> Result<(), PostgresError> {
        diesel::sql_query(
            "INSERT INTO api_tokens (jti, name, user_id, created_by, created_at_ms, expires_at_ms)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind::<Text, _>(jti)
        .bind::<Text, _>(name.trim())
        .bind::<Text, _>(user_id)
        .bind::<Text, _>(created_by)
        .bind::<BigInt, _>(crate::sqlite::now_ms())
        .bind::<BigInt, _>(expires_at_ms)
        .execute(&mut self.connection)?;
        Ok(())
    }

    pub fn api_tokens(&mut self) -> Result<Vec<ApiToken>, PostgresError> {
        let rows: Vec<ApiTokenRow> = diesel::sql_query(
            "SELECT t.jti, t.name, t.user_id, u.email AS user_email, t.created_by,
                    t.created_at_ms, t.expires_at_ms, t.revoked_at_ms
             FROM api_tokens t JOIN users u ON u.id = t.user_id
             ORDER BY t.created_at_ms DESC",
        )
        .load(&mut self.connection)?;
        Ok(rows.into_iter().map(ApiToken::from).collect())
    }

    pub fn revoke_api_token(&mut self, jti: &str) -> Result<(), PostgresError> {
        let n = diesel::sql_query(
            "UPDATE api_tokens SET revoked_at_ms = $2 WHERE jti = $1 AND revoked_at_ms IS NULL",
        )
        .bind::<Text, _>(jti)
        .bind::<BigInt, _>(crate::sqlite::now_ms())
        .execute(&mut self.connection)?;
        if n == 0 {
            return Err(PostgresError::NotFound(format!("live API token {jti}")));
        }
        Ok(())
    }

    pub fn auth_settings(&mut self) -> Result<Value, PostgresError> {
        let rows: Vec<SettingsRow> =
            diesel::sql_query("SELECT settings FROM auth_settings WHERE id = 1")
                .load(&mut self.connection)?;
        match rows.first() {
            Some(row) => Ok(serde_json::from_str(&row.settings)?),
            None => Ok(Value::Object(Default::default())),
        }
    }

    pub fn put_auth_settings(&mut self, settings: &Value) -> Result<(), PostgresError> {
        diesel::sql_query(
            "INSERT INTO auth_settings (id, settings, updated_at_ms) VALUES (1, $1, $2)
             ON CONFLICT(id) DO UPDATE SET settings = excluded.settings,
                 updated_at_ms = excluded.updated_at_ms",
        )
        .bind::<Text, _>(settings.to_string())
        .bind::<BigInt, _>(crate::sqlite::now_ms())
        .execute(&mut self.connection)?;
        Ok(())
    }

    pub fn app_settings(&mut self) -> Result<Value, PostgresError> {
        let rows: Vec<SettingsRow> =
            diesel::sql_query("SELECT settings FROM app_settings WHERE id = 1")
                .load(&mut self.connection)?;
        match rows.first() {
            Some(row) => Ok(serde_json::from_str(&row.settings)?),
            None => Ok(Value::Object(Default::default())),
        }
    }

    pub fn put_app_settings(
        &mut self,
        settings: &Value,
        actor: &str,
    ) -> Result<i64, PostgresError> {
        let mut evidence = settings.clone();
        if let Some(url) = evidence.get_mut("basemap_tiles_url")
            && url.as_str().is_some_and(|s| !s.is_empty())
        {
            *url = Value::String("(set)".into());
        }
        self.connection
            .transaction::<i64, PostgresError, _>(|connection| {
                let id = record_decision(
                    connection,
                    &Decision::new(actor, "app_settings").evidence(evidence),
                )?;
                diesel::sql_query(
                    "INSERT INTO app_settings (id, settings, updated_at_ms, decision_id)
                 VALUES (1, $1, $2, $3) ON CONFLICT(id) DO UPDATE SET
                 settings = excluded.settings, updated_at_ms = excluded.updated_at_ms,
                 decision_id = excluded.decision_id",
                )
                .bind::<Text, _>(settings.to_string())
                .bind::<BigInt, _>(crate::sqlite::now_ms())
                .bind::<BigInt, _>(id)
                .execute(connection)?;
                Ok(id)
            })
    }

    pub fn correlation_settings(&mut self) -> Result<Option<Value>, PostgresError> {
        let rows: Vec<SettingsRow> =
            diesel::sql_query("SELECT settings FROM correlation_settings WHERE id = 1")
                .load(&mut self.connection)?;
        rows.first()
            .map(|row| serde_json::from_str(&row.settings).map_err(PostgresError::from))
            .transpose()
    }

    pub fn save_correlation_settings(
        &mut self,
        settings: &Value,
        decision: &Decision,
    ) -> Result<i64, PostgresError> {
        self.connection
            .transaction::<i64, PostgresError, _>(|connection| {
                let id = record_decision(connection, decision)?;
                diesel::sql_query(
                    "INSERT INTO correlation_settings (id, settings, updated_at_ms, decision_id)
                     VALUES (1, $1, $2, $3) ON CONFLICT(id) DO UPDATE SET
                     settings = excluded.settings, updated_at_ms = excluded.updated_at_ms,
                     decision_id = excluded.decision_id",
                )
                .bind::<Text, _>(settings.to_string())
                .bind::<BigInt, _>(crate::sqlite::now_ms())
                .bind::<BigInt, _>(id)
                .execute(connection)?;
                Ok(id)
            })
    }

    pub fn correlation_version(&mut self) -> Result<String, PostgresError> {
        let row: VersionText = diesel::sql_query(
            "SELECT ((SELECT coalesce(max(updated_at_ms), 0) FROM correlation_settings)::text
                    || ':' ||
                    (SELECT count(*) FROM edges WHERE kind = 'DO_NOT_PAIR'
                     AND valid_to_ms IS NULL)::text || ':' ||
                    (SELECT coalesce(max(id), 0) FROM edges
                     WHERE kind = 'DO_NOT_PAIR')::text) AS version",
        )
        .get_result(&mut self.connection)?;
        Ok(row.version)
    }

    fn migrate(&mut self) -> Result<(), PostgresError> {
        let table: TableName =
            diesel::sql_query("SELECT to_regclass('public.control_schema_version')::text AS name")
                .get_result(&mut self.connection)?;
        if table.name.is_none() {
            self.connection
                .transaction::<_, diesel::result::Error, _>(|connection| {
                    connection.batch_execute(INITIAL_SCHEMA)
                })
                .map_err(|e| PostgresError::Migration(e.to_string()))?;
        }
        let found = self.schema_version()?;
        if found > SUPPORTED_SCHEMA {
            return Err(PostgresError::TooNew {
                found,
                supported: SUPPORTED_SCHEMA,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::PgControlStore;
    use diesel::prelude::*;
    use diesel::sql_types::Text;

    #[derive(QueryableByName)]
    struct EvidenceRow {
        #[diesel(sql_type = Text)]
        evidence: String,
    }

    #[derive(QueryableByName)]
    struct AuditRowForTest {
        #[diesel(sql_type = diesel::sql_types::BigInt)]
        seq: i64,
        #[diesel(sql_type = Text)]
        hash: String,
        #[diesel(sql_type = Text)]
        prev_hash: String,
        #[diesel(sql_type = Text)]
        op: String,
        #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::BigInt>)]
        decision_id: Option<i64>,
    }

    #[test]
    fn migrates_a_fresh_database_and_reopens_it() {
        let Ok(url) = std::env::var("OT_TEST_POSTGRES_URL") else {
            return;
        };
        let mut first = PgControlStore::connect(&url).unwrap();
        assert_eq!(first.schema_version().unwrap(), 1);
        let mut second = PgControlStore::connect(&url).unwrap();
        assert_eq!(second.schema_version().unwrap(), 1);
        let settings = serde_json::json!({"disable_password_login": false});
        second.put_auth_settings(&settings).unwrap();
        assert_eq!(second.auth_settings().unwrap(), settings);
        let app = serde_json::json!({"site_name": "Test", "basemap_tiles_url": "https://example.test/key"});
        let id = second.put_app_settings(&app, "test").unwrap();
        assert!(id > 0);
        assert_eq!(second.app_settings().unwrap(), app);
        let decision: EvidenceRow =
            diesel::sql_query("SELECT evidence FROM decisions WHERE id = $1")
                .bind::<diesel::sql_types::BigInt, _>(id)
                .get_result(&mut second.connection)
                .unwrap();
        assert!(!decision.evidence.contains("example.test/key"));
        let audit: AuditRowForTest = diesel::sql_query(
            "SELECT seq, hash, prev_hash, op, decision_id FROM audit
             WHERE decision_id = $1",
        )
        .bind::<diesel::sql_types::BigInt, _>(id)
        .get_result(&mut second.connection)
        .unwrap();
        assert_eq!(audit.op, "app_settings");
        assert_eq!(audit.decision_id, Some(id));
        assert_eq!(audit.hash.len(), 64);
        assert_eq!(audit.prev_hash.len(), 64);
        let event_seq = second
            .audit(&crate::AuditEvent::new("test", "pg_test_login"))
            .unwrap();
        assert_eq!(event_seq, audit.seq + 1);
        assert_eq!(second.audit_head().unwrap().0, event_seq);
        let found = second
            .audit_rows(&crate::audit::AuditFilter {
                ops: vec!["pg_test_login".to_owned()],
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert!(!found.is_empty());
        assert!(found.iter().all(|row| row.op == "pg_test_login"));
        assert_eq!(found[0].seq, event_seq);
        let correlation = serde_json::json!({"threshold": 0.8});
        let decision = crate::Decision::new("test", "correlation_settings");
        let id = second
            .save_correlation_settings(&correlation, &decision)
            .unwrap();
        assert!(id > 0);
        assert_eq!(second.correlation_settings().unwrap(), Some(correlation));
        let version = second.correlation_version().unwrap();
        assert!(version.ends_with(":0:0"), "{version}");
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let id = format!("pg-test-{unique}");
        let email = format!("pg-test-{unique}@example.test");
        let created = second
            .create_user(&crate::NewUser {
                id: &id,
                email: &email,
                name: "PG test",
                role: "viewer",
                password_hash: None,
                origin: "saml",
            })
            .unwrap();
        assert_eq!(created.email, email);
        assert_eq!(created.origin, "saml");
        assert!(!created.has_password);
        assert_eq!(second.user(&id).unwrap(), Some(created));
        let updated = second.update_user(&id, None, Some("admin"), None).unwrap();
        assert_eq!(updated.role, "admin");
        assert_eq!(second.user_by_email(&email).unwrap(), Some(updated));
        assert!(second.user_count().unwrap() > 0);
        assert!(second.users().unwrap().iter().any(|u| u.id == id));
        let session_id = format!("pg-session-{unique}");
        let expires_at_ms = crate::sqlite::now_ms() + 60_000;
        second
            .create_session(&crate::auth::NewSession {
                id: &session_id,
                user_id: &id,
                expires_at_ms,
                ip: Some("127.0.0.1"),
                user_agent: Some("PG test"),
                prev_login_at_ms: None,
                failed_before: 0,
            })
            .unwrap();
        assert_eq!(second.session(&session_id).unwrap().unwrap().user_id, id);
        assert_eq!(second.sessions(Some(&id), false, 10).unwrap().len(), 1);
        assert!(!second.token_revoked(&session_id).unwrap());
        assert!(second.end_session(&session_id, "test").unwrap());
        assert!(!second.end_session(&session_id, "test").unwrap());
        second.revoke_token(&session_id, expires_at_ms).unwrap();
        assert!(second.token_revoked(&session_id).unwrap());
        let source_id = format!("pg-source-{unique}");
        let spec = serde_json::json!({"transport": {"kind": "mqtt", "password": "secret"}});
        let source = second
            .put_source(
                &crate::SourceWrite {
                    id: &source_id,
                    name: "PG source",
                    transport: "mqtt",
                    codec: "json",
                    priority: 100,
                    spec: &spec,
                    problems: &serde_json::json!([]),
                },
                "test",
            )
            .unwrap();
        assert_eq!(source.revision, 1);
        assert!(!source.enabled);
        assert_eq!(second.source_revisions(&source_id).unwrap().len(), 1);
        assert!(
            second
                .set_source_enabled(&source_id, true, None, "test")
                .unwrap()
                .enabled
        );
        assert!(
            second
                .set_raw_output(&source_id, Some("raw.test"), "test")
                .unwrap()
                .raw_subject
                .is_some()
        );
        assert!(
            second
                .list_sources()
                .unwrap()
                .iter()
                .any(|s| s.id == source_id)
        );
        assert!(second.sources_version().unwrap().contains(':'));
        second.delete_source(&source_id, "test").unwrap();
        assert!(second.get_source(&source_id).unwrap().is_none());
        let mut selected = crate::Db::open_postgres(&url).unwrap();
        assert_eq!(selected.schema_version().unwrap(), 1);
        assert!(selected.latest_published_schema().unwrap() >= 1);
        assert!(!selected.schema_versions().unwrap().is_empty());
        let draft = selected
            .put_schema_draft(
                &[serde_json::json!({"key": "pg_test_field", "type": "string", "required": false})],
                Some("PostgreSQL integration test"),
                "test",
            )
            .unwrap();
        assert_eq!(draft.status, "draft");
        assert_eq!(draft.fields.len(), 1);
        let published = selected.publish_schema_draft("test").unwrap();
        assert_eq!(published.version, draft.version);
        assert_eq!(published.status, "published");
        assert!(selected.groups().unwrap().is_empty());
        assert!(selected.registry_rows().unwrap().is_empty());
        assert!(selected.registry_version().unwrap().contains(':'));
        assert_eq!(selected.list_entities("", 10, 0).unwrap().1, 0);
        assert!(selected.attribute_keys().unwrap().is_empty());
        assert!(
            selected
                .registry_resolve("mmsi", "missing")
                .unwrap()
                .is_none()
        );
        let registry_id = format!("pg-entity-{unique}");
        let registry_ident = format!("pg-ident-{unique}");
        let entity: crate::Entity = serde_json::from_value(serde_json::json!({
            "id": registry_id,
            "name": "PG registry test",
            "identifiers": [{"scheme": "test", "value": registry_ident}],
        }))
        .unwrap();
        let saved = selected.save_entity(&entity, "test").unwrap();
        assert_eq!(saved.id, registry_id);
        assert_eq!(
            selected.registry_resolve("test", &registry_ident).unwrap(),
            Some(saved.clone())
        );
        assert_eq!(selected.list_entities(&registry_id, 10, 0).unwrap().1, 1);
        assert!(
            selected
                .update_entity_fields(
                    &registry_id,
                    &[("name".into(), serde_json::json!("PG renamed"))],
                    "test"
                )
                .unwrap()
        );
        assert_eq!(
            selected
                .entity(&registry_id)
                .unwrap()
                .unwrap()
                .name
                .as_deref(),
            Some("PG renamed")
        );
        assert_eq!(
            selected.entity_revisions(&registry_id, 10).unwrap().len(),
            2
        );
        selected.delete_entity(&registry_id, "test").unwrap();
        assert!(selected.entity(&registry_id).unwrap().is_none());
        assert!(selected.user_by_email(&email).unwrap().is_some());
        assert!(!selected.config_present().unwrap().is_empty());
        let api_jti = format!("pg-api-{unique}");
        selected
            .add_api_token(&api_jti, "PG test", &id, "test", expires_at_ms)
            .unwrap();
        assert!(
            selected
                .api_tokens()
                .unwrap()
                .iter()
                .any(|t| t.jti == api_jti)
        );
        selected.revoke_api_token(&api_jti).unwrap();
        assert!(selected.token_revoked(&api_jti).unwrap());
        assert_eq!(selected.auth_settings().unwrap(), settings);
        assert_eq!(selected.app_settings().unwrap(), app);
        assert!(selected.correlation_settings().unwrap().is_some());
        assert!(
            selected
                .list_sources()
                .unwrap()
                .iter()
                .all(|s| s.id != source_id)
        );
        let selected_decision = selected
            .record(&crate::Decision::new("test", "pg_dispatch_test"))
            .unwrap();
        assert!(selected_decision > 0);
        assert_eq!(
            selected.decision(selected_decision).unwrap().unwrap()["op"],
            "pg_dispatch_test"
        );
        selected.live_reports().unwrap();
        let site = ot_core::SiteCode::new("OTK").unwrap();
        let next_seq = selected.sync_next_seq(site).unwrap();
        let synced = selected
            .sync_append_own(
                site,
                "test",
                "admin",
                &serde_json::json!({"op": "pg_test"}),
                crate::sqlite::now_ms() as u64,
            )
            .unwrap();
        assert_eq!(synced.id.seq, next_seq);
        assert_eq!(selected.sync_next_seq(site).unwrap(), next_seq + 1);
        assert_eq!(selected.sync_last_hlc().unwrap(), synced.hlc);
        assert!(selected.sync_peer_keys().unwrap().is_empty());
        assert!(selected.sync_pending().unwrap().is_empty());
        assert_eq!(selected.sync_expire_pending(0).unwrap(), 0);
        assert_eq!(selected.registry_counts().unwrap()["identifiers"], 0);
        let suggestion = selected
            .upsert_suggestion(
                "pair",
                &format!("a-{unique}"),
                Some(&format!("b-{unique}")),
                None,
                &serde_json::json!({"score": 0.9}),
            )
            .unwrap();
        assert_eq!(
            selected.suggestion(suggestion).unwrap().unwrap().status,
            "open"
        );
        assert_eq!(
            selected
                .suggestions(Some("open"), 100)
                .unwrap()
                .iter()
                .filter(|s| s.id == suggestion)
                .count(),
            1
        );
        assert!(selected.suggestion_counts().unwrap()["open"] >= 1);
        selected
            .close_suggestion(suggestion, "rejected", None)
            .unwrap();
        assert_eq!(
            selected.suggestion(suggestion).unwrap().unwrap().status,
            "rejected"
        );
        assert!(
            !selected
                .decisions_by_op(&["publish_schema"], 10)
                .unwrap()
                .is_empty()
        );
        let forbidden_a = format!("forbidden-a-{unique}");
        let forbidden_b = format!("forbidden-b-{unique}");
        selected
            .do_not_pair(
                &[forbidden_a.clone()],
                &[forbidden_b.clone()],
                crate::Decision::new("test", "do_not_pair"),
            )
            .unwrap();
        assert!(
            selected
                .do_not_pairs()
                .unwrap()
                .contains(&(forbidden_a, forbidden_b))
        );
        let graph_source = format!("pg-graph-{unique}");
        let (uid, decision_id) = selected
            .create_system_track(
                site,
                &graph_source,
                "track-1",
                crate::Decision::new("test", "create_system_track"),
            )
            .unwrap();
        assert!(decision_id > 0);
        assert!(
            selected
                .live_reports()
                .unwrap()
                .contains(&(format!("{graph_source}/track-1"), uid))
        );
        selected
            .note_source_track(
                &graph_source,
                "track-1",
                &serde_json::json!({"tracker": "test"}),
            )
            .unwrap();
        assert_eq!(
            selected.explain(uid).unwrap()[0].src_attrs["tracker"],
            "test"
        );
        selected
            .pair_source_track(
                &graph_source,
                "track-2",
                uid,
                &serde_json::json!({"pairing": "operator"}),
                crate::Decision::new("test", "pair"),
            )
            .unwrap();
        assert_eq!(selected.explain(uid).unwrap().len(), 2);
        let (split_uid, _) = selected
            .split_source_track(
                site,
                &graph_source,
                "track-2",
                crate::Decision::new("test", "split"),
            )
            .unwrap();
        assert_ne!(split_uid, uid);
        assert!(
            selected
                .live_reports()
                .unwrap()
                .contains(&(format!("{graph_source}/track-2"), split_uid))
        );
        assert!(
            selected
                .end_source_track(
                    &graph_source,
                    "track-2",
                    crate::Decision::new("test", "end_source_track"),
                )
                .unwrap()
                .is_some()
        );
        selected
            .retire_system_track(uid, crate::Decision::new("test", "retire_system_track"))
            .unwrap();
        assert!(
            selected
                .live_reports()
                .unwrap()
                .iter()
                .all(|(_, track)| *track != uid)
        );
        let next = selected.uid_next_sequence(site).unwrap();
        assert!(next > uid.sequence());
        assert!(!selected.uid_sightings(site).unwrap().is_empty());
        let advanced = selected
            .advance_uid_counter(
                site,
                &[crate::UidSighting::new("test", "external", next + 5)],
                serde_json::json!({"test": "checked"}),
            )
            .unwrap()
            .unwrap();
        assert_eq!(advanced.after, next + 6);
        assert_eq!(selected.uid_next_sequence(site).unwrap(), next + 6);
        let (from_uid, _) = selected
            .create_system_track(
                site,
                &graph_source,
                "merge-from",
                crate::Decision::new("test", "create_system_track"),
            )
            .unwrap();
        let (into_uid, _) = selected
            .create_system_track(
                site,
                &graph_source,
                "merge-into",
                crate::Decision::new("test", "create_system_track"),
            )
            .unwrap();
        selected
            .merge_system_tracks(from_uid, into_uid, crate::Decision::new("test", "merge"))
            .unwrap();
        assert!(
            selected
                .live_reports()
                .unwrap()
                .contains(&(format!("{graph_source}/merge-from"), into_uid))
        );
        assert!(
            selected
                .explain(into_uid)
                .unwrap()
                .iter()
                .any(|edge| edge.kind == crate::EdgeKind::MergedInto)
        );
        let plugin_name = format!("pg-plugin-{unique}");
        let manifest = serde_json::json!({"kinds": ["tracker"]});
        let grants = serde_json::json!({});
        let plugin = selected
            .put_plugin(
                &crate::PluginWrite {
                    name: &plugin_name,
                    version: "1",
                    manifest: &manifest,
                    wasm: None,
                    address: Some("127.0.0.1:9000"),
                    grants: &grants,
                    secret: Some("test-secret"),
                },
                "test",
            )
            .unwrap();
        assert_eq!(plugin.secret.as_deref(), Some("test-secret"));
        assert!(selected.plugins_version().unwrap().contains(':'));
        assert!(
            selected
                .list_plugins()
                .unwrap()
                .iter()
                .any(|p| p.name == plugin_name)
        );
        assert!(
            !selected
                .set_plugin(&plugin_name, Some(false), None, "test")
                .unwrap()
                .enabled
        );
        selected.delete_plugin(&plugin_name, "test").unwrap();
        assert!(selected.get_plugin(&plugin_name).unwrap().is_none());
        selected.set_must_change_password(&id, true).unwrap();
        assert!(selected.user(&id).unwrap().unwrap().must_change_password);
        selected
            .touch_sessions(&[(session_id.clone(), crate::sqlite::now_ms())])
            .unwrap();
        assert!(
            selected
                .end_stale_sessions(0, 0)
                .unwrap()
                .iter()
                .all(|s| s.id != session_id)
        );
        assert!(
            selected
                .disable_expired(crate::sqlite::now_ms())
                .unwrap()
                .is_empty()
        );
        assert!(selected.disable_inactive(0, &[]).unwrap().is_empty());
    }
}
