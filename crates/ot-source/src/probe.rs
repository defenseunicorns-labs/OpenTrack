//! Probe analysis: infer a feed's schema from captured records and suggest
//! a mapping. Suggestions are only proposals; the admin confirms them.

use std::collections::{BTreeMap, HashSet};

use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::expr::{as_f64, as_string};
use crate::mapping::{FieldType, targets};

/// Cap on distinct values tracked per field.
const DISTINCT_CAP: usize = 1000;
const SAMPLES: usize = 5;

/// What was observed at one field path across all records.
#[derive(Debug, Clone, Serialize)]
pub struct FieldStat {
    /// Display path; `[*]` marks "every element of an array".
    pub path: String,
    /// A concrete path usable in a mapping (`[*]` replaced by `[0]`).
    pub mapping_path: String,
    /// JSON type name → count.
    pub types: BTreeMap<&'static str, u64>,
    /// Records in which the field appears non-null.
    pub present: u64,
    /// `present` / records.
    pub presence: f64,
    /// Distinct values seen (capped; `distinct_capped` says so).
    pub distinct: usize,
    pub distinct_capped: bool,
    pub samples: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
}

#[derive(Default)]
struct Acc {
    types: BTreeMap<&'static str, u64>,
    present_in: HashSet<usize>,
    distinct: HashSet<String>,
    capped: bool,
    samples: Vec<Value>,
    min: Option<f64>,
    max: Option<f64>,
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn segment(key: &str) -> String {
    if key.is_empty() || key.contains(['.', '[', ']', '"']) {
        format!("[\"{}\"]", key.replace('"', "\\\""))
    } else {
        key.to_owned()
    }
}

fn join(prefix: &str, key: &str) -> String {
    let seg = segment(key);
    if prefix.is_empty() {
        seg
    } else if seg.starts_with('[') {
        format!("{prefix}{seg}")
    } else {
        format!("{prefix}.{seg}")
    }
}

fn walk(v: &Value, path: &str, record: usize, acc: &mut BTreeMap<String, Acc>) {
    match v {
        Value::Object(map) => {
            for (k, child) in map {
                walk(child, &join(path, k), record, acc);
            }
        }
        Value::Array(items) if items.iter().any(|i| i.is_object() || i.is_array()) => {
            let p = format!("{path}[*]");
            for item in items {
                walk(item, &p, record, acc);
            }
        }
        _ => {
            if path.is_empty() {
                return;
            }
            let a = acc.entry(path.to_owned()).or_default();
            *a.types.entry(type_name(v)).or_default() += 1;
            if v.is_null() {
                return;
            }
            a.present_in.insert(record);
            if a.distinct.len() < DISTINCT_CAP {
                let key = as_string(v);
                if a.distinct.insert(key) && a.samples.len() < SAMPLES {
                    a.samples.push(v.clone());
                }
            } else {
                a.capped = true;
            }
            if let Some(x) = v.as_number().and_then(|n| n.as_f64()) {
                a.min = Some(a.min.map_or(x, |m| m.min(x)));
                a.max = Some(a.max.map_or(x, |m| m.max(x)));
            }
        }
    }
}

/// Infer field statistics from decoded records.
pub fn infer(records: &[Value]) -> Vec<FieldStat> {
    let mut acc: BTreeMap<String, Acc> = BTreeMap::new();
    for (i, r) in records.iter().enumerate() {
        walk(r, "", i, &mut acc);
    }
    let n = records.len().max(1) as f64;
    acc.into_iter()
        .map(|(path, a)| FieldStat {
            mapping_path: path.replace("[*]", "[0]"),
            present: a.present_in.len() as u64,
            presence: a.present_in.len() as f64 / n,
            distinct: a.distinct.len(),
            distinct_capped: a.capped,
            samples: a.samples,
            min: a.min,
            max: a.max,
            types: a.types,
            path,
        })
        .collect()
}

/// If JSON frames wrap their records in an array of objects (e.g. `ac`),
/// the path to use as the codec's `records`.
pub fn suggest_records_path(frames: &[Value]) -> Option<String> {
    let first = frames.iter().find(|f| f.is_object())?;
    let obj = first.as_object()?;
    let (key, _) = obj
        .iter()
        .filter(|(_, v)| {
            v.as_array()
                .is_some_and(|a| !a.is_empty() && a.iter().all(Value::is_object))
        })
        .max_by_key(|(_, v)| v.as_array().map_or(0, Vec::len))?;
    // It must be the collection in (nearly) every frame.
    let hits = frames
        .iter()
        .filter(|f| f.get(key).is_some_and(|v| v.is_array() || v.is_null()))
        .count();
    (hits * 10 >= frames.len() * 9).then(|| segment(key))
}

/// A proposed field mapping with the evidence for it.
#[derive(Debug, Clone, Serialize)]
pub struct Proposal {
    pub target: String,
    pub spec: Value,
    pub confidence: f64,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Suggestion {
    /// A complete mapping spec to start from (one observation rule).
    pub mapping: Value,
    pub proposals: Vec<Proposal>,
    /// Suggested identifiers (scheme ← path). Always empty: a scheme names
    /// what a value identifies, which the values cannot tell; the admin adds them.
    pub identifiers: Vec<Value>,
    /// What could not be found; the admin must fill these in.
    pub missing: Vec<String>,
}

// Suggestions come from two things only: the values captured, and OpenTrack's
// own field names (`mapping::targets`). No feed's field names are known here.

/// A path's last part, lower case, without `@`, `_`, `-` or spaces.
fn norm(path: &str) -> String {
    let last = path
        .rsplit(['.', '['])
        .next()
        .unwrap_or(path)
        .trim_end_matches(']')
        .trim_matches('"')
        .trim_start_matches('@');
    last.to_ascii_lowercase().replace(['_', '-', ' '], "")
}

/// Unit suffixes of OpenTrack's field names (`course_deg`, `altitude_hae_m`).
const UNITS: &[&str] = &["_hae_m", "_mps", "_deg", "_m"];

/// The names a feed field can share with OpenTrack field `target`: its last
/// part (`course_deg`) and that without its unit (`course`), with the unit.
fn target_names(target: &str) -> (Vec<String>, Option<&'static str>) {
    let leaf = target.rsplit('.').next().unwrap_or(target);
    let mut names = vec![norm(leaf)];
    let mut unit = None;
    if let Some(u) = UNITS.iter().find(|u| leaf.ends_with(**u)) {
        names.push(norm(&leaf[..leaf.len() - u.len()]));
        unit = Some(&u[1..]);
    }
    (names, unit)
}

fn numeric(f: &FieldStat) -> bool {
    f.types.keys().any(|t| matches!(*t, "integer" | "number"))
        || (f.types.contains_key("string")
            && f.samples.iter().all(|s| as_f64(s).is_some())
            && !f.samples.is_empty())
}

/// The least and greatest value seen (over every record for numbers, the
/// samples for numeric strings).
fn bounds(f: &FieldStat) -> Option<(f64, f64)> {
    if let (Some(a), Some(b)) = (f.min, f.max) {
        return Some((a, b));
    }
    let xs: Vec<f64> = f.samples.iter().filter_map(as_f64).collect();
    (!xs.is_empty()).then(|| {
        (
            xs.iter().copied().fold(f64::MAX, f64::min),
            xs.iter().copied().fold(f64::MIN, f64::max),
        )
    })
}

fn in_range(f: &FieldStat, lo: f64, hi: f64) -> bool {
    bounds(f).is_some_and(|(mn, mx)| mn >= lo && mx <= hi)
}

/// Some sample has a fractional part.
fn fractional(f: &FieldStat) -> bool {
    f.samples
        .iter()
        .filter_map(as_f64)
        .any(|x| x.fract() != 0.0)
}

/// The most decimal places among the samples (coordinates carry several).
fn decimals(f: &FieldStat) -> usize {
    f.samples
        .iter()
        .map(|s| as_string(s).split_once('.').map_or(0, |(_, d)| d.len()))
        .max()
        .unwrap_or(0)
}

/// Every sample reads as a time within ten years before now and a day after.
fn is_time(f: &FieldStat) -> bool {
    let now = chrono::Utc::now();
    let window = (now - chrono::Duration::days(3653))..(now + chrono::Duration::days(1));
    !f.samples.is_empty()
        && f.samples.iter().all(|s| {
            crate::expr::parse_time_value(s)
                .as_str()
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                .is_some_and(|t| window.contains(&t.with_timezone(&chrono::Utc)))
        })
}

/// Every sample has the form of a CoT type (`a-f-G-U-C`).
fn is_cot_type(f: &FieldStat) -> bool {
    !f.samples.is_empty()
        && f.samples.iter().all(|s| {
            let s = as_string(s);
            s.starts_with("a-") && s.len() >= 5 && s.split('-').all(|p| !p.is_empty())
        })
}

/// Whether `f`'s values can be OpenTrack field `target`, which holds `t`.
fn fits(target: &str, t: FieldType, f: &FieldStat) -> bool {
    match t {
        FieldType::Number => {
            numeric(f)
                && match target {
                    "position.latitude" => in_range(f, -90.0, 90.0),
                    "position.longitude" => in_range(f, -180.0, 180.0),
                    "provenance.confidence" => in_range(f, 0.0, 1.0),
                    _ if target.ends_with("_deg") => in_range(f, -360.0, 360.0),
                    _ => true,
                }
        }
        FieldType::Time => is_time(f),
        FieldType::Text if target == "classification.cot_type" => is_cot_type(f),
        FieldType::Text => f.types.contains_key("string") && !numeric(f),
        // Enumerations, SIDCs and areas need their own forms: the admin maps them.
        _ => false,
    }
}

fn transform_for(t: FieldType) -> Option<&'static str> {
    match t {
        FieldType::Time => Some("time"),
        FieldType::Text => Some("trim"),
        _ => None,
    }
}

fn spec_for(f: &FieldStat, transform: Option<&str>) -> Value {
    match transform {
        Some(t) => json!({ "path": f.mapping_path, "transforms": [t] }),
        None => json!(f.mapping_path),
    }
}

fn confidence(x: f64) -> f64 {
    (x.min(1.0) * 100.0).round() / 100.0
}

/// The parent object's path.
fn parent(path: &str) -> &str {
    path.rfind(['.', '[']).map_or("", |i| &path[..i])
}

/// Suggest a mapping from inferred fields.
pub fn suggest(fields: &[FieldStat]) -> Suggestion {
    let usable: Vec<&FieldStat> = fields.iter().filter(|f| f.present > 0).collect();
    let mut proposals: Vec<Proposal> = Vec::new();
    let mut used: HashSet<&str> = HashSet::new();
    let mut missing = Vec::new();

    // A field named like an OpenTrack field whose values fit it. A name two
    // OpenTrack fields share (`name`, `platform.name`) goes to the first only.
    let mut taken_names: HashSet<String> = HashSet::new();
    for (target, t) in targets() {
        if target.starts_with("geometry.") {
            // A line of bearing or an area changes what the track is: never guessed.
            continue;
        }
        let (names, unit) = target_names(target);
        let names: Vec<String> = names
            .into_iter()
            .filter(|n| taken_names.insert(n.clone()))
            .collect();
        let best = usable
            .iter()
            .filter(|f| {
                !used.contains(f.path.as_str())
                    && names.contains(&norm(&f.path))
                    && fits(target, *t, f)
            })
            .max_by(|a, b| a.presence.total_cmp(&b.presence));
        if let Some(f) = best {
            used.insert(f.path.as_str());
            let transform = transform_for(*t);
            proposals.push(Proposal {
                target: (*target).to_owned(),
                spec: spec_for(f, transform),
                confidence: confidence(0.5 + 0.5 * f.presence),
                reason: format!(
                    "field {:?} is named like it and its values fit (present in {:.0}% of records{})",
                    f.path,
                    f.presence * 100.0,
                    unit.map(|u| format!("; assumed to be in {u}, check"))
                        .unwrap_or_default()
                ),
            });
        }
    }
    let proposed = |ps: &[Proposal], t: &str| ps.iter().any(|p| p.target == t);

    // A position from the values: two fractional coordinates in one object,
    // the one past ±90 the longitude.
    if !proposed(&proposals, "position.latitude") && !proposed(&proposals, "position.longitude") {
        let coords: Vec<&FieldStat> = usable
            .iter()
            .copied()
            .filter(|f| {
                !used.contains(f.path.as_str())
                    && numeric(f)
                    && fractional(f)
                    && in_range(f, -180.0, 180.0)
            })
            .collect();
        let mut pairs: Vec<(&FieldStat, &FieldStat)> = Vec::new();
        let mut unsure: Vec<String> = Vec::new();
        let parents: BTreeMap<&str, Vec<&FieldStat>> =
            coords.iter().fold(BTreeMap::new(), |mut m, f| {
                m.entry(parent(&f.path)).or_insert_with(Vec::new).push(f);
                m
            });
        for group in parents.values() {
            let lons: Vec<&FieldStat> = group
                .iter()
                .copied()
                .filter(|f| !in_range(f, -90.0, 90.0))
                .collect();
            let lats: Vec<&FieldStat> = group
                .iter()
                .copied()
                .filter(|f| in_range(f, -90.0, 90.0))
                .collect();
            // Beside the longitude, the latitude: the one with the most
            // decimal places, if one has more than the rest.
            let mut lats = lats;
            lats.sort_by_key(|f| std::cmp::Reverse(decimals(f)));
            match (lats.as_slice(), lons.as_slice()) {
                ([lat], [lon]) => pairs.push((*lat, *lon)),
                ([lat, next, ..], [lon]) if decimals(lat) > decimals(next) => {
                    pairs.push((*lat, *lon))
                }
                ([a, b], []) => unsure.push(format!("{:?} or {:?}", a.path, b.path)),
                _ => {}
            }
        }
        let best = pairs.into_iter().max_by(|a, b| {
            (a.0.presence.min(a.1.presence)).total_cmp(&b.0.presence.min(b.1.presence))
        });
        if let Some((lat, lon)) = best {
            for (target, f) in [("position.latitude", lat), ("position.longitude", lon)] {
                used.insert(f.path.as_str());
                proposals.push(Proposal {
                    target: target.to_owned(),
                    spec: json!(f.mapping_path),
                    confidence: confidence(0.3 + 0.4 * f.presence),
                    reason: format!(
                        "fractional coordinates {:?} and {:?} side by side; only {:?} goes past ±90",
                        lat.path, lon.path, lon.path
                    ),
                });
            }
        } else if let Some(u) = unsure.first() {
            missing.push(format!(
                "position (coordinates {u}: which is the latitude?)"
            ));
        }
    }

    // The time observed from the values: the most present field of times.
    if !proposed(&proposals, "observed_at") {
        let best = usable
            .iter()
            .filter(|f| !used.contains(f.path.as_str()) && is_time(f))
            .max_by(|a, b| a.presence.total_cmp(&b.presence).then(b.path.cmp(&a.path)));
        if let Some(f) = best {
            used.insert(f.path.as_str());
            proposals.push(Proposal {
                target: "observed_at".to_owned(),
                spec: spec_for(f, Some("time")),
                confidence: confidence(0.3 + 0.4 * f.presence),
                reason: format!(
                    "field {:?} holds times (present in {:.0}% of records)",
                    f.path,
                    f.presence * 100.0
                ),
            });
        }
    }

    // A CoT type from the values.
    if !proposed(&proposals, "classification.cot_type")
        && let Some(f) = usable
            .iter()
            .find(|f| !used.contains(f.path.as_str()) && is_cot_type(f))
    {
        used.insert(f.path.as_str());
        proposals.push(Proposal {
            target: "classification.cot_type".to_owned(),
            spec: json!(f.mapping_path),
            confidence: confidence(0.3 + 0.4 * f.presence),
            reason: format!("field {:?} holds CoT types", f.path),
        });
    }

    // The key from the values: in (nearly) every record, whole tokens of one
    // length (an id, not a name or a number with a fraction), the most distinct.
    let token = |f: &FieldStat| {
        f.types
            .keys()
            .all(|t| matches!(*t, "string" | "integer" | "null"))
            && f.samples.iter().all(|s| {
                let s = as_string(s);
                !s.is_empty() && !s.contains(char::is_whitespace)
            })
    };
    let one_length = |f: &FieldStat| {
        let lens: HashSet<usize> = f.samples.iter().map(|s| as_string(s).len()).collect();
        lens.len() == 1
    };
    let key = usable
        .iter()
        .filter(|f| !used.contains(f.path.as_str()) && f.presence >= 0.9 && token(f) && !is_time(f))
        .max_by(|a, b| {
            a.distinct
                .cmp(&b.distinct)
                .then(one_length(a).cmp(&one_length(b)))
                .then(a.presence.total_cmp(&b.presence))
                // Shallower, then first by name.
                .then(
                    b.path
                        .matches('.')
                        .count()
                        .cmp(&a.path.matches('.').count()),
                )
                .then(b.path.cmp(&a.path))
        });

    for t in ["position.latitude", "position.longitude"] {
        if !proposed(&proposals, t) && !missing.iter().any(|m| m.starts_with("position")) {
            missing.push(t.to_owned());
        }
    }
    if key.is_none() {
        missing.push("key".to_owned());
    }
    if !proposed(&proposals, "observed_at") {
        missing.push("observed_at (receive time will be used)".to_owned());
    }

    let mut field_map = Map::new();
    for p in &proposals {
        field_map.insert(p.target.clone(), p.spec.clone());
    }
    let mapping = json!({
        "schema_version": 1,
        "rules": [{
            "name": "observation",
            "key": key.map_or(Value::Null, |k| json!(k.mapping_path)),
            "identifiers": [],
            "fields": field_map,
        }]
    });
    Suggestion {
        mapping,
        proposals,
        identifiers: Vec::new(),
        missing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now(offset_s: i64) -> String {
        (chrono::Utc::now() - chrono::Duration::seconds(offset_s)).to_rfc3339()
    }

    #[test]
    fn infers_types_presence_and_samples() {
        let recs = vec![
            json!({"ref": "k01", "a": 32.1, "b": -117.0, "v": 250, "label": "R 1 ", "tags": [{"k": "a"}]}),
            json!({"ref": "k02", "a": 33.0, "b": -118.5, "v": 300.5, "label": null}),
            json!({"ref": "k03", "c": "ground"}),
        ];
        let stats = infer(&recs);
        let get = |p: &str| stats.iter().find(|f| f.path == p).unwrap();
        assert_eq!(get("ref").distinct, 3);
        assert!((get("a").presence - 2.0 / 3.0).abs() < 1e-9);
        assert_eq!(get("v").types.get("integer"), Some(&1));
        assert_eq!(get("v").types.get("number"), Some(&1));
        assert_eq!((get("b").min, get("b").max), (Some(-118.5), Some(-117.0)));
        assert_eq!(get("label").present, 1);
        assert_eq!(get("tags[*].k").mapping_path, "tags[0].k");
    }

    #[test]
    fn suggests_records_path() {
        let frames = vec![
            json!({"n": 1, "items": [{"r": "a"}]}),
            json!({"n": 2, "items": [{"r": "b"}, {"r": "c"}]}),
        ];
        assert_eq!(suggest_records_path(&frames).as_deref(), Some("items"));
        assert_eq!(suggest_records_path(&[json!({"a": 1})]), None);
    }

    #[test]
    fn fields_named_like_opentrack_fields_are_proposed_when_their_values_fit() {
        let recs: Vec<Value> = (0..10)
            .map(|i| json!({"ref": format!("k{i:03}"), "latitude": 30.5 + i as f64, "longitude": -117.25,
                            "speed": 3.5, "course": 90.5, "callsign": " R 1 ", "observed_at": now(i),
                            "heading": "north", "name": "UNIT", "extra": {"name": "OTHER"}}))
            .collect();
        let s = suggest(&infer(&recs));
        let f = &s.mapping["rules"][0]["fields"];
        assert_eq!(f["position.latitude"], "latitude");
        assert_eq!(f["position.longitude"], "longitude");
        assert_eq!(f["kinematics.speed_mps"], "speed");
        assert_eq!(f["kinematics.course_deg"], "course");
        assert_eq!(f["callsign"]["transforms"][0], "trim");
        assert_eq!(f["observed_at"]["transforms"][0], "time");
        // One `name`; the other is not taken for `platform.name`, which only ends like it.
        assert!(f.get("platform.name").is_none());
        // Named like it, but its values are not a number of degrees.
        assert!(f.get("kinematics.heading_deg").is_none());
        let speed = s
            .proposals
            .iter()
            .find(|p| p.target == "kinematics.speed_mps")
            .unwrap();
        assert!(
            speed.reason.contains("assumed to be in mps"),
            "{}",
            speed.reason
        );
        assert_eq!(s.mapping["rules"][0]["key"], "ref");
        assert!(s.identifiers.is_empty());
        let spec: crate::mapping::MappingSpec = serde_json::from_value(s.mapping).unwrap();
        spec.validate().unwrap();
    }

    #[test]
    fn a_position_time_and_key_are_found_from_the_values_alone() {
        let recs: Vec<Value> = (0..10)
            .map(|i| json!({"body": {"p": 32.12345 + i as f64 * 0.01, "q": -117.23456, "r": 4.5, "n": 7},
                            "when": now(i * 60), "id1": format!("{:06x}", 0xa0000 + i), "label": format!("UNIT {i}"),
                            "seq": i * 7}))
            .collect();
        let s = suggest(&infer(&recs));
        let rule = &s.mapping["rules"][0];
        assert_eq!(rule["fields"]["position.latitude"], "body.p");
        assert_eq!(rule["fields"]["position.longitude"], "body.q");
        assert_eq!(rule["fields"]["observed_at"]["path"], "when");
        // Ten distinct ids of one length beat a counter of varying length and a name with spaces.
        assert_eq!(rule["key"], "id1");
        assert!(s.missing.is_empty(), "{:?}", s.missing);
    }

    #[test]
    fn coordinates_the_values_cannot_tell_apart_are_asked_about() {
        let recs: Vec<Value> = (0..5)
            .map(|i| json!({"m": {"u": 12.3456 + i as f64, "w": 45.6789}}))
            .collect();
        let s = suggest(&infer(&recs));
        assert!(
            s.mapping["rules"][0]["fields"]
                .get("position.latitude")
                .is_none()
        );
        assert!(
            s.missing[0].contains("which is the latitude"),
            "{:?}",
            s.missing
        );
        assert!(s.missing.iter().any(|m| m.starts_with("observed_at")));
    }

    #[test]
    fn numbers_are_not_taken_for_times_outside_the_last_ten_years() {
        let recs: Vec<Value> = (0..5).map(|i| json!({"n": 500_000_000 + i})).collect();
        let s = suggest(&infer(&recs));
        assert!(s.mapping["rules"][0]["fields"].get("observed_at").is_none());
    }

    #[test]
    fn cot_attributes_are_recognised() {
        let recs = vec![
            json!({"event": {"@uid": "A1", "@type": "a-f-G-U-C", "@time": now(0),
            "point": {"@lat": "32.7", "@lon": "-117.2", "@hae": "10", "@ce": "5", "@le": "9999999"},
            "detail": {"contact": {"@callsign": "VIPER 1"}}}}),
        ];
        let s = suggest(&infer(&recs));
        let f = &s.mapping["rules"][0]["fields"];
        assert_eq!(f["position.latitude"], "event.point.@lat");
        assert_eq!(f["position.longitude"], "event.point.@lon");
        assert_eq!(f["classification.cot_type"], "event.@type");
        assert_eq!(f["callsign"]["path"], "event.detail.contact.@callsign");
        assert_eq!(f["observed_at"]["path"], "event.@time");
        assert_eq!(s.mapping["rules"][0]["key"], "event.@uid");
    }
}
