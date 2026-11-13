//! PostgreSQL plugin configuration.

use diesel::prelude::*;
use diesel::sql_types::{BigInt, Binary, Nullable, Text};
use serde_json::{Value, json};

use super::{PgControlStore, PostgresError, VersionText, record_decision};
use crate::Decision;
use crate::plugins::{PluginRow, PluginWrite};

#[derive(QueryableByName)]
struct PgPluginRow {
    #[diesel(sql_type = Text)]
    name: String,
    #[diesel(sql_type = Text)]
    runtime: String,
    #[diesel(sql_type = Text)]
    version: String,
    #[diesel(sql_type = Text)]
    manifest: String,
    #[diesel(sql_type = Nullable<Text>)]
    sha256: Option<String>,
    #[diesel(sql_type = Nullable<BigInt>)]
    size: Option<i64>,
    #[diesel(sql_type = Nullable<Text>)]
    address: Option<String>,
    #[diesel(sql_type = Text)]
    grants: String,
    #[diesel(sql_type = BigInt)]
    enabled: i64,
    #[diesel(sql_type = BigInt)]
    created_at_ms: i64,
    #[diesel(sql_type = BigInt)]
    updated_at_ms: i64,
    #[diesel(sql_type = Nullable<Text>)]
    secret: Option<String>,
}

impl PgPluginRow {
    fn into_plugin(self) -> PluginRow {
        PluginRow {
            name: self.name,
            runtime: self.runtime,
            version: self.version,
            manifest: serde_json::from_str(&self.manifest).unwrap_or(Value::Null),
            sha256: self.sha256,
            size: self.size,
            address: self.address,
            grants: serde_json::from_str(&self.grants).unwrap_or_else(|_| json!({})),
            secret: self.secret,
            enabled: self.enabled != 0,
            created_at_ms: self.created_at_ms,
            updated_at_ms: self.updated_at_ms,
        }
    }
}

#[derive(QueryableByName)]
struct WasmRow {
    #[diesel(sql_type = Nullable<Binary>)]
    wasm: Option<Vec<u8>>,
}

const COLUMNS: &str = "name, runtime, version, manifest, sha256,
    octet_length(wasm)::bigint AS size, address, grants, enabled,
    created_at_ms, updated_at_ms, secret";

fn summary(plugin: &PluginRow) -> Value {
    json!({
        "name": plugin.name,
        "runtime": plugin.runtime,
        "version": plugin.version,
        "sha256": plugin.sha256,
        "address": plugin.address,
        "grants": plugin.grants,
        "enabled": plugin.enabled,
        "secret": plugin.secret.is_some(),
    })
}

impl PgControlStore {
    pub fn list_plugins(&mut self) -> Result<Vec<PluginRow>, PostgresError> {
        let rows: Vec<PgPluginRow> =
            diesel::sql_query(format!("SELECT {COLUMNS} FROM plugins ORDER BY name"))
                .load(&mut self.connection)?;
        Ok(rows.into_iter().map(PgPluginRow::into_plugin).collect())
    }

    pub fn get_plugin(&mut self, name: &str) -> Result<Option<PluginRow>, PostgresError> {
        let rows: Vec<PgPluginRow> =
            diesel::sql_query(format!("SELECT {COLUMNS} FROM plugins WHERE name = $1"))
                .bind::<Text, _>(name)
                .load(&mut self.connection)?;
        Ok(rows.into_iter().next().map(PgPluginRow::into_plugin))
    }

    pub fn plugin_wasm(&mut self, name: &str) -> Result<Option<Vec<u8>>, PostgresError> {
        let rows: Vec<WasmRow> = diesel::sql_query("SELECT wasm FROM plugins WHERE name = $1")
            .bind::<Text, _>(name)
            .load(&mut self.connection)?;
        Ok(rows.into_iter().next().and_then(|row| row.wasm))
    }

    pub fn plugins_version(&mut self) -> Result<String, PostgresError> {
        let row: VersionText = diesel::sql_query(
            "SELECT (count(*)::text || ':' || coalesce(max(updated_at_ms), 0)::text)
             AS version FROM plugins",
        )
        .get_result(&mut self.connection)?;
        Ok(row.version)
    }

    pub fn put_plugin(
        &mut self,
        write: &PluginWrite<'_>,
        actor: &str,
    ) -> Result<PluginRow, PostgresError> {
        let before = self.get_plugin(write.name)?;
        let runtime = if write.wasm.is_some() {
            "wasm"
        } else {
            "external"
        };
        let (wasm, sha) = write
            .wasm
            .map_or((None, None), |(bytes, hash)| (Some(bytes), Some(hash)));
        let now = crate::sqlite::now_ms();
        let manifest = write.manifest.to_string();
        self.connection.transaction::<(), PostgresError, _>(|connection| {
            diesel::sql_query(
                "INSERT INTO plugins (name, runtime, version, manifest, wasm, sha256,
                 address, grants, enabled, created_at_ms, updated_at_ms, secret)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 1, $9, $9, $10)
                 ON CONFLICT (name) DO UPDATE SET runtime = excluded.runtime,
                 version = excluded.version, manifest = excluded.manifest,
                 wasm = excluded.wasm, sha256 = excluded.sha256,
                 address = excluded.address, updated_at_ms = excluded.updated_at_ms,
                 secret = coalesce(excluded.secret, plugins.secret)",
            )
            .bind::<Text, _>(write.name)
            .bind::<Text, _>(runtime)
            .bind::<Text, _>(write.version)
            .bind::<Text, _>(&manifest)
            .bind::<Nullable<Binary>, _>(wasm)
            .bind::<Nullable<Text>, _>(sha)
            .bind::<Nullable<Text>, _>(write.address)
            .bind::<Text, _>(write.grants.to_string())
            .bind::<BigInt, _>(now)
            .bind::<Nullable<Text>, _>(write.secret)
            .execute(connection)?;
            let op = if before.is_some() { "update_plugin" } else { "add_plugin" };
            let decision = Decision {
                before: before.as_ref().map(summary),
                after: Some(json!({
                    "name": write.name, "runtime": runtime, "version": write.version,
                    "sha256": sha, "address": write.address,
                    "secret": write.secret.is_some() || before.as_ref().is_some_and(|p| p.secret.is_some()),
                })),
                evidence: json!({ "plugin": write.name, "kinds": write.manifest["kinds"] }),
                ..Decision::new(actor, op)
            };
            record_decision(connection, &decision)?;
            Ok(())
        })?;
        self.get_plugin(write.name)?
            .ok_or_else(|| PostgresError::NotFound(format!("plugin {}", write.name)))
    }

    pub fn set_plugin(
        &mut self,
        name: &str,
        enabled: Option<bool>,
        grants: Option<&Value>,
        actor: &str,
    ) -> Result<PluginRow, PostgresError> {
        let before = self
            .get_plugin(name)?
            .ok_or_else(|| PostgresError::NotFound(format!("plugin {name}")))?;
        let now = crate::sqlite::now_ms();
        let enabled = enabled.unwrap_or(before.enabled);
        let grants = grants.cloned().unwrap_or_else(|| before.grants.clone());
        self.connection
            .transaction::<(), PostgresError, _>(|connection| {
                diesel::sql_query(
                    "UPDATE plugins SET enabled = $2, grants = $3, updated_at_ms = $4
                 WHERE name = $1",
                )
                .bind::<Text, _>(name)
                .bind::<BigInt, _>(i64::from(enabled))
                .bind::<Text, _>(grants.to_string())
                .bind::<BigInt, _>(now)
                .execute(connection)?;
                let mut after = summary(&before);
                after["enabled"] = json!(enabled);
                after["grants"] = grants;
                record_decision(
                    connection,
                    &Decision {
                        before: Some(summary(&before)),
                        after: Some(after),
                        evidence: json!({ "plugin": name }),
                        ..Decision::new(actor, "configure_plugin")
                    },
                )?;
                Ok(())
            })?;
        self.get_plugin(name)?
            .ok_or_else(|| PostgresError::NotFound(format!("plugin {name}")))
    }

    pub fn set_plugin_secret(
        &mut self,
        name: &str,
        secret: Option<&str>,
        actor: &str,
    ) -> Result<PluginRow, PostgresError> {
        let before = self
            .get_plugin(name)?
            .ok_or_else(|| PostgresError::NotFound(format!("plugin {name}")))?;
        let now = crate::sqlite::now_ms();
        self.connection
            .transaction::<(), PostgresError, _>(|connection| {
                diesel::sql_query(
                    "UPDATE plugins SET secret = $2, updated_at_ms = $3 WHERE name = $1",
                )
                .bind::<Text, _>(name)
                .bind::<Nullable<Text>, _>(secret)
                .bind::<BigInt, _>(now)
                .execute(connection)?;
                let mut after = summary(&before);
                after["secret"] = json!(secret.is_some());
                record_decision(
                    connection,
                    &Decision {
                        before: Some(summary(&before)),
                        after: Some(after),
                        evidence: json!({ "plugin": name }),
                        ..Decision::new(actor, "set_plugin_secret")
                    },
                )?;
                Ok(())
            })?;
        self.get_plugin(name)?
            .ok_or_else(|| PostgresError::NotFound(format!("plugin {name}")))
    }

    pub fn delete_plugin(&mut self, name: &str, actor: &str) -> Result<(), PostgresError> {
        let before = self
            .get_plugin(name)?
            .ok_or_else(|| PostgresError::NotFound(format!("plugin {name}")))?;
        self.connection
            .transaction::<(), PostgresError, _>(|connection| {
                diesel::sql_query("DELETE FROM plugins WHERE name = $1")
                    .bind::<Text, _>(name)
                    .execute(connection)?;
                record_decision(
                    connection,
                    &Decision {
                        before: Some(summary(&before)),
                        evidence: json!({ "plugin": name }),
                        ..Decision::new(actor, "delete_plugin")
                    },
                )?;
                Ok(())
            })
    }
}
