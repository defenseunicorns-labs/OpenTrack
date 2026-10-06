//! Registry lookup: resolve a track's identifiers against the identity
//! registry and grade how well the live report corroborates the entry.
//!
//! Grades, strongest first:
//! * `exact`: the broadcast name equals the identifier's expected name.
//! * `hull`: a token of the broadcast name is one of the entry's hull numbers.
//! * `name`: the distinctive (non-generic) tokens of the broadcast name are
//!   all in the entry's name, and add up to at least four characters.
//! * `generic`: the broadcast name is only generic words including a marker
//!   (e.g. "NAVY"), and the entry is a warship (has a hull code or a military
//!   classification).
//! * `stale`: the identifier is registered, but nothing corroborates it.
//!
//! Identifiers are open-ended: any scheme (`mmsi`, `icao`, `elnot`, `hull`,
//! `cot-uid`, ...) with any value, and a scheme implies nothing about the
//! platform's domain. The stage resolves every identifier a track carries
//! (optionally restricted to, and prioritised by, a list of schemes). When
//! identifiers resolve to different entities the match is recorded as a
//! conflict and nothing is applied.
//!
//! At a corroborated match the stage links entity fields and track fields,
//! each link one way: entity → track (the entity is the authority: its value
//! replaces what the feed reports, and a difference is recorded) or track →
//! entity (the feed updates the entity, e.g. an AIS destination). The token
//! lists, the grades that count as corroborated and the links are per-source
//! configuration.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::expr::{RegexString, as_string};
use crate::path::Path;

/// One registry entity as seen through one identifier.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RegistryEntry {
    pub entity_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Name the platform is expected to broadcast under this identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_name: Option<String>,
    /// The entity's other fields, flat: the OTH-GOLD minimum (`class_name`,
    /// `domain`, `affiliation`, `track_type`, `cot_type`, `sidc`) and its
    /// attributes by key (`flag`, `hull_code`, ...).
    #[serde(default)]
    pub fields: Map<String, Value>,
}

impl RegistryEntry {
    /// A field by name: `entity_id`, `name`, or one of `fields`.
    pub fn field(&self, name: &str) -> Option<Value> {
        match name {
            "entity_id" => Some(Value::String(self.entity_id.clone())),
            "name" => self.name.clone().map(Value::String),
            _ => self.fields.get(name).cloned(),
        }
        .filter(|v| !v.is_null() && !as_string(v).trim().is_empty())
    }

    fn text(&self, name: &str) -> Option<String> {
        self.field(name).map(|v| as_string(&v))
    }
}

/// Anything that can resolve identifiers. The worker holds a snapshot of the
/// SQLite registry that implements this.
pub trait RegistryLookup: Send + Sync {
    fn lookup(&self, scheme: &str, value: &str) -> Option<RegistryEntry>;
}

impl RegistryLookup for BTreeMap<(String, String), RegistryEntry> {
    fn lookup(&self, scheme: &str, value: &str) -> Option<RegistryEntry> {
        self.get(&(scheme.to_owned(), value.to_owned())).cloned()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Grade {
    Exact,
    Hull,
    Name,
    Generic,
    Stale,
}

impl Grade {
    pub fn as_str(self) -> &'static str {
        match self {
            Grade::Exact => "exact",
            Grade::Hull => "hull",
            Grade::Name => "name",
            Grade::Generic => "generic",
            Grade::Stale => "stale",
        }
    }
}

/// Which way a link copies a value.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkDirection {
    /// The entity's value populates the track (the entity is the authority).
    #[default]
    ToTrack,
    /// The track's value updates the entity.
    ToEntity,
}

/// One entity field linked to one track field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntityLink {
    /// Entity field: one of the minimum (`name`, `class_name`, `domain`,
    /// `affiliation`, `track_type`, `cot_type`, `sidc`), `entity_id`, or an
    /// attribute key.
    pub entity: String,
    /// Track (observation) field, e.g. `classification.domain` or `ext.destination`.
    pub track: Path,
    #[serde(default)]
    pub direction: LinkDirection,
}

impl EntityLink {
    fn to_track(entity: &str, track: &str) -> Self {
        Self {
            entity: entity.into(),
            track: track.parse().expect("path"),
            direction: LinkDirection::ToTrack,
        }
    }
}

/// Where mapped `entity.<key>` values travel on an observation
/// (`ext.entity.<key>`): at a corroborated match they update the entity, and
/// they are never published.
pub const ENTITY_EXT: &str = "entity";

/// The links a stage has unless it lists its own: the entity's OTH-GOLD
/// minimum populates the track.
pub fn default_links() -> Vec<EntityLink> {
    [
        ("name", "name"),
        ("class_name", "platform.class"),
        ("domain", "classification.domain"),
        ("affiliation", "classification.affiliation"),
        ("track_type", "track_type"),
        ("cot_type", "classification.cot_type"),
        ("sidc", "classification.sidc"),
    ]
    .into_iter()
    .map(|(e, t)| EntityLink::to_track(e, t))
    .collect()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "RawStage")]
pub struct RegistryStage {
    /// Identifier schemes to resolve, in priority order. Empty (the default)
    /// resolves every identifier the observation carries, in its order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub schemes: Vec<String>,
    /// Observation field holding the broadcast name to grade. At the default
    /// (`name`), a report without a name is graded on its `callsign`.
    #[serde(default = "default_name_field")]
    pub broadcast_name: Path,
    /// Words that do not identify a platform (prefixes, nationalities...).
    #[serde(default)]
    pub generic_tokens: BTreeSet<String>,
    /// Generic words that still indicate a warship.
    #[serde(default)]
    pub markers: BTreeSet<String>,
    /// Classification pattern that makes an entry military for the `generic`
    /// grade (besides having a hull code).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub military_cot: Option<RegexString>,
    /// Grades at which `apply` is used; default exact, hull, name, generic.
    #[serde(default = "corroborated")]
    pub apply_grades: BTreeSet<Grade>,
    /// Entity fields linked to track fields, used at the grades above.
    pub links: Vec<EntityLink>,
}

/// The stage as stored: `apply` (track field ← entity field) is an earlier
/// form of entity → track links, read once.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStage {
    #[serde(default)]
    schemes: Vec<String>,
    #[serde(default = "default_name_field")]
    broadcast_name: Path,
    #[serde(default)]
    generic_tokens: BTreeSet<String>,
    #[serde(default)]
    markers: BTreeSet<String>,
    #[serde(default)]
    military_cot: Option<RegexString>,
    #[serde(default = "corroborated")]
    apply_grades: BTreeSet<Grade>,
    #[serde(default)]
    links: Option<Vec<EntityLink>>,
    #[serde(default)]
    apply: BTreeMap<String, Path>,
}

impl From<RawStage> for RegistryStage {
    fn from(r: RawStage) -> Self {
        let links = match r.links {
            Some(links) => links,
            None if r.apply.is_empty() => default_links(),
            None => r
                .apply
                .into_iter()
                .map(|(track, entity)| EntityLink {
                    entity: match entity.to_string().as_str() {
                        "cot" => "cot_type".into(),
                        "ship_class" => "class_name".into(),
                        other => other.into(),
                    },
                    track: track.parse().unwrap_or_else(|_| default_name_field()),
                    direction: LinkDirection::ToTrack,
                })
                .collect(),
        };
        Self {
            schemes: r.schemes,
            broadcast_name: r.broadcast_name,
            generic_tokens: r.generic_tokens,
            markers: r.markers,
            military_cot: r.military_cot,
            apply_grades: r.apply_grades,
            links,
        }
    }
}

impl Default for RegistryStage {
    fn default() -> Self {
        serde_json::from_value(json!({})).expect("stage defaults")
    }
}

fn default_name_field() -> Path {
    "name".parse().expect("path")
}

fn corroborated() -> BTreeSet<Grade> {
    [Grade::Exact, Grade::Hull, Grade::Name, Grade::Generic].into()
}

/// What the stage found, as recorded on the observation under `ext.registry`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RegistryMatch {
    pub entity_id: String,
    pub grade: Grade,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub applied: bool,
    /// The match is trustworthy: an accepted grade and no disagreeing
    /// identifiers. The entity's links are then used.
    pub corroborated: bool,
    /// The identifier that produced the grade.
    pub scheme: String,
    pub value: String,
    /// Other entities this track's identifiers resolve to. Non-empty means
    /// the identifiers disagree, so nothing was applied.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub conflicts: Vec<String>,
    /// Track fields where the entity replaced a different feed value.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<Override>,
    /// The source whose report this is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    /// Entity fields the feed reports differently (track → entity links),
    /// for the worker to write.
    #[serde(skip)]
    pub updates: Vec<(String, Value)>,
    /// Track fields the entity set (entity → track links it had a value
    /// for), so later stages leave them: the entity is the authority.
    #[serde(skip)]
    pub set: Vec<String>,
}

/// A feed value the entity replaced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Override {
    /// Track field.
    pub field: String,
    pub reported: Value,
    pub entity: Value,
}

fn tokens(s: &str) -> Vec<String> {
    static SPLIT: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    SPLIT
        .get_or_init(|| Regex::new(r"[^A-Z0-9]+").expect("regex"))
        .split(&s.to_uppercase())
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .collect()
}

fn hull_numbers(hull: &str) -> HashSet<String> {
    static DIGITS: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = DIGITS.get_or_init(|| Regex::new(r"\d+").expect("regex"));
    let mut out = HashSet::new();
    for m in re.find_iter(hull) {
        out.insert(m.as_str().to_owned());
        let stripped = m.as_str().trim_start_matches('0');
        if !stripped.is_empty() {
            out.insert(stripped.to_owned());
        }
    }
    out
}

impl RegistryStage {
    /// Grade a broadcast name against an entry.
    pub fn grade(&self, broadcast: Option<&str>, entry: &RegistryEntry) -> Grade {
        let live = broadcast.unwrap_or("").trim().to_uppercase();
        if live.is_empty() {
            return Grade::Stale;
        }
        if entry
            .expected_name
            .as_deref()
            .is_some_and(|e| !e.trim().is_empty() && e.trim().to_uppercase() == live)
        {
            return Grade::Exact;
        }
        let live_tokens = tokens(&live);
        let hull = entry.text("hull_code");
        if let Some(h) = &hull {
            let numbers = hull_numbers(h);
            if live_tokens.iter().any(|t| numbers.contains(t)) {
                return Grade::Hull;
            }
        }
        let distinctive: Vec<&String> = live_tokens
            .iter()
            .filter(|t| !self.generic_tokens.contains(*t))
            .collect();
        if !distinctive.is_empty() {
            let entry_tokens: HashSet<String> = entry
                .name
                .as_deref()
                .map(tokens)
                .unwrap_or_default()
                .into_iter()
                .collect();
            let len: usize = distinctive.iter().map(|t| t.len()).sum();
            if len >= 4 && distinctive.iter().all(|t| entry_tokens.contains(*t)) {
                return Grade::Name;
            }
            return Grade::Stale;
        }
        let marked = live_tokens.iter().any(|t| self.markers.contains(t));
        let military = hull.is_some()
            || self
                .military_cot
                .as_ref()
                .is_some_and(|re| entry.text("cot_type").is_some_and(|c| re.0.0.is_match(&c)));
        if marked && military {
            Grade::Generic
        } else {
            Grade::Stale
        }
    }

    /// Resolve the observation's identifiers, grade the best match, apply the
    /// entry at corroborated grades (unless identifiers conflict), and record
    /// the match under `ext.registry`. Returns the match, if any.
    pub fn run(&self, obs: &mut Value, registry: &dyn RegistryLookup) -> Option<RegistryMatch> {
        let carried: Vec<(String, String)> = obs
            .get("identifiers")
            .and_then(Value::as_array)?
            .iter()
            .filter_map(|i| {
                let scheme = i.get("scheme").and_then(Value::as_str)?;
                let value = as_string(i.get("value")?);
                (!value.trim().is_empty()).then(|| (scheme.to_owned(), value))
            })
            .collect();
        let ordered: Vec<&(String, String)> = if self.schemes.is_empty() {
            carried.iter().collect()
        } else {
            self.schemes
                .iter()
                .flat_map(|s| carried.iter().filter(move |(scheme, _)| scheme == s))
                .collect()
        };
        let hits: Vec<(&(String, String), RegistryEntry)> = ordered
            .into_iter()
            .filter_map(|id| registry.lookup(&id.0, &id.1).map(|e| (id, e)))
            .collect();
        let primary = hits.first()?.1.entity_id.clone();
        let mut conflicts: Vec<String> = hits
            .iter()
            .map(|(_, e)| e.entity_id.clone())
            .filter(|e| *e != primary)
            .collect();
        conflicts.sort();
        conflicts.dedup();
        // The broadcast name: the configured field; a report without one at the
        // default (`name`) is graded on its callsign, which is all ADS-B sends.
        let broadcast = self
            .broadcast_name
            .get(obs)
            .map(as_string)
            .filter(|n| !n.trim().is_empty())
            .or_else(|| {
                (self.broadcast_name == default_name_field())
                    .then(|| obs.get("callsign").map(as_string))
                    .flatten()
            });
        // Each identifier carries its own expected name: grade them all and
        // keep the strongest.
        let (id, entry, grade) = hits
            .into_iter()
            .filter(|(_, e)| e.entity_id == primary)
            .map(|(id, e)| {
                let g = self.grade(broadcast.as_deref(), &e);
                (id, e, g)
            })
            .min_by_key(|(_, _, g)| *g)?;
        let corroborated = conflicts.is_empty() && self.apply_grades.contains(&grade);
        let (mut overrides, mut updates, mut set) = (Vec::new(), Vec::new(), Vec::new());
        // Values a mapping sent to the entity (`entity.<key>`).
        let mapped = obs
            .get_mut("ext")
            .and_then(Value::as_object_mut)
            .and_then(|ext| ext.remove(ENTITY_EXT));
        if corroborated && let Some(Value::Object(mapped)) = mapped {
            for (k, v) in mapped {
                if !v.is_null() && entry.field(&k).as_ref() != Some(&v) {
                    updates.push((k, v));
                }
            }
        }
        if corroborated {
            // Feed values first, before an entity value replaces one.
            for l in self
                .links
                .iter()
                .filter(|l| l.direction == LinkDirection::ToEntity)
            {
                if let Some(v) = l.track.get(obs).filter(|v| !v.is_null())
                    && entry.field(&l.entity).as_ref() != Some(v)
                {
                    updates.push((l.entity.clone(), v.clone()));
                }
            }
            for l in self
                .links
                .iter()
                .filter(|l| l.direction == LinkDirection::ToTrack)
            {
                let Some(v) = entry.field(&l.entity) else {
                    continue;
                };
                if let Some(reported) = l.track.get(obs).filter(|r| !r.is_null())
                    && as_string(reported) != as_string(&v)
                {
                    overrides.push(Override {
                        field: l.track.to_string(),
                        reported: reported.clone(),
                        entity: v.clone(),
                    });
                }
                l.track.set(obs, v);
                set.push(l.track.to_string());
            }
        }
        let m = RegistryMatch {
            entity_id: entry.entity_id.clone(),
            grade,
            name: entry.name.clone(),
            applied: corroborated,
            corroborated,
            scheme: id.0.clone(),
            value: id.1.clone(),
            conflicts,
            overrides,
            updates,
            set,
            source_id: obs
                .get("source_id")
                .and_then(Value::as_str)
                .map(str::to_owned),
        };
        let ext = obs
            .as_object_mut()
            .expect("observation is an object")
            .entry("ext")
            .or_insert_with(|| json!({}));
        if let Some(ext) = ext.as_object_mut() {
            ext.insert(
                "registry".into(),
                serde_json::to_value(&m).expect("match serialises"),
            );
        }
        Some(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stage() -> RegistryStage {
        serde_json::from_value(json!({
            "generic_tokens": ["USS", "USNS", "NAVY", "WARSHIP", "US", "GOV", "THE"],
            "markers": ["NAVY", "WARSHIP", "GOV", "USS"],
            "military_cot": "^a-.-(U|S-C)",
            "apply": { "classification.cot_type": "cot", "callsign": "name", "platform.flag": "flag" }
        }))
        .unwrap()
    }

    fn entry() -> RegistryEntry {
        RegistryEntry {
            entity_id: "e1".into(),
            name: Some("USS TED STEVENS".into()),
            expected_name: Some("TED STEVENS".into()),
            fields: serde_json::from_value(
                json!({"cot_type": "a-f-S-C-L-D-D", "flag": "US", "hull_code": "DDG-128"}),
            )
            .unwrap(),
        }
    }

    #[test]
    fn grades() {
        let s = stage();
        let e = entry();
        assert_eq!(s.grade(Some("ted stevens"), &e), Grade::Exact);
        assert_eq!(s.grade(Some("WARSHIP 128"), &e), Grade::Hull);
        assert_eq!(s.grade(Some("USS STEVENS"), &e), Grade::Name);
        assert_eq!(s.grade(Some("US NAVY"), &e), Grade::Generic);
        assert_eq!(s.grade(Some("FISHING BOAT"), &e), Grade::Stale);
        assert_eq!(s.grade(Some("  "), &e), Grade::Stale);
        assert_eq!(s.grade(None, &e), Grade::Stale);
        // Short distinctive tokens do not count as a name match.
        let mut short = e.clone();
        short.name = Some("USS ABC".into());
        assert_eq!(s.grade(Some("ABC"), &short), Grade::Stale);
        // Generic needs a warship entry.
        let mut civil = e.clone();
        civil.fields = serde_json::from_value(json!({"cot_type": "a-u-S-X"})).unwrap();
        assert_eq!(s.grade(Some("NAVY"), &civil), Grade::Stale);
    }

    #[test]
    fn hull_numbers_ignore_leading_zeros() {
        let mut e = entry();
        e.fields.insert("hull_code".into(), json!("FFG-062"));
        assert_eq!(stage().grade(Some("WARSHIP 62"), &e), Grade::Hull);
    }

    #[test]
    fn run_applies_only_when_corroborated() {
        let reg: BTreeMap<(String, String), RegistryEntry> =
            [(("mmsi".to_string(), "338924210".to_string()), entry())].into();
        let mut obs = json!({"identifiers": [{"scheme": "mmsi", "value": "338924210"}],
                             "name": "TED STEVENS", "classification": {"cot_type": "a-u-S"}});
        let m = stage().run(&mut obs, &reg).unwrap();
        assert_eq!(m.grade, Grade::Exact);
        assert_eq!(obs["classification"]["cot_type"], "a-f-S-C-L-D-D");
        assert_eq!(obs["callsign"], "USS TED STEVENS");
        assert_eq!(obs["platform"]["flag"], "US");
        assert_eq!(obs["ext"]["registry"]["grade"], "exact");

        let mut stale = json!({"identifiers": [{"scheme": "mmsi", "value": "338924210"}],
                               "name": "SOMETHING ELSE", "classification": {"cot_type": "a-u-S"}});
        let m = stage().run(&mut stale, &reg).unwrap();
        assert_eq!(m.grade, Grade::Stale);
        assert!(!m.applied);
        assert_eq!(stale["classification"]["cot_type"], "a-u-S");
        assert_eq!(stale["ext"]["registry"]["entity_id"], "e1");

        let mut miss = json!({"identifiers": [{"scheme": "mmsi", "value": "1"}]});
        assert!(stage().run(&mut miss, &reg).is_none());
    }

    #[test]
    fn a_report_without_a_name_is_graded_on_its_callsign() {
        // ADS-B: an ICAO address and a flight callsign, no name.
        let lost = RegistryEntry {
            entity_id: "e2".into(),
            name: Some("LOST56".into()),
            expected_name: None,
            fields: serde_json::from_value(json!({"cot_type": "a-f-A-M-V"})).unwrap(),
        };
        let reg: BTreeMap<(String, String), RegistryEntry> =
            [(("icao".to_string(), "080010".to_string()), lost)].into();
        let report = || {
            json!({"identifiers": [{"scheme": "icao", "value": "080010"}],
                   "callsign": "LOST56", "classification": {"cot_type": "a-u-A-M-F"}})
        };
        let mut obs = report();
        let m = stage().run(&mut obs, &reg).unwrap();
        assert_eq!(m.grade, Grade::Name);
        assert_eq!(obs["classification"]["cot_type"], "a-f-A-M-V");
        // A configured name field is graded as configured: no fallback.
        let mut s = stage();
        s.broadcast_name = "ext.shipname".parse().unwrap();
        let mut obs = report();
        assert_eq!(s.run(&mut obs, &reg).unwrap().grade, Grade::Stale);
    }

    #[test]
    fn links_go_both_ways_and_record_what_the_entity_replaced() {
        let reg: BTreeMap<(String, String), RegistryEntry> =
            [(("mmsi".to_string(), "338924210".to_string()), entry())].into();
        let mut s: RegistryStage = serde_json::from_value(json!({
            "links": [
                {"entity": "cot_type", "track": "classification.cot_type"},
                {"entity": "destination", "track": "ext.destination", "direction": "to_entity"},
                {"entity": "flag", "track": "platform.flag", "direction": "to_entity"}]
        }))
        .unwrap();
        let mut obs = json!({"identifiers": [{"scheme": "mmsi", "value": "338924210"}],
                             "name": "TED STEVENS", "classification": {"cot_type": "a-u-S"},
                             "platform": {"flag": "US"}, "ext": {"destination": "LONG BEACH"}});
        let m = s.run(&mut obs, &reg).unwrap();
        assert_eq!(obs["classification"]["cot_type"], "a-f-S-C-L-D-D");
        assert_eq!(
            obs["ext"]["registry"]["overrides"][0]["field"],
            "classification.cot_type"
        );
        assert_eq!(obs["ext"]["registry"]["overrides"][0]["reported"], "a-u-S");
        // Only what differs from the entity is sent back to it.
        assert_eq!(
            m.updates,
            [("destination".to_string(), json!("LONG BEACH"))]
        );

        // Uncorroborated: nothing either way.
        s.apply_grades = [Grade::Exact].into();
        let mut obs = json!({"identifiers": [{"scheme": "mmsi", "value": "338924210"}],
                             "name": "OTHER", "ext": {"destination": "LONG BEACH"}});
        let m = s.run(&mut obs, &reg).unwrap();
        assert!(m.updates.is_empty() && !m.applied);

        // No links configured: the OTH-GOLD minimum populates the track.
        let d = RegistryStage::default();
        assert_eq!(d.links, default_links());
        let mut e = entry();
        e.fields.insert("domain".into(), json!("surface"));
        e.fields.insert("affiliation".into(), json!("friend"));
        let reg: BTreeMap<(String, String), RegistryEntry> =
            [(("mmsi".to_string(), "338924210".to_string()), e)].into();
        let mut obs = json!({"identifiers": [{"scheme": "mmsi", "value": "338924210"}],
                             "name": "TED STEVENS"});
        d.run(&mut obs, &reg).unwrap();
        assert_eq!(obs["classification"]["domain"], "surface");
        assert_eq!(obs["classification"]["affiliation"], "friend");
        assert_eq!(obs["name"], "USS TED STEVENS");
        // A stored `apply` reads as entity → track links, round trip as links.
        let legacy = stage();
        assert!(legacy.links.iter().any(|l| l.entity == "cot_type"
            && l.track.to_string() == "classification.cot_type"));
        let back: RegistryStage =
            serde_json::from_value(serde_json::to_value(&legacy).unwrap()).unwrap();
        assert_eq!(back, legacy);
    }

    #[test]
    fn any_scheme_resolves_and_disagreement_is_a_conflict() {
        let mut other = entry();
        other.entity_id = "e2".into();
        let reg: BTreeMap<(String, String), RegistryEntry> = [
            (("elnot".to_string(), "X123A".to_string()), entry()),
            (("hull".to_string(), "DDG-128".to_string()), entry()),
            (("icao".to_string(), "ae1234".to_string()), other),
        ]
        .into();
        // An ELNOT alone resolves: schemes are open-ended.
        let mut obs =
            json!({"identifiers": [{"scheme": "elnot", "value": "X123A"}], "name": "TED STEVENS"});
        let m = stage().run(&mut obs, &reg).unwrap();
        assert_eq!(
            (m.scheme.as_str(), m.grade, m.applied),
            ("elnot", Grade::Exact, true)
        );

        // Two identifiers, same entity: fine. A third pointing elsewhere: conflict, nothing applied.
        let mut obs = json!({"identifiers": [
            {"scheme": "elnot", "value": "X123A"}, {"scheme": "hull", "value": "DDG-128"},
            {"scheme": "icao", "value": "ae1234"}], "name": "TED STEVENS",
            "classification": {"cot_type": "a-u-S"}});
        let m = stage().run(&mut obs, &reg).unwrap();
        assert_eq!(m.conflicts, ["e2"]);
        assert!(!m.applied);
        assert_eq!(obs["classification"]["cot_type"], "a-u-S");
        assert_eq!(obs["ext"]["registry"]["conflicts"][0], "e2");

        // A scheme list restricts and prioritises.
        let mut s = stage();
        s.schemes = vec!["icao".into(), "elnot".into()];
        let mut obs = json!({"identifiers": [
            {"scheme": "elnot", "value": "X123A"}, {"scheme": "icao", "value": "ae1234"}]});
        let m = s.run(&mut obs, &reg).unwrap();
        assert_eq!((m.entity_id.as_str(), m.scheme.as_str()), ("e2", "icao"));
    }
}
