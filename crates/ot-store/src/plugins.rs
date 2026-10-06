//! Plugins an operator added, in SQLite: a WebAssembly component (its
//! bytes and SHA-256) or an external plugin's address, its manifest as last
//! read, the grants it runs with and whether it is enabled; an external
//! plugin's shared secret too. The server checks a plugin loads before it
//! gets here. Every change records a decision (the component's hash, not
//! its bytes; whether a secret is set, never the secret).

use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use serde_json::{Value, json};

use crate::sqlite::{Db, Decision, Result, StoreError, now_ms, record_decision};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PluginRow {
    pub name: String,
    /// `wasm` or `external`.
    pub runtime: String,
    pub version: String,
    pub manifest: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// Size of the component (bytes).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    pub grants: Value,
    /// The secret shared with an external plugin, or a `${env:NAME}`
    /// reference to it. Never serialised: the API says only whether one
    /// is set.
    #[serde(skip)]
    pub secret: Option<String>,
    pub enabled: bool,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// What to save for a plugin.
#[derive(Debug, Clone)]
pub struct PluginWrite<'a> {
    pub name: &'a str,
    pub version: &'a str,
    pub manifest: &'a Value,
    /// The component (a WebAssembly plugin) and its SHA-256.
    pub wasm: Option<(&'a [u8], &'a str)>,
    /// The address (an external plugin).
    pub address: Option<&'a str>,
    pub grants: &'a Value,
    /// An external plugin's secret. None keeps the one a replaced plugin has.
    pub secret: Option<&'a str>,
}

const COLUMNS: &str = "name, runtime, version, manifest, sha256, length(wasm), address, grants, \
                       enabled, created_at_ms, updated_at_ms, secret";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<PluginRow> {
    let manifest: String = r.get(3)?;
    let grants: String = r.get(7)?;
    Ok(PluginRow {
        name: r.get(0)?,
        runtime: r.get(1)?,
        version: r.get(2)?,
        manifest: serde_json::from_str(&manifest).unwrap_or(Value::Null),
        sha256: r.get(4)?,
        size: r.get(5)?,
        address: r.get(6)?,
        grants: serde_json::from_str(&grants).unwrap_or_else(|_| json!({})),
        enabled: r.get(8)?,
        created_at_ms: r.get(9)?,
        updated_at_ms: r.get(10)?,
        secret: r.get(11)?,
    })
}

fn summary(p: &PluginRow) -> Value {
    json!({
        "name": p.name, "runtime": p.runtime, "version": p.version, "sha256": p.sha256,
        "address": p.address, "grants": p.grants, "enabled": p.enabled,
        "secret": p.secret.is_some(),
    })
}

impl Db {
    pub fn list_plugins(&self) -> Result<Vec<PluginRow>> {
        let mut stmt = self
            .connection()
            .prepare(&format!("SELECT {COLUMNS} FROM plugins ORDER BY name"))?;
        let rows = stmt.query_map([], row)?.collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn get_plugin(&self, name: &str) -> Result<Option<PluginRow>> {
        Ok(self
            .connection()
            .query_row(
                &format!("SELECT {COLUMNS} FROM plugins WHERE name = ?1"),
                [name],
                row,
            )
            .optional()?)
    }

    /// A WebAssembly plugin's component.
    pub fn plugin_wasm(&self, name: &str) -> Result<Option<Vec<u8>>> {
        Ok(self
            .connection()
            .query_row("SELECT wasm FROM plugins WHERE name = ?1", [name], |r| {
                r.get(0)
            })
            .optional()?
            .flatten())
    }

    /// Changes when any plugin is added, changed or removed: loaders compare
    /// it to know when to look again.
    pub fn plugins_version(&self) -> Result<String> {
        Ok(self.connection().query_row(
            "SELECT count(*) || ':' || coalesce(max(updated_at_ms), 0) FROM plugins",
            [],
            |r| r.get(0),
        )?)
    }

    /// Add a plugin, or replace one of the same name (a new build, or a new
    /// address). An existing plugin keeps its grants and enabled state.
    pub fn put_plugin(&mut self, w: &PluginWrite<'_>, actor: &str) -> Result<PluginRow> {
        let before = self.get_plugin(w.name)?;
        let runtime = if w.wasm.is_some() { "wasm" } else { "external" };
        self.write(|tx| {
            let now = now_ms();
            let (wasm, sha) = w.wasm.map_or((None, None), |(b, h)| (Some(b), Some(h)));
            let manifest = w.manifest.to_string();
            match &before {
                None => {
                    tx.execute(
                        "INSERT INTO plugins (name, runtime, version, manifest, wasm, sha256, address,
                                              grants, enabled, created_at_ms, updated_at_ms, secret)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9, ?9, ?10)",
                        params![
                            w.name, runtime, w.version, manifest, wasm, sha, w.address,
                            w.grants.to_string(), now, w.secret
                        ],
                    )?;
                }
                Some(_) => {
                    tx.execute(
                        "UPDATE plugins SET runtime = ?2, version = ?3, manifest = ?4, wasm = ?5,
                                sha256 = ?6, address = ?7, updated_at_ms = ?8,
                                secret = coalesce(?9, secret)
                         WHERE name = ?1",
                        params![
                            w.name, runtime, w.version, manifest, wasm, sha, w.address, now,
                            w.secret
                        ],
                    )?;
                }
            }
            let op = if before.is_some() { "update_plugin" } else { "add_plugin" };
            let after = json!({
                "name": w.name, "runtime": runtime, "version": w.version, "sha256": sha,
                "address": w.address,
                "secret": w.secret.is_some() || before.as_ref().is_some_and(|b| b.secret.is_some()),
            });
            record_decision(
                tx,
                &Decision {
                    before: before.as_ref().map(summary),
                    after: Some(after),
                    evidence: json!({ "plugin": w.name, "kinds": w.manifest["kinds"] }),
                    ..Decision::new(actor, op)
                },
                now,
            )?;
            Ok(())
        })?;
        self.get_plugin(w.name)?
            .ok_or_else(|| StoreError::NotFound(format!("plugin {}", w.name)))
    }

    /// Change whether a plugin runs, and the grants it runs with.
    pub fn set_plugin(
        &mut self,
        name: &str,
        enabled: Option<bool>,
        grants: Option<&Value>,
        actor: &str,
    ) -> Result<PluginRow> {
        let before = self
            .get_plugin(name)?
            .ok_or_else(|| StoreError::NotFound(format!("plugin {name}")))?;
        self.write(|tx| {
            let now = now_ms();
            let enabled = enabled.unwrap_or(before.enabled);
            let grants = grants.cloned().unwrap_or_else(|| before.grants.clone());
            tx.execute(
                "UPDATE plugins SET enabled = ?2, grants = ?3, updated_at_ms = ?4 WHERE name = ?1",
                params![name, enabled, grants.to_string(), now],
            )?;
            let mut after = summary(&before);
            after["enabled"] = json!(enabled);
            after["grants"] = grants;
            record_decision(
                tx,
                &Decision {
                    before: Some(summary(&before)),
                    after: Some(after),
                    evidence: json!({ "plugin": name }),
                    ..Decision::new(actor, "configure_plugin")
                },
                now,
            )?;
            Ok(())
        })?;
        self.get_plugin(name)?
            .ok_or_else(|| StoreError::NotFound(format!("plugin {name}")))
    }

    /// Set (or, with None, clear) the secret an external plugin shares.
    pub fn set_plugin_secret(
        &mut self,
        name: &str,
        secret: Option<&str>,
        actor: &str,
    ) -> Result<PluginRow> {
        let before = self
            .get_plugin(name)?
            .ok_or_else(|| StoreError::NotFound(format!("plugin {name}")))?;
        self.write(|tx| {
            let now = now_ms();
            tx.execute(
                "UPDATE plugins SET secret = ?2, updated_at_ms = ?3 WHERE name = ?1",
                params![name, secret, now],
            )?;
            let mut after = summary(&before);
            after["secret"] = json!(secret.is_some());
            record_decision(
                tx,
                &Decision {
                    before: Some(summary(&before)),
                    after: Some(after),
                    evidence: json!({ "plugin": name }),
                    ..Decision::new(actor, "set_plugin_secret")
                },
                now,
            )?;
            Ok(())
        })?;
        self.get_plugin(name)?
            .ok_or_else(|| StoreError::NotFound(format!("plugin {name}")))
    }

    pub fn delete_plugin(&mut self, name: &str, actor: &str) -> Result<()> {
        let before = self
            .get_plugin(name)?
            .ok_or_else(|| StoreError::NotFound(format!("plugin {name}")))?;
        self.write(|tx| {
            let now = now_ms();
            tx.execute("DELETE FROM plugins WHERE name = ?1", [name])?;
            record_decision(
                tx,
                &Decision {
                    before: Some(summary(&before)),
                    evidence: json!({ "plugin": name }),
                    ..Decision::new(actor, "delete_plugin")
                },
                now,
            )?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugins_are_added_configured_replaced_and_deleted_with_decisions() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = Db::open(dir.path().join("t.db")).unwrap();
        let v0 = db.plugins_version().unwrap();
        let manifest = json!({"name": "ab", "version": "1", "kinds": ["tracker"]});
        let p = db
            .put_plugin(
                &PluginWrite {
                    name: "ab",
                    version: "1",
                    manifest: &manifest,
                    wasm: Some((b"\0asm", "abc")),
                    address: None,
                    grants: &json!({}),
                    secret: None,
                },
                "op:test",
            )
            .unwrap();
        assert_eq!(
            (p.runtime.as_str(), p.size, p.enabled),
            ("wasm", Some(4), true)
        );
        assert_eq!(db.plugin_wasm("ab").unwrap().unwrap(), b"\0asm");
        assert_ne!(db.plugins_version().unwrap(), v0);
        let p = db
            .set_plugin(
                "ab",
                Some(false),
                Some(&json!({"memory_mb": 512})),
                "op:test",
            )
            .unwrap();
        assert!(!p.enabled);
        assert_eq!(p.grants["memory_mb"], 512);
        // A new build keeps the grants and the enabled state.
        let p = db
            .put_plugin(
                &PluginWrite {
                    name: "ab",
                    version: "2",
                    manifest: &manifest,
                    wasm: Some((b"\0asm2", "def")),
                    address: None,
                    grants: &json!({}),
                    secret: None,
                },
                "op:test",
            )
            .unwrap();
        assert_eq!((p.version.as_str(), p.enabled), ("2", false));
        assert_eq!(p.grants["memory_mb"], 512);
        // A secret is kept, and its decision says only that one is set.
        let p = db
            .set_plugin_secret("ab", Some("0123456789abcdef0123456789abcdef"), "op:test")
            .unwrap();
        assert_eq!(
            p.secret.as_deref(),
            Some("0123456789abcdef0123456789abcdef")
        );
        assert!(!serde_json::to_string(&p).unwrap().contains("0123456789"));
        let d: String = db
            .connection()
            .query_row(
                "SELECT after FROM decisions WHERE op = 'set_plugin_secret'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            d.contains(r#""secret":true"#) && !d.contains("0123456789"),
            "{d}"
        );
        db.delete_plugin("ab", "op:test").unwrap();
        assert!(db.list_plugins().unwrap().is_empty());
        let ops: Vec<String> = db
            .connection()
            .prepare("SELECT op FROM decisions ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            ops,
            [
                "add_plugin",
                "configure_plugin",
                "update_plugin",
                "set_plugin_secret",
                "delete_plugin"
            ]
        );
    }
}
