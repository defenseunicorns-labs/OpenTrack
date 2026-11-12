//! PostgreSQL control store connection and transactional schema migration.
//! The application uses Diesel's safe libpq wrapper, so password authentication
//! and TLS use the libpq/OpenSSL stack rather than a second Rust crypto stack.

use diesel::connection::SimpleConnection;
use diesel::pg::PgConnection;
use diesel::prelude::*;
use diesel::sql_types::{BigInt, Nullable, Text};
use serde_json::Value;

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

    #[test]
    fn migrates_a_fresh_database_and_reopens_it() {
        let Ok(url) = std::env::var("OT_TEST_POSTGRES_URL") else {
            return;
        };
        let mut first = PgControlStore::connect(&url).unwrap();
        assert_eq!(first.schema_version().unwrap(), 1);
        let mut second = PgControlStore::connect(&url).unwrap();
        assert_eq!(second.schema_version().unwrap(), 1);
        assert_eq!(second.auth_settings().unwrap(), serde_json::json!({}));
        let settings = serde_json::json!({"disable_password_login": false});
        second.put_auth_settings(&settings).unwrap();
        assert_eq!(second.auth_settings().unwrap(), settings);
    }
}
