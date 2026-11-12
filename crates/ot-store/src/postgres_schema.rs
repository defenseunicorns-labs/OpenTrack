//! PostgreSQL output schema reads.

use diesel::prelude::*;
use diesel::sql_types::{BigInt, Nullable, Text};
use serde_json::{Value, json};

use super::{PgControlStore, PostgresError, SchemaVersion as CountRow};
use crate::schema::SchemaVersion;

#[derive(QueryableByName)]
struct VersionRow {
    #[diesel(sql_type = BigInt)]
    version: i64,
    #[diesel(sql_type = Text)]
    status: String,
    #[diesel(sql_type = Nullable<BigInt>)]
    published_at_ms: Option<i64>,
    #[diesel(sql_type = Nullable<Text>)]
    notes: Option<String>,
}

#[derive(QueryableByName)]
struct FieldRow {
    #[diesel(sql_type = Text)]
    key: String,
    #[diesel(sql_type = Text)]
    field_type: String,
    #[diesel(sql_type = Nullable<Text>)]
    unit: Option<String>,
    #[diesel(sql_type = BigInt)]
    required: i64,
    #[diesel(sql_type = Nullable<Text>)]
    default_json: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    enum_values: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    description: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    builtin: Option<String>,
}

impl PgControlStore {
    pub fn latest_published_schema(&mut self) -> Result<u32, PostgresError> {
        let row: CountRow = diesel::sql_query(
            "SELECT coalesce(max(version), 0)::bigint AS version FROM schema_versions
             WHERE status = 'published'",
        )
        .get_result(&mut self.connection)?;
        Ok(row.version as u32)
    }

    pub fn schema_versions(&mut self) -> Result<Vec<SchemaVersion>, PostgresError> {
        let heads: Vec<VersionRow> = diesel::sql_query(
            "SELECT version, status, published_at_ms, notes FROM schema_versions
             ORDER BY version",
        )
        .load(&mut self.connection)?;
        let mut versions = Vec::with_capacity(heads.len());
        for head in heads {
            let rows: Vec<FieldRow> = diesel::sql_query(
                "SELECT key, type AS field_type, unit, required, default_json,
                 enum_values, description, builtin FROM extension_fields
                 WHERE schema_version = $1 ORDER BY key",
            )
            .bind::<BigInt, _>(head.version)
            .load(&mut self.connection)?;
            let mut fields = Vec::with_capacity(rows.len());
            for row in rows {
                let mut field = json!({
                    "key": row.key,
                    "type": row.field_type,
                    "required": row.required != 0,
                });
                if let Some(value) = row.unit {
                    field["unit"] = json!(value);
                }
                if let Some(value) = row.default_json {
                    field["default"] = serde_json::from_str::<Value>(&value)?;
                }
                if let Some(value) = row.enum_values {
                    field["enum_values"] = serde_json::from_str::<Value>(&value)?;
                }
                if let Some(value) = row.description {
                    field["description"] = json!(value);
                }
                if let Some(value) = row.builtin {
                    field["builtin"] = json!(value);
                }
                fields.push(field);
            }
            versions.push(SchemaVersion {
                version: head.version as u32,
                status: head.status,
                published_at_ms: head.published_at_ms,
                notes: head.notes,
                fields,
            });
        }
        Ok(versions)
    }

    pub fn schema_version_get(
        &mut self,
        version: u32,
    ) -> Result<Option<SchemaVersion>, PostgresError> {
        Ok(self
            .schema_versions()?
            .into_iter()
            .find(|row| row.version == version))
    }
}
