//! The authoritative track schema.
//!
//! Every observation from every source is mapped into an [`Observation`]: a
//! fixed core that correlation and published tracks depend on, plus admin-defined
//! extension fields carried in [`Observation::ext`]. Units are SI throughout
//! (metres, metres per second, degrees true).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::sidc::{Sidc, SidcStandard};

/// A named identifier a source reports for an object, e.g. `mmsi:338924210`,
/// `icao:ae1234` or `cot-uid:ANDROID-1234`. Identifier matches always pair.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Identifier {
    pub scheme: String,
    pub value: String,
}

impl Identifier {
    pub fn new(scheme: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            scheme: scheme.into(),
            value: value.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Position {
    pub latitude: f64,
    pub longitude: f64,
    /// Height above the WGS84 ellipsoid, metres.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub altitude_hae_m: Option<f64>,
}

/// A security label, in the shape OpenStare's ES index ICD reserves for its
/// `stare-security` component template: a nested `security` object with
/// `classification`, `restrictions` and `sharing` (a releasability
/// restriction, not the catalog's sharing tier). The vocabularies are not
/// defined yet, so the values are free text as an admin enters them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecurityLabel {
    pub classification: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub restrictions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sharing: Option<String>,
}

impl SecurityLabel {
    pub fn validate(&self) -> Result<(), String> {
        let ok = |s: &str| !s.trim().is_empty() && s.chars().count() <= 256;
        if !ok(&self.classification) {
            return Err("security.classification is 1 to 256 characters".into());
        }
        if !self.restrictions.iter().all(|r| ok(r)) || self.restrictions.len() > 32 {
            return Err("security.restrictions are up to 32 values of 1 to 256 characters".into());
        }
        if self.sharing.as_deref().is_some_and(|s| !ok(s)) {
            return Err("security.sharing is 1 to 256 characters".into());
        }
        Ok(())
    }
    /// The label of a track fused from several sources' reports (given
    /// highest priority first), so it is never marked below what went into
    /// it:
    ///
    /// - `classification`: the highest of theirs in `order` (lowest
    ///   first; see [`classification_rank`]). One not in the order ranks
    ///   above every known one (fail safe); among equals, the first.
    /// - `restrictions`: every one of theirs (case-insensitive, first
    ///   spelling kept).
    /// - `sharing`: a releasability list (comma-separated, e.g.
    ///   `USA, GBR, CAN`): only what every labelled source releases to, the
    ///   intersection; `NONE` when that is empty. A source without one
    ///   restricts nothing.
    ///
    /// `None` when no source has a label.
    pub fn combine<'a>(
        labels: impl IntoIterator<Item = &'a SecurityLabel>,
        order: &[String],
    ) -> Option<SecurityLabel> {
        let labels: Vec<&SecurityLabel> = labels.into_iter().collect();
        let rank =
            |l: &SecurityLabel| classification_rank(&l.classification, order).unwrap_or(usize::MAX);
        let mut top = *labels.first()?;
        for l in &labels[1..] {
            if rank(l) > rank(top) {
                top = l;
            }
        }
        let mut restrictions: Vec<String> = Vec::new();
        for r in labels.iter().flat_map(|l| &l.restrictions) {
            let r = r.trim();
            if !restrictions.iter().any(|x| x.eq_ignore_ascii_case(r)) {
                restrictions.push(r.to_owned());
            }
        }
        let mut sharing: Option<Vec<String>> = None;
        for list in labels.iter().filter_map(|l| l.sharing.as_deref()) {
            let items: Vec<String> = list
                .split(',')
                .map(str::trim)
                .filter(|x| !x.is_empty())
                .map(str::to_owned)
                .collect();
            sharing = Some(match sharing {
                None => items,
                Some(have) => have
                    .into_iter()
                    .filter(|x| items.iter().any(|y| y.eq_ignore_ascii_case(x)))
                    .collect(),
            });
        }
        Some(SecurityLabel {
            classification: top.classification.clone(),
            restrictions,
            sharing: sharing.map(|v| {
                if v.is_empty() {
                    "NONE".to_owned()
                } else {
                    v.join(", ")
                }
            }),
        })
    }
}

impl SecurityLabel {
    /// The label's portion marking, the one text every output shows for it
    /// (TAK remarks, exports, the UI), in common US marking syntax:
    /// `(S//NOFORN)`, `(U//FOUO)`, `(S//ORCON/REL TO USA, GBR)`.
    ///
    /// - The classification is abbreviated (U, C, S, TS); one without an
    ///   abbreviation (CUI, a foreign or unknown one) is upper-cased as it is.
    /// - After `//` come the controls, separated by `/`: any `//` parts of the
    ///   classification text, then the restrictions in their order (upper
    ///   case, each once), then the releasability.
    /// - Releasability is `REL TO` with USA first, then the other country
    ///   codes, then organisation codes (four letters or more), each in
    ///   alphabetical order. `NONE` (sources that release to nothing in
    ///   common) is `NOFORN`; a `NOFORN` among the restrictions drops the
    ///   `REL TO` list, as it is the stricter of the two.
    pub fn marking(&self) -> String {
        let mut parts = self.classification.split("//");
        let head = normalize_classification(parts.next().unwrap_or(""));
        let class = match head.as_str() {
            "UNCLASSIFIED" => "U".to_owned(),
            "CONFIDENTIAL" => "C".to_owned(),
            "SECRET" => "S".to_owned(),
            "TOP SECRET" => "TS".to_owned(),
            _ => head,
        };
        let mut controls: Vec<String> = Vec::new();
        let mut rel: Option<Vec<String>> = None;
        let add_rel = |list: &str, rel: &mut Option<Vec<String>>| {
            let items: Vec<String> = list
                .split(',')
                .map(|x| x.split_whitespace().collect::<Vec<_>>().join(" "))
                .map(|x| x.to_uppercase())
                .filter(|x| !x.is_empty())
                .collect();
            *rel = Some(match rel.take() {
                None => items,
                Some(have) => have.into_iter().filter(|x| items.contains(x)).collect(),
            });
        };
        let tail: Vec<&str> = parts.flat_map(|p| p.split('/')).collect();
        for c in tail
            .into_iter()
            .chain(self.restrictions.iter().map(String::as_str))
        {
            let c = c
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_uppercase();
            if c.is_empty() {
                continue;
            }
            if let Some(list) = c.strip_prefix("REL TO ") {
                add_rel(list, &mut rel);
            } else if !controls.contains(&c) {
                controls.push(c);
            }
        }
        if let Some(s) = &self.sharing {
            let s = s.trim();
            let upper = s.to_uppercase();
            add_rel(upper.strip_prefix("REL TO ").unwrap_or(s), &mut rel);
        }
        let nofo = |c: &Vec<String>| c.iter().any(|x| x == "NOFORN" || x == "NF");
        if let Some(mut list) = rel {
            if list.is_empty() || list.iter().any(|x| x == "NONE") {
                if !nofo(&controls) {
                    controls.push("NOFORN".into());
                }
            } else if !nofo(&controls) {
                list.sort_by_key(|x| (x != "USA", x.chars().count() > 3, x.clone()));
                list.dedup();
                controls.push(format!("REL TO {}", list.join(", ")));
            }
        }
        if controls.is_empty() {
            format!("({class})")
        } else {
            format!("({class}//{})", controls.join("/"))
        }
    }
}

/// The classification order a fused track's label is chosen by, lowest
/// first (settings can replace it).
pub const DEFAULT_CLASSIFICATION_ORDER: [&str; 4] =
    ["UNCLASSIFIED", "CUI", "CONFIDENTIAL", "SECRET"];

/// A classification as compared: the part before any `//` caveats, upper
/// case, `_` and `-` as spaces, spaces collapsed, and the usual
/// abbreviations (U, C, S, TS) spelt out.
pub fn normalize_classification(c: &str) -> String {
    let head = c.split("//").next().unwrap_or("");
    let words: Vec<String> = head
        .replace(['_', '-'], " ")
        .split_whitespace()
        .map(str::to_uppercase)
        .collect();
    let joined = words.join(" ");
    match joined.as_str() {
        "U" => "UNCLASSIFIED".into(),
        "C" => "CONFIDENTIAL".into(),
        "S" => "SECRET".into(),
        "TS" => "TOP SECRET".into(),
        _ => joined,
    }
}

/// Where a classification stands in `order` (lowest first); `None` when
/// it is not there.
pub fn classification_rank(c: &str, order: &[String]) -> Option<usize> {
    let c = normalize_classification(c);
    order.iter().position(|o| normalize_classification(o) == c)
}

/// A GOLD XPOS-style uncertainty ellipse: semi-axes are one standard deviation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Ellipse {
    pub semi_major_m: f64,
    pub semi_minor_m: f64,
    /// Orientation of the major axis, degrees clockwise from true north.
    pub orientation_deg: f64,
}

impl Ellipse {
    /// Circular error probable approximated from the ellipse, for consumers
    /// that only handle a circular error.
    pub fn cep_m(&self) -> f64 {
        0.59 * (self.semi_major_m + self.semi_minor_m)
    }

    /// The position covariance it describes, `[nn, ne, ee]` (m², north/east).
    pub fn covariance(&self) -> [f64; 3] {
        let (a2, b2) = (self.semi_major_m.powi(2), self.semi_minor_m.powi(2));
        let (s, c) = self.orientation_deg.to_radians().sin_cos();
        [
            a2 * c * c + b2 * s * s,
            (a2 - b2) * s * c,
            a2 * s * s + b2 * c * c,
        ]
    }

    /// The ellipse of a position covariance `[nn, ne, ee]` (m²).
    pub fn from_covariance([nn, ne, ee]: [f64; 3]) -> Self {
        let mid = (nn + ee) / 2.0;
        let r = (((nn - ee) / 2.0).powi(2) + ne * ne).sqrt();
        let orientation = (0.5 * (2.0 * ne).atan2(nn - ee))
            .to_degrees()
            .rem_euclid(180.0);
        Self {
            semi_major_m: (mid + r).max(0.0).sqrt(),
            semi_minor_m: (mid - r).max(0.0).sqrt(),
            orientation_deg: orientation,
        }
    }
}

/// A report's full error covariance, in metres and m/s along north and east
/// at its position: what a tracker knows about its estimate, so a consumer
/// can propagate it in time rather than guess.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Covariance {
    /// Position `[nn, ne, ee]` (m²).
    pub position: [f64; 3],
    /// Velocity `[vn·vn, vn·ve, ve·ve]` (m²/s²).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub velocity: Option<[f64; 3]>,
    /// Position against velocity `[n·vn, n·ve, e·vn, e·ve]` (m²/s).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cross: Option<[f64; 4]>,
}

impl Covariance {
    fn check(&self) -> bool {
        let psd =
            |[a, b, c]: [f64; 3]| a >= 0.0 && c >= 0.0 && a * c - b * b >= -1e-6 * (a * c).max(1.0);
        let finite = self
            .position
            .iter()
            .chain(self.velocity.iter().flatten())
            .chain(self.cross.iter().flatten())
            .all(|x| x.is_finite());
        finite && psd(self.position) && self.velocity.is_none_or(psd)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Uncertainty {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ellipse: Option<Ellipse>,
    /// Used when a source reports only a circular error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub circular_error_m: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vertical_error_m: Option<f64>,
    /// The full covariance, when the source has one (a tracker); the ellipse
    /// and circular error are derived from it for consumers that need them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub covariance: Option<Covariance>,
}

impl Uncertainty {
    /// The position covariance `[nn, ne, ee]` (m²): the full covariance, else
    /// the ellipse, else a circle from the circular error (CEP = 1.1774 σ).
    pub fn position_covariance(&self) -> Option<[f64; 3]> {
        if let Some(c) = &self.covariance {
            return Some(c.position);
        }
        if let Some(e) = &self.ellipse {
            return Some(e.covariance());
        }
        self.circular_error_m.map(|c| {
            let v = (c / 1.1774).powi(2);
            [v, 0.0, v]
        })
    }

    /// The circular error to publish: the reported one, else derived from the
    /// ellipse.
    pub fn cep_m(&self) -> Option<f64> {
        self.circular_error_m
            .or_else(|| self.ellipse.map(|e| e.cep_m()))
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Kinematics {
    /// Course over ground, degrees true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub course_deg: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed_mps: Option<f64>,
    /// Where the platform points, which can differ from its course.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading_deg: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vertical_rate_mps: Option<f64>,
}

/// Operating domain, matching the CoT / MIL-STD-2525 battle dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Domain {
    Air,
    Surface,
    Subsurface,
    Ground,
    Space,
}

impl Domain {
    /// The CoT battle-dimension atom (3rd atom of an `a-` type).
    pub fn cot_atom(self) -> char {
        match self {
            Domain::Air => 'A',
            Domain::Surface => 'S',
            Domain::Subsurface => 'U',
            Domain::Ground => 'G',
            Domain::Space => 'P',
        }
    }

    pub fn from_cot_atom(atom: &str) -> Option<Self> {
        Some(match atom {
            "A" => Domain::Air,
            "S" => Domain::Surface,
            "U" => Domain::Subsurface,
            "G" => Domain::Ground,
            "P" => Domain::Space,
            _ => return None,
        })
    }
}

/// Standard identity, matching the CoT affiliation atom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Affiliation {
    Pending,
    Unknown,
    AssumedFriend,
    Friend,
    Neutral,
    Suspect,
    Hostile,
    Joker,
    Faker,
    None,
}

impl Affiliation {
    /// The CoT affiliation atom (2nd atom of an `a-` type).
    pub fn cot_atom(self) -> char {
        match self {
            Affiliation::Pending => 'p',
            Affiliation::Unknown => 'u',
            Affiliation::AssumedFriend => 'a',
            Affiliation::Friend => 'f',
            Affiliation::Neutral => 'n',
            Affiliation::Suspect => 's',
            Affiliation::Hostile => 'h',
            Affiliation::Joker => 'j',
            Affiliation::Faker => 'k',
            Affiliation::None => 'o',
        }
    }

    pub fn from_cot_atom(atom: &str) -> Option<Self> {
        Some(match atom {
            "p" => Affiliation::Pending,
            "u" => Affiliation::Unknown,
            "a" => Affiliation::AssumedFriend,
            "f" => Affiliation::Friend,
            "n" => Affiliation::Neutral,
            "s" => Affiliation::Suspect,
            "h" => Affiliation::Hostile,
            "j" => Affiliation::Joker,
            "k" => Affiliation::Faker,
            "o" => Affiliation::None,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Classification {
    /// CoT type, e.g. `a-f-S-C-L-D-D` (MIL-STD-2525 based).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cot_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<Domain>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub affiliation: Option<Affiliation>,
    /// Symbol identification code as a feed reports it: MIL-STD-2525C,
    /// MIL-STD-2525D or a CoT type (see [`crate::sidc`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidc: Option<String>,
}

impl Classification {
    fn parsed_sidc(&self) -> Option<Sidc> {
        Sidc::parse(self.sidc.as_deref()?)
    }

    /// Domain, falling back to the CoT type's battle dimension, then the SIDC.
    pub fn effective_domain(&self) -> Option<Domain> {
        self.domain
            .or_else(|| {
                cot_atoms(self.cot_type.as_deref()?)
                    .2
                    .and_then(Domain::from_cot_atom)
            })
            .or_else(|| self.parsed_sidc()?.domain())
    }

    /// Affiliation, falling back to the CoT type's affiliation atom, then the
    /// SIDC's standard identity.
    pub fn effective_affiliation(&self) -> Option<Affiliation> {
        self.affiliation
            .or_else(|| {
                cot_atoms(self.cot_type.as_deref()?)
                    .1
                    .and_then(Affiliation::from_cot_atom)
            })
            .or_else(|| self.parsed_sidc()?.affiliation())
    }

    /// The SIDC to publish: the feed's (2525C, 2525D or CoT) with its
    /// standard identity rewritten when an affiliation is set explicitly,
    /// else the CoT type OpenTrack derives. Always present.
    pub fn sidc_or_derived(&self) -> Sidc {
        match self.parsed_sidc() {
            Some(s) => match self.affiliation {
                Some(a) => s.with_affiliation(a),
                None => s,
            },
            None => Sidc {
                standard: SidcStandard::Cot,
                code: self.cot_type_or_derived(),
            },
        }
    }

    /// The CoT type to publish. An explicit type wins, with its affiliation
    /// atom replaced when an affiliation is set explicitly (affiliation policy
    /// outranks a feed's default `u`); otherwise one is built from domain and
    /// affiliation (`a-<affiliation>-<domain>`).
    pub fn cot_type_or_derived(&self) -> String {
        if let Some(t) = self.cot_type.as_deref().filter(|t| !t.is_empty()) {
            let mut parts: Vec<&str> = t.split('-').collect();
            return match self.affiliation {
                Some(aff) if parts.len() >= 2 && parts[0] == "a" => {
                    let atom = aff.cot_atom().to_string();
                    parts[1] = &atom;
                    parts.join("-")
                }
                _ => t.to_owned(),
            };
        }
        let aff = self.effective_affiliation().unwrap_or(Affiliation::Unknown);
        match self.effective_domain() {
            Some(d) => format!("a-{}-{}", aff.cot_atom(), d.cot_atom()),
            None => format!("a-{}", aff.cot_atom()),
        }
    }
}

/// First three atoms of a CoT type, when it is an `a-` (atom) type.
fn cot_atoms(cot_type: &str) -> (Option<&str>, Option<&str>, Option<&str>) {
    let mut parts = cot_type.split('-');
    let first = parts.next().filter(|p| *p == "a");
    if first.is_none() {
        return (None, None, None);
    }
    (first, parts.next(), parts.next())
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Platform {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hull: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Provenance {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sensor_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_code: Option<String>,
    /// The source's own confidence in this report, 0 to 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    /// The tracker (and its version) that formed this report from
    /// detections, e.g. `mht-1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracker: Option<String>,
}

/// OTH-GOLD track type (CTC field 13): whether the track is real-world
/// tactical data or training/simulation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackType {
    /// Real-world track (GOLD's default, a null entry).
    #[default]
    Tactical,
    /// Real-world friendly track designated as a training track (GOLD 2).
    LiveTraining,
    /// Manually created or generated as part of a training scenario (GOLD 3).
    SimulatedTraining,
    /// Demand entry: an actual unit that receivers should not filter (GOLD 4).
    DemandEntry,
}

/// Track lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackState {
    Tentative,
    Confirmed,
    Lost,
    Dropped,
}

/// One report about one object from one source, in the authoritative schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Observation {
    /// Extension schema version the source's mapping targets.
    pub schema_version: u32,
    pub source_id: String,
    /// The source's own key for the object (its track number, MMSI, CoT uid...).
    pub source_track_key: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub identifiers: Vec<Identifier>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callsign: Option<String>,
    pub observed_at: DateTime<Utc>,
    pub received_at: DateTime<Utc>,
    pub position: Position,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uncertainty: Option<Uncertainty>,
    #[serde(default)]
    pub kinematics: Kinematics,
    #[serde(default)]
    pub classification: Classification,
    #[serde(default)]
    pub platform: Platform,
    #[serde(default)]
    pub provenance: Provenance,
    /// The security label of the source that reported it, when the source
    /// has one (OpenStare's reserved `stare-security` shape).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security: Option<SecurityLabel>,
    /// State reported by the source, if it reports one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<TrackState>,
    /// OTH-GOLD track type; tactical when the source does not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_type: Option<TrackType>,
    /// Admin-defined extension fields, published under `attributes_json.ext`.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub ext: serde_json::Map<String, serde_json::Value>,
    /// Not a point: a line of bearing from `position`, or an area around it
    /// (see [`crate::geometry`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geometry: Option<crate::geometry::Geometry>,
    /// Another OpenTrack node's report of its track: when the node that
    /// minted the track's UID made it (which of two numbers for one object
    /// survives).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ValidationError {
    #[error("{0} is required")]
    Missing(&'static str),
    #[error("{field} = {value} is outside {range}")]
    OutOfRange {
        field: &'static str,
        value: f64,
        range: &'static str,
    },
    #[error("{0} is not a finite number")]
    NotFinite(&'static str),
    #[error("ellipse semi-minor axis exceeds semi-major axis")]
    EllipseAxes,
    #[error("uncertainty covariance is not a finite, positive semi-definite matrix")]
    Covariance,
    #[error("geometry: {0}")]
    Geometry(String),
}

impl Observation {
    /// Check the core invariants the rest of the system relies on. Mapping
    /// rejects an observation that fails, and counts it per source.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.source_id.trim().is_empty() {
            return Err(ValidationError::Missing("source_id"));
        }
        if self.source_track_key.trim().is_empty() {
            return Err(ValidationError::Missing("source_track_key"));
        }
        range("latitude", self.position.latitude, -90.0, 90.0, "[-90, 90]")?;
        range(
            "longitude",
            self.position.longitude,
            -180.0,
            180.0,
            "[-180, 180]",
        )?;
        finite_opt("altitude_hae_m", self.position.altitude_hae_m)?;

        if let Some(g) = &self.geometry {
            g.validate().map_err(ValidationError::Geometry)?;
        }
        let k = &self.kinematics;
        opt_range("course_deg", k.course_deg, 0.0, 360.0, "[0, 360]")?;
        opt_range("heading_deg", k.heading_deg, 0.0, 360.0, "[0, 360]")?;
        opt_range("speed_mps", k.speed_mps, 0.0, f64::MAX, "[0, inf)")?;
        finite_opt("vertical_rate_mps", k.vertical_rate_mps)?;

        if let Some(u) = &self.uncertainty {
            opt_range(
                "circular_error_m",
                u.circular_error_m,
                0.0,
                f64::MAX,
                "[0, inf)",
            )?;
            opt_range(
                "vertical_error_m",
                u.vertical_error_m,
                0.0,
                f64::MAX,
                "[0, inf)",
            )?;
            if let Some(e) = &u.ellipse {
                range("semi_major_m", e.semi_major_m, 0.0, f64::MAX, "[0, inf)")?;
                range("semi_minor_m", e.semi_minor_m, 0.0, f64::MAX, "[0, inf)")?;
                range("orientation_deg", e.orientation_deg, 0.0, 360.0, "[0, 360]")?;
                if e.semi_minor_m > e.semi_major_m {
                    return Err(ValidationError::EllipseAxes);
                }
            }
            if let Some(c) = &u.covariance
                && !c.check()
            {
                return Err(ValidationError::Covariance);
            }
        }
        opt_range("confidence", self.provenance.confidence, 0.0, 1.0, "[0, 1]")?;
        Ok(())
    }
}

fn range(
    field: &'static str,
    value: f64,
    lo: f64,
    hi: f64,
    label: &'static str,
) -> Result<(), ValidationError> {
    if !value.is_finite() {
        return Err(ValidationError::NotFinite(field));
    }
    if value < lo || value > hi {
        return Err(ValidationError::OutOfRange {
            field,
            value,
            range: label,
        });
    }
    Ok(())
}

fn opt_range(
    field: &'static str,
    value: Option<f64>,
    lo: f64,
    hi: f64,
    label: &'static str,
) -> Result<(), ValidationError> {
    value.map_or(Ok(()), |v| range(field, v, lo, hi, label))
}

fn finite_opt(field: &'static str, value: Option<f64>) -> Result<(), ValidationError> {
    match value {
        Some(v) if !v.is_finite() => Err(ValidationError::NotFinite(field)),
        _ => Ok(()),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use chrono::TimeZone;

    pub(crate) fn sample() -> Observation {
        let t = Utc.with_ymd_and_hms(2026, 9, 24, 12, 0, 0).unwrap();
        Observation {
            schema_version: 1,
            source_id: "synthetic".into(),
            source_track_key: "T00001".into(),
            identifiers: vec![Identifier::new("mmsi", "338924210")],
            name: Some("TED STEVENS".into()),
            callsign: None,
            observed_at: t,
            received_at: t,
            position: Position {
                latitude: 32.68,
                longitude: -117.23,
                altitude_hae_m: None,
            },
            uncertainty: Some(Uncertainty {
                ellipse: Some(Ellipse {
                    semi_major_m: 100.0,
                    semi_minor_m: 50.0,
                    orientation_deg: 45.0,
                }),
                ..Default::default()
            }),
            kinematics: Kinematics {
                course_deg: Some(270.0),
                speed_mps: Some(5.0),
                heading_deg: Some(268.0),
                vertical_rate_mps: None,
            },
            classification: Classification {
                cot_type: None,
                domain: Some(Domain::Surface),
                affiliation: Some(Affiliation::Friend),
                sidc: None,
            },
            platform: Platform::default(),
            provenance: Provenance {
                confidence: Some(0.9),
                ..Default::default()
            },
            security: None,
            state: None,
            track_type: None,
            ext: Default::default(),
            origin: None,
            geometry: None,
        }
    }

    #[test]
    fn sample_is_valid_and_round_trips() {
        let obs = sample();
        obs.validate().unwrap();
        let json = serde_json::to_string(&obs).unwrap();
        assert_eq!(serde_json::from_str::<Observation>(&json).unwrap(), obs);
    }

    #[test]
    fn rejects_out_of_range_and_non_finite() {
        let mut obs = sample();
        obs.position.latitude = 91.0;
        assert!(matches!(
            obs.validate(),
            Err(ValidationError::OutOfRange {
                field: "latitude",
                ..
            })
        ));

        let mut obs = sample();
        obs.kinematics.speed_mps = Some(f64::NAN);
        assert_eq!(obs.validate(), Err(ValidationError::NotFinite("speed_mps")));

        let mut obs = sample();
        obs.source_track_key = " ".into();
        assert_eq!(
            obs.validate(),
            Err(ValidationError::Missing("source_track_key"))
        );

        let mut obs = sample();
        obs.uncertainty
            .as_mut()
            .unwrap()
            .ellipse
            .as_mut()
            .unwrap()
            .semi_minor_m = 200.0;
        assert_eq!(obs.validate(), Err(ValidationError::EllipseAxes));
    }

    #[test]
    fn cep_prefers_reported_circular_error() {
        let e = Ellipse {
            semi_major_m: 100.0,
            semi_minor_m: 50.0,
            orientation_deg: 0.0,
        };
        assert!((e.cep_m() - 88.5).abs() < 1e-9);
        let u = Uncertainty {
            ellipse: Some(e),
            circular_error_m: Some(10.0),
            ..Default::default()
        };
        assert_eq!(u.cep_m(), Some(10.0));
    }

    #[test]
    fn cot_type_derivation_and_fallbacks() {
        let c = Classification {
            domain: Some(Domain::Surface),
            affiliation: Some(Affiliation::Hostile),
            ..Default::default()
        };
        assert_eq!(c.cot_type_or_derived(), "a-h-S");

        let c = Classification {
            cot_type: Some("a-f-A-M-F".into()),
            ..Default::default()
        };
        assert_eq!(c.effective_domain(), Some(Domain::Air));
        assert_eq!(c.effective_affiliation(), Some(Affiliation::Friend));
        assert_eq!(c.cot_type_or_derived(), "a-f-A-M-F");

        let c = Classification {
            cot_type: Some("a-u-S-C".into()),
            affiliation: Some(Affiliation::Hostile),
            ..Default::default()
        };
        assert_eq!(c.cot_type_or_derived(), "a-h-S-C");

        let c = Classification {
            cot_type: Some("b-m-p-s-p-i".into()),
            ..Default::default()
        };
        assert_eq!(c.effective_domain(), None);
        assert_eq!(Classification::default().cot_type_or_derived(), "a-u");
    }

    #[test]
    fn ellipse_and_covariance_round_trip() {
        for (a, b, o) in [
            (30.0, 10.0, 0.0),
            (30.0, 10.0, 90.0),
            (50.0, 5.0, 37.0),
            (8.0, 8.0, 0.0),
        ] {
            let e = Ellipse {
                semi_major_m: a,
                semi_minor_m: b,
                orientation_deg: o,
            };
            let c = e.covariance();
            let back = Ellipse::from_covariance(c);
            assert!((back.semi_major_m - a).abs() < 1e-9 && (back.semi_minor_m - b).abs() < 1e-9);
            if a != b {
                assert!((back.orientation_deg - o).abs() < 1e-9, "{o} -> {back:?}");
            }
        }
        // Major axis east-west: all the variance on east.
        let c = Ellipse {
            semi_major_m: 3.0,
            semi_minor_m: 1.0,
            orientation_deg: 90.0,
        }
        .covariance();
        assert!((c[0] - 1.0).abs() < 1e-9 && c[1].abs() < 1e-9 && (c[2] - 9.0).abs() < 1e-9);
        let u = Uncertainty {
            circular_error_m: Some(11.774),
            ..Default::default()
        };
        let p = u.position_covariance().unwrap();
        assert!((p[0] - 100.0).abs() < 1e-6 && (p[2] - 100.0).abs() < 1e-6);
        let bad = Covariance {
            position: [1.0, 5.0, 1.0],
            velocity: None,
            cross: None,
        };
        assert!(!bad.check());
    }

    fn order() -> Vec<String> {
        DEFAULT_CLASSIFICATION_ORDER.map(String::from).to_vec()
    }

    fn label(c: &str, r: &[&str], sharing: Option<&str>) -> SecurityLabel {
        SecurityLabel {
            classification: c.into(),
            restrictions: r.iter().map(|x| x.to_string()).collect(),
            sharing: sharing.map(str::to_owned),
        }
    }

    #[test]
    fn classifications_rank_by_the_order_with_abbreviations() {
        let o = order();
        let r = |c: &str| classification_rank(c, &o);
        assert_eq!(r("unclassified"), Some(0));
        assert_eq!(r("U"), Some(0));
        assert_eq!(r("cui"), Some(1));
        assert_eq!(r("C"), Some(2));
        assert_eq!(r("s"), Some(3));
        assert_eq!(r("Secret//NOFORN"), Some(3));
        // Not in the default order: ranks above every classification in it.
        assert_eq!(r("TS"), None);
        assert_eq!(r("top_secret"), None);
        assert_eq!(r("COSMIC TOP SECRET"), None);
        let with_ts: Vec<String> = ["U", "S", "TS"].map(String::from).to_vec();
        assert_eq!(classification_rank("top_secret", &with_ts), Some(2));
    }

    #[test]
    fn a_fused_label_is_the_highest_with_every_restriction() {
        let o = order();
        let low = label("UNCLASSIFIED", &["FOUO"], Some("USA, GBR, CAN, AUS"));
        let high = label("S", &["NOFORN", "fouo"], Some("usa, gbr"));
        let mid = label("CONFIDENTIAL", &[], None);
        // Highest priority first: the low one leads, the high one wins.
        let l = SecurityLabel::combine([&low, &mid, &high], &o).unwrap();
        assert_eq!(l.classification, "S");
        assert_eq!(l.restrictions, ["FOUO", "NOFORN"]);
        assert_eq!(l.sharing.as_deref(), Some("USA, GBR"));
        // Nothing in common: releasable to none.
        let other = label("SECRET", &[], Some("FRA"));
        let l = SecurityLabel::combine([&high, &other], &o).unwrap();
        assert_eq!(l.sharing.as_deref(), Some("NONE"));
        assert_eq!(l.classification, "S", "equals: the first");
        // An unknown classification outranks every known one.
        let odd = label("SPECIAL HANDLING", &[], None);
        let top = label("SECRET", &[], None);
        let l = SecurityLabel::combine([&top, &odd], &o).unwrap();
        assert_eq!(l.classification, "SPECIAL HANDLING");
        assert!(SecurityLabel::combine([], &o).is_none());
        assert!(l.validate().is_ok());
    }

    #[test]
    fn markings_follow_us_syntax() {
        let m = |c: &str, r: &[&str], s: Option<&str>| label(c, r, s).marking();
        assert_eq!(m("UNCLASSIFIED", &[], None), "(U)");
        assert_eq!(m("unclassified", &["fouo"], None), "(U//FOUO)");
        assert_eq!(m("Secret", &[], Some("GBR, usa")), "(S//REL TO USA, GBR)");
        assert_eq!(m("confidential", &[], None), "(C)");
        assert_eq!(m("top_secret", &[], None), "(TS)");
        assert_eq!(m("CUI", &[], None), "(CUI)");
        assert_eq!(m("NATO SECRET", &[], None), "(NATO SECRET)");
        // Restrictions in order, once each, then the releasability last:
        // USA, the other countries, then organisations.
        assert_eq!(
            m(
                "S",
                &["ORCON", "propin", "orcon"],
                Some("NATO, FVEY, CAN, usa, AUS, GBR")
            ),
            "(S//ORCON/PROPIN/REL TO USA, AUS, CAN, GBR, FVEY, NATO)"
        );
        // Written out already, or in the classification text.
        assert_eq!(m("S", &[], Some("REL TO USA, GBR")), "(S//REL TO USA, GBR)");
        assert_eq!(m("SECRET//NOFORN", &[], None), "(S//NOFORN)");
        assert_eq!(
            m("SECRET//REL TO USA, GBR, CAN", &[], Some("USA, CAN")),
            "(S//REL TO USA, CAN)"
        );
        // Released to nothing in common, or NOFORN as well: NOFORN.
        assert_eq!(m("S", &[], Some("NONE")), "(S//NOFORN)");
        assert_eq!(m("S", &["NOFORN"], Some("USA, GBR")), "(S//NOFORN)");
    }

    #[test]
    fn a_fused_marking_is_the_highest() {
        let o = order();
        let low = label("U", &["FOUO"], Some("USA, GBR, CAN"));
        let high = label("SECRET", &[], Some("GBR, USA"));
        let l = SecurityLabel::combine([&low, &high], &o).unwrap();
        assert_eq!(l.marking(), "(S//FOUO/REL TO USA, GBR)");
        // Nothing released in common; an unknown classification wins.
        let odd = label("Special Handling", &[], Some("FRA"));
        let l = SecurityLabel::combine([&high, &odd], &o).unwrap();
        assert_eq!(l.marking(), "(SPECIAL HANDLING//NOFORN)");
    }
}
