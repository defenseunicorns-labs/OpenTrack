//! The entity registry in SQLite.
//!
//! An entity is one real-world object. It has identifiers (one to many, any
//! scheme: `mmsi:338924210`, `icao:ae1234`, `elnot:NL504`...) that tracks
//! resolve through, a status, the OTH-GOLD minimum a track publishes (name,
//! class name, domain, affiliation, track type, symbol) and free-form typed
//! attributes. Source pipelines choose which entity fields populate their
//! tracks and which the feed updates. Every save is a revision and a decision.

use std::collections::BTreeSet;

use ot_core::{Affiliation, Domain, TrackType};
use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::sqlite::{Db, Decision, Result, StoreError, now_ms, record_decision};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegistryIdentifier {
    pub scheme: String,
    pub value: String,
    /// Name the object is expected to broadcast under this identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl RegistryIdentifier {
    /// `scheme:value`, the form identifiers are shown and logged in.
    pub fn label(&self) -> String {
        format!("{}:{}", self.scheme, self.value)
    }
}

/// The type of an attribute's value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttrType {
    #[default]
    Text,
    Number,
    Boolean,
    /// RFC 3339 date and time, as text.
    Datetime,
    /// Any JSON value (a list, an object...).
    Json,
}

impl AttrType {
    /// The type a JSON value most naturally has.
    pub fn infer(v: &Value) -> Self {
        match v {
            Value::Number(_) => Self::Number,
            Value::Bool(_) => Self::Boolean,
            Value::String(_) | Value::Null => Self::Text,
            _ => Self::Json,
        }
    }

    /// `v` as this type: numbers and booleans may arrive as text (a sheet
    /// cell, a feed field), anything becomes text. `Null` stays null.
    pub fn coerce(self, v: &Value) -> std::result::Result<Value, String> {
        let text = |v: &Value| match v {
            Value::String(s) => s.trim().to_owned(),
            other => other.to_string(),
        };
        Ok(match (self, v) {
            (_, Value::Null) => Value::Null,
            (Self::Text, Value::String(_)) | (Self::Json, _) => v.clone(),
            (Self::Text, other) => Value::String(other.to_string()),
            (Self::Number, Value::Number(_)) | (Self::Boolean, Value::Bool(_)) => v.clone(),
            (Self::Number, other) => {
                let t = text(other);
                if t.is_empty() {
                    return Ok(Value::Null);
                }
                let n = t
                    .parse::<f64>()
                    .ok()
                    .filter(|n| n.is_finite())
                    .ok_or_else(|| format!("{t:?} is not a number"))?;
                if n.fract() == 0.0 && n.abs() < 9e15 {
                    json!(n as i64)
                } else {
                    json!(n)
                }
            }
            (Self::Boolean, other) => match text(other).to_ascii_lowercase().as_str() {
                "" => Value::Null,
                "true" | "yes" | "1" => Value::Bool(true),
                "false" | "no" | "0" => Value::Bool(false),
                t => return Err(format!("{t:?} is not true or false")),
            },
            (Self::Datetime, other) => {
                let t = text(other);
                if t.is_empty() {
                    return Ok(Value::Null);
                }
                chrono::DateTime::parse_from_rfc3339(&t)
                    .map_err(|_| format!("{t:?} is not a date and time (RFC 3339)"))?;
                Value::String(t)
            }
        })
    }
}

/// A track manager's override of whether an entity's tracks are published.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Publish {
    /// Published as soon as they exist, confirmed or not, whatever reports
    /// for them and whatever the output filter says.
    Always,
    /// Kept inside OpenTrack (withdrawn downstream if already published).
    Never,
}

/// A free-form attribute: a key, its type and its value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attribute {
    pub key: String,
    #[serde(rename = "type", default)]
    pub kind: AttrType,
    #[serde(default)]
    pub value: Value,
}

/// Entity field names that are not attributes.
pub const RESERVED: &[&str] = &[
    "id",
    "entity_id",
    "status",
    "name",
    "class_name",
    "domain",
    "affiliation",
    "track_type",
    "cot_type",
    "sidc",
    "identifiers",
    "attributes",
    "publish",
];

/// The OTH-GOLD minimum an entity carries, by field name.
pub const MINIMUM: &[&str] = &[
    "name",
    "class_name",
    "domain",
    "affiliation",
    "track_type",
    "cot_type",
    "sidc",
];

/// One real-world object: the API, import and revision shape.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Entity {
    /// Empty when creating: one is assigned.
    #[serde(default)]
    pub id: String,
    #[serde(default = "active")]
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// OTH-GOLD class name (e.g. the ship class).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<Domain>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub affiliation: Option<Affiliation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_type: Option<TrackType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cot_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidc: Option<String>,
    /// Whether its tracks are published: always, never, or (unset) by the
    /// engine's rules.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publish: Option<Publish>,
    #[serde(default)]
    pub identifiers: Vec<RegistryIdentifier>,
    #[serde(default)]
    pub attributes: Vec<Attribute>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default)]
    pub updated_at_ms: i64,
    /// Registry fields in an earlier import shape: read once into the
    /// minimum and attributes (see [`Entity::normalize`]), never written.
    #[serde(default, skip_serializing)]
    pub fields: Option<Map<String, Value>>,
}

fn active() -> String {
    "active".into()
}

/// Normalise an identifier scheme. Schemes are open-ended (`mmsi`, `icao`,
/// `imo`, `elnot`, `hull`, ...), but every identifier must name one: lower
/// case, starting with a letter or digit, then letters, digits, `_`, `-`
/// or `.`, at most 32 characters.
pub fn normalize_scheme(scheme: &str) -> Result<String> {
    let s = scheme.trim().to_ascii_lowercase();
    let ok = !s.is_empty()
        && s.len() <= 32
        && s.as_bytes()[0].is_ascii_alphanumeric()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'));
    if ok {
        Ok(s)
    } else {
        Err(StoreError::Conflict(format!(
            "invalid identifier scheme {scheme:?}"
        )))
    }
}

fn nonempty(s: &Option<String>) -> Option<String> {
    s.as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn enum_text<T: Serialize>(v: &Option<T>) -> Option<String> {
    v.as_ref()
        .and_then(|v| serde_json::to_value(v).ok())
        .and_then(|v| v.as_str().map(str::to_owned))
}

fn parse_enum<T: serde::de::DeserializeOwned>(
    what: &str,
    v: &Value,
) -> std::result::Result<Option<T>, String> {
    match v {
        Value::Null => Ok(None),
        Value::String(s) if s.trim().is_empty() => Ok(None),
        Value::String(s) => serde_json::from_value(Value::String(
            s.trim().to_ascii_lowercase().replace([' ', '-'], "_"),
        ))
        .map(Some)
        .map_err(|_| format!("{s:?} is not a {what}")),
        other => Err(format!("{other} is not a {what}")),
    }
}

impl Entity {
    /// e.g. `USS HARRY S TRUMAN mmsi:338000001 imo:9876543 elnot:NL504`.
    pub fn label(&self) -> String {
        let mut parts = vec![self.name.clone().unwrap_or_else(|| self.id.clone())];
        parts.extend(self.identifiers.iter().map(RegistryIdentifier::label));
        parts.join(" ")
    }

    pub fn attribute(&self, key: &str) -> Option<&Attribute> {
        self.attributes.iter().find(|a| a.key == key)
    }

    /// A field by name: one of the minimum, or an attribute's value.
    pub fn field(&self, key: &str) -> Option<Value> {
        let s = |v: &Option<String>| v.clone().map(Value::String);
        match key {
            "id" | "entity_id" => Some(Value::String(self.id.clone())),
            "status" => Some(Value::String(self.status.clone())),
            "name" => s(&self.name),
            "class_name" => s(&self.class_name),
            "domain" => enum_text(&self.domain).map(Value::String),
            "affiliation" => enum_text(&self.affiliation).map(Value::String),
            "track_type" => enum_text(&self.track_type).map(Value::String),
            "cot_type" => s(&self.cot_type),
            "sidc" => s(&self.sidc),
            "publish" => enum_text(&self.publish).map(Value::String),
            _ => self
                .attribute(key)
                .map(|a| a.value.clone())
                .filter(|v| !v.is_null()),
        }
    }

    /// Every field with a value, flat: the minimum and the attributes.
    pub fn fields_map(&self) -> Map<String, Value> {
        let mut out: Map<String, Value> = MINIMUM
            .iter()
            .filter_map(|k| self.field(k).map(|v| ((*k).to_owned(), v)))
            .collect();
        for a in &self.attributes {
            if !a.value.is_null() {
                out.insert(a.key.clone(), a.value.clone());
            }
        }
        if let Some(p) = self.field("publish") {
            out.insert("publish".into(), p);
        }
        out
    }

    /// Set a field from a feed or a sheet: a minimum field is parsed (an
    /// enum from its name), an attribute keeps its type (a new one takes the
    /// value's). Returns whether anything changed.
    pub fn set_field(&mut self, key: &str, v: &Value) -> std::result::Result<bool, String> {
        let text = || match v {
            Value::Null => Ok(None),
            Value::String(s) => Ok(Some(s.trim().to_owned()).filter(|s| !s.is_empty())),
            Value::Number(_) | Value::Bool(_) => Ok(Some(v.to_string())),
            _ => Err(format!("{key} takes text")),
        };
        let before = self.clone();
        match key {
            "name" => self.name = text()?,
            "class_name" => self.class_name = text()?,
            "cot_type" => self.cot_type = text()?,
            "sidc" => self.sidc = text()?,
            "domain" => self.domain = parse_enum("domain", v)?,
            "affiliation" => self.affiliation = parse_enum("affiliation", v)?,
            "track_type" => self.track_type = parse_enum("track type", v)?,
            "publish" => {
                self.publish = match v.as_str().map(|s| s.trim().to_ascii_lowercase()) {
                    Some(s) if s == "automatic" || s == "auto" => None,
                    _ => parse_enum("publish setting (always or never)", v)?,
                }
            }
            k if RESERVED.contains(&k) => return Err(format!("{k} cannot be set")),
            k => match self.attributes.iter_mut().find(|a| a.key == k) {
                Some(a) => a.value = a.kind.coerce(v)?,
                None if v.is_null() => {}
                None => self.attributes.push(Attribute {
                    key: k.to_owned(),
                    kind: AttrType::infer(v),
                    value: v.clone(),
                }),
            },
        }
        Ok(*self != before)
    }

    /// Fold an earlier import shape's `fields` in: `cot` is the CoT type,
    /// `ship_class` the class name, everything else an attribute. Values
    /// already set win.
    pub fn normalize(&mut self) {
        let Some(fields) = self.fields.take() else {
            return;
        };
        for (k, v) in fields {
            let target = match k.as_str() {
                "cot" => "cot_type",
                "ship_class" => "class_name",
                other => other,
            };
            if v.is_null() || self.field(target).is_some() {
                continue;
            }
            let _ = self.set_field(target, &v);
        }
    }

    /// Check the entity before saving: status, identifiers, attributes.
    /// Trims text and normalises identifier schemes.
    pub fn validate(&mut self) -> Result<()> {
        let bad = |m: String| Err(StoreError::Conflict(m));
        self.normalize();
        if self.status.trim().is_empty() {
            self.status = active();
        }
        if !matches!(self.status.as_str(), "active" | "retired") {
            return bad("status must be active or retired".into());
        }
        self.name = nonempty(&self.name);
        self.class_name = nonempty(&self.class_name);
        self.cot_type = nonempty(&self.cot_type);
        self.sidc = nonempty(&self.sidc);
        let mut seen = BTreeSet::new();
        for i in &mut self.identifiers {
            i.scheme = normalize_scheme(&i.scheme)?;
            i.value = i.value.trim().to_owned();
            if i.value.is_empty() {
                return bad(format!("identifier of scheme {} has no value", i.scheme));
            }
            i.expected_name = nonempty(&i.expected_name);
            if !seen.insert((i.scheme.clone(), i.value.clone())) {
                return bad(format!("identifier {} is listed twice", i.label()));
            }
        }
        let mut keys = BTreeSet::new();
        for a in &mut self.attributes {
            a.key = a.key.trim().to_owned();
            let k = a.key.as_str();
            if k.is_empty() || k.chars().count() > 64 {
                return bad("an attribute key is 1 to 64 characters".into());
            }
            if RESERVED.contains(&k) {
                return bad(format!("{k} is an entity field, not an attribute"));
            }
            if !keys.insert(k.to_owned()) {
                return bad(format!("attribute {k} is listed twice"));
            }
            a.value = a
                .kind
                .coerce(&a.value)
                .map_err(|e| StoreError::Conflict(format!("attribute {k}: {e}")))?;
        }
        Ok(())
    }
}

/// A short, readable id for an entity created from the UI or a sheet.
pub fn new_entity_id() -> String {
    let t = chrono::Utc::now().timestamp_micros() as u64;
    let mut n = t ^ (u64::from(std::process::id()) << 40);
    const ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut s = Vec::new();
    while n > 0 {
        s.push(ALPHABET[(n % 36) as usize]);
        n /= 36;
    }
    s.reverse();
    format!("ent-{}", String::from_utf8(s).unwrap_or_default())
}

/// One active identifier with its entity's fields, for lookup snapshots.
#[derive(Debug, Clone, PartialEq)]
pub struct RegistryRow {
    pub scheme: String,
    pub value: String,
    pub entity_id: String,
    pub name: Option<String>,
    pub expected_name: Option<String>,
    /// The entity's fields, flat: the minimum and the attributes.
    pub fields: Map<String, Value>,
}

/// One saved version of an entity.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EntityRevision {
    pub id: i64,
    pub entity: Value,
    pub saved_at_ms: i64,
    pub decision_id: Option<i64>,
    pub actor: Option<String>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ImportCounts {
    pub entities_created: u64,
    pub entities_updated: u64,
    pub identifiers: u64,
    /// Identifiers skipped because another entity already holds them.
    pub conflicts: u64,
}

type EntityColumns = (
    Option<String>,
    String,
    Option<String>,
    [Option<String>; 4],
    Option<String>,
    Option<String>,
    String,
    Option<String>,
    i64,
);

fn load(conn: &rusqlite::Connection, id: &str) -> Result<Option<Entity>> {
    let row: Option<EntityColumns> = conn
        .query_row(
            "SELECT name, status, class_name, domain, affiliation, track_type,
                    cot_type, sidc, attributes, source, updated_at_ms, publish
             FROM registry_entities WHERE id = ?1",
            [id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    [r.get(3)?, r.get(4)?, r.get(5)?, r.get(11)?],
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                    r.get(9)?,
                    r.get(10)?,
                ))
            },
        )
        .optional()?;
    let Some((
        name,
        status,
        class_name,
        [domain, affiliation, track_type, publish],
        cot_type,
        sidc,
        attributes,
        source,
        updated_at_ms,
    )) = row
    else {
        return Ok(None);
    };
    let text = |v: Option<String>| v.map(Value::String).unwrap_or(Value::Null);
    let mut stmt = conn.prepare(
        "SELECT scheme, value, expected_name, source FROM registry_identifiers
         WHERE entity_id = ?1 ORDER BY added_at_ms, rowid",
    )?;
    let identifiers = stmt
        .query_map([id], |r| {
            Ok(RegistryIdentifier {
                scheme: r.get(0)?,
                value: r.get(1)?,
                expected_name: r.get(2)?,
                source: r.get(3)?,
            })
        })?
        .collect::<std::result::Result<_, _>>()?;
    Ok(Some(Entity {
        id: id.to_owned(),
        status,
        name,
        class_name,
        // Written by `save`, so they parse; anything else reads as unset.
        domain: parse_enum("domain", &text(domain)).unwrap_or(None),
        affiliation: parse_enum("affiliation", &text(affiliation)).unwrap_or(None),
        track_type: parse_enum("track type", &text(track_type)).unwrap_or(None),
        publish: parse_enum("publish setting", &text(publish)).unwrap_or(None),
        cot_type,
        sidc,
        identifiers,
        attributes: serde_json::from_str(&attributes)?,
        source,
        updated_at_ms,
        fields: None,
    }))
}

fn holder(tx: &Transaction<'_>, i: &RegistryIdentifier) -> Result<Option<String>> {
    Ok(tx
        .query_row(
            "SELECT entity_id FROM registry_identifiers WHERE scheme = ?1 AND value = ?2",
            params![i.scheme, i.value],
            |r| r.get(0),
        )
        .optional()?)
}

/// Write the entity row (not its identifiers).
fn write_row(tx: &Transaction<'_>, e: &Entity, now: i64) -> Result<()> {
    tx.execute(
        "INSERT INTO registry_entities (id, name, status, class_name, domain, affiliation, track_type,
                                        cot_type, sidc, attributes, source, created_at_ms, updated_at_ms, publish)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12, ?13)
         ON CONFLICT(id) DO UPDATE SET name = excluded.name, status = excluded.status,
             class_name = excluded.class_name, domain = excluded.domain,
             affiliation = excluded.affiliation, track_type = excluded.track_type,
             cot_type = excluded.cot_type, sidc = excluded.sidc, attributes = excluded.attributes,
             source = coalesce(excluded.source, registry_entities.source), publish = excluded.publish,
             updated_at_ms = excluded.updated_at_ms",
        params![
            e.id,
            e.name,
            e.status,
            e.class_name,
            enum_text(&e.domain),
            enum_text(&e.affiliation),
            enum_text(&e.track_type),
            e.cot_type,
            e.sidc,
            serde_json::to_string(&e.attributes)?,
            e.source,
            now,
            enum_text(&e.publish)
        ],
    )?;
    Ok(())
}

fn write_revision(tx: &Transaction<'_>, e: &Entity, now: i64, decision_id: i64) -> Result<()> {
    let snapshot = Entity {
        updated_at_ms: now,
        ..e.clone()
    };
    tx.execute(
        "INSERT INTO entity_revisions (entity_id, entity, saved_at_ms, decision_id)
         VALUES (?1, ?2, ?3, ?4)",
        params![e.id, serde_json::to_string(&snapshot)?, now, decision_id],
    )?;
    Ok(())
}

/// Write an entity as a configuration import restores it: its row (as it
/// was last updated), its identifiers and a revision under the import's
/// decision. An identifier another entity holds fails the import.
pub(crate) fn restore_entity(
    tx: &Transaction<'_>,
    e: &Entity,
    decision_id: i64,
    now: i64,
) -> Result<()> {
    let at = if e.updated_at_ms > 0 {
        e.updated_at_ms
    } else {
        now
    };
    write_row(tx, e, at)?;
    for i in &e.identifiers {
        if let Some(h) = holder(tx, i)? {
            return Err(StoreError::Conflict(format!(
                "{} is listed by entities {h} and {}",
                i.label(),
                e.id
            )));
        }
        tx.execute(
            "INSERT INTO registry_identifiers (scheme, value, entity_id, expected_name, source, added_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![i.scheme, i.value, e.id, i.expected_name, i.source, at],
        )?;
    }
    write_revision(tx, e, now, decision_id)
}

impl Db {
    pub fn entity(&self, id: &str) -> Result<Option<Entity>> {
        load(self.connection(), id)
    }

    /// Create or replace an entity, identifiers included: identifiers it no
    /// longer lists are removed, and one another entity holds is refused.
    /// An empty id creates a new entity. Returns the saved entity.
    pub fn save_entity(&mut self, entity: &Entity, actor: &str) -> Result<Entity> {
        let mut e = entity.clone();
        e.validate()?;
        let before = if e.id.trim().is_empty() {
            e.id = new_entity_id();
            None
        } else {
            e.id = e.id.trim().to_owned();
            self.entity(&e.id)?
        };
        if before.is_none() && e.source.is_none() {
            e.source = Some(actor.to_owned());
        }
        let id = e.id.clone();
        self.write(|tx| {
            let now = now_ms();
            for i in &e.identifiers {
                if let Some(h) = holder(tx, i)?
                    && h != e.id
                {
                    return Err(StoreError::Conflict(format!(
                        "{} already belongs to entity {h}",
                        i.label()
                    )));
                }
            }
            let op = if before.is_some() {
                "edit_entity"
            } else {
                "create_entity"
            };
            let mut d = Decision::new(actor, op)
                .evidence(json!({ "entity": e.id, "name": e.name }))
                .after(serde_json::to_value(&e)?);
            if let Some(b) = &before {
                d = d.before(serde_json::to_value(b)?);
            }
            let decision_id = record_decision(tx, &d, now)?;
            write_row(tx, &e, now)?;
            let kept: BTreeSet<(&str, &str)> = e
                .identifiers
                .iter()
                .map(|i| (i.scheme.as_str(), i.value.as_str()))
                .collect();
            for old in before.iter().flat_map(|b| &b.identifiers) {
                if !kept.contains(&(old.scheme.as_str(), old.value.as_str())) {
                    tx.execute(
                        "DELETE FROM registry_identifiers
                         WHERE scheme = ?1 AND value = ?2 AND entity_id = ?3",
                        params![old.scheme, old.value, e.id],
                    )?;
                }
            }
            for i in &e.identifiers {
                tx.execute(
                    "INSERT INTO registry_identifiers (scheme, value, entity_id, expected_name, source, added_at_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT(scheme, value) DO UPDATE SET expected_name = excluded.expected_name",
                    params![
                        i.scheme,
                        i.value,
                        e.id,
                        i.expected_name,
                        i.source.as_deref().unwrap_or(actor),
                        now
                    ],
                )?;
            }
            write_revision(tx, &e, now, decision_id)
        })?;
        self.entity(&id)?
            .ok_or_else(|| StoreError::NotFound(format!("entity {id}")))
    }

    /// Delete an entity and its identifiers. Its revisions stay.
    pub fn delete_entity(&mut self, id: &str, actor: &str) -> Result<()> {
        let before = self
            .entity(id)?
            .ok_or_else(|| StoreError::NotFound(format!("entity {id}")))?;
        self.write(|tx| {
            let now = now_ms();
            let d = Decision::new(actor, "delete_entity")
                .evidence(json!({ "entity": id, "name": before.name }))
                .before(serde_json::to_value(&before)?);
            record_decision(tx, &d, now)?;
            tx.execute(
                "DELETE FROM registry_identifiers WHERE entity_id = ?1",
                [id],
            )?;
            tx.execute("DELETE FROM registry_entities WHERE id = ?1", [id])?;
            Ok(())
        })
    }

    /// Set fields a feed reports (a source pipeline's track → entity links).
    /// Values that do not fit their field are skipped. Returns whether the
    /// entity changed; an unchanged entity writes nothing.
    pub fn update_entity_fields(
        &mut self,
        id: &str,
        updates: &[(String, Value)],
        actor: &str,
    ) -> Result<bool> {
        let Some(before) = self.entity(id)? else {
            return Ok(false);
        };
        let mut e = before.clone();
        let mut changed = Vec::new();
        for (k, v) in updates {
            if let Ok(true) = e.set_field(k, v) {
                changed.push(k.clone());
            }
        }
        if changed.is_empty() {
            return Ok(false);
        }
        e.validate()?;
        self.write(|tx| {
            let now = now_ms();
            let d = Decision::new(actor, "update_entity")
                .evidence(json!({ "entity": id, "changed": changed }))
                .before(serde_json::to_value(&before)?)
                .after(serde_json::to_value(&e)?);
            let decision_id = record_decision(tx, &d, now)?;
            write_row(tx, &e, now)?;
            write_revision(tx, &e, now, decision_id)
        })?;
        Ok(true)
    }

    /// An entity's revisions, newest first.
    pub fn entity_revisions(&self, id: &str, limit: usize) -> Result<Vec<EntityRevision>> {
        let mut stmt = self.connection().prepare(
            "SELECT r.id, r.entity, r.saved_at_ms, r.decision_id, d.actor
             FROM entity_revisions r LEFT JOIN decisions d ON d.id = r.decision_id
             WHERE r.entity_id = ?1 ORDER BY r.id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![id, limit as i64], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, Option<i64>>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        })?;
        rows.map(|r| {
            let (id, entity, saved_at_ms, decision_id, actor) = r?;
            Ok(EntityRevision {
                id,
                entity: serde_json::from_str(&entity)?,
                saved_at_ms,
                decision_id,
                actor,
            })
        })
        .collect()
    }

    /// A page of the entities whose name or any identifier value contains
    /// `q` (case-insensitive; empty: every entity), by name then id; and how
    /// many match.
    pub fn list_entities(
        &self,
        q: &str,
        limit: usize,
        offset: usize,
    ) -> Result<(Vec<Entity>, usize)> {
        let q = q.trim();
        let pattern = format!("%{}%", q.replace(['%', '_'], ""));
        let filter = "(?1 = '%%' OR e.name LIKE ?1 COLLATE NOCASE OR e.id = ?2
             OR EXISTS (SELECT 1 FROM registry_identifiers i
                        WHERE i.entity_id = e.id AND i.value LIKE ?1 COLLATE NOCASE))";
        let total: i64 = self.connection().query_row(
            &format!("SELECT count(*) FROM registry_entities e WHERE {filter}"),
            params![pattern, q],
            |r| r.get(0),
        )?;
        let mut stmt = self.connection().prepare(&format!(
            "SELECT e.id FROM registry_entities e WHERE {filter}
             ORDER BY e.name IS NULL, e.name COLLATE NOCASE, e.id LIMIT ?3 OFFSET ?4"
        ))?;
        let ids: Vec<String> = stmt
            .query_map(
                params![
                    pattern,
                    q,
                    limit.min(i64::MAX as usize) as i64,
                    offset as i64
                ],
                |r| r.get(0),
            )?
            .collect::<std::result::Result<_, _>>()?;
        let page = ids
            .iter()
            .filter_map(|id| self.entity(id).transpose())
            .collect::<Result<Vec<_>>>()?;
        Ok((page, total as usize))
    }

    /// Upsert entities from an import: each entity's fields are replaced,
    /// its identifiers only ever added. An identifier already held by a
    /// different entity is left alone and counted as a conflict, so an
    /// import can never silently move an identifier.
    pub fn registry_import(
        &mut self,
        entities: &[Entity],
        actor: &str,
        label: &str,
    ) -> Result<ImportCounts> {
        let mut checked = Vec::with_capacity(entities.len());
        for e in entities {
            let mut e = e.clone();
            e.validate()
                .map_err(|err| StoreError::Conflict(format!("entity {}: {err}", e.id)))?;
            if e.id.trim().is_empty() {
                e.id = new_entity_id();
            }
            checked.push(e);
        }
        self.write(|tx| {
            let now = now_ms();
            let mut counts = ImportCounts::default();
            // Typed identifiers added or refused, for the decision log.
            let mut added: Vec<Value> = Vec::new();
            let mut refused: Vec<Value> = Vec::new();
            let d = Decision::new(actor, "registry_import").reason(label.to_owned());
            let decision_id = record_decision(tx, &d, now)?;
            for e in &checked {
                if load(tx, &e.id)?.is_some() {
                    counts.entities_updated += 1;
                } else {
                    counts.entities_created += 1;
                }
                write_row(tx, e, now)?;
                for i in &e.identifiers {
                    match holder(tx, i)? {
                        Some(h) if h != e.id => {
                            counts.conflicts += 1;
                            refused.push(json!({
                                "identifier": i.label(), "held_by": h, "claimed_by": e.id,
                            }));
                        }
                        held => {
                            tx.execute(
                                "INSERT INTO registry_identifiers (scheme, value, entity_id, expected_name, source, added_at_ms)
                                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                                 ON CONFLICT(scheme, value) DO UPDATE SET expected_name = excluded.expected_name,
                                     source = excluded.source",
                                params![i.scheme, i.value, e.id, i.expected_name, i.source, now],
                            )?;
                            counts.identifiers += 1;
                            if held.is_none() {
                                added.push(json!({
                                    "entity": e.id, "name": e.name, "identifier": i.label(),
                                }));
                            }
                        }
                    }
                }
                let saved = load(tx, &e.id)?
                    .ok_or_else(|| StoreError::NotFound(format!("entity {}", e.id)))?;
                write_revision(tx, &saved, now, decision_id)?;
            }
            tx.execute(
                "UPDATE decisions SET evidence = ?2 WHERE id = ?1",
                params![
                    decision_id,
                    json!({
                        "counts": counts,
                        "identifiers_added": added,
                        "identifiers_refused": refused,
                    })
                    .to_string()
                ],
            )?;
            Ok(counts)
        })
    }

    /// Every identifier of every active entity, with the entity's fields.
    pub fn registry_rows(&self) -> Result<Vec<RegistryRow>> {
        let mut stmt = self.connection().prepare(
            "SELECT i.scheme, i.value, e.id, i.expected_name
             FROM registry_identifiers i JOIN registry_entities e ON e.id = i.entity_id
             WHERE e.status = 'active' ORDER BY e.id",
        )?;
        type IdRow = (String, String, String, Option<String>);
        let ids: Vec<IdRow> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<std::result::Result<_, _>>()?;
        let mut out = Vec::with_capacity(ids.len());
        let mut current: Option<Entity> = None;
        for (scheme, value, entity_id, expected_name) in ids {
            if current.as_ref().is_none_or(|e| e.id != entity_id) {
                current = self.entity(&entity_id)?;
            }
            let Some(e) = &current else { continue };
            out.push(RegistryRow {
                scheme,
                value,
                entity_id,
                name: e.name.clone(),
                expected_name,
                fields: e.fields_map(),
            });
        }
        Ok(out)
    }

    /// Changes whenever the registry does; lookup snapshots poll it.
    pub fn registry_version(&self) -> Result<String> {
        Ok(self.connection().query_row(
            "SELECT (SELECT count(*) FROM registry_entities) || ':' ||
                    (SELECT coalesce(max(updated_at_ms), 0) FROM registry_entities) || ':' ||
                    (SELECT count(*) FROM registry_identifiers) || ':' ||
                    (SELECT coalesce(max(added_at_ms), 0) FROM registry_identifiers)",
            [],
            |r| r.get(0),
        )?)
    }

    /// The entity an identifier resolves to, if any.
    pub fn registry_resolve(&self, scheme: &str, value: &str) -> Result<Option<Entity>> {
        let id: Option<String> = self
            .connection()
            .query_row(
                "SELECT entity_id FROM registry_identifiers WHERE scheme = ?1 AND value = ?2",
                params![scheme, value],
                |r| r.get(0),
            )
            .optional()?;
        match id {
            Some(id) => self.entity(&id),
            None => Ok(None),
        }
    }

    /// Every attribute key in use, with how many entities have it.
    pub fn attribute_keys(&self) -> Result<Vec<(String, i64)>> {
        let mut stmt = self.connection().prepare(
            "SELECT a.value ->> '$.key' AS k, count(*) FROM registry_entities e, json_each(e.attributes) a
             GROUP BY k ORDER BY k",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn registry_counts(&self) -> Result<Value> {
        let (active, retired, ids): (i64, i64, i64) = self.connection().query_row(
            "SELECT (SELECT count(*) FROM registry_entities WHERE status = 'active'),
                    (SELECT count(*) FROM registry_entities WHERE status = 'retired'),
                    (SELECT count(*) FROM registry_identifiers)",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        Ok(json!({ "active": active, "retired": retired, "identifiers": ids }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ident(scheme: &str, value: &str) -> RegistryIdentifier {
        RegistryIdentifier {
            scheme: scheme.into(),
            value: value.into(),
            expected_name: None,
            source: None,
        }
    }

    fn legacy(id: &str, mmsi: &str) -> Entity {
        serde_json::from_value(json!({
            "id": id, "name": "USS EXAMPLE",
            "fields": {"cot": "a-f-S-C", "hull_code": "DDG-1", "ship_class": "Arleigh Burke", "length": 155},
            "identifiers": [{"scheme": "mmsi", "value": mmsi, "expected_name": "EXAMPLE"}]
        }))
        .unwrap()
    }

    #[test]
    fn identifiers_of_any_scheme_are_typed_and_labelled() {
        let mut db = Db::open_in_memory().unwrap();
        let e: Entity = serde_json::from_value(json!({
            "id": "cvn75", "name": "USS HARRY S TRUMAN",
            "identifiers": [
                {"scheme": "MMSI", "value": " 338000001 "},
                {"scheme": "imo", "value": "9876543"},
                {"scheme": "elnot", "value": "NL504"}]
        }))
        .unwrap();
        db.registry_import(&[e], "op:test", "typed").unwrap();
        let back = db.entity("cvn75").unwrap().unwrap();
        assert_eq!(
            back.label(),
            "USS HARRY S TRUMAN mmsi:338000001 imo:9876543 elnot:NL504"
        );
        assert_eq!(
            db.registry_resolve("elnot", "NL504").unwrap().unwrap().id,
            "cvn75"
        );
        let evidence: String = db
            .connection()
            .query_row(
                "SELECT evidence FROM decisions WHERE reason = 'typed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            evidence.contains("\"identifier\":\"elnot:NL504\""),
            "{evidence}"
        );

        for bad in ["", " ", "has space", "-lead"] {
            let e: Entity = serde_json::from_value(json!({
                "id": "x", "identifiers": [{"scheme": bad, "value": "1"}]}))
            .unwrap();
            assert!(
                db.registry_import(&[e], "op:test", "bad").is_err(),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn import_upserts_folds_legacy_fields_and_refuses_to_move_identifiers() {
        let mut db = Db::open_in_memory().unwrap();
        let c = db
            .registry_import(&[legacy("e1", "1"), legacy("e2", "2")], "op:test", "seed")
            .unwrap();
        assert_eq!((c.entities_created, c.identifiers, c.conflicts), (2, 2, 0));
        let e1 = db.entity("e1").unwrap().unwrap();
        assert_eq!(e1.cot_type.as_deref(), Some("a-f-S-C"));
        assert_eq!(e1.class_name.as_deref(), Some("Arleigh Burke"));
        assert_eq!(e1.attribute("hull_code").unwrap().value, "DDG-1");
        assert_eq!(e1.attribute("length").unwrap().kind, AttrType::Number);
        let v1 = db.registry_version().unwrap();

        // e3 claims e1's identifier: conflict, nothing moves.
        let c = db
            .registry_import(&[legacy("e1", "1"), legacy("e3", "1")], "op:test", "again")
            .unwrap();
        assert_eq!(
            (c.entities_created, c.entities_updated, c.conflicts),
            (1, 1, 1)
        );
        assert_eq!(db.registry_resolve("mmsi", "1").unwrap().unwrap().id, "e1");
        assert_ne!(db.registry_version().unwrap(), v1);
        let evidence: String = db
            .connection()
            .query_row(
                "SELECT evidence FROM decisions WHERE reason = 'again'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let evidence: Value = serde_json::from_str(&evidence).unwrap();
        assert_eq!(evidence["identifiers_refused"][0]["identifier"], "mmsi:1");
        assert_eq!(evidence["identifiers_refused"][0]["held_by"], "e1");

        let rows = db.registry_rows().unwrap();
        assert_eq!(rows.len(), 2);
        let row = rows.iter().find(|r| r.value == "1").unwrap();
        assert_eq!(row.fields["cot_type"], "a-f-S-C");
        assert_eq!(row.fields["hull_code"], "DDG-1");

        let mut retired = legacy("e2", "2");
        retired.status = "retired".into();
        db.registry_import(&[retired], "op:test", "retire").unwrap();
        assert_eq!(db.registry_rows().unwrap().len(), 1);
        assert_eq!(db.registry_counts().unwrap()["retired"], 1);
    }

    #[test]
    fn save_adds_and_removes_identifiers_and_keeps_history() {
        let mut db = Db::open_in_memory().unwrap();
        let e: Entity = serde_json::from_value(json!({
            "name": "TED STEVENS", "domain": "surface", "affiliation": "friend",
            "identifiers": [{"scheme": "MMSI", "value": "338924210"}, {"scheme": "imo", "value": "9876543"}],
            "attributes": [{"key": "contact_phone", "type": "text", "value": "+1 555 0100"},
                           {"key": "length", "type": "number", "value": "103.5"}]
        }))
        .unwrap();
        let saved = db.save_entity(&e, "op:alice").unwrap();
        assert!(saved.id.starts_with("ent-"));
        assert_eq!(saved.identifiers[0].scheme, "mmsi");
        assert_eq!(saved.attribute("length").unwrap().value, json!(103.5));

        // Another entity cannot take an identifier.
        let thief = Entity {
            identifiers: vec![ident("mmsi", "338924210")],
            ..Default::default()
        };
        assert!(matches!(
            db.save_entity(&thief, "op:bob"),
            Err(StoreError::Conflict(_))
        ));

        // Replace: drop the IMO, add an ELNOT, change the affiliation.
        let mut e = saved.clone();
        e.identifiers.retain(|i| i.scheme != "imo");
        e.identifiers.push(ident("elnot", "NL504"));
        e.affiliation = Some(Affiliation::Neutral);
        let saved = db.save_entity(&e, "op:bob").unwrap();
        let labels: Vec<String> = saved
            .identifiers
            .iter()
            .map(RegistryIdentifier::label)
            .collect();
        assert_eq!(labels, ["mmsi:338924210", "elnot:NL504"]);
        assert!(db.registry_resolve("imo", "9876543").unwrap().is_none());
        assert_eq!(saved.field("affiliation"), Some(json!("neutral")));

        // A feed updates an attribute and adds one; the same values again
        // change nothing, and a value that does not fit is skipped.
        let updates = [
            ("destination".to_string(), json!("LONG BEACH")),
            ("length".to_string(), json!("104")),
            ("domain".to_string(), json!("not a domain")),
        ];
        assert!(
            db.update_entity_fields(&saved.id, &updates, "source:ais")
                .unwrap()
        );
        assert!(
            !db.update_entity_fields(&saved.id, &updates, "source:ais")
                .unwrap()
        );
        let now = db.entity(&saved.id).unwrap().unwrap();
        assert_eq!(now.field("destination"), Some(json!("LONG BEACH")));
        let mut forced = now.clone();
        forced.publish = Some(Publish::Always);
        let forced = db.save_entity(&forced, "op:test").unwrap();
        assert_eq!(forced.publish, Some(Publish::Always));
        assert_eq!(
            db.registry_rows()
                .unwrap()
                .iter()
                .find(|r| r.entity_id == saved.id)
                .unwrap()
                .fields["publish"],
            "always"
        );
        let mut auto = forced.clone();
        auto.set_field("publish", &json!("automatic")).unwrap();
        let now = db.save_entity(&auto, "op:test").unwrap();
        assert_eq!(now.publish, None);
        assert_eq!(now.field("length"), Some(json!(104)));
        assert_eq!(now.field("domain"), Some(json!("surface")));

        let revs = db.entity_revisions(&saved.id, 10).unwrap();
        assert_eq!(revs.len(), 5);
        assert_eq!(revs[2].actor.as_deref(), Some("source:ais"));
        assert_eq!(revs[4].entity["affiliation"], "friend");

        // Reserved and duplicate keys, bad values.
        for bad in [
            json!([{"key": "name", "type": "text", "value": "x"}]),
            json!([{"key": "a", "type": "text", "value": "x"}, {"key": "a", "type": "text", "value": "y"}]),
            json!([{"key": "n", "type": "number", "value": "ten"}]),
            json!([{"key": "t", "type": "datetime", "value": "yesterday"}]),
        ] {
            let mut e = now.clone();
            e.attributes = serde_json::from_value(bad.clone()).unwrap();
            assert!(db.save_entity(&e, "op:test").is_err(), "{bad}");
        }

        db.delete_entity(&saved.id, "op:alice").unwrap();
        assert!(db.entity(&saved.id).unwrap().is_none());
        assert!(db.registry_resolve("mmsi", "338924210").unwrap().is_none());
    }

    #[test]
    fn list_entities_pages_and_filters() {
        let mut db = Db::open_in_memory().unwrap();
        for (name, mmsi) in [("BRAVO", "2"), ("ALPHA", "1"), ("CHARLIE", "3")] {
            let e = Entity {
                name: Some(name.into()),
                identifiers: vec![ident("mmsi", mmsi)],
                ..Default::default()
            };
            db.save_entity(&e, "op:test").unwrap();
        }
        let (all, total) = db.list_entities("", 10, 0).unwrap();
        assert_eq!(total, 3);
        let names: Vec<_> = all.iter().map(|e| e.name.clone().unwrap()).collect();
        assert_eq!(names, ["ALPHA", "BRAVO", "CHARLIE"]);
        let (page, total) = db.list_entities("", 1, 1).unwrap();
        assert_eq!((page[0].name.as_deref(), total), (Some("BRAVO"), 3));
        let (hit, total) = db.list_entities("3", 10, 0).unwrap();
        assert_eq!((hit[0].name.as_deref(), total), (Some("CHARLIE"), 1));

        let mut e = hit[0].clone();
        e.attributes =
            serde_json::from_value(json!([{"key": "flag", "type": "text", "value": "US"}]))
                .unwrap();
        db.save_entity(&e, "op:test").unwrap();
        assert_eq!(db.attribute_keys().unwrap(), [("flag".to_string(), 1)]);
    }
}
