//! The published track message: `opentrack.track.v2`.
//!
//! Every system track is published as JSON on its own subject,
//! `<prefix>.<track_id>` (by default `tracks.tms-OTK000000123`). A consumer
//! subscribes to `tracks.>`; the stream keeps the latest message per subject,
//! so a consumer that starts late still receives the whole current picture.
//!
//! The body is the OTH-GOLD mandatory minimum (see [`crate::gold`]), which is
//! fixed, plus `attributes`: exactly the fields of the admin-designed output
//! schema that have a value for this track, whether from a feed mapping, the
//! entity through a source's entity links, or an OpenTrack built-in. Nothing
//! else is published.
//!
//! Two operations, carried both in the `OT-Op` header and in the body's `op`:
//!
//! * `upsert`: the full current state of the track. Replaces anything the
//!   consumer holds for that `track_id`.
//! * `delete`: the track was retired (dropped, merged away or deleted by an
//!   operator). The consumer removes it. The delete stays in the stream as
//!   that subject's latest message until it ages out.
//!
//! Conventions: timestamps are RFC 3339 UTC, units are SI (metres, metres per
//! second, degrees true), enums are lower-case strings, and absent attributes
//! are omitted rather than sent as null.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::gold;
use crate::schema::TrackType;
use crate::sidc::Sidc;
use crate::track::SystemTrack;
use crate::uid::Uid;

/// Schema name, in the `OT-Schema` header and the body's `schema`.
pub const TRACK_SCHEMA: &str = "opentrack.track.v2";

/// Subject prefix and schema of non-point contacts no track took (lines of
/// bearing), published live on core NATS: `contacts.bearing.<source>.<key>`.
pub const CONTACTS_SUBJECT: &str = "contacts";
pub const CONTACT_SCHEMA: &str = "opentrack.contact.v1";

/// Default subject prefix for system tracks.
pub const TRACKS_SUBJECT: &str = "tracks";

/// Header names.
pub const HEADER_OP: &str = "OT-Op";
pub const HEADER_SCHEMA: &str = "OT-Schema";

/// Who is publishing, stamped into each message's `publisher`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishContext {
    /// Identifies this OpenTrack instance, e.g. `opentrack-OTK`.
    pub node_id: String,
    /// OpenTrack version.
    pub version: String,
    /// Version of the correlation engine that produced the track.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub correlation: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Upsert,
    Delete,
    /// A track manager deleted a point of the track's history.
    DeleteHistoryPoint,
}

impl Op {
    pub fn as_str(self) -> &'static str {
        match self {
            Op::Upsert => "upsert",
            Op::Delete => "delete",
            Op::DeleteHistoryPoint => "delete_history_point",
        }
    }
}

/// The subject a track is published on.
pub fn subject(prefix: &str, uid: Uid) -> String {
    format!("{prefix}.{}", uid.doc_id())
}

/// The subject a deleted history point is published on: one per point, so
/// the stream keeps every deletion (until it ages out), never in place of
/// the track's own latest message.
pub fn history_subject(prefix: &str, uid: Uid, t_ms: i64) -> String {
    format!("{prefix}.history.{}.{t_ms}", uid.doc_id())
}

/// A deleted history point: consumers drop the track's point at
/// `observed_at` from what they show of its history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryDeleteMessage {
    pub schema: String,
    pub op: Op,
    pub track_id: String,
    pub uid: Uid,
    /// The point's time, the key consumers match it by.
    pub observed_at: DateTime<Utc>,
    /// Where the track was said to be then.
    pub position: HistoryPosition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The decision that deleted it (OpenTrack's decision log).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_id: Option<i64>,
    pub deleted_at: DateTime<Utc>,
    pub publisher: PublishContext,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryPosition {
    pub latitude: f64,
    pub longitude: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackMessage {
    pub schema: String,
    pub op: Op,
    /// Track number / UID (GOLD CTC 1): `tms-<UID>`, the key consumers use.
    pub track_id: String,
    pub uid: Uid,
    /// Class (GOLD CTC 2), `UNEQUATED` when unknown.
    pub class: String,
    /// Name (GOLD CTC 2), `UNKNOWN` when unknown.
    pub name: String,
    /// Force code position (GOLD CTC 11): `air`, `surface`, `subsurface`,
    /// `ground`, `space` or `unknown`.
    pub domain: String,
    /// Force code threat identity (GOLD CTC 11), `unknown` when unknown.
    pub affiliation: String,
    /// The GOLD force code itself (Table 5-1).
    pub force_code: u8,
    /// GOLD CTC 13.
    pub track_type: TrackType,
    /// Symbol identification code, with its standard (`2525c`, `2525d` or
    /// `cot`). Always present: a CoT type is derived when no feed gives one.
    pub sidc: Sidc,
    /// Time of the position (GOLD POS 1-2).
    pub time: DateTime<Utc>,
    /// Position (GOLD POS 3-4), degrees WGS84.
    pub lat: f64,
    pub lon: f64,
    /// The admin-designed output schema's fields that have a value.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub attributes: Map<String, Value>,
    /// The track's security label (see [`crate::SecurityLabel`]), when its
    /// sources carry one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security: Option<crate::SecurityLabel>,
    /// `group` for a group a track manager formed; absent for a track.
    #[serde(default, skip_serializing_if = "crate::TrackKind::is_track")]
    pub kind: crate::TrackKind,
    /// A group's member tracks (`tms-<UID>`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<String>,
    /// The groups this track belongs to (`tms-<UID>`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<String>,
    /// Tracks paired with this one (GOLD PAIR: the same object, kept as
    /// separate tracks), `tms-<UID>`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paired_with: Vec<String>,
    /// When the position comes from an area of uncertainty: the area, as a
    /// polygon (`[lat, lon]` vertices) or, failing that, the 1-sigma error
    /// ellipse around `lat`/`lon`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area: Option<AreaOut>,
    /// Lines of bearing pointing at this track, the latest from each sensor:
    /// where each starts, its direction and error, for drawing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bearings: Vec<crate::BearingContact>,
    pub publisher: PublishContext,
    pub published_at: DateTime<Utc>,
}

/// A track's area of uncertainty in the published message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AreaOut {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub polygon: Option<Vec<[f64; 2]>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ellipse: Option<crate::schema::Ellipse>,
}

/// Body of a `delete`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeleteMessage {
    pub schema: String,
    pub op: Op,
    pub track_id: String,
    pub uid: Uid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The track it became one with (`tms-<UID>`), when it was merged:
    /// consumers re-point what they held for this one there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged_into: Option<String>,
    pub deleted_at: DateTime<Utc>,
    pub publisher: PublishContext,
}

/// The area to publish: the view's polygon, or its error ellipse when that
/// is large enough to be an area rather than a point (over 2 km).
fn area_of(view: &crate::Observation) -> Option<AreaOut> {
    if let Some(crate::Geometry::Area { polygon }) = &view.geometry {
        return Some(AreaOut {
            polygon: Some(polygon.clone()),
            ellipse: None,
        });
    }
    let cov = view.uncertainty.as_ref()?.position_covariance()?;
    ((cov[0] + cov[2]).sqrt() > 2000.0).then(|| AreaOut {
        polygon: None,
        ellipse: Some(crate::schema::Ellipse::from_covariance(cov)),
    })
}

fn lower(v: impl Serialize) -> Option<String> {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
}

fn non_empty(s: Option<&str>) -> Option<&str> {
    s.map(str::trim).filter(|s| !s.is_empty())
}

/// Which observation fields (mapping targets) fill each OTH-GOLD field of
/// the published message, in the order [`to_message`] tries them. Fields
/// with none are fixed or computed (`track_id` from the UID).
pub const GOLD_SOURCES: &[(&str, &[&str])] = &[
    ("track_id", &[]),
    ("class", &["platform.class"]),
    ("name", &["name", "platform.name"]),
    (
        "domain",
        &[
            "classification.domain",
            "classification.cot_type",
            "classification.sidc",
        ],
    ),
    (
        "affiliation",
        &[
            "classification.affiliation",
            "classification.cot_type",
            "classification.sidc",
        ],
    ),
    (
        "force_code",
        &["classification.domain", "classification.affiliation"],
    ),
    ("track_type", &["track_type"]),
    (
        "sidc",
        &[
            "classification.sidc",
            "classification.affiliation",
            "classification.cot_type",
        ],
    ),
    ("time", &["observed_at"]),
    ("lat", &["position.latitude"]),
    ("lon", &["position.longitude"]),
];

/// Encode a system track as an `upsert`.
pub fn to_message(
    track: &SystemTrack,
    ctx: &PublishContext,
    published_at: DateTime<Utc>,
) -> TrackMessage {
    let v = &track.view;
    let domain = v.classification.effective_domain();
    let affiliation = v.classification.effective_affiliation();
    TrackMessage {
        schema: TRACK_SCHEMA.into(),
        op: Op::Upsert,
        track_id: track.uid.doc_id(),
        uid: track.uid,
        // OTH-GOLD writes class-name in capitals.
        class: non_empty(v.platform.class.as_deref())
            .unwrap_or(gold::UNEQUATED)
            .to_uppercase(),
        name: non_empty(v.name.as_deref())
            .or(non_empty(v.platform.name.as_deref()))
            .unwrap_or(gold::UNKNOWN)
            .to_uppercase(),
        domain: domain.and_then(lower).unwrap_or_else(|| "unknown".into()),
        affiliation: affiliation
            .and_then(lower)
            .unwrap_or_else(|| "unknown".into()),
        force_code: gold::force_code(domain, affiliation),
        track_type: v.track_type.unwrap_or_default(),
        sidc: v.classification.sidc_or_derived(),
        time: v.observed_at,
        lat: v.position.latitude,
        lon: v.position.longitude,
        attributes: track.attributes.clone(),
        security: track.view.security.clone(),
        kind: track.kind,
        members: track.members.iter().map(|u| u.doc_id()).collect(),
        area: area_of(&track.view),
        bearings: track.bearings.clone(),
        groups: track.groups.clone(),
        paired_with: track.paired_with.iter().map(|u| u.doc_id()).collect(),
        publisher: ctx.clone(),
        published_at,
    }
}

/// Encode a retired track as a `delete`.
pub fn delete_message(
    uid: Uid,
    reason: Option<String>,
    ctx: &PublishContext,
    deleted_at: DateTime<Utc>,
) -> DeleteMessage {
    // The engine retires a merged track "merged into tms-<UID>".
    let merged_into = reason
        .as_deref()
        .and_then(|r| r.strip_prefix("merged into "))
        .filter(|d| Uid::from_doc_id(d).is_ok())
        .map(str::to_owned);
    DeleteMessage {
        schema: TRACK_SCHEMA.into(),
        op: Op::Delete,
        track_id: uid.doc_id(),
        uid,
        reason,
        merged_into,
        deleted_at,
        publisher: ctx.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::sample;
    use serde_json::json;

    fn ctx() -> PublishContext {
        PublishContext {
            node_id: "opentrack-OTK".into(),
            version: "0.1.0".into(),
            correlation: "correlation-1".into(),
        }
    }

    fn at() -> DateTime<Utc> {
        "2026-09-25T00:00:00Z".parse().unwrap()
    }

    #[test]
    fn gold_sources_cover_every_published_gold_field() {
        let uid: Uid = "OTK000000001".parse().unwrap();
        let v = serde_json::to_value(to_message(
            &SystemTrack::from_first_observation(uid, sample()),
            &ctx(),
            at(),
        ))
        .unwrap();
        let envelope = [
            "schema",
            "op",
            "uid",
            "attributes",
            "publisher",
            "published_at",
        ];
        let mut gold: Vec<&str> = v
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .filter(|k| !envelope.contains(k))
            .collect();
        let mut listed: Vec<&str> = GOLD_SOURCES.iter().map(|(k, _)| *k).collect();
        gold.sort_unstable();
        listed.sort_unstable();
        assert_eq!(gold, listed);
    }

    #[test]
    fn upsert_is_the_gold_minimum_plus_attributes() {
        let uid: Uid = "OTK000000001".parse().unwrap();
        let mut track = SystemTrack::from_first_observation(uid, sample());
        track
            .attributes
            .insert("contact_phone".into(), json!("+1 555 0100"));
        let v = serde_json::to_value(to_message(&track, &ctx(), at())).unwrap();

        assert_eq!(subject(TRACKS_SUBJECT, uid), "tracks.tms-OTK000000001");
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "affiliation",
                "attributes",
                "class",
                "domain",
                "force_code",
                "lat",
                "lon",
                "name",
                "op",
                "published_at",
                "publisher",
                "schema",
                "sidc",
                "time",
                "track_id",
                "track_type",
                "uid"
            ]
        );
        assert_eq!(v["schema"], "opentrack.track.v2");
        assert_eq!(v["track_id"], "tms-OTK000000001");
        assert_eq!(v["class"], "UNEQUATED");
        assert_eq!(v["name"], "TED STEVENS");
        assert_eq!(v["domain"], "surface");
        assert_eq!(v["affiliation"], "friend");
        assert_eq!(v["force_code"], 9);
        assert_eq!(v["track_type"], "tactical");
        assert_eq!(v["sidc"], json!({"standard": "cot", "code": "a-f-S"}));
        assert_eq!(v["time"], "2026-09-24T12:00:00Z");
        assert_eq!(v["attributes"], json!({"contact_phone": "+1 555 0100"}));
        let back: TrackMessage = serde_json::from_value(v).unwrap();
        assert_eq!(back.uid, uid);
    }

    #[test]
    fn unknowns_use_gold_placeholders() {
        let mut obs = sample();
        obs.name = None;
        obs.classification = Default::default();
        let uid: Uid = "OTK000000002".parse().unwrap();
        let v = serde_json::to_value(to_message(
            &SystemTrack::from_first_observation(uid, obs),
            &ctx(),
            at(),
        ))
        .unwrap();
        assert_eq!(v["class"], "UNEQUATED");
        assert_eq!(v["name"], "UNKNOWN");
        assert_eq!(v["domain"], "unknown");
        assert_eq!(v["affiliation"], "unknown");
        assert_eq!(v["force_code"], 32);
        assert_eq!(v["sidc"], json!({"standard": "cot", "code": "a-u"}));
        assert!(v.get("attributes").is_none());
    }

    #[test]
    fn a_feed_sidc_is_published_and_drives_the_force_code() {
        let mut obs = sample();
        obs.classification = Default::default();
        obs.classification.sidc = Some("10063000001211000000".into());
        let uid: Uid = "OTK000000004".parse().unwrap();
        let t = SystemTrack::from_first_observation(uid, obs.clone());
        let v = serde_json::to_value(to_message(&t, &ctx(), at())).unwrap();
        assert_eq!(
            v["sidc"],
            json!({"standard": "2525d", "code": "10063000001211000000"})
        );
        assert_eq!(
            (v["domain"].as_str(), v["affiliation"].as_str()),
            (Some("surface"), Some("hostile"))
        );
        assert_eq!(v["force_code"], 7);

        // An explicit affiliation rewrites the SIDC's identity too.
        obs.classification.affiliation = Some(crate::Affiliation::Friend);
        let t = SystemTrack::from_first_observation(uid, obs);
        let v = serde_json::to_value(to_message(&t, &ctx(), at())).unwrap();
        assert_eq!(v["sidc"]["code"], "10033000001211000000");
        assert_eq!(v["force_code"], 9);
    }

    #[test]
    fn a_merged_track_says_which_it_became() {
        let ctx = PublishContext {
            node_id: "opentrack-AAA".into(),
            version: "0".into(),
            correlation: String::new(),
        };
        let uid: Uid = "AAA000000002".parse().unwrap();
        let m = delete_message(
            uid,
            Some("merged into tms-BBB000000007".into()),
            &ctx,
            Utc::now(),
        );
        assert_eq!(m.merged_into.as_deref(), Some("tms-BBB000000007"));
        let m = delete_message(uid, Some("dropped".into()), &ctx, Utc::now());
        assert!(m.merged_into.is_none());
        assert!(
            serde_json::to_value(&m)
                .unwrap()
                .get("merged_into")
                .is_none()
        );
    }

    #[test]
    fn delete_carries_the_id_and_reason() {
        let uid: Uid = "OTK000000003".parse().unwrap();
        let v = serde_json::to_value(delete_message(uid, Some("dropped".into()), &ctx(), at()))
            .unwrap();
        assert_eq!(v["op"], "delete");
        assert_eq!(v["track_id"], "tms-OTK000000003");
        assert_eq!(v["reason"], "dropped");
        assert_eq!(v["deleted_at"], "2026-09-25T00:00:00Z");
    }
}
