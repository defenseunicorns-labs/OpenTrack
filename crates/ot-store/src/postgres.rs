//! PostgreSQL control store connection and transactional schema migration.
//! The application uses Diesel's safe libpq wrapper, so password authentication
//! and TLS use the libpq/OpenSSL stack rather than a second Rust crypto stack.

use diesel::connection::SimpleConnection;
use diesel::pg::PgConnection;
use diesel::prelude::*;
use diesel::sql_types::{BigInt, Nullable, Text};
use serde_json::Value;

use crate::Decision;

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
    #[error("PostgreSQL schema migration failed: {0}")]
    Migration(String),
    #[error("PostgreSQL schema version {found} is newer than this build supports ({supported})")]
    TooNew { found: i64, supported: i64 },
    #[error("PostgreSQL control schema version is missing")]
    MissingVersion,
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
struct VersionText {
    #[diesel(sql_type = Text)]
    version: String,
}

fn record_decision(
    connection: &mut PgConnection,
    decision: &Decision,
) -> Result<i64, PostgresError> {
    let row: DecisionId = diesel::sql_query(
        "INSERT INTO decisions (at_ms, actor, op, reason, evidence, before, after)
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id",
    )
    .bind::<BigInt, _>(crate::sqlite::now_ms())
    .bind::<Text, _>(&decision.actor)
    .bind::<Text, _>(&decision.op)
    .bind::<Nullable<Text>, _>(decision.reason.as_deref())
    .bind::<Text, _>(decision.evidence.to_string())
    .bind::<Nullable<Text>, _>(decision.before.as_ref().map(Value::to_string))
    .bind::<Nullable<Text>, _>(decision.after.as_ref().map(Value::to_string))
    .get_result(connection)?;
    Ok(row.id)
}

pub struct PgControlStore {
    connection: PgConnection,
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
        let correlation = serde_json::json!({"threshold": 0.8});
        let decision = crate::Decision::new("test", "correlation_settings");
        let id = second
            .save_correlation_settings(&correlation, &decision)
            .unwrap();
        assert!(id > 0);
        assert_eq!(second.correlation_settings().unwrap(), Some(correlation));
        let version = second.correlation_version().unwrap();
        assert!(version.ends_with(":0:0"), "{version}");
    }
}
