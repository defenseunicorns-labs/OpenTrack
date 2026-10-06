//! Probe analysis: infer a feed's schema from captured records and suggest
//! a mapping. Suggestions are only proposals; the admin confirms them.

use std::collections::{BTreeMap, HashSet};

use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::expr::{as_f64, as_string};

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
    /// Suggested identifiers (scheme ← path).
    pub identifiers: Vec<Value>,
    /// What could not be found; the admin must fill these in.
    pub missing: Vec<String>,
}

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

fn numeric(f: &FieldStat) -> bool {
    f.types.keys().any(|t| matches!(*t, "integer" | "number"))
        || (f.types.contains_key("string")
            && f.samples.iter().all(|s| as_f64(s).is_some())
            && !f.samples.is_empty())
}

fn in_range(f: &FieldStat, lo: f64, hi: f64) -> bool {
    let (mn, mx) = match (f.min, f.max) {
        (Some(a), Some(b)) => (a, b),
        _ => {
            let xs: Vec<f64> = f.samples.iter().filter_map(as_f64).collect();
            if xs.is_empty() {
                return false;
            }
            (
                xs.iter().copied().fold(f64::MAX, f64::min),
                xs.iter().copied().fold(f64::MIN, f64::max),
            )
        }
    };
    mn >= lo && mx <= hi
}

struct Candidate {
    names: &'static [&'static str],
    target: &'static str,
    check: fn(&FieldStat) -> bool,
    transform: Option<&'static str>,
}

fn any(_: &FieldStat) -> bool {
    true
}
fn is_lat(f: &FieldStat) -> bool {
    numeric(f) && in_range(f, -90.0, 90.0)
}
fn is_lon(f: &FieldStat) -> bool {
    numeric(f) && in_range(f, -180.0, 180.0)
}
fn is_angle(f: &FieldStat) -> bool {
    numeric(f) && in_range(f, 0.0, 360.0)
}
fn is_num(f: &FieldStat) -> bool {
    numeric(f)
}
fn is_cot_type(f: &FieldStat) -> bool {
    !f.samples.is_empty()
        && f.samples.iter().all(|s| {
            let s = as_string(s);
            s.starts_with("a-") && s.len() >= 5
        })
}

const CANDIDATES: &[Candidate] = &[
    Candidate {
        names: &["lat", "latitude", "y"],
        target: "position.latitude",
        check: is_lat,
        transform: None,
    },
    Candidate {
        names: &["lon", "lng", "long", "longitude", "x"],
        target: "position.longitude",
        check: is_lon,
        transform: None,
    },
    Candidate {
        names: &[
            "hae",
            "altitude",
            "altitudem",
            "altm",
            "height",
            "altgeom",
            "alt",
        ],
        target: "position.altitude_hae_m",
        check: is_num,
        transform: None,
    },
    Candidate {
        names: &["altft", "altitudeft", "altbaro", "altitudefeet"],
        target: "position.altitude_hae_m",
        check: is_num,
        transform: Some("feet_to_m"),
    },
    Candidate {
        names: &["course", "cog", "track", "trk", "bearing", "direction"],
        target: "kinematics.course_deg",
        check: is_angle,
        transform: None,
    },
    Candidate {
        names: &["heading", "hdg", "trueheading", "true_heading"],
        target: "kinematics.heading_deg",
        check: is_angle,
        transform: None,
    },
    Candidate {
        names: &["sog", "gs", "speedkt", "speedknots", "groundspeed", "knots"],
        target: "kinematics.speed_mps",
        check: is_num,
        transform: Some("knots_to_mps"),
    },
    Candidate {
        names: &["speedkmh", "kmh", "kph"],
        target: "kinematics.speed_mps",
        check: is_num,
        transform: Some("kmh_to_mps"),
    },
    Candidate {
        names: &["speed", "speedmps", "velocity", "spd", "mps"],
        target: "kinematics.speed_mps",
        check: is_num,
        transform: None,
    },
    Candidate {
        names: &["verticalrate", "climbrate", "vs", "vrate"],
        target: "kinematics.vertical_rate_mps",
        check: is_num,
        transform: None,
    },
    Candidate {
        names: &["barorate", "geomrate"],
        target: "kinematics.vertical_rate_mps",
        check: is_num,
        transform: Some("fpm_to_mps"),
    },
    Candidate {
        names: &[
            "time",
            "timestamp",
            "ts",
            "timeutc",
            "datetime",
            "observedat",
            "lastupdate",
            "updated",
            "start",
        ],
        target: "observed_at",
        check: any,
        transform: Some("time"),
    },
    Candidate {
        names: &["name", "shipname", "vesselname", "title"],
        target: "name",
        check: any,
        transform: Some("trim"),
    },
    Candidate {
        names: &["callsign", "flight", "cs"],
        target: "callsign",
        check: any,
        transform: Some("trim"),
    },
    Candidate {
        names: &["type", "cottype"],
        target: "classification.cot_type",
        check: is_cot_type,
        transform: None,
    },
    Candidate {
        names: &["ce", "cep", "rc", "circularerror"],
        target: "uncertainty.circular_error_m",
        check: is_num,
        transform: None,
    },
    Candidate {
        names: &["le", "verticalerror"],
        target: "uncertainty.vertical_error_m",
        check: is_num,
        transform: None,
    },
    Candidate {
        names: &["confidence", "conf", "quality"],
        target: "provenance.confidence",
        check: |f| is_num(f) && in_range(f, 0.0, 1.0),
        transform: None,
    },
];

/// Identifier schemes recognised from field names. Anything else can be
/// added by the admin; schemes are open-ended.
const IDENTIFIER_NAMES: &[(&str, &str)] = &[
    ("mmsi", "mmsi"),
    ("icao", "icao"),
    ("hex", "icao"),
    ("icao24", "icao"),
    ("imo", "imo"),
    ("imonumber", "imo"),
    ("elnot", "elnot"),
    ("uid", "cot-uid"),
    ("registration", "registration"),
    ("tailnumber", "registration"),
    ("hull", "hull"),
    ("hullnumber", "hull"),
];

const KEY_NAMES: &[&str] = &[
    "id",
    "uid",
    "mmsi",
    "icao",
    "hex",
    "icao24",
    "trackid",
    "uuid",
    "tracknumber",
    "tn",
];

/// Suggest a mapping from inferred fields.
pub fn suggest(fields: &[FieldStat]) -> Suggestion {
    let usable: Vec<&FieldStat> = fields.iter().filter(|f| f.present > 0).collect();
    let mut proposals: Vec<Proposal> = Vec::new();
    let mut used: HashSet<&str> = HashSet::new();
    for c in CANDIDATES {
        if proposals.iter().any(|p| p.target == c.target) {
            continue;
        }
        let best = usable
            .iter()
            .filter(|f| {
                !used.contains(f.path.as_str())
                    && c.names.contains(&norm(&f.path).as_str())
                    && (c.check)(f)
            })
            .max_by(|a, b| a.presence.total_cmp(&b.presence));
        if let Some(f) = best {
            used.insert(f.path.as_str());
            let spec = match c.transform {
                Some(t) => json!({ "path": f.mapping_path, "transforms": [t] }),
                None => json!(f.mapping_path),
            };
            let confidence = (0.5 + 0.5 * f.presence).min(1.0);
            proposals.push(Proposal {
                target: c.target.to_owned(),
                spec,
                confidence: (confidence * 100.0).round() / 100.0,
                reason: format!(
                    "field {:?} (present in {:.0}% of records{})",
                    f.path,
                    f.presence * 100.0,
                    c.transform
                        .map(|t| format!(", transform {t}"))
                        .unwrap_or_default()
                ),
            });
        }
    }

    // Key: a named id field, preferring the one closest to unique per record.
    let key = usable
        .iter()
        .filter(|f| KEY_NAMES.contains(&norm(&f.path).as_str()) && f.presence >= 0.9)
        .max_by(|a, b| {
            a.distinct
                .cmp(&b.distinct)
                .then(a.presence.total_cmp(&b.presence))
        });
    let identifiers: Vec<Value> = usable
        .iter()
        .filter_map(|f| {
            IDENTIFIER_NAMES
                .iter()
                .find(|(n, _)| *n == norm(&f.path))
                .map(|(_, scheme)| json!({ "scheme": scheme, "value": f.mapping_path }))
        })
        .collect();

    let mut missing = Vec::new();
    for t in ["position.latitude", "position.longitude"] {
        if !proposals.iter().any(|p| p.target == t) {
            missing.push(t.to_owned());
        }
    }
    if key.is_none() {
        missing.push("key".to_owned());
    }
    if !proposals.iter().any(|p| p.target == "observed_at") {
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
            "identifiers": identifiers,
            "fields": field_map,
        }]
    });
    Suggestion {
        mapping,
        proposals,
        identifiers,
        missing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infers_types_presence_and_samples() {
        let recs = vec![
            json!({"hex": "ae01", "lat": 32.1, "lon": -117.0, "gs": 250, "flight": "RCH1 ", "tags": [{"k": "a"}]}),
            json!({"hex": "ae02", "lat": 33.0, "lon": -118.5, "gs": 300.5, "flight": null}),
            json!({"hex": "ae03", "alt_baro": "ground"}),
        ];
        let stats = infer(&recs);
        let get = |p: &str| stats.iter().find(|f| f.path == p).unwrap();
        assert_eq!(get("hex").distinct, 3);
        assert!((get("lat").presence - 2.0 / 3.0).abs() < 1e-9);
        assert_eq!(get("gs").types.get("integer"), Some(&1));
        assert_eq!(get("gs").types.get("number"), Some(&1));
        assert_eq!(
            (get("lon").min, get("lon").max),
            (Some(-118.5), Some(-117.0))
        );
        assert_eq!(get("flight").present, 1);
        assert_eq!(get("tags[*].k").mapping_path, "tags[0].k");
    }

    #[test]
    fn suggests_records_path_and_mapping() {
        let frames = vec![
            json!({"now": 1, "ac": [{"hex": "a"}]}),
            json!({"now": 2, "ac": [{"hex": "b"}, {"hex": "c"}]}),
        ];
        assert_eq!(suggest_records_path(&frames).as_deref(), Some("ac"));
        assert_eq!(suggest_records_path(&[json!({"a": 1})]), None);

        let recs: Vec<Value> = (0..10)
            .map(|i| json!({"hex": format!("ae{i:02}"), "lat": 30.0 + i as f64, "lon": -117.0, "gs": 200,
                            "track": 90, "flight": "RCH1", "seen_at": "2026-09-25T00:00:00Z", "type": "adsb_icao"}))
            .collect();
        let s = suggest(&infer(&recs));
        let rule = &s.mapping["rules"][0];
        assert_eq!(rule["key"], "hex");
        assert_eq!(rule["fields"]["position.latitude"], "lat");
        assert_eq!(
            rule["fields"]["kinematics.speed_mps"]["transforms"][0],
            "knots_to_mps"
        );
        assert_eq!(rule["fields"]["kinematics.course_deg"], "track");
        assert_eq!(
            rule["identifiers"][0],
            json!({"scheme": "icao", "value": "hex"})
        );
        // "type" holds adsb.lol's message type, not a CoT type: not proposed.
        assert!(rule["fields"].get("classification.cot_type").is_none());
        assert!(
            s.missing.iter().any(|m| m.starts_with("observed_at")),
            "{:?}",
            s.missing
        );
        // The suggestion is a valid mapping spec.
        let spec: crate::mapping::MappingSpec = serde_json::from_value(s.mapping).unwrap();
        spec.validate().unwrap();
    }

    #[test]
    fn cot_attributes_are_recognised() {
        let recs = vec![
            json!({"event": {"@uid": "A1", "@type": "a-f-G-U-C", "@time": "2026-09-25T00:00:00Z",
            "point": {"@lat": "32.7", "@lon": "-117.2", "@hae": "10", "@ce": "5", "@le": "9999999"},
            "detail": {"contact": {"@callsign": "VIPER 1"}}}}),
        ];
        let s = suggest(&infer(&recs));
        let f = &s.mapping["rules"][0]["fields"];
        assert_eq!(f["position.latitude"], "event.point.@lat");
        assert_eq!(f["classification.cot_type"], "event.@type");
        assert_eq!(f["callsign"]["path"], "event.detail.contact.@callsign");
        assert_eq!(f["observed_at"]["path"], "event.@time");
        assert_eq!(s.mapping["rules"][0]["key"], "event.@uid");
        assert_eq!(s.identifiers[0]["scheme"], "cot-uid");
    }
}
