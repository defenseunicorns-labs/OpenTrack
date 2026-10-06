//! System tracks: the correlated picture OpenTrack publishes.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::schema::{Observation, TrackState};
use crate::uid::Uid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairingType {
    /// Paired by the correlation engine.
    Auto,
    /// Paired by an operator.
    Manual,
}

/// A source track reporting for a system track (a `REPORTS_FOR` edge).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Contributor {
    pub source_id: String,
    pub source_track_key: String,
    pub pairing: PairingType,
    /// Probability that it reports the same object as the rest of the track
    /// (1 for the source track that started it, an operator's pairing or a
    /// shared identity until the kinematics say otherwise).
    pub confidence: f64,
    pub last_report: DateTime<Utc>,
    /// The source's own probability that the object exists: its latest
    /// report's `provenance.confidence` (a tracker's existence probability).
    /// Absent: the source does not say, and a track it reports is taken as
    /// real.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub existence: Option<f64>,
}

/// A track field where the entity's value replaced a different one the
/// feed reported (an entity → track link). The entity wins; the difference
/// is surfaced to operators.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttributeNotice {
    /// Track field, e.g. `classification.cot_type` or `ext.destination`.
    pub key: String,
    /// The entity's value (published).
    #[serde(alias = "card")]
    pub entity: serde_json::Value,
    /// What the feed reported (not published).
    pub feed: serde_json::Value,
    pub source_id: String,
}

/// A line of bearing associated with a track.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BearingContact {
    pub source_id: String,
    pub source_track_key: String,
    pub observed_at: DateTime<Utc>,
    /// Where the sensor was: the line's start.
    pub latitude: f64,
    pub longitude: f64,
    pub bearing_deg: f64,
    pub sigma_deg: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range_m: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range_sigma_m: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_range_m: Option<f64>,
    /// Measured bearing minus the bearing to the track, degrees.
    pub residual_deg: f64,
    /// The emitter's identity, as the sensor reported it (an ELNOT…).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub identifiers: Vec<crate::schema::Identifier>,
}

/// What a system track stands for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackKind {
    /// One object, reported by sources.
    #[default]
    Track,
    /// A group of tracks a track manager formed (a battle group, a flight, a
    /// convoy): its position is its live members' centre.
    Group,
}

impl TrackKind {
    pub fn is_track(&self) -> bool {
        *self == TrackKind::Track
    }
}

/// Current state of one system track, as held in `tms:sys:<uid>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SystemTrack {
    pub uid: Uid,
    #[serde(default, skip_serializing_if = "TrackKind::is_track")]
    pub kind: TrackKind,
    pub state: TrackState,
    /// The best-source view of the object. Until best-source selection
    /// arrives with correlation (phase 3) this is the latest observation.
    pub view: Observation,
    /// Which contributor supplied each field group, keyed by group name.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub provenance: std::collections::BTreeMap<String, String>,
    pub contributors: Vec<Contributor>,
    /// UIDs merged into this track, still resolvable as aliases.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<Uid>,
    /// Groups this track is a member of, by their track id (`tms-<UID>`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<String>,
    /// Tracks an operator paired with this one (GOLD PAIR): the same object,
    /// kept as separate tracks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paired_with: Vec<Uid>,
    /// A group's member tracks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<Uid>,
    /// Registry entity this track resolves to, when the
    /// registry match is corroborated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_id: Option<String>,
    /// Output schema values, resolved from the track's values (feeds and
    /// entity links) and built-ins;
    /// published as `attributes`.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub attributes: serde_json::Map<String, serde_json::Value>,
    /// Track fields where the entity replaced what a feed reports.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notices: Vec<AttributeNotice>,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    pub observation_count: u64,
    /// Whether it has been published: a track is kept inside OpenTrack until
    /// it is confirmed and authoritative (see the engine's publish rule).
    /// Absent on tracks stored before the rule, which were all published.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published: Option<bool>,
    /// Why the output filter holds it back from publishing, when it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filtered: Option<String>,
    /// Lines of bearing that point at this track, the latest from each
    /// sensor's source track (see `docs/non-point-contacts.md`). They add
    /// evidence and identity; they never move the track.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bearings: Vec<BearingContact>,
    /// With other OpenTrack nodes sharing the picture: the site code of the
    /// node that reports this track to the others (this node's own when it
    /// does), if any does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported_by: Option<String>,
}

impl SystemTrack {
    /// Whether consumers have seen this track (tracks from before the
    /// publish rule count as published).
    pub fn is_published(&self) -> bool {
        self.published != Some(false)
    }

    /// Probability that the track is a real object: that at least one of its
    /// source tracks is a real report of it, each independently with its
    /// source's existence times its pairing confidence,
    /// `1 − Π(1 − existence · confidence)`. Plots associated from a detection
    /// source (`~` keys) are not counted: they only update a track others
    /// hold up.
    pub fn confidence(&self) -> f64 {
        let none = self
            .contributors
            .iter()
            .filter(|c| !c.source_track_key.starts_with('~'))
            .map(|c| {
                1.0 - c.existence.unwrap_or(1.0).clamp(0.0, 1.0) * c.confidence.clamp(0.0, 1.0)
            })
            .product::<f64>();
        ((1.0 - none) * 1e4).round() / 1e4
    }

    /// A new tentative system track seeded from one observation.
    pub fn from_first_observation(uid: Uid, obs: Observation) -> Self {
        let contributor = Contributor {
            source_id: obs.source_id.clone(),
            source_track_key: obs.source_track_key.clone(),
            pairing: PairingType::Auto,
            confidence: 1.0,
            last_report: obs.observed_at,
            existence: obs.provenance.confidence,
        };
        Self {
            uid,
            kind: TrackKind::Track,
            state: TrackState::Tentative,
            first_seen: obs.observed_at,
            last_seen: obs.observed_at,
            observation_count: 1,
            view: obs,
            provenance: Default::default(),
            contributors: vec![contributor],
            aliases: Vec::new(),
            groups: Vec::new(),
            paired_with: Vec::new(),
            members: Vec::new(),
            entity_id: None,
            attributes: Default::default(),
            notices: Vec::new(),
            published: Some(false),
            filtered: None,
            reported_by: None,
            bearings: Vec::new(),
        }
    }
}
