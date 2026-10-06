//! Declarative mapping: records to the authoritative track schema.
//!
//! A mapping is JSON. `reject` rules drop records before mapping (counted per
//! reason). Every `rule` whose `when` holds is applied, so one record can feed
//! both the static cache and a position report (AIS message 19 does). An
//! `observation` rule yields a track report; a `static` rule yields identity
//! fields that the static join attaches to later reports with the same key.
//!
//! ```json
//! {
//!   "reject": [ { "reason": "placeholder id", "when": { "path": "mmsi", "in": ["000000000"] } } ],
//!   "rules": [ {
//!     "name": "position",
//!     "when": { "path": "MessageType", "eq": "PositionReport" },
//!     "key": "MetaData.MMSI",
//!     "identifiers": [ { "scheme": "mmsi", "value": "MetaData.MMSI" } ],
//!     "fields": {
//!       "position.latitude": "Message.PositionReport.Latitude",
//!       "position.longitude": "Message.PositionReport.Longitude",
//!       "kinematics.speed_mps": { "path": "Message.PositionReport.Sog", "transforms": ["knots_to_mps"] }
//!     }
//!   } ]
//! }
//! ```

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use ot_core::{Affiliation, Domain, Identifier, Observation, TrackState};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::expr::{Condition, ValueSpec, as_f64, as_string};
use crate::path::Path;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MappingSpec {
    /// Extension schema version this mapping targets.
    #[serde(default = "one")]
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reject: Vec<RejectRule>,
    pub rules: Vec<Rule>,
}

fn one() -> u32 {
    1
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RejectRule {
    pub reason: String,
    pub when: Condition,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleKind {
    /// A report of an object; on a source with a tracker stage, a detection
    /// for the tracker.
    #[default]
    Observation,
    /// Identity fields joined onto later observations with the same key.
    Static,
    /// A report of an object that carries its own identity (e.g. a sensor's
    /// own platform): it bypasses the source's tracker stage.
    Track,
}

impl RuleKind {
    /// Whether the rule's output is a report of an object.
    pub fn reports(self) -> bool {
        matches!(self, RuleKind::Observation | RuleKind::Track)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<Condition>,
    #[serde(default)]
    pub kind: RuleKind,
    /// The source's key for the object; becomes `source_track_key`.
    pub key: ValueSpec,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub identifiers: Vec<IdentifierSpec>,
    /// Values computed first and added to the record under these names
    /// (use a `_` prefix to avoid clashing with record fields), e.g. `_body`.
    #[serde(default, rename = "let", skip_serializing_if = "BTreeMap::is_empty")]
    pub bindings: BTreeMap<String, ValueSpec>,
    /// Track-schema field path to value spec.
    pub fields: BTreeMap<String, ValueSpec>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentifierSpec {
    pub scheme: String,
    pub value: ValueSpec,
}

/// What a target field holds, for coercion and validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FieldType {
    Text,
    Number,
    Time,
    Domain,
    Affiliation,
    State,
    TrackType,
    Sidc,
    /// `[[lat, lon], …]`, `[{lat, lon}, …]` or "lat lon, lat lon, …".
    Polygon,
}

/// Every core field a mapping may write. Extension fields are `ext.<key>`.
const TARGETS: &[(&str, FieldType)] = &[
    ("name", FieldType::Text),
    ("callsign", FieldType::Text),
    ("observed_at", FieldType::Time),
    ("position.latitude", FieldType::Number),
    ("position.longitude", FieldType::Number),
    ("position.altitude_hae_m", FieldType::Number),
    ("uncertainty.circular_error_m", FieldType::Number),
    ("uncertainty.vertical_error_m", FieldType::Number),
    ("uncertainty.ellipse.semi_major_m", FieldType::Number),
    ("uncertainty.ellipse.semi_minor_m", FieldType::Number),
    ("uncertainty.ellipse.orientation_deg", FieldType::Number),
    ("kinematics.course_deg", FieldType::Number),
    ("kinematics.speed_mps", FieldType::Number),
    ("kinematics.heading_deg", FieldType::Number),
    ("kinematics.vertical_rate_mps", FieldType::Number),
    ("classification.cot_type", FieldType::Text),
    ("classification.domain", FieldType::Domain),
    ("classification.affiliation", FieldType::Affiliation),
    ("classification.sidc", FieldType::Sidc),
    ("platform.type_code", FieldType::Text),
    ("platform.class", FieldType::Text),
    ("platform.name", FieldType::Text),
    ("platform.flag", FieldType::Text),
    ("platform.hull", FieldType::Text),
    ("provenance.sensor_code", FieldType::Text),
    ("provenance.source_code", FieldType::Text),
    ("provenance.confidence", FieldType::Number),
    ("state", FieldType::State),
    ("track_type", FieldType::TrackType),
    // A line of bearing from the position (the sensor's): its direction,
    // one-sigma error, and the sensor's range, all optional but the first.
    ("geometry.bearing_deg", FieldType::Number),
    ("geometry.sigma_deg", FieldType::Number),
    ("geometry.range_m", FieldType::Number),
    ("geometry.range_sigma_m", FieldType::Number),
    ("geometry.max_range_m", FieldType::Number),
    ("geometry.elevation_deg", FieldType::Number),
    // An area of uncertainty; with no position mapped, its centre is.
    ("geometry.polygon", FieldType::Polygon),
];

fn target_type(field: &str) -> Option<FieldType> {
    if let Some(key) = field
        .strip_prefix("ext.")
        .or_else(|| field.strip_prefix("entity."))
    {
        return (!key.is_empty() && !key.contains('.')).then_some(FieldType::Text);
    }
    TARGETS.iter().find(|(f, _)| *f == field).map(|(_, t)| *t)
}

/// Names of every mappable core field, for the UI.
pub fn target_fields() -> impl Iterator<Item = &'static str> {
    TARGETS.iter().map(|(f, _)| *f)
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum MappingError {
    #[error("rule {rule:?}: unknown target field {field:?}")]
    UnknownField { rule: String, field: String },
    #[error("rule {0:?}: an observation rule must map position.latitude and position.longitude")]
    NoPosition(String),
    #[error("mapping has no rules")]
    NoRules,
    #[error("rule names must be unique: {0:?}")]
    DuplicateRule(String),
}

/// One mapped output of a record.
#[derive(Debug, Clone, PartialEq)]
pub struct Mapped {
    pub rule: String,
    pub kind: RuleKind,
    pub key: String,
    pub identifiers: Vec<Identifier>,
    /// Target fields as a nested object (`position.latitude` → `{"position":{"latitude":..}}`).
    pub fields: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MapOutcome {
    Rejected(String),
    /// No rule matched, or every matching rule lacked a key.
    Unmatched,
    Mapped(Vec<Mapped>),
}

impl MappingSpec {
    pub fn validate(&self) -> Result<(), MappingError> {
        if self.rules.is_empty() {
            return Err(MappingError::NoRules);
        }
        let mut names = std::collections::HashSet::new();
        for rule in &self.rules {
            if !names.insert(rule.name.as_str()) {
                return Err(MappingError::DuplicateRule(rule.name.clone()));
            }
            for field in rule.fields.keys() {
                if target_type(field).is_none() {
                    return Err(MappingError::UnknownField {
                        rule: rule.name.clone(),
                        field: field.clone(),
                    });
                }
            }
            if rule.kind.reports()
                && !rule.fields.contains_key("geometry.polygon")
                && !(rule.fields.contains_key("position.latitude")
                    && rule.fields.contains_key("position.longitude"))
            {
                return Err(MappingError::NoPosition(rule.name.clone()));
            }
        }
        Ok(())
    }

    pub fn apply(&self, record: &Value) -> MapOutcome {
        if let Some(r) = self.reject.iter().find(|r| r.when.eval(record)) {
            return MapOutcome::Rejected(r.reason.clone());
        }
        let mut out = Vec::new();
        for rule in &self.rules {
            if rule.when.as_ref().is_some_and(|c| !c.eval(record)) {
                continue;
            }
            let bound;
            let record = if rule.bindings.is_empty() {
                record
            } else {
                let mut r = record.clone();
                for (name, spec) in &rule.bindings {
                    let v = spec.eval(&r);
                    if let Value::Object(m) = &mut r {
                        m.insert(name.clone(), v);
                    }
                }
                bound = r;
                &bound
            };
            let key = rule.key.eval(record);
            if key.is_null() || as_string(&key).trim().is_empty() {
                continue;
            }
            let identifiers = rule
                .identifiers
                .iter()
                .filter_map(|i| {
                    let v = i.value.eval(record);
                    let s = as_string(&v);
                    (!v.is_null() && !s.trim().is_empty())
                        .then(|| Identifier::new(i.scheme.clone(), s.trim()))
                })
                .collect();
            let mut fields = Value::Object(Map::new());
            for (target, spec) in &rule.fields {
                let v = coerce(target, spec.eval(record));
                if !v.is_null() {
                    // `entity.<key>` updates the track's entity (see
                    // `registry::ENTITY_EXT`); it travels as `ext.entity.<key>`.
                    let target = match target.strip_prefix("entity.") {
                        Some(key) => format!("ext.{}.{key}", crate::registry::ENTITY_EXT),
                        None => target.clone(),
                    };
                    let path: Path = target.parse().expect("validated target");
                    path.set(&mut fields, v);
                }
            }
            out.push(Mapped {
                rule: rule.name.clone(),
                kind: rule.kind,
                key: as_string(&key).trim().to_owned(),
                identifiers,
                fields,
            });
        }
        if out.is_empty() {
            MapOutcome::Unmatched
        } else {
            MapOutcome::Mapped(out)
        }
    }
}

/// Coerce a value to its target's type; values that cannot be coerced become
/// null (the field is then simply absent).
fn coerce(target: &str, v: Value) -> Value {
    if v.is_null() {
        return v;
    }
    match target_type(target) {
        Some(FieldType::Number) => as_f64(&v).map_or(Value::Null, |x| {
            serde_json::Number::from_f64(x).map_or(Value::Null, Value::Number)
        }),
        Some(FieldType::Text) if target.starts_with("ext.") || target.starts_with("entity.") => v,
        Some(FieldType::Text) => {
            let s = as_string(&v);
            if s.trim().is_empty() {
                Value::Null
            } else {
                Value::String(s.trim().to_owned())
            }
        }
        Some(FieldType::Time) => crate::expr::parse_time_value(&v),
        Some(FieldType::Domain) => parse_domain(&as_string(&v))
            .and_then(|d| serde_json::to_value(d).ok())
            .unwrap_or(Value::Null),
        Some(FieldType::Affiliation) => parse_affiliation(&as_string(&v))
            .and_then(|a| serde_json::to_value(a).ok())
            .unwrap_or(Value::Null),
        Some(FieldType::State) => {
            serde_json::from_value::<TrackState>(Value::String(as_string(&v).trim().to_lowercase()))
                .ok()
                .and_then(|s| serde_json::to_value(s).ok())
                .unwrap_or(Value::Null)
        }
        // 2525C, 2525D or CoT; anything else is dropped (the track then
        // publishes a derived CoT type).
        Some(FieldType::Sidc) => ot_core::Sidc::parse(&as_string(&v))
            .map(|s| Value::String(s.code))
            .unwrap_or(Value::Null),
        Some(FieldType::TrackType) => {
            // GOLD codes (2, 3, 4) or names; a null entry means tactical.
            let s = as_string(&v).trim().to_lowercase().replace([' ', '-'], "_");
            let named = match s.as_str() {
                "" | "0" | "1" => "tactical",
                "2" => "live_training",
                "3" => "simulated_training",
                "4" => "demand_entry",
                other => other,
            };
            serde_json::from_value::<ot_core::TrackType>(Value::String(named.to_owned()))
                .ok()
                .and_then(|t| serde_json::to_value(t).ok())
                .unwrap_or(Value::Null)
        }
        Some(FieldType::Polygon) => parse_polygon(&v)
            .map(|p| serde_json::to_value(p).unwrap_or(Value::Null))
            .unwrap_or(Value::Null),
        None => Value::Null,
    }
}

/// Accepts enum names (`surface`) or CoT battle-dimension atoms (`S`).
fn parse_domain(s: &str) -> Option<Domain> {
    let s = s.trim();
    serde_json::from_value(Value::String(s.to_lowercase()))
        .ok()
        .or_else(|| Domain::from_cot_atom(s))
        .or_else(|| match s.to_lowercase().as_str() {
            "sea" | "maritime" => Some(Domain::Surface),
            "subsea" | "underwater" => Some(Domain::Subsurface),
            "land" => Some(Domain::Ground),
            _ => None,
        })
}

/// Accepts enum names (`friend`, `assumed_friend`) or CoT atoms (`f`).
fn parse_affiliation(s: &str) -> Option<Affiliation> {
    let s = s.trim();
    serde_json::from_value(Value::String(s.to_lowercase().replace([' ', '-'], "_")))
        .ok()
        .or_else(|| Affiliation::from_cot_atom(&s.to_lowercase()))
}

/// Merge `extra` into `base` wherever `base` has no value (static join).
pub fn fill_missing(base: &mut Value, extra: &Value) {
    let (Value::Object(b), Value::Object(e)) = (base, extra) else {
        return;
    };
    for (k, v) in e {
        match b.get_mut(k) {
            None | Some(Value::Null) => {
                b.insert(k.clone(), v.clone());
            }
            Some(existing @ Value::Object(_)) => fill_missing(existing, v),
            Some(_) => {}
        }
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum FinalizeError {
    #[error("no position")]
    NoPosition,
    #[error("invalid observation: {0}")]
    Invalid(String),
}

/// A polygon as `[[lat, lon], …]` from the shapes feeds use.
fn parse_polygon(v: &Value) -> Option<Vec<[f64; 2]>> {
    let pair = |p: &Value| -> Option<[f64; 2]> {
        match p {
            Value::Array(a) if a.len() >= 2 => Some([as_f64(&a[0])?, as_f64(&a[1])?]),
            Value::Object(o) => {
                let get = |ks: &[&str]| ks.iter().find_map(|k| o.get(*k)).and_then(as_f64);
                Some([
                    get(&["lat", "latitude"])?,
                    get(&["lon", "lng", "longitude"])?,
                ])
            }
            _ => None,
        }
    };
    let pts: Vec<[f64; 2]> = match v {
        Value::Array(a) => a.iter().map(pair).collect::<Option<_>>()?,
        Value::String(s) => s
            .split(',')
            .map(|p| {
                let mut it = p.split_whitespace().map(|x| x.parse::<f64>().ok());
                Some([it.next()??, it.next()??])
            })
            .collect::<Option<_>>()?,
        _ => return None,
    };
    // A closed ring repeats its first vertex: keep one.
    let mut pts = pts;
    if pts.len() > 3 && pts.first() == pts.last() {
        pts.pop();
    }
    Some(pts)
}

/// Build the typed observation from mapped fields plus pipeline context.
pub fn finalize(
    source_id: &str,
    schema_version: u32,
    mapped: &Mapped,
    received_at: DateTime<Utc>,
) -> Result<Observation, FinalizeError> {
    let mut f = mapped.fields.clone();
    // Non-point contacts: the geometry's type follows from what was mapped,
    // and an area with no position mapped stands at its centre.
    if let Some(g) = f.get_mut("geometry").and_then(Value::as_object_mut) {
        if g.contains_key("bearing_deg") {
            g.insert("type".into(), "bearing".into());
        } else if g.contains_key("polygon") {
            g.insert("type".into(), "area".into());
        } else {
            f.as_object_mut().map(|o| o.remove("geometry"));
        }
    }
    if let Some(poly) = f.pointer("/geometry/polygon").cloned()
        && f.pointer("/position/latitude").is_none()
        && let Ok(poly) = serde_json::from_value::<Vec<[f64; 2]>>(poly)
        && !poly.is_empty()
    {
        let (lat, lon, cov) = ot_core::geometry::area_estimate(&poly);
        let e = ot_core::schema::Ellipse::from_covariance(cov);
        f["position"] = serde_json::json!({"latitude": lat, "longitude": lon});
        if f.pointer("/uncertainty/ellipse").is_none() {
            f["uncertainty"] = serde_json::json!({"ellipse": e});
        }
    }
    if f.pointer("/position/latitude").is_none() || f.pointer("/position/longitude").is_none() {
        return Err(FinalizeError::NoPosition);
    }
    let mut obj = match f {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    obj.insert("schema_version".into(), schema_version.into());
    obj.insert("source_id".into(), source_id.into());
    obj.insert("source_track_key".into(), mapped.key.clone().into());
    obj.insert(
        "identifiers".into(),
        serde_json::to_value(&mapped.identifiers).expect("identifiers serialise"),
    );
    let received = received_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    obj.insert("received_at".into(), received.clone().into());
    obj.entry("observed_at").or_insert_with(|| received.into());
    let obs: Observation = serde_json::from_value(Value::Object(obj))
        .map_err(|e| FinalizeError::Invalid(e.to_string()))?;
    obs.validate()
        .map_err(|e| FinalizeError::Invalid(e.to_string()))?;
    Ok(obs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn spec(v: Value) -> MappingSpec {
        let s: MappingSpec = serde_json::from_value(v).unwrap();
        s.validate().unwrap();
        s
    }

    fn ais() -> MappingSpec {
        spec(json!({
            "reject": [ { "reason": "placeholder MMSI", "when": { "path": "MetaData.MMSI", "in": [123456789] } } ],
            "rules": [
                { "name": "static", "kind": "static",
                  "when": { "path": "MessageType", "eq": "ShipStaticData" },
                  "key": "MetaData.MMSI",
                  "fields": { "name": { "path": "Message.ShipStaticData.Name", "transforms": ["trim", "nonempty"] },
                              "callsign": "Message.ShipStaticData.CallSign" } },
                { "name": "position",
                  "when": { "path": "MessageType", "eq": "PositionReport" },
                  "key": "MetaData.MMSI",
                  "identifiers": [ { "scheme": "mmsi", "value": "MetaData.MMSI" } ],
                  "fields": {
                      "position.latitude": "Message.PositionReport.Latitude",
                      "position.longitude": "Message.PositionReport.Longitude",
                      "kinematics.speed_mps": { "path": "Message.PositionReport.Sog", "null_if": [102.3], "transforms": ["knots_to_mps"] },
                      "kinematics.heading_deg": { "path": "Message.PositionReport.TrueHeading", "null_if": [511] },
                      "observed_at": { "path": "MetaData.time_utc", "transforms": ["time"] },
                      "classification.domain": { "const": "S" },
                      "provenance.confidence": "Message.PositionReport.Conf"
                  } }
            ]
        }))
    }

    #[test]
    fn maps_a_position_report_end_to_end() {
        let rec = json!({
            "MessageType": "PositionReport",
            "MetaData": { "MMSI": 338924210, "time_utc": "2026-09-24 12:00:00.5 +0000 UTC" },
            "Message": { "PositionReport": { "Latitude": 32.68, "Longitude": -117.23, "Sog": 10.0,
                                             "TrueHeading": 511, "Conf": "0.9" } }
        });
        let MapOutcome::Mapped(out) = ais().apply(&rec) else {
            panic!("not mapped")
        };
        assert_eq!(out.len(), 1);
        let m = &out[0];
        assert_eq!(m.key, "338924210");
        assert_eq!(m.identifiers, vec![Identifier::new("mmsi", "338924210")]);
        let obs = finalize("ais", 1, m, Utc::now()).unwrap();
        assert_eq!(obs.position.latitude, 32.68);
        assert!((obs.kinematics.speed_mps.unwrap() - 5.14444).abs() < 1e-9);
        assert_eq!(obs.kinematics.heading_deg, None);
        assert_eq!(obs.classification.domain, Some(Domain::Surface));
        assert_eq!(obs.provenance.confidence, Some(0.9));
        assert_eq!(
            obs.observed_at.to_rfc3339(),
            "2026-09-24T12:00:00.500+00:00"
        );
    }

    #[test]
    fn static_rules_reject_rules_and_the_join() {
        let spec = ais();
        let rec = json!({"MessageType": "ShipStaticData", "MetaData": {"MMSI": 1},
                         "Message": {"ShipStaticData": {"Name": "TED STEVENS  ", "CallSign": "NTED"}}});
        let MapOutcome::Mapped(out) = spec.apply(&rec) else {
            panic!()
        };
        assert_eq!(out[0].kind, RuleKind::Static);
        assert_eq!(
            out[0].fields,
            json!({"name": "TED STEVENS", "callsign": "NTED"})
        );

        let mut base = json!({"name": null, "position": {"latitude": 1.0}});
        fill_missing(&mut base, &out[0].fields);
        assert_eq!(base["name"], "TED STEVENS");

        let bad = json!({"MessageType": "PositionReport", "MetaData": {"MMSI": 123456789}});
        assert_eq!(
            spec.apply(&bad),
            MapOutcome::Rejected("placeholder MMSI".into())
        );
        assert_eq!(
            spec.apply(&json!({"MessageType": "Other"})),
            MapOutcome::Unmatched
        );
    }

    #[test]
    fn sidc_and_track_type_are_recognised_or_dropped() {
        let spec: MappingSpec = serde_json::from_value(json!({"rules": [{"name": "p", "key": "id",
            "fields": {"position.latitude": "lat", "position.longitude": "lon",
                       "classification.sidc": "sidc", "track_type": "tt"}}]}))
        .unwrap();
        let run = |sidc: &str, tt: &str| {
            let rec = json!({"id": "1", "lat": 1.0, "lon": 2.0, "sidc": sidc, "tt": tt});
            let MapOutcome::Mapped(out) = spec.apply(&rec) else {
                panic!("not mapped")
            };
            out[0].fields.clone()
        };
        let f = run("shsp*------****", "3");
        assert_eq!(f["classification"]["sidc"], "SHSP-----------");
        assert_eq!(f["track_type"], "simulated_training");
        assert_eq!(
            run("10033000001211000000", "")["classification"]["sidc"],
            "10033000001211000000"
        );
        assert_eq!(
            run("a-f-A-M-F", "demand entry")["track_type"],
            "demand_entry"
        );
        let bad = run("not a symbol", "tactical");
        assert!(
            bad["classification"].get("sidc").is_none_or(Value::is_null),
            "{bad}"
        );
    }

    #[test]
    fn bearings_and_areas_map_to_non_point_observations() {
        let spec: MappingSpec = serde_json::from_value(json!({"rules": [
            {"name": "lob", "key": "id", "when": {"path": "kind", "eq": "lob"},
             "fields": {"position.latitude": "lat", "position.longitude": "lon",
                        "geometry.bearing_deg": "brg", "geometry.sigma_deg": "err"}},
            {"name": "aou", "key": "id", "when": {"path": "kind", "eq": "aou"},
             "fields": {"geometry.polygon": "poly"}}]}))
        .unwrap();
        spec.validate().unwrap();
        let obs = |rec: Value| {
            let MapOutcome::Mapped(out) = spec.apply(&rec) else {
                panic!("not mapped")
            };
            finalize("esm", 1, &out[0], Utc::now()).unwrap()
        };
        let lob = obs(
            json!({"kind": "lob", "id": "E1", "lat": 50.6, "lon": -1.9, "brg": "47.5", "err": 2}),
        );
        assert_eq!(
            lob.geometry,
            Some(ot_core::Geometry::Bearing {
                bearing_deg: 47.5,
                sigma_deg: 2.0,
                range_m: None,
                range_sigma_m: None,
                max_range_m: None,
                elevation_deg: None
            })
        );
        assert_eq!(
            lob.position.latitude, 50.6,
            "a bearing's position is the sensor's"
        );
        let aou = obs(
            json!({"kind": "aou", "id": "A1", "poly": "50.0 -1.0, 50.0 -0.9, 50.1 -0.9, 50.1 -1.0, 50.0 -1.0"}),
        );
        let Some(ot_core::Geometry::Area { polygon }) = &aou.geometry else {
            panic!("{:?}", aou.geometry)
        };
        assert_eq!(polygon.len(), 4, "the closing vertex is dropped");
        assert!(
            (aou.position.latitude - 50.05).abs() < 1e-9,
            "an area stands at its centre"
        );
        assert!(aou.uncertainty.unwrap().ellipse.unwrap().semi_major_m > 1000.0);
        let pairs = obs(
            json!({"kind": "aou", "id": "A2", "poly": [{"lat": 50.0, "lon": -1.0}, {"lat": 50.0, "lon": -0.9}, {"lat": 50.1, "lon": -0.95}]}),
        );
        assert!(matches!(
            pairs.geometry,
            Some(ot_core::Geometry::Area { .. })
        ));
    }

    #[test]
    fn validation_catches_typos_and_missing_position() {
        let typo = json!({"rules": [{"name": "p", "key": "id", "fields": {
            "position.latitude": "a", "position.longitude": "b", "kinematic.speed_mps": "c"}}]});
        let s: MappingSpec = serde_json::from_value(typo).unwrap();
        assert!(matches!(
            s.validate(),
            Err(MappingError::UnknownField { .. })
        ));
        let nopos = json!({"rules": [{"name": "p", "key": "id", "fields": {"name": "n"}}]});
        let s: MappingSpec = serde_json::from_value(nopos).unwrap();
        assert_eq!(s.validate(), Err(MappingError::NoPosition("p".into())));
        let ext = json!({"rules": [{"name": "p", "key": "id", "fields": {
            "position.latitude": "a", "position.longitude": "b", "ext.squawk": "sq"}}]});
        serde_json::from_value::<MappingSpec>(ext)
            .unwrap()
            .validate()
            .unwrap();
    }

    #[test]
    fn invalid_values_fail_finalize_or_drop_quietly() {
        let s = spec(json!({"rules": [{"name": "p", "key": "id", "fields": {
            "position.latitude": "lat", "position.longitude": "lon",
            "classification.affiliation": "aff", "kinematics.speed_mps": "spd"}}]}));
        let MapOutcome::Mapped(out) =
            s.apply(&json!({"id": 7, "lat": 91, "lon": 0, "aff": "zz", "spd": "fast"}))
        else {
            panic!()
        };
        assert!(
            out[0]
                .fields
                .pointer("/classification/affiliation")
                .is_none()
        );
        assert!(out[0].fields.pointer("/kinematics/speed_mps").is_none());
        assert!(matches!(
            finalize("s", 1, &out[0], Utc::now()),
            Err(FinalizeError::Invalid(_))
        ));
    }
}
