//! Cursor-on-Target events for system tracks: what each track becomes, and
//! the delete sent when it ends. Pure, so it can be tested alone.
//!
//! A track's event:
//!
//! ```xml
//! <?xml version="1.0" encoding="UTF-8" standalone="yes"?>
//! <event version="2.0" uid="tms-OTK000000001" type="a-f-S-C-L-D-D" how="m-f"
//!        time="…" start="…" stale="…">
//!   <point lat="32.68" lon="-117.23" hae="9999999.0" ce="61.2" le="9999999.0"/>
//!   <detail>
//!     <track course="270.0" speed="5.00"/>
//!     <contact callsign="TED STEVENS"/>
//!     <__security classification="SECRET" caveats="ORCON" releasability="USA, GBR"
//!                 marking="(S//ORCON/REL TO USA, GBR)"/>
//!     <remarks>(S//ORCON/REL TO USA, GBR) OpenTrack OTK000000001; sources: ais</remarks>
//!   </detail>
//! </event>
//! ```
//!
//! A track with no security label has no `__security` element and no
//! marking in its remarks. One with a label always has both, whether or not
//! the output adds remarks: without them, the remarks are the marking alone.
//!
//! and its delete, in the form ATAK itself sends (a `t-x-d-d` event whose
//! `link` names the item, with `__forcedelete`, stale at once):
//!
//! ```xml
//! <event version="2.0" uid="tms-OTK000000001" type="t-x-d-d" how="m-g" time="…" start="…" stale="…">
//!   <point lat="0.0" lon="0.0" hae="0.0" ce="9999999.0" le="9999999.0"/>
//!   <detail><link uid="tms-OTK000000001" relation="none" type="a-f-S"/><__forcedelete/></detail>
//! </event>
//! ```

use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use ot_core::{Domain, SystemTrack, Uid};
use quick_xml::Writer;
use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};

/// CoT's "unknown" for `hae`, `ce` and `le`.
pub const UNKNOWN: f64 = 9_999_999.0;

/// At most this many sources are named in the remarks.
const MOST_SOURCES: usize = 5;

/// What a system track's event carries, taken from the track once and
/// rendered (with the time of sending) as often as it is sent.
#[derive(Debug, Clone, PartialEq)]
pub struct CotTrack {
    pub uid: Uid,
    pub cot_type: String,
    /// When the track's position was observed.
    pub observed_at: DateTime<Utc>,
    pub lat: f64,
    pub lon: f64,
    pub hae: Option<f64>,
    /// Circular 1-sigma horizontal error, metres.
    pub ce: Option<f64>,
    /// Vertical 1-sigma error, metres.
    pub le: Option<f64>,
    pub course: Option<f64>,
    pub speed: Option<f64>,
    pub callsign: String,
    pub remarks: String,
    /// The track's security label, when it has one.
    pub security: Option<ot_core::SecurityLabel>,
}

/// A track's CoT type: the more specific of its explicit CoT type and the one
/// its SIDC gives (2525C, 2525D or CoT; both with an explicit affiliation
/// applied), the explicit one on a tie, else from affiliation and domain. An
/// entity stores both, kept in step by the editor; a 2525D SIDC gives only
/// identity and symbol set here, so its CoT type's full function must not lose
/// to it, nor a feed's generic CoT type to a detailed SIDC.
/// An `a-` type always has a battle dimension (ground when unknown), so TAK
/// can draw it: an unknown track is `a-u-G`.
pub fn cot_type(track: &SystemTrack) -> String {
    let c = &track.view.classification;
    let aff = c
        .effective_affiliation()
        .unwrap_or(ot_core::Affiliation::Unknown)
        .cot_atom();
    let dim = c.effective_domain().unwrap_or(Domain::Ground).cot_atom();
    let explicit = c
        .cot_type
        .as_deref()
        .is_some_and(|t| t.starts_with("a-"))
        .then(|| c.cot_type_or_derived());
    let from_sidc = c.sidc_or_derived().cot_type();
    let atoms = |t: &Option<String>| t.as_deref().map_or(0, |t| t.split('-').count());
    let t = if atoms(&from_sidc) > atoms(&explicit) {
        from_sidc
    } else {
        explicit.or(from_sidc)
    }
    .unwrap_or_else(|| format!("a-{aff}-{dim}"));
    let mut atoms = t.split('-');
    match (atoms.next(), atoms.next(), atoms.next()) {
        (Some("a"), Some(a), None) => format!("a-{a}-{dim}"),
        _ => t,
    }
}

/// The name TAK shows: the track's callsign, a `callsign` identifier, its
/// name, its platform's name, else its track number.
pub fn callsign(track: &SystemTrack) -> String {
    let v = &track.view;
    let non_empty = |s: Option<&str>| {
        s.map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    non_empty(v.callsign.as_deref())
        .or_else(|| {
            v.identifiers
                .iter()
                .find(|i| i.scheme.eq_ignore_ascii_case("callsign"))
                .and_then(|i| non_empty(Some(&i.value)))
        })
        .or_else(|| non_empty(v.name.as_deref()))
        .or_else(|| non_empty(v.platform.name.as_deref()))
        .unwrap_or_else(|| track.uid.to_string())
}

impl CotTrack {
    pub fn of(track: &SystemTrack) -> Self {
        let v = &track.view;
        let known = |x: Option<f64>| x.filter(|x| x.is_finite() && *x >= 0.0);
        let ce = v.uncertainty.as_ref().and_then(|u| {
            u.position_covariance()
                .map(|[nn, _, ee]| ((nn + ee) / 2.0).max(0.0).sqrt())
        });
        let mut sources: Vec<&str> = Vec::new();
        for c in &track.contributors {
            if !sources.contains(&c.source_id.as_str()) {
                sources.push(&c.source_id);
            }
        }
        let mut remarks = format!("OpenTrack {}", track.uid);
        if !sources.is_empty() {
            let more = sources.len().saturating_sub(MOST_SOURCES);
            remarks.push_str("; sources: ");
            remarks.push_str(&sources[..sources.len().min(MOST_SOURCES)].join(", "));
            if more > 0 {
                remarks.push_str(&format!(" and {more} more"));
            }
        }
        Self {
            uid: track.uid,
            cot_type: cot_type(track),
            observed_at: v.observed_at,
            lat: v.position.latitude,
            lon: v.position.longitude,
            hae: v.position.altitude_hae_m.filter(|h| h.is_finite()),
            ce: known(ce),
            le: known(v.uncertainty.as_ref().and_then(|u| u.vertical_error_m)),
            course: v.kinematics.course_deg.filter(|c| c.is_finite()),
            speed: known(v.kinematics.speed_mps),
            callsign: callsign(track),
            remarks,
            security: v.security.clone(),
        }
    }
}

/// The CoT uid of a track: its published document id, `tms-<UID>`.
pub fn event_uid(uid: Uid) -> String {
    uid.doc_id()
}

fn stamp(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn num(x: f64, digits: usize) -> String {
    format!("{x:.digits$}")
}

fn write(w: &mut Writer<Vec<u8>>, e: Event<'_>) {
    // Writing to a Vec cannot fail.
    w.write_event(e).expect("writing XML to memory");
}

fn start(
    w: &mut Writer<Vec<u8>>,
    uid: &str,
    cot_type: &str,
    how: &str,
    now: DateTime<Utc>,
    stale: DateTime<Utc>,
) {
    write(
        w,
        Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), Some("yes"))),
    );
    let (time, stale) = (stamp(now), stamp(stale));
    let e = BytesStart::new("event").with_attributes([
        ("version", "2.0"),
        ("uid", uid),
        ("type", cot_type),
        ("how", how),
        ("time", time.as_str()),
        ("start", time.as_str()),
        ("stale", stale.as_str()),
    ]);
    write(w, Event::Start(e));
}

fn point(w: &mut Writer<Vec<u8>>, lat: f64, lon: f64, hae: f64, ce: f64, le: f64) {
    let (lat, lon) = (lat.to_string(), lon.to_string());
    let (hae, ce, le) = (num(hae, 1), num(ce, 1), num(le, 1));
    write(
        w,
        Event::Empty(BytesStart::new("point").with_attributes([
            ("lat", lat.as_str()),
            ("lon", lon.as_str()),
            ("hae", hae.as_str()),
            ("ce", ce.as_str()),
            ("le", le.as_str()),
        ])),
    );
}

/// A track's event: timed at its last report (never later than `now`, for a
/// source whose clock runs ahead) and stale `stale` after it, so TAK lets a
/// track go when its reports stop, however often it is re-sent. `None` once
/// that is past: nothing to send. `remarks`: add the remarks (track number
/// and sources). A labelled track's marking goes out either way: a
/// `__security` element and the start of the remarks.
pub fn event_xml(
    t: &CotTrack,
    now: DateTime<Utc>,
    stale: Duration,
    remarks: bool,
) -> Option<Vec<u8>> {
    let seen = t.observed_at.min(now);
    let stale_at = seen + chrono::Duration::from_std(stale).unwrap_or(chrono::Duration::MAX);
    if stale_at <= now {
        return None;
    }
    let mut w = Writer::new(Vec::with_capacity(512));
    start(
        &mut w,
        &event_uid(t.uid),
        &t.cot_type,
        "m-f",
        seen,
        stale_at,
    );
    point(
        &mut w,
        t.lat,
        t.lon,
        t.hae.unwrap_or(UNKNOWN),
        t.ce.unwrap_or(UNKNOWN),
        t.le.unwrap_or(UNKNOWN),
    );
    write(&mut w, Event::Start(BytesStart::new("detail")));
    if t.course.is_some() || t.speed.is_some() {
        let mut track = BytesStart::new("track");
        if let Some(c) = t.course {
            track.push_attribute(("course", num(c.rem_euclid(360.0), 1).as_str()));
        }
        if let Some(s) = t.speed {
            track.push_attribute(("speed", num(s, 2).as_str()));
        }
        write(&mut w, Event::Empty(track));
    }
    write(
        &mut w,
        Event::Empty(
            BytesStart::new("contact").with_attributes([("callsign", t.callsign.as_str())]),
        ),
    );
    let marking = t.security.as_ref().map(ot_core::SecurityLabel::marking);
    if let (Some(l), Some(m)) = (&t.security, &marking) {
        let mut e = BytesStart::new("__security");
        e.push_attribute(("classification", l.classification.trim()));
        if !l.restrictions.is_empty() {
            e.push_attribute(("caveats", l.restrictions.join("/").as_str()));
        }
        if let Some(r) = l.sharing.as_deref() {
            e.push_attribute(("releasability", r));
        }
        e.push_attribute(("marking", m.as_str()));
        write(&mut w, Event::Empty(e));
    }
    let text = match (marking, remarks && !t.remarks.is_empty()) {
        (Some(m), true) => Some(format!("{m} {}", t.remarks)),
        (Some(m), false) => Some(m),
        (None, true) => Some(t.remarks.clone()),
        (None, false) => None,
    };
    if let Some(text) = text {
        write(&mut w, Event::Start(BytesStart::new("remarks")));
        write(&mut w, Event::Text(BytesText::new(&text)));
        write(&mut w, Event::End(BytesEnd::new("remarks")));
    }
    write(&mut w, Event::End(BytesEnd::new("detail")));
    write(&mut w, Event::End(BytesEnd::new("event")));
    Some(w.into_inner())
}

/// The delete for a track that ended: `t-x-d-d`, linking to the track's uid
/// (with the type it was last sent as, when known), stale at once.
pub fn delete_xml(uid: Uid, last_type: Option<&str>, now: DateTime<Utc>) -> Vec<u8> {
    let mut w = Writer::new(Vec::with_capacity(384));
    let target = event_uid(uid);
    start(&mut w, &target, "t-x-d-d", "m-g", now, now);
    point(&mut w, 0.0, 0.0, 0.0, UNKNOWN, UNKNOWN);
    write(&mut w, Event::Start(BytesStart::new("detail")));
    write(
        &mut w,
        Event::Empty(BytesStart::new("link").with_attributes([
            ("uid", target.as_str()),
            ("relation", "none"),
            ("type", last_type.unwrap_or("none")),
        ])),
    );
    write(&mut w, Event::Empty(BytesStart::new("__forcedelete")));
    write(&mut w, Event::End(BytesEnd::new("detail")));
    write(&mut w, Event::End(BytesEnd::new("event")));
    w.into_inner()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use quick_xml::Reader;
    use std::collections::BTreeMap;

    pub(crate) fn uid(n: u64) -> Uid {
        Uid::new(ot_core::SiteCode::new("OTK").unwrap(), n).unwrap()
    }

    pub(crate) fn track(n: u64, json: serde_json::Value) -> SystemTrack {
        let t: DateTime<Utc> = "2026-09-29T12:00:00Z".parse().unwrap();
        let mut v = serde_json::json!({
            "schema_version": 1, "source_id": "ais", "source_track_key": format!("k{n}"),
            "observed_at": t, "received_at": t,
            "position": { "latitude": 32.68, "longitude": -117.23 }
        });
        if let (Some(v), Some(extra)) = (v.as_object_mut(), json.as_object()) {
            for (k, x) in extra {
                v.insert(k.clone(), x.clone());
            }
        }
        let mut t = SystemTrack::from_first_observation(uid(n), serde_json::from_value(v).unwrap());
        t.published = Some(true);
        t
    }

    /// Every element of a document with its attributes, in order, and the
    /// text inside elements: parsed back with quick-xml, which fails on
    /// anything malformed.
    pub(crate) fn parse(xml: &[u8]) -> Vec<(String, BTreeMap<String, String>)> {
        use quick_xml::XmlVersion;
        let mut r = Reader::from_reader(xml);
        let mut out = Vec::new();
        let mut text = String::new();
        let flush = |text: &mut String, out: &mut Vec<(String, BTreeMap<String, String>)>| {
            if !text.trim().is_empty() {
                out.push((
                    "#text".into(),
                    BTreeMap::from([(String::new(), text.clone())]),
                ));
            }
            text.clear();
        };
        loop {
            match r.read_event().expect("well-formed XML") {
                Event::Start(e) | Event::Empty(e) => {
                    flush(&mut text, &mut out);
                    let attrs = e
                        .attributes()
                        .map(|a| {
                            let a = a.expect("a well-formed attribute");
                            (
                                (a.key.as_ref() as &str).to_owned(),
                                a.normalized_value(XmlVersion::Implicit1_0)
                                    .expect("a well-formed value")
                                    .into_owned(),
                            )
                        })
                        .collect();
                    out.push(((e.name().as_ref() as &str).to_owned(), attrs));
                }
                Event::Text(t) => text.push_str(&t.xml_content(XmlVersion::Implicit1_0)),
                Event::GeneralRef(g) => match g.resolve_char_ref() {
                    Ok(Some(c)) => text.push(c),
                    _ => text.push_str(match &*g {
                        "amp" => "&",
                        "lt" => "<",
                        "gt" => ">",
                        "quot" => "\"",
                        "apos" => "'",
                        other => panic!("unknown entity {other}"),
                    }),
                },
                Event::End(_) => flush(&mut text, &mut out),
                Event::Eof => break,
                _ => {}
            }
        }
        out
    }

    fn at() -> DateTime<Utc> {
        "2026-09-29T12:00:05.250Z".parse().unwrap()
    }

    #[test]
    fn a_track_event_is_golden() {
        let mut t = track(
            1,
            serde_json::json!({
                "name": "TED STEVENS",
                "position": { "latitude": 32.68, "longitude": -117.23, "altitude_hae_m": 12.5 },
                "uncertainty": { "circular_error_m": 117.74, "vertical_error_m": 20.0 },
                "kinematics": { "course_deg": 270.0, "speed_mps": 5.0 },
                "classification": { "sidc": "SFSPCLDD-------" }
            }),
        );
        t.contributors.push(ot_core::Contributor {
            source_id: "radar-1".into(),
            ..t.contributors[0].clone()
        });
        let xml = event_xml(&CotTrack::of(&t), at(), Duration::from_secs(60), true).unwrap();
        let text = String::from_utf8(xml.clone()).unwrap();
        assert_eq!(
            text,
            concat!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
                r#"<event version="2.0" uid="tms-OTK000000001" type="a-f-S-C-L-D-D" how="m-f" "#,
                r#"time="2026-09-29T12:00:00.000Z" start="2026-09-29T12:00:00.000Z" stale="2026-09-29T12:01:00.000Z">"#,
                r#"<point lat="32.68" lon="-117.23" hae="12.5" ce="100.0" le="20.0"/>"#,
                r#"<detail><track course="270.0" speed="5.00"/><contact callsign="TED STEVENS"/>"#,
                r#"<remarks>OpenTrack OTK000000001; sources: ais, radar-1</remarks></detail></event>"#,
            )
        );
        let parsed = parse(&xml);
        assert_eq!(parsed[0].0, "event");
        assert_eq!(parsed[0].1["uid"], "tms-OTK000000001");
    }

    #[test]
    fn an_event_is_timed_at_the_last_report_and_goes_stale_after_it() {
        let c = CotTrack::of(&track(4, serde_json::json!({})));
        let stale = Duration::from_secs(60);
        let times = |now: &str| {
            event_xml(&c, now.parse().unwrap(), stale, false).map(|xml| {
                let e = parse(&xml)[0].1.clone();
                (e["time"].clone(), e["start"].clone(), e["stale"].clone())
            })
        };
        // Re-sent 50 s after the report: still the report's times, not the send time.
        let at_report = (
            "2026-09-29T12:00:00.000Z".to_string(),
            "2026-09-29T12:00:00.000Z".to_string(),
            "2026-09-29T12:01:00.000Z".to_string(),
        );
        assert_eq!(times("2026-09-29T12:00:50Z"), Some(at_report));
        // Past its stale time: nothing to send, however long the track stays live.
        assert_eq!(times("2026-09-29T12:01:00Z"), None);
        assert_eq!(times("2026-09-29T17:00:00Z"), None);
        // A report stamped ahead of this clock is timed now, not in the future.
        let early = times("2026-09-29T11:59:30Z").unwrap();
        assert_eq!(
            (early.0.as_str(), early.2.as_str()),
            ("2026-09-29T11:59:30.000Z", "2026-09-29T12:00:30.000Z")
        );
    }

    #[test]
    fn unknowns_use_cot_conventions() {
        let t = track(2, serde_json::json!({}));
        let c = CotTrack::of(&t);
        assert_eq!(c.cot_type, "a-u-G");
        assert_eq!(c.callsign, "OTK000000002");
        let xml = event_xml(&c, at(), Duration::from_secs(30), false).unwrap();
        let parsed = parse(&xml);
        let point = &parsed.iter().find(|(n, _)| n == "point").unwrap().1;
        assert_eq!(
            (
                point["hae"].as_str(),
                point["ce"].as_str(),
                point["le"].as_str()
            ),
            ("9999999.0", "9999999.0", "9999999.0")
        );
        assert!(parsed.iter().all(|(n, _)| n != "track" && n != "remarks"));
        assert_eq!(parsed[0].1["stale"], "2026-09-29T12:00:30.000Z");
    }

    #[test]
    fn attributes_and_text_are_escaped() {
        let mut c = CotTrack::of(&track(3, serde_json::json!({})));
        c.callsign = r#"A<B>&"C" 'D'"#.into();
        c.remarks = "x < y & z > w ]]>".into();
        let xml = event_xml(&c, at(), Duration::from_secs(60), true).unwrap();
        let text = String::from_utf8(xml.clone()).unwrap();
        assert!(!text.contains("A<B>"), "{text}");
        let parsed = parse(&xml);
        let contact = &parsed.iter().find(|(n, _)| n == "contact").unwrap().1;
        assert_eq!(contact["callsign"], r#"A<B>&"C" 'D'"#);
        let remarks = parsed.iter().find(|(n, _)| n == "#text").unwrap();
        assert_eq!(remarks.1[""], "x < y & z > w ]]>");
    }

    #[test]
    fn types_and_callsigns_follow_the_track() {
        let cases = [
            (
                serde_json::json!({"classification": {"affiliation": "hostile", "domain": "air"}}),
                "a-h-A",
            ),
            (
                serde_json::json!({"classification": {"affiliation": "friend", "domain": "surface"}}),
                "a-f-S",
            ),
            (
                serde_json::json!({"classification": {"domain": "ground"}}),
                "a-u-G",
            ),
            (
                serde_json::json!({"classification": {"affiliation": "neutral"}}),
                "a-n-G",
            ),
            (
                serde_json::json!({"classification": {"cot_type": "a-f-A-M-F-Q"}}),
                "a-f-A-M-F-Q",
            ),
            (
                serde_json::json!({"classification": {"sidc": "SUGPUCI----", "affiliation": "hostile"}}),
                "a-h-G-U-C-I",
            ),
            (
                serde_json::json!({"classification": {"sidc": "10061000001211000000"}}),
                "a-h-G-U",
            ),
            // Both stored: the more specific, with an explicit affiliation
            // applied; a feed's generic type does not hide a detailed SIDC.
            (
                serde_json::json!({"classification": {"sidc": "10031000001211000000", "cot_type": "a-f-G-U-C-I"}}),
                "a-f-G-U-C-I",
            ),
            (
                serde_json::json!({"classification": {"sidc": "10031000001211000000", "cot_type": "a-f-G-U-C-I", "affiliation": "hostile"}}),
                "a-h-G-U-C-I",
            ),
            (
                serde_json::json!({"classification": {"sidc": "SFGPUCI----", "cot_type": "a-u-G"}}),
                "a-f-G-U-C-I",
            ),
            // A tactical graphic has no atom: affiliation and domain instead.
            (
                serde_json::json!({"classification": {"sidc": "GHGPGLB----", "domain": "ground"}}),
                "a-h-G",
            ),
        ];
        for (json, want) in cases {
            assert_eq!(
                CotTrack::of(&track(4, json.clone())).cot_type,
                want,
                "{json}"
            );
        }
        let named = |json| callsign(&track(5, json));
        assert_eq!(
            named(serde_json::json!({"callsign": "VIPER 1", "name": "X"})),
            "VIPER 1"
        );
        assert_eq!(
            named(
                serde_json::json!({"identifiers": [{"scheme": "CALLSIGN", "value": "RCH123"}], "name": "X"})
            ),
            "RCH123"
        );
        assert_eq!(
            named(serde_json::json!({"platform": {"name": "Hull 7"}})),
            "Hull 7"
        );
        let many = {
            let mut t = track(6, serde_json::json!({}));
            for i in 0..7 {
                t.contributors.push(ot_core::Contributor {
                    source_id: format!("s{i}"),
                    ..t.contributors[0].clone()
                });
            }
            CotTrack::of(&t).remarks
        };
        assert_eq!(
            many,
            "OpenTrack OTK000000006; sources: ais, s0, s1, s2, s3 and 3 more"
        );
    }

    #[test]
    fn a_labelled_track_carries_its_marking() {
        let t = track(
            8,
            serde_json::json!({"security": {
                "classification": "SECRET", "restrictions": ["orcon", "<x&y>"],
                "sharing": "GBR, USA"
            }}),
        );
        let c = CotTrack::of(&t);
        for with_remarks in [true, false] {
            let xml = event_xml(&c, at(), Duration::from_secs(60), with_remarks).unwrap();
            let parsed = parse(&xml);
            let sec = &parsed.iter().find(|(n, _)| n == "__security").unwrap().1;
            assert_eq!(sec["classification"], "SECRET");
            assert_eq!(sec["caveats"], "orcon/<x&y>");
            assert_eq!(sec["releasability"], "GBR, USA");
            let marking = "(S//ORCON/<X&Y>/REL TO USA, GBR)";
            assert_eq!(sec["marking"], marking);
            // Inside detail, before the remarks.
            let names: Vec<&str> = parsed.iter().map(|(n, _)| n.as_str()).collect();
            let at_sec = names.iter().position(|n| *n == "__security").unwrap();
            let at_detail = names.iter().position(|n| *n == "detail").unwrap();
            assert!(at_detail < at_sec);
            let text = &parsed.iter().find(|(n, _)| n == "#text").unwrap().1[""];
            if with_remarks {
                assert_eq!(
                    text,
                    &format!("{marking} OpenTrack OTK000000008; sources: ais")
                );
            } else {
                assert_eq!(text, marking, "the marking alone when remarks are off");
            }
        }
        // Unlabelled: neither.
        let plain = CotTrack::of(&track(9, serde_json::json!({})));
        let parsed = parse(&event_xml(&plain, at(), Duration::from_secs(60), true).unwrap());
        assert!(parsed.iter().all(|(n, _)| n != "__security"));
        let text = &parsed.iter().find(|(n, _)| n == "#text").unwrap().1[""];
        assert!(text.starts_with("OpenTrack "), "{text}");
        let parsed = parse(&event_xml(&plain, at(), Duration::from_secs(60), false).unwrap());
        assert!(
            parsed
                .iter()
                .all(|(n, _)| n != "remarks" && n != "__security")
        );
    }

    #[test]
    fn a_delete_links_to_the_track() {
        let xml = delete_xml(uid(7), Some("a-h-A"), at());
        assert_eq!(
            String::from_utf8(xml.clone()).unwrap(),
            concat!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
                r#"<event version="2.0" uid="tms-OTK000000007" type="t-x-d-d" how="m-g" "#,
                r#"time="2026-09-29T12:00:05.250Z" start="2026-09-29T12:00:05.250Z" stale="2026-09-29T12:00:05.250Z">"#,
                r#"<point lat="0" lon="0" hae="0.0" ce="9999999.0" le="9999999.0"/>"#,
                r#"<detail><link uid="tms-OTK000000007" relation="none" type="a-h-A"/><__forcedelete/></detail></event>"#,
            )
        );
        let parsed = parse(&xml);
        let link = &parsed.iter().find(|(n, _)| n == "link").unwrap().1;
        assert_eq!(link["uid"], "tms-OTK000000007");
        assert_eq!(
            delete_xml(uid(7), None, at())
                .windows(11)
                .filter(|w| w == br#"type="none""#)
                .count(),
            1
        );
    }
}
