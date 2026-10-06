//! STANAG 4607 (Edition 3) Ground Moving Target Indicator format decoder.
//!
//! The byte layouts follow Annex A of the standard (packet header Table 2-1,
//! segment header Table 2-2, segments Tables 2-3 to 2-15 and 3-1/3-2) and the
//! data conventions of Annex B para 6 (big-endian; I/S integers, B16/B32/H32
//! sign-magnitude binary decimals, BA16/BA32 and SA16/SA32 binary angles).
//! The decoder does not branch on the packet version (P1); the reference
//! sample corpus is version "20" and decodes cleanly with these layouts.
//!
//! Source: STANAG 4607 Ed. 3 (AEDP-7, 2013), from the NSG Standards
//! Registry, https://nsgreg.nga.mil/ (see this crate's README, "Source").
//!
//! Two layers:
//! * [`parse_packet`] gives a typed [`Packet`] with one [`Segment`] variant per
//!   segment type, every field decoded (existence masks honoured) and
//!   converted to engineering units.
//! * [`Decoder`] is stateful over a stream: it remembers the latest mission
//!   segment (reference day, platform) and job definitions (by job id), and
//!   flattens packets into JSON records.
//!
//! # Records
//!
//! [`Options::emit`] selects the records:
//!
//! * `targets` (default): one `"record": "target"` object per dwell target
//!   report, e.g.
//!   ```json
//!   { "record": "target", "time": "2015-10-13T00:15:10.123Z",
//!     "time_basis": "mission_reference_date",
//!     "lat": -34.8, "lon": 138.5, "height_m": 12.0,
//!     "radial_velocity_mps": -4.2, "wrap_velocity_mps": 30.1, "snr_db": 14.0, "rcs_dbsm": 5.5,
//!     "classification": { "code": 2, "name": "Wheeled Vehicle, Live Target", "probability": 0.8 },
//!     "location_error": { "source": "target_report", "slant_range_m": 10.0, "cross_range_m": 25.0,
//!                         "height_m": 5.0, "radial_velocity_mps": 0.5 },
//!     "ground_error": { "semi_major_m", "semi_minor_m", "orientation_deg", "nn_m2", "ne_m2", "ee_m2",
//!                       "range_sigma_m", "cross_range_sigma_m", "slant_range_m", "ground_range_m",
//!                       "bearing_deg", "source", "radial_velocity_sigma_mps" },
//!     "truth_tag": { "application": 1, "entity": 42 },
//!     "report_index": 0, "mti_report_index": 7,
//!     "dwell": { "revisit_index": 1, "dwell_index": 3, "last_dwell_of_revisit": false,
//!                "time_ms": 910123, "target_count": 12, "mdv_mps": 1.5,
//!                "sensor": { "lat", "lon", "alt_m", "track_deg", "speed_mps", "vertical_velocity_mps",
//!                            "along_track_uncertainty_m", "cross_track_uncertainty_m", "alt_uncertainty_m",
//!                            "track_uncertainty_deg", "speed_uncertainty_mps",
//!                            "vertical_velocity_uncertainty_mps" },
//!                "area": { "center_lat", "center_lon", "range_half_extent_m", "dwell_angle_half_extent_deg" },
//!                "platform_orientation": { "heading_deg", "pitch_deg", "roll_deg" },
//!                "sensor_orientation": { "heading_deg", "pitch_deg", "roll_deg" } },
//!     "job_id": 5,
//!     "job": { ...the job definition for job_id, see below },
//!     "mission": { "plan", "flight_plan", "platform_type", "platform_type_code",
//!                  "platform_config", "reference_date" },
//!     "packet": { "version", "nationality", "classification", "classification_code",
//!                 "classification_system", "code", "code_names", "exercise", "exercise_indicator",
//!                 "exercise_indicator_code", "platform_id", "mission_id", "job_id" } }
//!   ```
//!   Fields that are absent from the data (existence mask bit clear, "No
//!   Statement", no mission/job seen) are omitted, never `null`.
//! * `segments`: one record per segment, `{ "record": "<segment name>",
//!   "packet": {...}, ...the typed segment's fields }` with the names
//!   `mission`, `dwell` (with a `targets` array), `hrr`, `range_doppler`,
//!   `job_definition`, `free_text`, `low_reflectivity_index`, `group`,
//!   `attached_target`, `test_and_status`, `system_specific`,
//!   `processing_history`, `platform_location`, `job_request`,
//!   `job_acknowledge` and `unknown`. Dwell, test-and-status and platform
//!   location records also carry `time`/`time_basis`.
//! * `all`: both, in stream order (each dwell record precedes its targets).
//!
//! In every mode a segment that cannot be decoded (bad size, truncated body,
//! mandatory field missing from the existence mask) is skipped and reported
//! as `{ "record": "warning", "segment_type", "segment_name", "bytes_len",
//! "error", "packet" }`; the rest of the packet still decodes.
//!
//! ## Conventions
//! * Longitudes are normalised to [-180, 180) (the standard transmits 0..360 east).
//! * Target location: high-resolution D32.2/D32.3 when present, else the
//!   reduced form `centre + delta x scale` (D24/D25, D32.4/D32.5, D10/D11,
//!   para 2.4.10-2.4.11). A target with neither has no `lat`/`lon`; so does
//!   one whose high-resolution latitude is exactly -90 (SA32 0x80000000),
//!   which producers use as a "no location" placeholder (seen with
//!   longitude 180 in real data).
//! * `classification.probability` is D32.11 as a fraction 0..1.
//! * `location_error`: the target report's measurement uncertainty
//!   (D32.12-D32.15, one standard deviation) with `"source": "target_report"`;
//!   when the report carries none, the job definition's nominal values with
//!   `"source": "job_nominal"` (`slant_range_m` from J21, `cross_range_deg`
//!   from J22 - an angle, not a distance - and `radial_velocity_mps` from J23),
//!   as para 2.7.16 directs.
//! * `ground_error`: the one-sigma error ellipse on the ground (metres,
//!   major axis in degrees clockwise from north, in [0, 180)) and its
//!   north/east covariance (m²), propagated to first order from the slant
//!   range and cross-range sigmas through the sensor-target geometry on a
//!   flat-earth tangent plane at the sensor: σ_g = σ_R / cos(grazing),
//!   σ_x = slant range × σ_θ (or the report's cross range in metres), rotated
//!   by the bearing. Each sigma comes from the report (D32.12/D32.13), else
//!   the job (J21/J22), else [`Options::range_sigma_m`] /
//!   [`Options::cross_range_sigma_deg`]; `source` names the least specific.
//!   Omitted when a sigma or the target position is unknown. See the README.
//! * `job`: the job definition segment fields (Table 2-7) for the packet's job
//!   id: `sensor_type`/`radar_mode`/... as `{code, name}`, `sensor_model`,
//!   `target_filtering_flag` and its bits, `radar_priority`, `bounding_area`
//!   and the `nominal_*` accuracies in SI units.
//!
//! ## Time basis
//! Dwell time D6 (and T4, L1) counts milliseconds from 00:00 UTC of the
//! mission reference day (para 2.4.6, Annex B para 5). With a mission segment
//! seen, `time` = reference day + ms and `time_basis` =
//! `"mission_reference_date"`. The packet header carries no date, and the
//! standard says the day is unresolved until a mission segment arrives, so
//! before one is seen the decoder takes the millisecond count modulo one day
//! as a UTC time of day and places it on the calendar day that puts it
//! nearest the decoder's current UTC clock (`time_basis` =
//! `"receipt_clock_time_of_day"`). That is right for live feeds; replayed
//! files normally lead with a mission segment (it is required at least every
//! two minutes, para 2.3).

#![forbid(unsafe_code)]

mod cursor;
pub mod enums;
mod ground_error;
mod records;
mod revisit;
pub mod segments;

use std::collections::HashMap;

pub use enums::Enumeration;
pub use revisit::{Revisit, RevisitSource};
pub use segments::*;

use cursor::Cursor;

/// Version of this codec's behaviour (what records it produces for the same bytes). Bump when output changes.
pub const VERSION: &str = "stanag4607-2";

/// Decoder options.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Options {
    /// Which records to emit: `targets` (default: one record per dwell target report), `segments` (one record per segment, targets not flattened), or `all` (both).
    pub emit: Emit,
    /// Also emit a `platform` record, the sensor platform's own position and
    /// motion, for every dwell and platform location segment.
    pub platform: bool,
    /// One-sigma slant range error (m) for target records' `ground_error`,
    /// used only when neither the target report (D32.12) nor the job
    /// definition (J21) gives one. Ignored unless positive.
    pub range_sigma_m: Option<f64>,
    /// One-sigma cross-range error (degrees of azimuth) for `ground_error`,
    /// used only when neither the report (D32.13) nor the job (J22) gives
    /// one. Ignored unless positive.
    pub cross_range_sigma_deg: Option<f64>,
}

/// Record selection, see [`Options::emit`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Emit {
    #[default]
    Targets,
    Segments,
    All,
}

/// Decode errors.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Input ended early: `need` bytes were required, `have` were available.
    #[error("truncated: need {need} bytes, have {have}")]
    Truncated { need: usize, have: usize },
    /// The packet header is invalid (e.g. a packet size below the header size).
    #[error("bad packet: {0}")]
    BadPacket(String),
    /// A segment body is invalid (reported as a warning record by [`Decoder`]).
    #[error("bad segment: {0}")]
    BadSegment(String),
}

/// One packet: header plus its segments in order.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Packet {
    pub header: PacketHeader,
    pub segments: Vec<Segment>,
}

/// For stream framing: the total size of the packet starting at `header` (the packet header's size field), or None if fewer header bytes than needed are present.
pub fn packet_len(header: &[u8]) -> Option<usize> {
    let b = header.get(2..6)?;
    let size = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    usize::try_from(size).ok()
}

/// Low-level: parse one packet at the start of `bytes`; returns it and the bytes consumed.
pub fn parse_packet(bytes: &[u8]) -> Result<(Packet, usize), Error> {
    let head = bytes.get(..PACKET_HEADER_LEN).ok_or(Error::Truncated {
        need: PACKET_HEADER_LEN,
        have: bytes.len(),
    })?;
    let header = PacketHeader::parse(&mut Cursor::new(head))?;
    let size = usize::try_from(header.packet_size)
        .map_err(|_| Error::BadPacket("packet size does not fit in memory".into()))?;
    if size < PACKET_HEADER_LEN {
        return Err(Error::BadPacket(format!(
            "packet size {size} is smaller than the {PACKET_HEADER_LEN}-byte packet header"
        )));
    }
    let body = bytes.get(PACKET_HEADER_LEN..size).ok_or(Error::Truncated {
        need: size,
        have: bytes.len(),
    })?;

    let mut segments = Vec::new();
    let mut rest = body;
    while !rest.is_empty() {
        let segment_type = rest.first().copied().unwrap_or(0);
        let Some(size_bytes) = rest.get(1..SEGMENT_HEADER_LEN) else {
            segments.push(Segment::Malformed {
                segment_type,
                bytes_len: rest.len(),
                error: format!(
                    "{} trailing bytes are too short for a segment header",
                    rest.len()
                ),
            });
            break;
        };
        let seg_size =
            u32::from_be_bytes([size_bytes[0], size_bytes[1], size_bytes[2], size_bytes[3]]);
        let seg_len = usize::try_from(seg_size).unwrap_or(usize::MAX);
        let Some(seg) = rest
            .get(..seg_len)
            .filter(|_| seg_len >= SEGMENT_HEADER_LEN)
        else {
            segments.push(Segment::Malformed {
                segment_type,
                bytes_len: rest.len(),
                error: format!(
                    "segment size {seg_size} is invalid ({} bytes remain in the packet)",
                    rest.len()
                ),
            });
            break;
        };
        let seg_body = seg.get(SEGMENT_HEADER_LEN..).unwrap_or_default();
        segments.push(Segment::parse(segment_type, seg_body).unwrap_or_else(|e| {
            Segment::Malformed {
                segment_type,
                bytes_len: seg_len,
                error: e.to_string(),
            }
        }));
        rest = rest.get(seg_len..).unwrap_or_default();
    }
    Ok((Packet { header, segments }, size))
}

/// Stateful decoder for one stream: remembers the latest mission segment (reference date, platform) and job definitions (by job id) so later dwells can be timed and described.
pub struct Decoder {
    options: Options,
    mission: Option<MissionSegment>,
    jobs: HashMap<u32, JobDefinitionSegment>,
    revisits: revisit::Revisits,
}

impl Decoder {
    pub fn new(options: Options) -> Self {
        Self {
            options,
            mission: None,
            jobs: HashMap::new(),
            revisits: Default::default(),
        }
    }

    /// How often the sensor comes back to the same ground: the revisit
    /// period of the jobs dwelling now (the longest, when several interleave),
    /// measured from their dwells, or the job's nominal revisit interval (J15)
    /// until a whole revisit has been seen. None before any dwell.
    pub fn revisit(&self) -> Option<Revisit> {
        self.revisits.current(&self.jobs)
    }

    /// Decode one or more whole packets laid back to back in `bytes`. A trailing partial packet is an Err (Truncated). Returns the records in stream order.
    pub fn decode(&mut self, bytes: &[u8]) -> Result<Vec<serde_json::Value>, Error> {
        let mut out = Vec::new();
        let mut rest = bytes;
        while !rest.is_empty() {
            let (packet, used) = parse_packet(rest)?;
            self.packet_records(&packet, &mut out);
            rest = rest.get(used..).unwrap_or_default();
        }
        Ok(out)
    }

    fn packet_records(&mut self, packet: &Packet, out: &mut Vec<serde_json::Value>) {
        let emit = self.options.emit;
        let segs = matches!(emit, Emit::Segments | Emit::All);
        let targets = matches!(emit, Emit::Targets | Emit::All);
        let pkt = records::packet_json(&packet.header);
        for seg in &packet.segments {
            match seg {
                Segment::Mission(m) => self.mission = Some(m.clone()),
                Segment::Dwell(d) => self.revisits.dwell(packet.header.job_id, d),
                Segment::JobDefinition(j) => {
                    self.jobs.insert(j.job_id, j.clone());
                }
                _ => {}
            }
            let ctx = records::Context {
                mission: self.mission.as_ref(),
                job: self.jobs.get(&packet.header.job_id),
                packet: &pkt,
                job_id: packet.header.job_id,
                range_sigma_m: self.options.range_sigma_m,
                cross_range_sigma_deg: self.options.cross_range_sigma_deg,
            };
            if let Segment::Malformed { .. } = seg {
                out.push(records::segment_record(seg, &ctx));
                continue;
            }
            if segs {
                out.push(records::segment_record(seg, &ctx));
            }
            if self.options.platform
                && let Some(r) = records::platform_record(seg, &ctx)
            {
                out.push(r);
            }
            if targets && let Segment::Dwell(d) = seg {
                for t in &d.targets {
                    out.push(records::target_record(d, t, &ctx));
                }
            }
        }
    }
}
