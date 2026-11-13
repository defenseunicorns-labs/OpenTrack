//! PostgreSQL output schema reads.

use diesel::prelude::*;
use diesel::sql_types::{BigInt, Nullable, Text};
use serde_json::{Value, json};

use super::{PgControlStore, PostgresError, SchemaVersion as CountRow, record_decision};
use crate::Decision;

fn lock_schema(connection: &mut diesel::PgConnection) -> Result<(), PostgresError> {
    let _: CountRow =
        diesel::sql_query("SELECT 1::bigint AS version FROM pg_advisory_xact_lock(6869400123458)")
            .get_result(connection)?;
    Ok(())
}

fn insert_fields(
    connection: &mut diesel::PgConnection,
    version: i64,
    fields: &[Value],
) -> Result<(), PostgresError> {
    for field in fields {
        let text = |key: &str| field.get(key).and_then(Value::as_str);
        diesel::sql_query(
            "INSERT INTO extension_fields
             (schema_version, key, type, unit, required, default_json, enum_values, description, builtin)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind::<BigInt, _>(version)
        .bind::<Text, _>(text("key").unwrap_or_default())
        .bind::<Text, _>(text("type").unwrap_or_default())
        .bind::<Nullable<Text>, _>(text("unit"))
        .bind::<BigInt, _>(i64::from(field.get("required").and_then(Value::as_bool).unwrap_or(false)))
        .bind::<Nullable<Text>, _>(field.get("default").filter(|v| !v.is_null()).map(Value::to_string))
        .bind::<Nullable<Text>, _>(field.get("enum_values").filter(|v| !v.is_null()).map(Value::to_string))
        .bind::<Nullable<Text>, _>(text("description"))
        .bind::<Nullable<Text>, _>(text("builtin"))
        .execute(connection)?;
    }
    Ok(())
}
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
    pub fn publish_schema_as(
        &mut self,
        fields: &[Value],
        notes: Option<&str>,
        actor: &str,
    ) -> Result<SchemaVersion, PostgresError> {
        let version = self.connection.transaction::<u32, PostgresError, _>(|connection| {
            lock_schema(connection)?;
            let drafts: Vec<CountRow> = diesel::sql_query("SELECT version FROM schema_versions WHERE status = 'draft'").load(connection)?;
            let version = if let Some(draft) = drafts.first() {
                let version = draft.version;
                diesel::sql_query("INSERT INTO schema_versions (version, status, notes) SELECT $1, 'draft', notes FROM schema_versions WHERE version = $2")
                    .bind::<BigInt, _>(version + 1).bind::<BigInt, _>(version).execute(connection)?;
                diesel::sql_query("UPDATE extension_fields SET schema_version = $1 WHERE schema_version = $2")
                    .bind::<BigInt, _>(version + 1).bind::<BigInt, _>(version).execute(connection)?;
                diesel::sql_query("DELETE FROM schema_versions WHERE version = $1")
                    .bind::<BigInt, _>(version).execute(connection)?;
                version
            } else {
                let row: CountRow = diesel::sql_query("SELECT (coalesce(max(version), 0) + 1)::bigint AS version FROM schema_versions").get_result(connection)?;
                row.version
            };
            let now = crate::sqlite::now_ms();
            diesel::sql_query("INSERT INTO schema_versions (version, status, published_at_ms, notes) VALUES ($1, 'published', $2, $3)")
                .bind::<BigInt, _>(version).bind::<BigInt, _>(now).bind::<Nullable<Text>, _>(notes).execute(connection)?;
            insert_fields(connection, version, fields)?;
            record_decision(connection, &Decision::new(actor, "publish_schema")
                .evidence(json!({"version":version,"from_another_node":true})))?;
            Ok(version as u32)
        })?;
        self.schema_version_get(version)?
            .ok_or_else(|| PostgresError::NotFound(format!("schema version {version}")))
    }

    pub fn put_schema_draft(
        &mut self,
        fields: &[Value],
        notes: Option<&str>,
        actor: &str,
    ) -> Result<SchemaVersion, PostgresError> {
        let version = self.connection.transaction::<u32, PostgresError, _>(|connection| {
            lock_schema(connection)?;
            let drafts: Vec<CountRow> = diesel::sql_query(
                "SELECT version FROM schema_versions WHERE status = 'draft'",
            ).load(connection)?;
            let version = if let Some(draft) = drafts.first() {
                draft.version
            } else {
                let next: CountRow = diesel::sql_query(
                    "SELECT (coalesce(max(version), 0) + 1)::bigint AS version FROM schema_versions",
                ).get_result(connection)?;
                diesel::sql_query(
                    "INSERT INTO schema_versions (version, status) VALUES ($1, 'draft')",
                ).bind::<BigInt, _>(next.version).execute(connection)?;
                next.version
            };
            diesel::sql_query("UPDATE schema_versions SET notes = $2 WHERE version = $1")
                .bind::<BigInt, _>(version)
                .bind::<Nullable<Text>, _>(notes)
                .execute(connection)?;
            diesel::sql_query("DELETE FROM extension_fields WHERE schema_version = $1")
                .bind::<BigInt, _>(version).execute(connection)?;
            insert_fields(connection, version, fields)?;
            record_decision(connection, &Decision::new(actor, "edit_schema_draft")
                .evidence(json!({"version": version, "fields": fields.len()})))?;
            Ok(version as u32)
        })?;
        self.schema_version_get(version)?
            .ok_or_else(|| PostgresError::NotFound(format!("schema version {version}")))
    }

    pub fn publish_schema_draft(&mut self, actor: &str) -> Result<SchemaVersion, PostgresError> {
        let version = self.connection.transaction::<u32, PostgresError, _>(|connection| {
            lock_schema(connection)?;
            let drafts: Vec<CountRow> = diesel::sql_query(
                "SELECT version FROM schema_versions WHERE status = 'draft'",
            ).load(connection)?;
            let version = drafts.first().ok_or_else(|| PostgresError::NotFound("schema draft".into()))?.version;
            diesel::sql_query(
                "UPDATE schema_versions SET status = 'published', published_at_ms = $2 WHERE version = $1",
            ).bind::<BigInt, _>(version)
                .bind::<BigInt, _>(crate::sqlite::now_ms())
                .execute(connection)?;
            record_decision(connection, &Decision::new(actor, "publish_schema")
                .evidence(json!({"version": version})))?;
            Ok(version as u32)
        })?;
        self.schema_version_get(version)?
            .ok_or_else(|| PostgresError::NotFound(format!("schema version {version}")))
    }

    pub fn discard_schema_draft(&mut self, actor: &str) -> Result<(), PostgresError> {
        self.connection
            .transaction::<(), PostgresError, _>(|connection| {
                lock_schema(connection)?;
                let fields = diesel::sql_query(
                    "DELETE FROM extension_fields WHERE schema_version IN
                 (SELECT version FROM schema_versions WHERE status = 'draft')",
                )
                .execute(connection)?;
                let versions =
                    diesel::sql_query("DELETE FROM schema_versions WHERE status = 'draft'")
                        .execute(connection)?;
                if versions == 0 {
                    return Err(PostgresError::NotFound("schema draft".into()));
                }
                record_decision(
                    connection,
                    &Decision::new(actor, "discard_schema_draft")
                        .evidence(json!({"fields": fields})),
                )?;
                Ok(())
            })
    }

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
