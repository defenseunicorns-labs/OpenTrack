//! Extension schema versions in SQLite. Field definitions are stored as
//! JSON-ish columns; the server validates them (against `ot-source`) first.
//! There is at most one draft, always numbered after the newest version.

use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use serde_json::{Value, json};

use crate::sqlite::{Db, Decision, Result, StoreError, now_ms, record_decision};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SchemaVersion {
    pub version: u32,
    /// `draft` or `published`.
    pub status: String,
    pub published_at_ms: Option<i64>,
    pub notes: Option<String>,
    /// Field definitions: key, type, unit, required, default, enum_values, description.
    pub fields: Vec<Value>,
}

impl Db {
    /// The newest published output schema version (0 before any).
    pub fn latest_published_schema(&self) -> Result<u32> {
        Ok(self.connection().query_row(
            "SELECT coalesce(max(version), 0) FROM schema_versions WHERE status = 'published'",
            [],
            |r| r.get(0),
        )?)
    }

    pub fn schema_versions(&self) -> Result<Vec<SchemaVersion>> {
        let mut stmt = self.connection().prepare(
            "SELECT version, status, published_at_ms, notes FROM schema_versions ORDER BY version",
        )?;
        let heads: Vec<(u32, String, Option<i64>, Option<String>)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<std::result::Result<_, _>>()?;
        heads
            .into_iter()
            .map(|(version, status, published_at_ms, notes)| {
                Ok(SchemaVersion {
                    fields: self.schema_fields(version)?,
                    version,
                    status,
                    published_at_ms,
                    notes,
                })
            })
            .collect()
    }

    pub fn schema_version_get(&self, version: u32) -> Result<Option<SchemaVersion>> {
        Ok(self
            .schema_versions()?
            .into_iter()
            .find(|v| v.version == version))
    }

    fn schema_fields(&self, version: u32) -> Result<Vec<Value>> {
        let mut stmt = self.connection().prepare(
            "SELECT key, type, unit, required, default_json, enum_values, description, builtin
             FROM extension_fields WHERE schema_version = ?1 ORDER BY key",
        )?;
        let rows = stmt.query_map([version], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, bool>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, Option<String>>(7)?,
            ))
        })?;
        rows.map(|r| {
            let (key, kind, unit, required, default, enum_values, description, builtin) = r?;
            let mut f = json!({ "key": key, "type": kind, "required": required });
            if let Some(u) = unit {
                f["unit"] = json!(u);
            }
            if let Some(d) = default {
                f["default"] = serde_json::from_str(&d)?;
            }
            if let Some(e) = enum_values {
                f["enum_values"] = serde_json::from_str(&e)?;
            }
            if let Some(d) = description {
                f["description"] = json!(d);
            }
            if let Some(b) = builtin {
                f["builtin"] = json!(b);
            }
            Ok(f)
        })
        .collect()
    }

    /// Create or replace the draft version's fields. Returns the draft.
    pub fn put_schema_draft(
        &mut self,
        fields: &[Value],
        notes: Option<&str>,
        actor: &str,
    ) -> Result<SchemaVersion> {
        let version = self.write(|tx| {
            let now = now_ms();
            let draft: Option<u32> = tx
                .query_row("SELECT version FROM schema_versions WHERE status = 'draft'", [], |r| r.get(0))
                .optional()?;
            let version = match draft {
                Some(v) => v,
                None => {
                    let v: u32 = tx.query_row(
                        "SELECT coalesce(max(version), 0) + 1 FROM schema_versions",
                        [],
                        |r| r.get(0),
                    )?;
                    tx.execute(
                        "INSERT INTO schema_versions (version, status) VALUES (?1, 'draft')",
                        [v],
                    )?;
                    v
                }
            };
            tx.execute("UPDATE schema_versions SET notes = ?2 WHERE version = ?1", params![version, notes])?;
            tx.execute("DELETE FROM extension_fields WHERE schema_version = ?1", [version])?;
            for f in fields {
                let text = |k: &str| f.get(k).and_then(Value::as_str).map(str::to_owned);
                tx.execute(
                    "INSERT INTO extension_fields
                       (schema_version, key, type, unit, required, default_json, enum_values, description, builtin)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        version,
                        text("key"),
                        text("type"),
                        text("unit"),
                        f.get("required").and_then(Value::as_bool).unwrap_or(false),
                        f.get("default").filter(|v| !v.is_null()).map(Value::to_string),
                        f.get("enum_values").filter(|v| !v.is_null()).map(Value::to_string),
                        text("description"),
                        text("builtin"),
                    ],
                )?;
            }
            let d = Decision::new(actor, "edit_schema_draft")
                .evidence(json!({ "version": version, "fields": fields.len() }));
            record_decision(tx, &d, now)?;
            Ok(version)
        })?;
        self.schema_version_get(version)?
            .ok_or_else(|| StoreError::NotFound(format!("schema version {version}")))
    }

    /// Publish the draft; it can never change again.
    pub fn publish_schema_draft(&mut self, actor: &str) -> Result<SchemaVersion> {
        let version = self.write(|tx| {
            let draft: u32 = tx
                .query_row("SELECT version FROM schema_versions WHERE status = 'draft'", [], |r| r.get(0))
                .optional()?
                .ok_or_else(|| StoreError::NotFound("schema draft".into()))?;
            let now = now_ms();
            tx.execute(
                "UPDATE schema_versions SET status = 'published', published_at_ms = ?2 WHERE version = ?1",
                params![draft, now],
            )?;
            let d = Decision::new(actor, "publish_schema").evidence(json!({ "version": draft }));
            record_decision(tx, &d, now)?;
            Ok(draft)
        })?;
        self.schema_version_get(version)?
            .ok_or_else(|| StoreError::NotFound(format!("schema version {version}")))
    }

    /// Publish another node's output schema here as the next version. A
    /// draft in progress keeps its fields and moves up one number, so it
    /// still comes after every published version.
    pub fn publish_schema_as(
        &mut self,
        fields: &[Value],
        notes: Option<&str>,
        actor: &str,
    ) -> Result<SchemaVersion> {
        let draft: Option<u32> = self
            .connection()
            .query_row(
                "SELECT version FROM schema_versions WHERE status = 'draft'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(d) = draft {
            self.write(|tx| {
                tx.execute(
                    "INSERT INTO schema_versions (version, status, notes)
                     SELECT ?2, 'draft', notes FROM schema_versions WHERE version = ?1",
                    params![d, d + 1],
                )?;
                tx.execute(
                    "UPDATE extension_fields SET schema_version = ?2 WHERE schema_version = ?1",
                    params![d, d + 1],
                )?;
                tx.execute("DELETE FROM schema_versions WHERE version = ?1", [d])?;
                Ok(())
            })?;
            // The new draft sits above the version being published: stage
            // it in the freed number, below the draft.
            let version = self.write(|tx| {
                tx.execute(
                    "INSERT INTO schema_versions (version, status, notes) VALUES (?1, 'published', ?2)",
                    params![d, notes],
                )?;
                Ok(d)
            })?;
            self.fill_published(version, fields, actor)
        } else {
            let version = self.write(|tx| {
                let v: u32 = tx.query_row(
                    "SELECT coalesce(max(version), 0) + 1 FROM schema_versions",
                    [],
                    |r| r.get(0),
                )?;
                tx.execute(
                    "INSERT INTO schema_versions (version, status, notes) VALUES (?1, 'published', ?2)",
                    params![v, notes],
                )?;
                Ok(v)
            })?;
            self.fill_published(version, fields, actor)
        }
    }

    fn fill_published(
        &mut self,
        version: u32,
        fields: &[Value],
        actor: &str,
    ) -> Result<SchemaVersion> {
        self.write(|tx| {
            let now = now_ms();
            for f in fields {
                let text = |k: &str| f.get(k).and_then(Value::as_str).map(str::to_owned);
                tx.execute(
                    "INSERT INTO extension_fields
                       (schema_version, key, type, unit, required, default_json, enum_values, description, builtin)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        version,
                        text("key"),
                        text("type"),
                        text("unit"),
                        f.get("required").and_then(Value::as_bool).unwrap_or(false),
                        f.get("default").filter(|v| !v.is_null()).map(Value::to_string),
                        f.get("enum_values").filter(|v| !v.is_null()).map(Value::to_string),
                        text("description"),
                        text("builtin"),
                    ],
                )?;
            }
            tx.execute(
                "UPDATE schema_versions SET published_at_ms = ?2 WHERE version = ?1",
                params![version, now],
            )?;
            let d = Decision::new(actor, "publish_schema")
                .evidence(json!({ "version": version, "from_another_node": true }));
            record_decision(tx, &d, now)?;
            Ok(())
        })?;
        self.schema_version_get(version)?
            .ok_or_else(|| StoreError::NotFound(format!("schema version {version}")))
    }

    pub fn discard_schema_draft(&mut self, actor: &str) -> Result<()> {
        self.write(|tx| {
            let n = tx.execute(
                "DELETE FROM extension_fields WHERE schema_version IN
                   (SELECT version FROM schema_versions WHERE status = 'draft')",
                [],
            )?;
            let v = tx.execute("DELETE FROM schema_versions WHERE status = 'draft'", [])?;
            if v == 0 {
                return Err(StoreError::NotFound("schema draft".into()));
            }
            let d = Decision::new(actor, "discard_schema_draft").evidence(json!({ "fields": n }));
            record_decision(tx, &d, now_ms())?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draft_publish_is_immutable_and_versions_advance() {
        let mut db = Db::open_in_memory().unwrap();
        let v = db.schema_versions().unwrap();
        assert_eq!(
            (v.len(), v[0].status.as_str(), v[0].fields.len()),
            (1, "published", 0)
        );

        let fields = vec![
            json!({"key": "squawk", "type": "string"}),
            json!({"key": "nav_status", "type": "enum", "enum_values": ["0", "5"], "default": "0"}),
        ];
        let d = db
            .put_schema_draft(&fields, Some("adsb + ais"), "op:test")
            .unwrap();
        assert_eq!(
            (d.version, d.status.as_str(), d.fields.len()),
            (2, "draft", 2)
        );
        // Editing the draft replaces its fields and keeps its number.
        let d = db.put_schema_draft(&fields[..1], None, "op:test").unwrap();
        assert_eq!((d.version, d.fields.len()), (2, 1));

        let p = db.publish_schema_draft("op:test").unwrap();
        assert_eq!(p.status, "published");
        assert!(matches!(
            db.publish_schema_draft("op:test"),
            Err(StoreError::NotFound(_))
        ));
        // Another node's schema arrives while a draft is open here: it is
        // published next, and the draft moves up with its fields.
        let d = db.put_schema_draft(&fields, None, "op:test").unwrap();
        assert_eq!(d.version, 3);
        let p = db
            .publish_schema_as(&fields[1..], Some("from BBB"), "tm@bbb")
            .unwrap();
        assert_eq!(
            (p.version, p.status.as_str(), p.fields.len()),
            (3, "published", 1)
        );
        assert_eq!(db.latest_published_schema().unwrap(), 3);
        let d = db
            .schema_versions()
            .unwrap()
            .into_iter()
            .find(|v| v.status == "draft")
            .unwrap();
        assert_eq!((d.version, d.fields.len()), (4, 2));
        db.discard_schema_draft("op:test").unwrap();
        let p = db.publish_schema_as(&fields, None, "tm@bbb").unwrap();
        assert_eq!(p.version, 4);
        db.connection()
            .execute("DELETE FROM extension_fields WHERE schema_version >= 3", [])
            .unwrap();
        db.connection()
            .execute("DELETE FROM schema_versions WHERE version >= 3", [])
            .unwrap();

        // The next draft is version 3; version 2 is untouched.
        let d = db.put_schema_draft(&fields, None, "op:test").unwrap();
        assert_eq!(d.version, 3);
        assert_eq!(db.schema_version_get(2).unwrap().unwrap().fields.len(), 1);
        assert_eq!(
            db.schema_version_get(3).unwrap().unwrap().fields[0]["default"],
            "0"
        );
        db.discard_schema_draft("op:test").unwrap();
        assert!(db.schema_version_get(3).unwrap().is_none());
    }
}
