//! The sync messages nodes exchange, in their binary form (`docs/sync-icd.md`
//! is the specification; this is the implementation, and the tests pin it).
//!
//! Every message is one envelope: who sent it, their clock, and one kind of
//! body. Integers are big-endian. A message is at most [`MAX_MESSAGE`]
//! bytes once signed ([`crate::sign`] appends [`crate::sign::TRAILER`]
//! bytes); bodies that hold many items (reports, decisions) are split over
//! several messages by the `encode_*` helpers.

use ot_core::{Domain, SiteCode, TrackState, Uid};
use serde_json::Value;

use crate::{Entry, Hlc};

/// First two bytes of every message.
pub const MAGIC: [u8; 2] = *b"OT";
/// Format version; a node drops messages of a version it does not know.
/// Version 2 is version 1 signed (see [`crate::sign`]).
pub const VERSION: u8 = 2;
/// Largest message a node sends, signature included, in bytes.
pub const MAX_MESSAGE: usize = 1024;
/// Largest message before it is signed.
pub const MAX_UNSIGNED: usize = MAX_MESSAGE - crate::sign::TRAILER;
/// Envelope: magic, version, kind, site, clock.
pub const HEADER: usize = 2 + 1 + 1 + 3 + 8;
/// A report without its optional parts.
pub const REPORT_BASE: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    Report = 1,
    Attrs = 2,
    Decision = 3,
    Summary = 4,
    Want = 5,
    Release = 6,
}

impl Kind {
    /// Its name in NATS subjects (`ot.sync.out.<name>`).
    pub fn name(self) -> &'static str {
        match self {
            Kind::Report => "report",
            Kind::Attrs => "attrs",
            Kind::Decision => "decision",
            Kind::Summary => "summary",
            Kind::Want => "want",
            Kind::Release => "release",
        }
    }

    fn from_u8(b: u8) -> Option<Self> {
        Some(match b {
            1 => Kind::Report,
            2 => Kind::Attrs,
            3 => Kind::Decision,
            4 => Kind::Summary,
            5 => Kind::Want,
            6 => Kind::Release,
            _ => return None,
        })
    }
}

/// A track's position error as an ellipse: semi-axes in metres (95%),
/// orientation of the major axis in degrees true.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Ellipse {
    pub major_m: f64,
    pub minor_m: f64,
    pub orientation_deg: f64,
}

/// What a node's own sources say about a track it reports.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub uid: Uid,
    /// Time of the estimate, Unix ms.
    pub time_ms: i64,
    pub lat: f64,
    pub lon: f64,
    /// Metres above mean sea level.
    pub alt_m: Option<f64>,
    pub course_deg: Option<f64>,
    pub speed_mps: Option<f64>,
    pub error: Ellipse,
    /// Track quality 0–15 (see [`crate::quality`]).
    pub quality: u8,
    pub domain: Option<Domain>,
    pub state: TrackState,
    /// When the node that minted the UID made the track, Unix ms: decides
    /// which of two numbers for one object survives. OpenTrack sends it with
    /// every report.
    pub origin_ms: Option<i64>,
    /// Identifiers `(scheme, value)`, sent when they change.
    pub identifiers: Option<Vec<(String, String)>>,
}

/// Every few seconds: which decisions this node holds from each site.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Summary {
    /// Highest sequence held, per site (gaps below it are asked for).
    pub heads: Vec<(SiteCode, u64)>,
    /// Tracks this node reports.
    pub reporting: u32,
}

/// A request for what the sender lacks.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Want {
    /// Ranges of a site's decisions, inclusive.
    pub decisions: Vec<(SiteCode, Vec<(u64, u64)>)>,
    /// The latest report of every track the receiver reports.
    pub snapshot: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    Reports(Vec<Report>),
    /// A track's attributes (the published message's `attributes`, plus
    /// name, callsign and classification), as JSON.
    Attrs(Vec<(Uid, Value)>),
    Decisions(Vec<Entry>),
    Summary(Summary),
    Want(Want),
    /// The sender stops reporting these tracks (it is leaving).
    Release(Vec<Uid>),
}

impl Body {
    pub fn kind(&self) -> Kind {
        match self {
            Body::Reports(_) => Kind::Report,
            Body::Attrs(_) => Kind::Attrs,
            Body::Decisions(_) => Kind::Decision,
            Body::Summary(_) => Kind::Summary,
            Body::Want(_) => Kind::Want,
            Body::Release(_) => Kind::Release,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub site: SiteCode,
    pub hlc: Hlc,
    pub body: Body,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WireError {
    #[error("not an OpenTrack sync message")]
    Magic,
    #[error("sync message version {0} is not supported")]
    Version(u8),
    #[error("unknown message kind {0}")]
    Kind(u8),
    #[error("message ends early")]
    Short,
    #[error("bad {0}")]
    Bad(&'static str),
    #[error("message has {0} bytes left over")]
    Trailing(usize),
}

// --- writing ---------------------------------------------------------------

struct W(Vec<u8>);

impl W {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    fn i16(&mut self, v: i16) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    fn i32(&mut self, v: i32) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    fn i64(&mut self, v: i64) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    fn site(&mut self, s: SiteCode) {
        self.0.extend_from_slice(s.as_str().as_bytes());
    }
    fn uid(&mut self, u: Uid) {
        self.site(u.site());
        self.u32(u.sequence() as u32);
    }
    fn short_str(&mut self, s: &str) {
        let b = &s.as_bytes()[..s.len().min(255)];
        self.u8(b.len() as u8);
        self.0.extend_from_slice(b);
    }
    fn blob(&mut self, b: &[u8]) {
        self.u16(b.len() as u16);
        self.0.extend_from_slice(b);
    }
}

fn domain_code(d: Option<Domain>) -> u8 {
    match d {
        None => 0,
        Some(Domain::Air) => 1,
        Some(Domain::Surface) => 2,
        Some(Domain::Subsurface) => 3,
        Some(Domain::Ground) => 4,
        Some(Domain::Space) => 5,
    }
}

fn domain_of(c: u8) -> Result<Option<Domain>, WireError> {
    Ok(match c {
        0 => None,
        1 => Some(Domain::Air),
        2 => Some(Domain::Surface),
        3 => Some(Domain::Subsurface),
        4 => Some(Domain::Ground),
        5 => Some(Domain::Space),
        _ => return Err(WireError::Bad("domain")),
    })
}

fn state_code(s: TrackState) -> u8 {
    match s {
        TrackState::Tentative => 0,
        TrackState::Confirmed => 1,
        TrackState::Lost => 2,
        TrackState::Dropped => 3,
    }
}

fn state_of(c: u8) -> TrackState {
    match c {
        0 => TrackState::Tentative,
        1 => TrackState::Confirmed,
        2 => TrackState::Lost,
        _ => TrackState::Dropped,
    }
}

const NONE_U16: u16 = u16::MAX;

fn opt_u16(v: Option<f64>, scale: f64) -> u16 {
    v.filter(|x| x.is_finite())
        .map(|x| (x * scale).round().clamp(0.0, f64::from(NONE_U16 - 1)) as u16)
        .unwrap_or(NONE_U16)
}

fn metres_u16(v: f64) -> u16 {
    if v.is_finite() {
        v.round().clamp(0.0, f64::from(NONE_U16)) as u16
    } else {
        NONE_U16
    }
}

fn write_report(w: &mut W, r: &Report, base_ms: u64) {
    w.uid(r.uid);
    let dt = (r.time_ms - base_ms as i64).clamp(i32::MIN.into(), i32::MAX.into()) as i32;
    w.i32(dt);
    w.i32((r.lat * 1e7).round() as i32);
    w.i32((r.lon * 1e7).round() as i32);
    w.i16(
        r.alt_m
            .filter(|a| a.is_finite())
            .map(|a| a.round().clamp(-32767.0, 32767.0) as i16)
            .unwrap_or(i16::MIN),
    );
    w.u16(opt_u16(r.course_deg.map(|c| c.rem_euclid(360.0)), 100.0));
    w.u16(opt_u16(r.speed_mps, 100.0));
    w.u16(metres_u16(r.error.major_m));
    w.u16(metres_u16(r.error.minor_m));
    w.u8((r.error.orientation_deg.rem_euclid(180.0))
        .round()
        .min(179.0) as u8);
    w.u8((r.quality.min(15) << 4) | domain_code(r.domain));
    let flags = u8::from(r.origin_ms.is_some())
        | (u8::from(r.identifiers.is_some()) << 1)
        | (state_code(r.state) << 2);
    w.u8(flags);
    if let Some(o) = r.origin_ms {
        w.i64(o);
    }
    if let Some(ids) = &r.identifiers {
        w.u8(ids.len().min(255) as u8);
        for (scheme, value) in ids.iter().take(255) {
            w.short_str(scheme);
            w.short_str(value);
        }
    }
}

impl Message {
    pub fn new(site: SiteCode, hlc: Hlc, body: Body) -> Self {
        Self { site, hlc, body }
    }

    /// The message's bytes, unsigned. A body may come out larger than
    /// [`MAX_UNSIGNED`]; use the `encode_*` helpers to split long ones.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = W(Vec::with_capacity(128));
        w.0.extend_from_slice(&MAGIC);
        w.u8(VERSION);
        w.u8(self.body.kind() as u8);
        w.site(self.site);
        w.u64(self.hlc.0);
        match &self.body {
            Body::Reports(rs) => {
                w.u8(rs.len() as u8);
                for r in rs {
                    write_report(&mut w, r, self.hlc.ms());
                }
            }
            Body::Attrs(items) => {
                w.u8(items.len() as u8);
                for (uid, v) in items {
                    w.uid(*uid);
                    w.blob(v.to_string().as_bytes());
                }
            }
            Body::Decisions(es) => {
                w.u8(es.len() as u8);
                for e in es {
                    w.blob(&serde_json::to_vec(e).expect("an entry serializes"));
                }
            }
            Body::Summary(s) => {
                w.u32(s.reporting);
                w.u8(s.heads.len() as u8);
                for (site, seq) in &s.heads {
                    w.site(*site);
                    w.u32(*seq as u32);
                }
            }
            Body::Want(want) => {
                w.u8(u8::from(want.snapshot));
                w.u8(want.decisions.len() as u8);
                for (site, ranges) in &want.decisions {
                    w.site(*site);
                    w.u8(ranges.len() as u8);
                    for (a, b) in ranges {
                        w.u32(*a as u32);
                        w.u32(*b as u32);
                    }
                }
            }
            Body::Release(uids) => {
                w.u16(uids.len() as u16);
                for u in uids {
                    w.uid(*u);
                }
            }
        }
        w.0
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
        let mut r = R(bytes);
        if r.take(2)? != MAGIC {
            return Err(WireError::Magic);
        }
        let version = r.u8()?;
        if version != VERSION {
            return Err(WireError::Version(version));
        }
        let kind_byte = r.u8()?;
        let kind = Kind::from_u8(kind_byte).ok_or(WireError::Kind(kind_byte))?;
        let site = r.site()?;
        let hlc = Hlc(r.u64()?);
        let body = match kind {
            Kind::Report => {
                let n = r.u8()?;
                let mut rs = Vec::with_capacity(n as usize);
                for _ in 0..n {
                    rs.push(read_report(&mut r, hlc.ms())?);
                }
                Body::Reports(rs)
            }
            Kind::Attrs => {
                let n = r.u8()?;
                let mut items = Vec::with_capacity(n as usize);
                for _ in 0..n {
                    let uid = r.uid()?;
                    let v = serde_json::from_slice(r.blob()?)
                        .map_err(|_| WireError::Bad("attributes"))?;
                    items.push((uid, v));
                }
                Body::Attrs(items)
            }
            Kind::Decision => {
                let n = r.u8()?;
                let mut es = Vec::with_capacity(n as usize);
                for _ in 0..n {
                    es.push(
                        serde_json::from_slice(r.blob()?)
                            .map_err(|_| WireError::Bad("decision"))?,
                    );
                }
                Body::Decisions(es)
            }
            Kind::Summary => {
                let reporting = r.u32()?;
                let n = r.u8()?;
                let mut heads = Vec::with_capacity(n as usize);
                for _ in 0..n {
                    heads.push((r.site()?, u64::from(r.u32()?)));
                }
                Body::Summary(Summary { heads, reporting })
            }
            Kind::Want => {
                let snapshot = r.u8()? & 1 == 1;
                let n = r.u8()?;
                let mut decisions = Vec::with_capacity(n as usize);
                for _ in 0..n {
                    let site = r.site()?;
                    let k = r.u8()?;
                    let mut ranges = Vec::with_capacity(k as usize);
                    for _ in 0..k {
                        ranges.push((u64::from(r.u32()?), u64::from(r.u32()?)));
                    }
                    decisions.push((site, ranges));
                }
                Body::Want(Want {
                    decisions,
                    snapshot,
                })
            }
            Kind::Release => {
                let n = r.u16()?;
                let mut uids = Vec::with_capacity(n as usize);
                for _ in 0..n {
                    uids.push(r.uid()?);
                }
                Body::Release(uids)
            }
        };
        if !r.0.is_empty() {
            return Err(WireError::Trailing(r.0.len()));
        }
        Ok(Self { site, hlc, body })
    }
}

/// The sending site of a signed message, read from its envelope before the
/// signature is checked (to find the key to check it with). Only magic and
/// version are checked here.
pub fn sender(bytes: &[u8]) -> Result<SiteCode, WireError> {
    let mut r = R(bytes);
    if r.take(2)? != MAGIC {
        return Err(WireError::Magic);
    }
    let version = r.u8()?;
    if version != VERSION {
        return Err(WireError::Version(version));
    }
    r.u8()?;
    r.site()
}

/// Reports in as few messages as fit in [`MAX_UNSIGNED`] bytes each (so
/// in [`MAX_MESSAGE`] once signed).
pub fn encode_reports(site: SiteCode, hlc: Hlc, reports: &[Report]) -> Vec<Vec<u8>> {
    chunks(reports, HEADER + 1, 255, |r| {
        let mut w = W(Vec::new());
        write_report(&mut w, r, hlc.ms());
        w.0.len()
    })
    .into_iter()
    .map(|c| Message::new(site, hlc, Body::Reports(c.to_vec())).encode())
    .collect()
}

/// Decisions likewise. One entry larger than a message on its own (a very
/// large group) still goes, alone.
pub fn encode_decisions(site: SiteCode, hlc: Hlc, entries: &[Entry]) -> Vec<Vec<u8>> {
    chunks(entries, HEADER + 1, 255, |e| {
        2 + serde_json::to_vec(e).map(|v| v.len()).unwrap_or(0)
    })
    .into_iter()
    .map(|c| Message::new(site, hlc, Body::Decisions(c.to_vec())).encode())
    .collect()
}

/// Attributes likewise.
pub fn encode_attrs(site: SiteCode, hlc: Hlc, items: &[(Uid, Value)]) -> Vec<Vec<u8>> {
    chunks(items, HEADER + 1, 255, |(_, v)| 7 + 2 + v.to_string().len())
        .into_iter()
        .map(|c| Message::new(site, hlc, Body::Attrs(c.to_vec())).encode())
        .collect()
}

fn chunks<T>(items: &[T], header: usize, most: usize, size: impl Fn(&T) -> usize) -> Vec<&[T]> {
    let mut out = Vec::new();
    let (mut start, mut len) = (0, header);
    for (i, item) in items.iter().enumerate() {
        let s = size(item);
        if i > start && (len + s > MAX_UNSIGNED || i - start == most) {
            out.push(&items[start..i]);
            start = i;
            len = header;
        }
        len += s;
    }
    if start < items.len() {
        out.push(&items[start..]);
    }
    out
}

// --- reading ---------------------------------------------------------------

struct R<'a>(&'a [u8]);

impl<'a> R<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], WireError> {
        if self.0.len() < n {
            return Err(WireError::Short);
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn arr<const N: usize>(&mut self) -> Result<[u8; N], WireError> {
        Ok(self.take(N)?.try_into().expect("length checked"))
    }
    fn u8(&mut self) -> Result<u8, WireError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, WireError> {
        Ok(u16::from_be_bytes(self.arr()?))
    }
    fn u32(&mut self) -> Result<u32, WireError> {
        Ok(u32::from_be_bytes(self.arr()?))
    }
    fn i16(&mut self) -> Result<i16, WireError> {
        Ok(i16::from_be_bytes(self.arr()?))
    }
    fn i32(&mut self) -> Result<i32, WireError> {
        Ok(i32::from_be_bytes(self.arr()?))
    }
    fn i64(&mut self) -> Result<i64, WireError> {
        Ok(i64::from_be_bytes(self.arr()?))
    }
    fn u64(&mut self) -> Result<u64, WireError> {
        Ok(u64::from_be_bytes(self.arr()?))
    }
    fn site(&mut self) -> Result<SiteCode, WireError> {
        let b = self.take(3)?;
        std::str::from_utf8(b)
            .ok()
            .and_then(|s| s.parse().ok())
            .ok_or(WireError::Bad("site code"))
    }
    fn uid(&mut self) -> Result<Uid, WireError> {
        let site = self.site()?;
        Uid::new(site, u64::from(self.u32()?)).map_err(|_| WireError::Bad("UID"))
    }
    fn short_str(&mut self) -> Result<String, WireError> {
        let n = self.u8()? as usize;
        String::from_utf8(self.take(n)?.to_vec()).map_err(|_| WireError::Bad("text"))
    }
    fn blob(&mut self) -> Result<&'a [u8], WireError> {
        let n = self.u16()? as usize;
        self.take(n)
    }
}

fn read_report(r: &mut R<'_>, base_ms: u64) -> Result<Report, WireError> {
    let uid = r.uid()?;
    let time_ms = base_ms as i64 + i64::from(r.i32()?);
    let lat = f64::from(r.i32()?) / 1e7;
    let lon = f64::from(r.i32()?) / 1e7;
    let alt = r.i16()?;
    let course = r.u16()?;
    let speed = r.u16()?;
    let major = r.u16()?;
    let minor = r.u16()?;
    let orientation = r.u8()?;
    let qd = r.u8()?;
    let flags = r.u8()?;
    let origin_ms = if flags & 1 == 1 { Some(r.i64()?) } else { None };
    let identifiers = if flags & 2 == 2 {
        let n = r.u8()?;
        let mut ids = Vec::with_capacity(n as usize);
        for _ in 0..n {
            ids.push((r.short_str()?, r.short_str()?));
        }
        Some(ids)
    } else {
        None
    };
    let unknown = |v: u16| {
        if v == NONE_U16 {
            f64::INFINITY
        } else {
            f64::from(v)
        }
    };
    Ok(Report {
        uid,
        time_ms,
        lat,
        lon,
        alt_m: (alt != i16::MIN).then_some(f64::from(alt)),
        course_deg: (course != NONE_U16).then_some(f64::from(course) / 100.0),
        speed_mps: (speed != NONE_U16).then_some(f64::from(speed) / 100.0),
        error: Ellipse {
            major_m: unknown(major),
            minor_m: unknown(minor),
            orientation_deg: f64::from(orientation),
        },
        quality: qd >> 4,
        domain: domain_of(qd & 0x0f)?,
        state: state_of((flags >> 2) & 0b11),
        origin_ms,
        identifiers,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn site(s: &str) -> SiteCode {
        s.parse().unwrap()
    }

    fn report(seq: u64) -> Report {
        Report {
            uid: Uid::new(site("AAA"), seq).unwrap(),
            time_ms: 1_790_000_000_500,
            lat: 50.7654321,
            lon: -1.2345678,
            alt_m: None,
            course_deg: Some(271.25),
            speed_mps: Some(7.5),
            error: Ellipse {
                major_m: 42.0,
                minor_m: 12.0,
                orientation_deg: 30.0,
            },
            quality: 11,
            domain: Some(Domain::Surface),
            state: TrackState::Confirmed,
            origin_ms: None,
            identifiers: None,
        }
    }

    const NOW: Hlc = Hlc(1_790_000_001_000 << 16);

    #[test]
    fn a_report_is_32_bytes_and_reads_back() {
        let one = Message::new(site("BBB"), NOW, Body::Reports(vec![report(7)]));
        let bytes = one.encode();
        assert_eq!(bytes.len(), HEADER + 1 + REPORT_BASE);
        assert_eq!(&bytes[..4], &[b'O', b'T', VERSION, Kind::Report as u8]);
        assert_eq!(Message::decode(&bytes).unwrap(), one);

        // With its origin and identifiers.
        let mut r = report(8);
        r.origin_ms = Some(1_789_999_000_000);
        r.identifiers = Some(vec![("mmsi".into(), "235009876".into())]);
        r.alt_m = Some(-12.0);
        r.course_deg = None;
        let m = Message::new(site("BBB"), NOW, Body::Reports(vec![r]));
        assert_eq!(Message::decode(&m.encode()).unwrap(), m);
    }

    #[test]
    fn many_reports_split_into_messages_that_fit() {
        let rs: Vec<Report> = (1..=100).map(report).collect();
        let msgs = encode_reports(site("BBB"), NOW, &rs);
        assert_eq!(msgs.len(), 4);
        let mut back = Vec::new();
        for m in &msgs {
            assert!(m.len() <= MAX_UNSIGNED, "{}", m.len());
            match Message::decode(m).unwrap().body {
                Body::Reports(r) => back.extend(r),
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(back, rs);
    }

    #[test]
    fn every_other_kind_reads_back() {
        let entry = Entry {
            id: "AAA:3".parse().unwrap(),
            hlc: Hlc::new(5, 1),
            actor: "tm".into(),
            role: "track_manager".into(),
            command: json!({"op": "pair", "tracks": ["AAA000000001", "BBB000000002"]}),
        };
        let uid = Uid::new(site("AAA"), 1).unwrap();
        for body in [
            Body::Attrs(vec![(
                uid,
                json!({"name": "EVER GIVEN", "attributes": {"flag": "PA"}}),
            )]),
            Body::Decisions(vec![entry.clone(), entry]),
            Body::Summary(Summary {
                heads: vec![(site("AAA"), 12), (site("BBB"), 3)],
                reporting: 140,
            }),
            Body::Want(Want {
                decisions: vec![(site("CCC"), vec![(4, 9), (12, 12)])],
                snapshot: true,
            }),
            Body::Release(vec![uid, Uid::new(site("BBB"), 77).unwrap()]),
        ] {
            let m = Message::new(site("AAA"), NOW, body);
            assert_eq!(Message::decode(&m.encode()).unwrap(), m);
        }
    }

    #[test]
    fn damaged_messages_are_refused() {
        let bytes = Message::new(site("BBB"), NOW, Body::Reports(vec![report(7)])).encode();
        assert_eq!(Message::decode(&bytes[..20]), Err(WireError::Short));
        assert_eq!(Message::decode(b"XX"), Err(WireError::Magic));
        let mut v = bytes.clone();
        v[2] = 9;
        assert_eq!(Message::decode(&v), Err(WireError::Version(9)));
        let mut v = bytes.clone();
        v[3] = 42;
        assert_eq!(Message::decode(&v), Err(WireError::Kind(42)));
        let mut v = bytes;
        v.push(0);
        assert_eq!(Message::decode(&v), Err(WireError::Trailing(1)));
    }
}
