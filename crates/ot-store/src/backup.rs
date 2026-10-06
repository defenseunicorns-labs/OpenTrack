//! The node's configuration as one document (`opentrack-config`, version
//! 2), and restoring it into an empty node: a backup, or the recipe to
//! rebuild a node from.
//!
//! The document carries everything an admin configured here, secrets
//! included: sources (as stored), output schema versions, correlation,
//! instance and sign-in settings, accounts with their password hashes and
//! history, API token records, the registry, plugins (components and
//! external plugins' secrets included), the track number counters, and (filled by the server, which
//! keeps them as files) the tracker profiles imported here. It never
//! carries the session signing key, sessions, the audit record, the
//! decision log, the track graph or anything in Redis.
//!
//! [`Db::restore_config`] writes a document into a node with no
//! configuration yet (see [`Db::config_present`]), all of it in one
//! transaction. The server validates each section before it gets here.

use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::registry::Entity;
use crate::sqlite::{Db, Decision, Result, StoreError, now_ms, record_decision};

/// The document's `format`.
pub const FORMAT: &str = "opentrack-config";
/// The document's `version`. Version 1 (0.4.0 and earlier) had no `format`
/// and only sources, schema versions, correlation and instance settings.
pub const VERSION: u32 = 2;

/// The whole configuration of a node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigFile {
    /// Always [`FORMAT`].
    pub format: String,
    /// Always [`VERSION`].
    pub version: u32,
    /// The OpenTrack release that wrote it.
    pub opentrack: String,
    /// The site code of the node it came from.
    pub site_code: String,
    pub exported_at: DateTime<Utc>,
    /// Who exported it.
    #[serde(default)]
    pub exported_by: String,
    /// A reminder that the file holds secrets.
    #[serde(default)]
    pub notice: String,
    /// The file's security marking: the highest of its sources' labels,
    /// else the classification banner's text (set by the server).
    #[serde(default)]
    pub marking: String,
    pub sources: Vec<SourceEntry>,
    pub schema_versions: Vec<SchemaEntry>,
    /// The saved correlation settings (`null`: never saved, the defaults).
    pub correlation_settings: Option<Value>,
    /// The saved instance settings (`{}`: never saved).
    pub app_settings: Value,
    /// The saved sign-in settings (`{}`: never saved).
    pub auth_settings: Value,
    pub accounts: Vec<Account>,
    pub api_tokens: Vec<TokenRecord>,
    pub registry: RegistrySection,
    pub plugins: Vec<PluginEntry>,
    /// Tracker profiles imported on the node (not the shipped ones), each
    /// as its file.
    pub tracker_profiles: Vec<Value>,
    /// The next system track number per site code, so a rebuilt node never
    /// issues a number again.
    pub uid_sequences: Vec<UidSequence>,
}

/// A source as stored: its specification with its secrets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceEntry {
    pub id: String,
    pub name: String,
    pub transport: String,
    pub codec: String,
    pub enabled: bool,
    pub priority: i64,
    #[serde(default = "one")]
    pub revision: i64,
    pub spec: Value,
    /// The NATS subject its raw tracks go to, if an admin consented.
    #[serde(default)]
    pub raw_subject: Option<String>,
    #[serde(default)]
    pub created_at_ms: i64,
    #[serde(default)]
    pub updated_at_ms: i64,
}

fn one() -> i64 {
    1
}

/// An output schema version.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchemaEntry {
    pub version: u32,
    /// `draft` or `published`.
    pub status: String,
    #[serde(default)]
    pub published_at_ms: Option<i64>,
    #[serde(default)]
    pub notes: Option<String>,
    pub fields: Vec<Value>,
}

/// An account, with what signing in needs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Account {
    pub id: String,
    pub email: String,
    #[serde(default)]
    pub name: String,
    /// `viewer`, `track_manager` or `admin`.
    pub role: String,
    pub active: bool,
    /// `local` or `saml`.
    pub origin: String,
    /// The password hash (PBKDF2, or Argon2id from before 0.4.0); `null`:
    /// no password sign-in.
    #[serde(default)]
    pub password_hash: Option<String>,
    #[serde(default)]
    pub password_changed_at_ms: Option<i64>,
    #[serde(default)]
    pub must_change_password: bool,
    #[serde(default)]
    pub password_history: Vec<PasswordHistoryEntry>,
    #[serde(default)]
    pub created_at_ms: i64,
    #[serde(default)]
    pub updated_at_ms: i64,
    #[serde(default)]
    pub last_login_at_ms: Option<i64>,
    /// Tokens issued before this are not accepted.
    #[serde(default)]
    pub tokens_valid_from_ms: i64,
    /// Locked until then (an admin's unlock or the lock running out).
    #[serde(default)]
    pub locked_until_ms: Option<i64>,
    #[serde(default)]
    pub active_since_ms: Option<i64>,
    #[serde(default)]
    pub disabled_reason: Option<String>,
    /// A temporary account: turned off when this passes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_ms: Option<i64>,
}

/// An earlier password hash (a new password cannot repeat a recent one).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PasswordHistoryEntry {
    pub hash: String,
    pub set_at_ms: i64,
}

/// An API token's record. The token itself is never stored; it is signed
/// with the session key, so it works again only on a node with that key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenRecord {
    pub jti: String,
    pub name: String,
    /// The account it acts as (an `accounts[].id`).
    pub user_id: String,
    #[serde(default)]
    pub created_by: String,
    #[serde(default)]
    pub created_at_ms: i64,
    pub expires_at_ms: i64,
    #[serde(default)]
    pub revoked_at_ms: Option<i64>,
}

/// The entity registry.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistrySection {
    /// Every entity, as the registry API shapes it.
    pub entities: Vec<Entity>,
}

/// A plugin an operator added.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginEntry {
    pub name: String,
    /// `wasm` or `external`.
    pub runtime: String,
    pub version: String,
    pub manifest: Value,
    /// The component (a WebAssembly plugin), base64.
    #[serde(default, with = "base64_bytes")]
    pub wasm: Option<Vec<u8>>,
    /// The component's SHA-256 (hex).
    #[serde(default)]
    pub sha256: Option<String>,
    /// Where an external plugin listens.
    #[serde(default)]
    pub address: Option<String>,
    /// The secret an external plugin shares (or a `${env:NAME}` reference).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    pub grants: Value,
    pub enabled: bool,
    #[serde(default)]
    pub created_at_ms: i64,
    #[serde(default)]
    pub updated_at_ms: i64,
}

/// The next system track number for a site code.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UidSequence {
    pub site: String,
    pub next_sequence: i64,
}

mod base64_bytes {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &Option<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(b) => s.serialize_str(&STANDARD.encode(b)),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {
        Option::<String>::deserialize(d)?
            .map(|t| STANDARD.decode(t.trim()).map_err(serde::de::Error::custom))
            .transpose()
    }
}

impl ConfigFile {
    /// How much of each section it holds.
    pub fn counts(&self) -> Value {
        json!({
            "sources": self.sources.len(),
            "schema_versions": self.schema_versions.len(),
            "accounts": self.accounts.len(),
            "api_tokens": self.api_tokens.len(),
            "entities": self.registry.entities.len(),
            "plugins": self.plugins.len(),
            "tracker_profiles": self.tracker_profiles.len(),
            "uid_sequences": self.uid_sequences.len(),
            "correlation_settings": self.correlation_settings.is_some(),
            "app_settings": saved(&self.app_settings),
            "auth_settings": saved(&self.auth_settings),
        })
    }
}

/// Whether a settings document says anything (`{}` and `null` do not).
fn saved(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Object(m) => !m.is_empty(),
        _ => true,
    }
}

fn count(conn: &Connection, sql: &str) -> Result<i64> {
    Ok(conn.query_row(sql, [], |r| r.get(0))?)
}

/// What configuration a database holds, one line each (empty: none). An
/// empty node may have its first admin account (made on first start) and
/// no API tokens; the built-in output schema version 1; track state. A
/// node that imported a configuration once is never empty again.
fn present(conn: &Connection) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let note = |out: &mut Vec<String>, n: i64, what: &str| {
        if n > 0 {
            out.push(format!("{n} {what}"));
        }
    };
    note(
        &mut out,
        count(conn, "SELECT count(*) FROM sources")?,
        "source(s)",
    );
    note(
        &mut out,
        count(
            conn,
            "SELECT count(*) FROM schema_versions WHERE version > 1",
        )?,
        "output schema version(s) beyond the built-in one",
    );
    if count(conn, "SELECT count(*) FROM correlation_settings")? > 0 {
        out.push("saved correlation settings".into());
    }
    if count(conn, "SELECT count(*) FROM app_settings")? > 0 {
        out.push("saved instance settings (Settings → General)".into());
    }
    if count(conn, "SELECT count(*) FROM auth_settings")? > 0 {
        out.push("saved sign-in settings (Settings → Security)".into());
    }
    let users = count(conn, "SELECT count(*) FROM users")?;
    let admins = count(conn, "SELECT count(*) FROM users WHERE role = 'admin'")?;
    if users > 1 || users == 1 && admins == 0 {
        note(
            &mut out,
            users,
            "account(s) (an empty node has at most its first admin)",
        );
    }
    note(
        &mut out,
        count(conn, "SELECT count(*) FROM api_tokens")?,
        "API token(s)",
    );
    note(
        &mut out,
        count(conn, "SELECT count(*) FROM registry_entities")?,
        "registry entit(ies)",
    );
    note(
        &mut out,
        count(conn, "SELECT count(*) FROM plugins")?,
        "plugin(s)",
    );
    // Its one admin may be an imported one: a node imports once.
    if count(
        conn,
        "SELECT count(*) FROM decisions WHERE op = 'import_config'",
    )? > 0
    {
        out.push("an imported configuration".into());
    }
    Ok(out)
}

impl Db {
    /// What configuration this node holds (see [`present`]); empty when a
    /// configuration may be imported.
    pub fn config_present(&self) -> Result<Vec<String>> {
        present(self.connection())
    }

    /// The configuration in the database, as a document (its header and
    /// tracker profiles are the caller's to fill).
    pub fn config_snapshot(&self) -> Result<ConfigFile> {
        let conn = self.connection();
        let sources = self
            .list_sources()?
            .into_iter()
            .map(|r| SourceEntry {
                id: r.id,
                name: r.name,
                transport: r.transport,
                codec: r.codec,
                enabled: r.enabled,
                priority: r.priority,
                revision: r.revision,
                spec: r.spec,
                raw_subject: r.raw_subject,
                created_at_ms: r.created_at_ms,
                updated_at_ms: r.updated_at_ms,
            })
            .collect();
        let schema_versions = self
            .schema_versions()?
            .into_iter()
            .map(|v| SchemaEntry {
                version: v.version,
                status: v.status,
                published_at_ms: v.published_at_ms,
                notes: v.notes,
                fields: v.fields,
            })
            .collect();
        let mut accounts: Vec<Account> = conn
            .prepare(
                "SELECT id, email, name, role, active, origin, password_hash,
                        password_changed_at_ms, must_change_password, created_at_ms,
                        updated_at_ms, last_login_at_ms, tokens_valid_from_ms,
                        locked_until_ms, active_since_ms, disabled_reason, expires_at_ms
                 FROM users ORDER BY email",
            )?
            .query_map([], |r| {
                Ok(Account {
                    id: r.get(0)?,
                    email: r.get(1)?,
                    name: r.get(2)?,
                    role: r.get(3)?,
                    active: r.get(4)?,
                    origin: r.get(5)?,
                    password_hash: r.get(6)?,
                    password_changed_at_ms: r.get(7)?,
                    must_change_password: r.get(8)?,
                    password_history: Vec::new(),
                    created_at_ms: r.get(9)?,
                    updated_at_ms: r.get(10)?,
                    last_login_at_ms: r.get(11)?,
                    tokens_valid_from_ms: r.get(12)?,
                    locked_until_ms: r.get(13)?,
                    active_since_ms: r.get(14)?,
                    disabled_reason: r.get(15)?,
                    expires_at_ms: r.get(16)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        let mut history = conn.prepare(
            "SELECT hash, set_at_ms FROM password_history WHERE user_id = ?1
             ORDER BY set_at_ms, rowid",
        )?;
        for a in &mut accounts {
            a.password_history = history
                .query_map([&a.id], |r| {
                    Ok(PasswordHistoryEntry {
                        hash: r.get(0)?,
                        set_at_ms: r.get(1)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?;
        }
        let api_tokens = conn
            .prepare(
                "SELECT jti, name, user_id, created_by, created_at_ms, expires_at_ms, revoked_at_ms
                 FROM api_tokens ORDER BY created_at_ms, jti",
            )?
            .query_map([], |r| {
                Ok(TokenRecord {
                    jti: r.get(0)?,
                    name: r.get(1)?,
                    user_id: r.get(2)?,
                    created_by: r.get(3)?,
                    created_at_ms: r.get(4)?,
                    expires_at_ms: r.get(5)?,
                    revoked_at_ms: r.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        let entity_ids: Vec<String> = conn
            .prepare("SELECT id FROM registry_entities ORDER BY id")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let entities = entity_ids
            .iter()
            .filter_map(|id| self.entity(id).transpose())
            .collect::<Result<Vec<_>>>()?;
        let plugins = conn
            .prepare(
                "SELECT name, runtime, version, manifest, wasm, sha256, address, grants, enabled,
                        created_at_ms, updated_at_ms, secret
                 FROM plugins ORDER BY name",
            )?
            .query_map([], |r| {
                let manifest: String = r.get(3)?;
                let grants: String = r.get(7)?;
                Ok(PluginEntry {
                    name: r.get(0)?,
                    runtime: r.get(1)?,
                    version: r.get(2)?,
                    manifest: serde_json::from_str(&manifest).unwrap_or(Value::Null),
                    wasm: r.get(4)?,
                    sha256: r.get(5)?,
                    address: r.get(6)?,
                    secret: r.get(11)?,
                    grants: serde_json::from_str(&grants).unwrap_or_else(|_| json!({})),
                    enabled: r.get(8)?,
                    created_at_ms: r.get(9)?,
                    updated_at_ms: r.get(10)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        let uid_sequences = conn
            .prepare("SELECT site, next_sequence FROM uid_sequences ORDER BY site")?
            .query_map([], |r| {
                Ok(UidSequence {
                    site: r.get(0)?,
                    next_sequence: r.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(ConfigFile {
            format: FORMAT.into(),
            version: VERSION,
            opentrack: String::new(),
            site_code: String::new(),
            exported_at: Utc::now(),
            exported_by: String::new(),
            notice: String::new(),
            marking: String::new(),
            sources,
            schema_versions,
            correlation_settings: self.correlation_settings()?,
            app_settings: self.app_settings()?,
            auth_settings: self.auth_settings()?,
            accounts,
            api_tokens,
            registry: RegistrySection { entities },
            plugins,
            tracker_profiles: Vec::new(),
            uid_sequences,
        })
    }

    /// Restore a configuration (checked by the caller) into this node, which
    /// must hold none ([`Db::config_present`]): one transaction, recorded as
    /// one `import_config` decision. The imported accounts replace the first
    /// admin an empty node may have (a document with no accounts keeps it). `finish` runs last, before the commit
    /// (the server writes the tracker profile files there); if it fails,
    /// nothing is written. Returns the decision id.
    pub fn restore_config(
        &mut self,
        f: &ConfigFile,
        actor: &str,
        finish: impl FnOnce() -> Result<()>,
    ) -> Result<i64> {
        if f.format != FORMAT || f.version != VERSION {
            return Err(StoreError::Conflict(format!(
                "not an {FORMAT} version {VERSION} document"
            )));
        }
        self.write(|tx| {
            let there = present(tx)?;
            if !there.is_empty() {
                return Err(StoreError::Conflict(format!(
                    "this node is not empty: it has {}",
                    there.join(", ")
                )));
            }
            let now = now_ms();
            let replaced: Vec<String> = if f.accounts.is_empty() {
                Vec::new()
            } else {
                tx
                .prepare("SELECT email FROM users ORDER BY email")?
                    .query_map([], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?
            };
            let d = Decision::new(actor, "import_config")
                .reason("configuration imported into an empty node")
                .evidence(json!({
                    "counts": f.counts(),
                    "from_site": f.site_code,
                    "from_opentrack": f.opentrack,
                    "exported_at": f.exported_at,
                    "exported_by": f.exported_by,
                    "accounts_replaced": replaced,
                }));
            let decision_id = record_decision(tx, &d, now)?;

            // Accounts: the first admin (and its sessions) make way, unless
            // the file has none (its node ran with sign-in off).
            if !f.accounts.is_empty() {
                tx.execute("DELETE FROM users", [])?;
            }
            for a in &f.accounts {
                tx.execute(
                    "INSERT INTO users (id, email, name, role, active, password_hash, origin,
                         created_at_ms, updated_at_ms, last_login_at_ms, tokens_valid_from_ms,
                         password_changed_at_ms, must_change_password, locked_until_ms,
                         active_since_ms, disabled_reason, expires_at_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
                    params![
                        a.id,
                        a.email.trim(),
                        a.name,
                        a.role,
                        a.active,
                        a.password_hash,
                        a.origin,
                        if a.created_at_ms > 0 { a.created_at_ms } else { now },
                        if a.updated_at_ms > 0 { a.updated_at_ms } else { now },
                        a.last_login_at_ms,
                        a.tokens_valid_from_ms,
                        a.password_changed_at_ms,
                        a.must_change_password,
                        a.locked_until_ms,
                        a.active_since_ms,
                        a.disabled_reason,
                        a.expires_at_ms,
                    ],
                )?;
                for h in &a.password_history {
                    tx.execute(
                        "INSERT INTO password_history (user_id, hash, set_at_ms) VALUES (?1, ?2, ?3)",
                        params![a.id, h.hash, h.set_at_ms],
                    )?;
                }
            }
            for t in &f.api_tokens {
                tx.execute(
                    "INSERT INTO api_tokens (jti, name, user_id, created_by, created_at_ms,
                         expires_at_ms, revoked_at_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        t.jti,
                        t.name,
                        t.user_id,
                        t.created_by,
                        t.created_at_ms,
                        t.expires_at_ms,
                        t.revoked_at_ms
                    ],
                )?;
            }

            // Output schema versions (1 is built in).
            for v in f.schema_versions.iter().filter(|v| v.version > 1) {
                tx.execute(
                    "INSERT INTO schema_versions (version, status, published_at_ms, notes)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![v.version, v.status, v.published_at_ms, v.notes],
                )?;
                for fd in &v.fields {
                    let text = |k: &str| fd.get(k).and_then(Value::as_str).map(str::to_owned);
                    tx.execute(
                        "INSERT INTO extension_fields
                           (schema_version, key, type, unit, required, default_json, enum_values,
                            description, builtin)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                        params![
                            v.version,
                            text("key"),
                            text("type"),
                            text("unit"),
                            fd.get("required").and_then(Value::as_bool).unwrap_or(false),
                            fd.get("default").filter(|x| !x.is_null()).map(Value::to_string),
                            fd.get("enum_values")
                                .filter(|x| !x.is_null())
                                .map(Value::to_string),
                            text("description"),
                            text("builtin"),
                        ],
                    )?;
                }
            }

            // Sources, at the revision they had; a raw output's consent is
            // this import (the original consent is in the old node's log).
            for s in &f.sources {
                let spec = s.spec.to_string();
                let revision = s.revision.max(1);
                tx.execute(
                    "INSERT INTO sources (id, name, transport, codec, settings, enabled, priority,
                         raw_subject, raw_consent, revision, created_at_ms, updated_at_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                    params![
                        s.id,
                        s.name,
                        s.transport,
                        s.codec,
                        spec,
                        s.enabled,
                        s.priority,
                        s.raw_subject,
                        s.raw_subject.as_ref().map(|_| decision_id),
                        revision,
                        if s.created_at_ms > 0 { s.created_at_ms } else { now },
                        now,
                    ],
                )?;
                tx.execute(
                    "INSERT INTO source_revisions (source_id, revision, config, decision_id, created_at_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![s.id, revision, spec, decision_id, now],
                )?;
            }

            // Settings: only those the old node had saved.
            if let Some(c) = &f.correlation_settings {
                tx.execute(
                    "INSERT INTO correlation_settings (id, settings, updated_at_ms, decision_id)
                     VALUES (1, ?1, ?2, ?3)",
                    params![c.to_string(), now, decision_id],
                )?;
            }
            if saved(&f.app_settings) {
                tx.execute(
                    "INSERT INTO app_settings (id, settings, updated_at_ms, decision_id)
                     VALUES (1, ?1, ?2, ?3)",
                    params![f.app_settings.to_string(), now, decision_id],
                )?;
            }
            if saved(&f.auth_settings) {
                tx.execute(
                    "INSERT INTO auth_settings (id, settings, updated_at_ms) VALUES (1, ?1, ?2)",
                    params![f.auth_settings.to_string(), now],
                )?;
            }

            for e in &f.registry.entities {
                crate::registry::restore_entity(tx, e, decision_id, now)?;
            }

            for p in &f.plugins {
                tx.execute(
                    "INSERT INTO plugins (name, runtime, version, manifest, wasm, sha256, address,
                         grants, enabled, created_at_ms, updated_at_ms, secret)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                    params![
                        p.name,
                        p.runtime,
                        p.version,
                        p.manifest.to_string(),
                        p.wasm,
                        p.sha256,
                        p.address,
                        p.grants.to_string(),
                        p.enabled,
                        if p.created_at_ms > 0 { p.created_at_ms } else { now },
                        now,
                        p.secret,
                    ],
                )?;
            }

            // Track numbers: never below what either node has issued.
            for u in &f.uid_sequences {
                tx.execute(
                    "INSERT INTO uid_sequences (site, next_sequence) VALUES (?1, ?2)
                     ON CONFLICT(site) DO UPDATE SET
                         next_sequence = max(next_sequence, excluded.next_sequence)",
                    params![u.site, u.next_sequence],
                )?;
            }

            finish()?;
            Ok(decision_id)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NewUser, PluginWrite, SourceWrite};
    use ot_core::SiteCode;

    fn configured() -> Db {
        let mut db = Db::open_in_memory().unwrap();
        let spec = json!({"id": "ais", "secret": "s3cret"});
        db.put_source(
            &SourceWrite {
                id: "ais",
                name: "AIS",
                transport: "websocket",
                codec: "json",
                priority: 50,
                spec: &spec,
            },
            "op:test",
        )
        .unwrap();
        db.set_source_enabled("ais", true, "op:test").unwrap();
        db.set_raw_output("ais", Some("opentrack.raw.ais"), "op:test")
            .unwrap();
        db.put_schema_draft(&[json!({"key": "hull", "type": "string"})], None, "op:test")
            .unwrap();
        db.publish_schema_draft("op:test").unwrap();
        db.put_app_settings(&json!({"site_name": "Garden Island"}), "op:test")
            .unwrap();
        db.put_auth_settings(&json!({"inactivity": {"exempt": ["ann@example.org"]}}))
            .unwrap();
        db.save_correlation_settings(&json!({"mode": "suggest"}), Decision::new("op", "c"))
            .unwrap();
        db.create_user(&NewUser {
            id: "u1",
            email: "ann@example.org",
            name: "Ann",
            role: "admin",
            password_hash: Some("hash-1"),
            origin: "local",
        })
        .unwrap();
        db.set_password("u1", Some("hash-2"), false, 5).unwrap();
        db.add_api_token("t1", "feed", "u1", "ann@example.org", now_ms() + 60_000)
            .unwrap();
        db.save_entity(
            &Entity {
                name: Some("TED STEVENS".into()),
                identifiers: vec![crate::RegistryIdentifier {
                    scheme: "mmsi".into(),
                    value: "338000001".into(),
                    expected_name: None,
                    source: None,
                }],
                ..Default::default()
            },
            "op:test",
        )
        .unwrap();
        db.put_plugin(
            &PluginWrite {
                name: "ab",
                version: "1",
                manifest: &json!({"name": "ab"}),
                wasm: Some((b"\0asm", "abc")),
                address: None,
                grants: &json!({"memory_mb": 64}),
                secret: None,
            },
            "op:test",
        )
        .unwrap();
        let site = SiteCode::new("OTK").unwrap();
        for k in ["a", "b", "c"] {
            db.create_system_track(site, "ais", k, Decision::new("engine", "create"))
                .unwrap();
        }
        db
    }

    #[test]
    fn a_snapshot_restores_into_an_empty_node() {
        let db = configured();
        let snap = db.config_snapshot().unwrap();
        assert_eq!(snap.accounts[0].password_hash.as_deref(), Some("hash-2"));
        assert_eq!(snap.accounts[0].password_history.len(), 2);
        assert_eq!(snap.plugins[0].wasm.as_deref(), Some(&b"\0asm"[..]));
        assert_eq!(snap.uid_sequences[0].next_sequence, 4);
        // Through JSON, as a file would go.
        let text = serde_json::to_string(&snap).unwrap();
        assert!(text.contains("\"wasm\":\"AGFzbQ==\""), "base64");
        let back: ConfigFile = serde_json::from_str(&text).unwrap();
        assert_eq!(back, snap);

        let mut fresh = Db::open_in_memory().unwrap();
        // Its first admin does not make it non-empty.
        fresh
            .create_user(&NewUser {
                id: "boot",
                email: "admin@opentrack.local",
                name: "Administrator",
                role: "admin",
                password_hash: Some("x"),
                origin: "local",
            })
            .unwrap();
        assert!(fresh.config_present().unwrap().is_empty());
        fresh.restore_config(&back, "cli", || Ok(())).unwrap();
        let mut again = fresh.config_snapshot().unwrap();
        again.exported_at = snap.exported_at;
        // Sources' and plugins' updated times are the import's.
        for (a, b) in again.sources.iter_mut().zip(&snap.sources) {
            a.updated_at_ms = b.updated_at_ms;
        }
        for (a, b) in again.plugins.iter_mut().zip(&snap.plugins) {
            a.updated_at_ms = b.updated_at_ms;
        }
        assert_eq!(again, snap);
        assert!(
            fresh
                .user_by_email("admin@opentrack.local")
                .unwrap()
                .is_none()
        );
        let src = fresh.get_source("ais").unwrap().unwrap();
        assert_eq!(src.raw_subject.as_deref(), Some("opentrack.raw.ais"));
        assert_eq!(fresh.source_revisions("ais").unwrap().len(), 1);
        // Track numbers carry on.
        let (uid, _) = fresh
            .create_system_track(
                SiteCode::new("OTK").unwrap(),
                "ais",
                "d",
                Decision::new("engine", "create"),
            )
            .unwrap();
        assert_eq!(uid.to_string(), "OTK000000004");
        let ops: Vec<String> = fresh
            .connection()
            .prepare("SELECT op FROM audit ORDER BY seq")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert!(ops.contains(&"import_config".to_owned()), "{ops:?}");
    }

    #[test]
    fn a_file_without_accounts_keeps_the_first_admin() {
        let mut snap = configured().config_snapshot().unwrap();
        snap.accounts.clear();
        snap.api_tokens.clear();
        let mut fresh = Db::open_in_memory().unwrap();
        fresh
            .create_user(&NewUser {
                id: "boot",
                email: "admin@opentrack.local",
                name: "Administrator",
                role: "admin",
                password_hash: Some("x"),
                origin: "local",
            })
            .unwrap();
        fresh.restore_config(&snap, "cli", || Ok(())).unwrap();
        assert!(
            fresh
                .user_by_email("admin@opentrack.local")
                .unwrap()
                .is_some()
        );
        assert_eq!(fresh.list_sources().unwrap().len(), 1);
    }

    #[test]
    fn a_configured_node_refuses_and_a_failure_writes_nothing() {
        let db = configured();
        let snap = db.config_snapshot().unwrap();
        let mut other = configured();
        let err = other.restore_config(&snap, "cli", || Ok(())).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("not empty") && msg.contains("1 source(s)"),
            "{msg}"
        );
        // Nor is a node that imported one, even with only its admin left.
        let mut once = Db::open_in_memory().unwrap();
        let mut only_admin = snap.clone();
        only_admin.accounts.retain(|a| a.role == "admin");
        only_admin.api_tokens.clear();
        only_admin.sources.clear();
        only_admin.schema_versions.retain(|v| v.version == 1);
        only_admin.correlation_settings = None;
        only_admin.app_settings = json!({});
        only_admin.auth_settings = json!({});
        only_admin.registry.entities.clear();
        only_admin.plugins.clear();
        once.restore_config(&only_admin, "cli", || Ok(())).unwrap();
        let there = once.config_present().unwrap();
        assert_eq!(there, ["an imported configuration"]);

        let mut fresh = Db::open_in_memory().unwrap();
        let err = fresh
            .restore_config(&snap, "cli", || {
                Err(StoreError::Conflict("profiles: disk full".into()))
            })
            .unwrap_err();
        assert!(err.to_string().contains("disk full"));
        assert!(fresh.config_present().unwrap().is_empty());
        assert!(fresh.list_sources().unwrap().is_empty());
        assert_eq!(fresh.user_count().unwrap(), 0);
    }
}
