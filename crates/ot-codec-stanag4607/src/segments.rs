//! Typed segment structures and their parsers (Annex A, Parts 2 and 3).
//!
//! Values are converted to engineering units where the standard defines a
//! scaling: angles in degrees, distances in metres, speeds in m/s, powers in
//! dB. Longitudes (BA32, transmitted as 0..360 degrees east) are normalised
//! to [-180, 180). "No Statement" values of mandatory fields become `None`.
//! Existence masks (Dwell D1, HRR H1) are honoured bit by bit.
//!
//! Source: STANAG 4607 Ed. 3 (AEDP-7, 2013), from the NSG Standards
//! Registry, https://nsgreg.nga.mil/ (see this crate's README, "Source").

use serde::Serialize;

use crate::Error;
use crate::cursor::{Cursor, norm_lon, text};
use crate::enums::{self, Enumeration};

/// Packet header, Table 2-1 (32 bytes).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PacketHeader {
    /// P1, "mn": edition m, amendment n.
    pub version: String,
    /// P2, total packet size in bytes including this header.
    pub packet_size: u32,
    /// P3, FIPS 10-4 digraph.
    pub nationality: String,
    /// P4, Table 2-1.1.
    pub classification: Enumeration,
    /// P5, digraph (empty when all spaces: no system applies).
    pub classification_system: String,
    /// P6, Table 2-1.3 flag field.
    pub code: u16,
    /// P6 decoded to the names of the set bits.
    pub code_names: Vec<&'static str>,
    /// P7, Table 2-1.4.
    pub exercise_indicator: Enumeration,
    /// P8.
    pub platform_id: String,
    /// P9.
    pub mission_id: u32,
    /// P10 (0 = no specific job).
    pub job_id: u32,
}

/// Size of the packet header (Table 2-1).
pub const PACKET_HEADER_LEN: usize = 32;
/// Size of the segment header (Table 2-2).
pub const SEGMENT_HEADER_LEN: usize = 5;

impl PacketHeader {
    pub(crate) fn parse(c: &mut Cursor<'_>) -> Result<Self, Error> {
        let version = c.text(2)?;
        let packet_size = c.u32()?;
        let nationality = c.text(2)?;
        let classification = Enumeration::new(c.u8()?, enums::classification_name);
        let classification_system = c.text(2)?.trim().to_string();
        let code = c.u16()?;
        let exercise_indicator = Enumeration::new(c.u8()?, enums::exercise_indicator_name);
        let platform_id = c.text(10)?;
        let mission_id = c.u32()?;
        let job_id = c.u32()?;
        Ok(Self {
            version,
            packet_size,
            nationality,
            classification,
            classification_system,
            code,
            code_names: enums::security_code_names(code),
            exercise_indicator,
            platform_id,
            mission_id,
            job_id,
        })
    }

    /// True for exercise data (P7 values 128..=255, Table 2-1.4).
    pub fn is_exercise(&self) -> bool {
        self.exercise_indicator.code >= 128
    }
}

/// One decoded segment. Every Edition 3 segment type (Table 2-2) has a
/// variant; the types Edition 3 leaves undefined carry only their size.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum Segment {
    /// Type 1, Table 2-3.
    Mission(MissionSegment),
    /// Type 2, Table 2-4 with Table 2-4.1 target reports.
    Dwell(DwellSegment),
    /// Type 3, Table 2-5 with Table 2-5.1 scatterer records (HRR and range-Doppler maps).
    Hrr(HrrSegment),
    /// Type 4: listed as Range-Doppler in earlier editions; "Reserved" (para 2.6)
    /// in Edition 3, whose HRR segment carries range-Doppler maps. No layout defined.
    RangeDoppler(ReservedSegment),
    /// Type 5, Table 2-7.
    JobDefinition(JobDefinitionSegment),
    /// Type 6, Table 2-8.
    FreeText(FreeTextSegment),
    /// Type 7, para 2.9: reserved, no layout defined.
    LowReflectivityIndex(ReservedSegment),
    /// Type 8, para 2.10: reserved, no layout defined.
    Group(ReservedSegment),
    /// Type 9, para 2.11: reserved, no layout defined.
    AttachedTarget(ReservedSegment),
    /// Type 10, Table 2-12.
    TestAndStatus(TestAndStatusSegment),
    /// Type 11, para 2.13: reserved, no layout defined.
    SystemSpecific(ReservedSegment),
    /// Type 12, Table 2-14.
    ProcessingHistory(ProcessingHistorySegment),
    /// Type 13, Table 2-15.
    PlatformLocation(PlatformLocationSegment),
    /// Type 101, Table 3-1.
    JobRequest(JobRequestSegment),
    /// Type 102, Table 3-2.
    JobAcknowledge(JobAcknowledgeSegment),
    /// Reserved (14-100, 103-127) or extension (128-255) types.
    Unknown { segment_type: u8, bytes_len: usize },
    /// A segment that could not be decoded (bad size, truncated body, missing
    /// mandatory field). It is skipped; the decoder emits a warning record.
    Malformed {
        segment_type: u8,
        bytes_len: usize,
        error: String,
    },
}

impl Segment {
    /// Segment type code (S1).
    pub fn segment_type(&self) -> u8 {
        match self {
            Segment::Mission(_) => 1,
            Segment::Dwell(_) => 2,
            Segment::Hrr(_) => 3,
            Segment::RangeDoppler(_) => 4,
            Segment::JobDefinition(_) => 5,
            Segment::FreeText(_) => 6,
            Segment::LowReflectivityIndex(_) => 7,
            Segment::Group(_) => 8,
            Segment::AttachedTarget(_) => 9,
            Segment::TestAndStatus(_) => 10,
            Segment::SystemSpecific(_) => 11,
            Segment::ProcessingHistory(_) => 12,
            Segment::PlatformLocation(_) => 13,
            Segment::JobRequest(_) => 101,
            Segment::JobAcknowledge(_) => 102,
            Segment::Unknown { segment_type, .. } | Segment::Malformed { segment_type, .. } => {
                *segment_type
            }
        }
    }

    /// Record name (snake_case segment name; `warning` for malformed segments).
    pub fn name(&self) -> &'static str {
        match self {
            Segment::Unknown { .. } => "unknown",
            Segment::Malformed { .. } => "warning",
            s => enums::segment_type_name(s.segment_type()),
        }
    }

    /// Segment size (S2) in bytes, including the 5-byte segment header.
    pub fn segment_size(&self) -> usize {
        match self {
            Segment::Mission(s) => s.segment_size,
            Segment::Dwell(s) => s.segment_size,
            Segment::Hrr(s) => s.segment_size,
            Segment::JobDefinition(s) => s.segment_size,
            Segment::FreeText(s) => s.segment_size,
            Segment::TestAndStatus(s) => s.segment_size,
            Segment::ProcessingHistory(s) => s.segment_size,
            Segment::PlatformLocation(s) => s.segment_size,
            Segment::JobRequest(s) => s.segment_size,
            Segment::JobAcknowledge(s) => s.segment_size,
            Segment::RangeDoppler(s)
            | Segment::LowReflectivityIndex(s)
            | Segment::Group(s)
            | Segment::AttachedTarget(s)
            | Segment::SystemSpecific(s) => s.segment_size,
            Segment::Unknown { bytes_len, .. } | Segment::Malformed { bytes_len, .. } => *bytes_len,
        }
    }

    /// Parse a segment body (`body` excludes the 5-byte segment header).
    pub(crate) fn parse(segment_type: u8, body: &[u8]) -> Result<Segment, Error> {
        let size = body.len() + SEGMENT_HEADER_LEN;
        let mut c = Cursor::new(body);
        let reserved = || ReservedSegment { segment_size: size };
        Ok(match segment_type {
            1 => Segment::Mission(MissionSegment::parse(&mut c, size)?),
            2 => Segment::Dwell(DwellSegment::parse(&mut c, size)?),
            3 => Segment::Hrr(HrrSegment::parse(&mut c, size)?),
            4 => Segment::RangeDoppler(reserved()),
            5 => Segment::JobDefinition(JobDefinitionSegment::parse(&mut c, size)?),
            6 => Segment::FreeText(FreeTextSegment::parse(&mut c, size)?),
            7 => Segment::LowReflectivityIndex(reserved()),
            8 => Segment::Group(reserved()),
            9 => Segment::AttachedTarget(reserved()),
            10 => Segment::TestAndStatus(TestAndStatusSegment::parse(&mut c, size)?),
            11 => Segment::SystemSpecific(reserved()),
            12 => Segment::ProcessingHistory(ProcessingHistorySegment::parse(&mut c, size)?),
            13 => Segment::PlatformLocation(PlatformLocationSegment::parse(&mut c, size)?),
            101 => Segment::JobRequest(JobRequestSegment::parse(&mut c, size)?),
            102 => Segment::JobAcknowledge(JobAcknowledgeSegment::parse(&mut c, size)?),
            t => Segment::Unknown {
                segment_type: t,
                bytes_len: size,
            },
        })
    }
}

/// A segment type Edition 3 reserves without defining a layout.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReservedSegment {
    pub segment_size: usize,
}

/// Mission segment, Table 2-3 (para 2.3).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MissionSegment {
    pub segment_size: usize,
    /// M1.
    pub mission_plan: String,
    /// M2.
    pub flight_plan: String,
    /// M3, Table 2-3.1.
    pub platform_type: Enumeration,
    /// M4.
    pub platform_configuration: String,
    /// M5-M7, reference day (UTC).
    pub year: u16,
    pub month: u8,
    pub day: u8,
    /// M5-M7 as `YYYY-MM-DD`, when a valid calendar date.
    pub reference_date: Option<String>,
}

impl MissionSegment {
    fn parse(c: &mut Cursor<'_>, segment_size: usize) -> Result<Self, Error> {
        let mission_plan = c.text(12)?;
        let flight_plan = c.text(12)?;
        let platform_type = Enumeration::new(c.u8()?, enums::platform_type_name);
        let platform_configuration = c.text(10)?;
        let year = c.u16()?;
        let month = c.u8()?;
        let day = c.u8()?;
        let reference_date =
            chrono::NaiveDate::from_ymd_opt(i32::from(year), u32::from(month), u32::from(day))
                .map(|d| d.format("%Y-%m-%d").to_string());
        Ok(Self {
            segment_size,
            mission_plan,
            flight_plan,
            platform_type,
            platform_configuration,
            year,
            month,
            day,
            reference_date,
        })
    }

    /// The reference day, if valid.
    pub fn reference_day(&self) -> Option<chrono::NaiveDate> {
        chrono::NaiveDate::from_ymd_opt(
            i32::from(self.year),
            u32::from(self.month),
            u32::from(self.day),
        )
    }
}

/// Bit test for an existence mask left-aligned in a u64: index 0 is the first
/// field after the mask (bit 7 of the high-order byte).
fn has(mask: u64, index: u32) -> bool {
    index < 64 && mask & (1u64 << (63 - index)) != 0
}

/// Dwell segment, Table 2-4 (para 2.4); existence mask per Figure 2-1.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DwellSegment {
    pub segment_size: usize,
    /// D1 as 16 hex digits.
    pub existence_mask: String,
    /// D2.
    pub revisit_index: u16,
    /// D3.
    pub dwell_index: u16,
    /// D4.
    pub last_dwell_of_revisit: bool,
    /// D5.
    pub target_report_count: u16,
    /// D6, ms after 00:00 UTC of the mission reference day.
    pub dwell_time_ms: u32,
    /// D7 (SA32).
    pub sensor_lat: f64,
    /// D8 (BA32), normalised.
    pub sensor_lon: f64,
    /// D9 (S32 cm), metres above the WGS 84 ellipsoid.
    pub sensor_alt_m: f64,
    /// D10 (SA32), degrees per Delta Lat unit.
    pub lat_scale_deg: Option<f64>,
    /// D11 (BA32), degrees per Delta Long unit.
    pub lon_scale_deg: Option<f64>,
    /// D12 (I32 cm).
    pub sensor_along_track_uncertainty_m: Option<f64>,
    /// D13 (I32 cm).
    pub sensor_cross_track_uncertainty_m: Option<f64>,
    /// D14 (I16 cm).
    pub sensor_alt_uncertainty_m: Option<f64>,
    /// D15 (BA16).
    pub sensor_track_deg: Option<f64>,
    /// D16 (I32 mm/s).
    pub sensor_speed_mps: Option<f64>,
    /// D17 (S8 dm/s).
    pub sensor_vertical_velocity_mps: Option<f64>,
    /// D18 (I8 deg).
    pub sensor_track_uncertainty_deg: Option<f64>,
    /// D19 (I16 mm/s).
    pub sensor_speed_uncertainty_mps: Option<f64>,
    /// D20 (I16 cm/s).
    pub sensor_vertical_velocity_uncertainty_mps: Option<f64>,
    /// D21 (BA16).
    pub platform_heading_deg: Option<f64>,
    /// D22 (SA16).
    pub platform_pitch_deg: Option<f64>,
    /// D23 (SA16).
    pub platform_roll_deg: Option<f64>,
    /// D24 (SA32).
    pub center_lat: f64,
    /// D25 (BA32), normalised.
    pub center_lon: f64,
    /// D26 (B16 km), metres.
    pub range_half_extent_m: f64,
    /// D27 (BA16).
    pub dwell_angle_half_extent_deg: f64,
    /// D28 (BA16). If any of D28-D30 is present the omitted ones are 0 (para 2.4.28).
    pub sensor_heading_deg: Option<f64>,
    /// D29 (SA16).
    pub sensor_pitch_deg: Option<f64>,
    /// D30 (SA16).
    pub sensor_roll_deg: Option<f64>,
    /// D31 (I8 dm/s).
    pub mdv_mps: Option<f64>,
    /// D32, Table 2-4.1.
    pub targets: Vec<TargetReport>,
}

/// Target report, Table 2-4.1 (para 2.4.32).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TargetReport {
    /// Position of this report within its dwell segment (0-based).
    pub report_index: usize,
    /// D32.1.
    pub mti_report_index: Option<u16>,
    /// Resolved target latitude: D32.2, else D24 + D32.4 x D10. `None` when
    /// neither form is present or D32.2 is the -90 degree placeholder.
    pub lat: Option<f64>,
    /// Resolved target longitude (normalised): D32.3, else D25 + D32.5 x D11.
    pub lon: Option<f64>,
    /// D32.2 (SA32).
    pub hi_res_lat: Option<f64>,
    /// D32.3 (BA32), normalised.
    pub hi_res_lon: Option<f64>,
    /// D32.4 (S16, units of D10).
    pub delta_lat: Option<i16>,
    /// D32.5 (S16, units of D11).
    pub delta_lon: Option<i16>,
    /// D32.6 (S16 m), geodetic height above WGS 84.
    pub height_m: Option<f64>,
    /// D32.7 (S16 cm/s), positive = range increasing.
    pub radial_velocity_mps: Option<f64>,
    /// D32.8 (I16 cm/s).
    pub wrap_velocity_mps: Option<f64>,
    /// D32.9 (S8 dB).
    pub snr_db: Option<f64>,
    /// D32.10, Table 2-4.2.
    pub classification: Option<Enumeration>,
    /// D32.11 (I8 percent).
    pub classification_probability_pct: Option<u8>,
    /// D32.12 (I16 cm).
    pub slant_range_uncertainty_m: Option<f64>,
    /// D32.13 (I16 dm).
    pub cross_range_uncertainty_m: Option<f64>,
    /// D32.14 (I8 m).
    pub height_uncertainty_m: Option<f64>,
    /// D32.15 (I16 cm/s).
    pub radial_velocity_uncertainty_mps: Option<f64>,
    /// D32.16.
    pub truth_tag_application: Option<u8>,
    /// D32.17.
    pub truth_tag_entity: Option<u32>,
    /// D32.18 (S8 dB/2), dBsm.
    pub rcs_dbsm: Option<f64>,
}

fn opt<T>(present: bool, f: impl FnOnce() -> Result<T, Error>) -> Result<Option<T>, Error> {
    if present { f().map(Some) } else { Ok(None) }
}

fn mandatory<T>(
    present: bool,
    field: &str,
    f: impl FnOnce() -> Result<T, Error>,
) -> Result<T, Error> {
    if present {
        f()
    } else {
        Err(Error::BadSegment(format!(
            "mandatory field {field} absent from existence mask"
        )))
    }
}

impl DwellSegment {
    fn parse(c: &mut Cursor<'_>, segment_size: usize) -> Result<Self, Error> {
        let m = c.mask(8)?;
        // Field Dn (n = 2..=31) is mask index n - 2; D32.k is index 29 + k.
        let d = |n: u32| has(m, n - 2);
        let revisit_index = mandatory(d(2), "D2", || c.u16())?;
        let dwell_index = mandatory(d(3), "D3", || c.u16())?;
        let last_dwell_of_revisit = mandatory(d(4), "D4", || c.u8())? != 0;
        let target_report_count = mandatory(d(5), "D5", || c.u16())?;
        let dwell_time_ms = mandatory(d(6), "D6", || c.u32())?;
        let sensor_lat = mandatory(d(7), "D7", || c.sa32())?;
        let sensor_lon = norm_lon(mandatory(d(8), "D8", || c.ba32())?);
        let sensor_alt_m = f64::from(mandatory(d(9), "D9", || c.i32())?) / 100.0;
        let lat_scale_deg = opt(d(10), || c.sa32())?;
        let lon_scale_deg = opt(d(11), || c.ba32())?;
        let cm = |v: u32| f64::from(v) / 100.0;
        let sensor_along_track_uncertainty_m = opt(d(12), || c.u32().map(cm))?;
        let sensor_cross_track_uncertainty_m = opt(d(13), || c.u32().map(cm))?;
        let sensor_alt_uncertainty_m = opt(d(14), || c.u16().map(|v| f64::from(v) / 100.0))?;
        let sensor_track_deg = opt(d(15), || c.ba16())?;
        let sensor_speed_mps = opt(d(16), || c.u32().map(|v| f64::from(v) / 1000.0))?;
        let sensor_vertical_velocity_mps = opt(d(17), || c.i8().map(|v| f64::from(v) / 10.0))?;
        let sensor_track_uncertainty_deg = opt(d(18), || c.u8().map(f64::from))?;
        let sensor_speed_uncertainty_mps = opt(d(19), || c.u16().map(|v| f64::from(v) / 1000.0))?;
        let sensor_vertical_velocity_uncertainty_mps =
            opt(d(20), || c.u16().map(|v| f64::from(v) / 100.0))?;
        let platform_heading_deg = opt(d(21), || c.ba16())?;
        let platform_pitch_deg = opt(d(22), || c.sa16())?;
        let platform_roll_deg = opt(d(23), || c.sa16())?;
        let center_lat = mandatory(d(24), "D24", || c.sa32())?;
        let center_lon_raw = mandatory(d(25), "D25", || c.ba32())?;
        let range_half_extent_m = mandatory(d(26), "D26", || c.b16())? * 1000.0;
        let dwell_angle_half_extent_deg = mandatory(d(27), "D27", || c.ba16())?;
        let mut sensor_heading_deg = opt(d(28), || c.ba16())?;
        let mut sensor_pitch_deg = opt(d(29), || c.sa16())?;
        let mut sensor_roll_deg = opt(d(30), || c.sa16())?;
        if sensor_heading_deg.is_some() || sensor_pitch_deg.is_some() || sensor_roll_deg.is_some() {
            // Para 2.4.28-2.4.30: omitted orientation angles mean zero.
            sensor_heading_deg.get_or_insert(0.0);
            sensor_pitch_deg.get_or_insert(0.0);
            sensor_roll_deg.get_or_insert(0.0);
        }
        let mdv_mps = opt(d(31), || c.u8().map(|v| f64::from(v) / 10.0))?;

        let t = |k: u32| has(m, 29 + k);
        let mut targets = Vec::with_capacity(usize::from(target_report_count).min(c.remaining()));
        for report_index in 0..usize::from(target_report_count) {
            let mti_report_index = opt(t(1), || c.u16())?;
            let hi_res_lat = opt(t(2), || c.sa32())?;
            let hi_res_lon = opt(t(3), || c.ba32())?;
            let delta_lat = opt(t(4), || c.i16())?;
            let delta_lon = opt(t(5), || c.i16())?;
            let height_m = opt(t(6), || c.i16().map(f64::from))?;
            let radial_velocity_mps = opt(t(7), || c.i16().map(|v| f64::from(v) / 100.0))?;
            let wrap_velocity_mps = opt(t(8), || c.u16().map(|v| f64::from(v) / 100.0))?;
            let snr_db = opt(t(9), || c.i8().map(f64::from))?;
            let classification = opt(t(10), || {
                c.u8()
                    .map(|v| Enumeration::new(v, enums::target_classification_name))
            })?;
            let classification_probability_pct = opt(t(11), || c.u8())?;
            let slant_range_uncertainty_m = opt(t(12), || c.u16().map(|v| f64::from(v) / 100.0))?;
            let cross_range_uncertainty_m = opt(t(13), || c.u16().map(|v| f64::from(v) / 10.0))?;
            let height_uncertainty_m = opt(t(14), || c.u8().map(f64::from))?;
            let radial_velocity_uncertainty_mps =
                opt(t(15), || c.u16().map(|v| f64::from(v) / 100.0))?;
            let truth_tag_application = opt(t(16), || c.u8())?;
            let truth_tag_entity = opt(t(17), || c.u32())?;
            let rcs_dbsm = opt(t(18), || c.i8().map(|v| f64::from(v) / 2.0))?;

            // Location: high-resolution form, else the reduced-bandwidth delta
            // form (para 2.4.10/2.4.11): value = delta x scale + centre.
            // A high-resolution latitude of exactly -90 (SA32 0x80000000) is
            // not a feasible GMTI detection; producers use it as a "no
            // location" placeholder, so it resolves to no location.
            let (lat, lon) = match (hi_res_lat, hi_res_lon, delta_lat, delta_lon) {
                (Some(la), Some(_), _, _) if la <= -90.0 => (None, None),
                (Some(la), Some(lo), _, _) => (Some(la), Some(lo)),
                (_, _, Some(dla), Some(dlo)) => match (lat_scale_deg, lon_scale_deg) {
                    (Some(ls), Some(os)) => (
                        Some(f64::from(dla) * ls + center_lat),
                        Some(f64::from(dlo) * os + center_lon_raw),
                    ),
                    _ => (None, None),
                },
                _ => (None, None),
            };
            targets.push(TargetReport {
                report_index,
                mti_report_index,
                lat,
                lon: lon.map(norm_lon),
                hi_res_lat,
                hi_res_lon: hi_res_lon.map(norm_lon),
                delta_lat,
                delta_lon,
                height_m,
                radial_velocity_mps,
                wrap_velocity_mps,
                snr_db,
                classification,
                classification_probability_pct,
                slant_range_uncertainty_m,
                cross_range_uncertainty_m,
                height_uncertainty_m,
                radial_velocity_uncertainty_mps,
                truth_tag_application,
                truth_tag_entity,
                rcs_dbsm,
            });
        }
        Ok(Self {
            segment_size,
            existence_mask: format!("{m:016x}"),
            revisit_index,
            dwell_index,
            last_dwell_of_revisit,
            target_report_count,
            dwell_time_ms,
            sensor_lat,
            sensor_lon,
            sensor_alt_m,
            lat_scale_deg,
            lon_scale_deg,
            sensor_along_track_uncertainty_m,
            sensor_cross_track_uncertainty_m,
            sensor_alt_uncertainty_m,
            sensor_track_deg,
            sensor_speed_mps,
            sensor_vertical_velocity_mps,
            sensor_track_uncertainty_deg,
            sensor_speed_uncertainty_mps,
            sensor_vertical_velocity_uncertainty_mps,
            platform_heading_deg,
            platform_pitch_deg,
            platform_roll_deg,
            center_lat,
            center_lon: norm_lon(center_lon_raw),
            range_half_extent_m,
            dwell_angle_half_extent_deg,
            sensor_heading_deg,
            sensor_pitch_deg,
            sensor_roll_deg,
            mdv_mps,
            targets,
        })
    }
}

/// HRR segment, Table 2-5 (para 2.5); existence mask per Figure 2-5.
/// All fields honour the mask, so each is optional.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HrrSegment {
    pub segment_size: usize,
    /// H1 as 10 hex digits.
    pub existence_mask: String,
    /// H2.
    pub revisit_index: Option<u16>,
    /// H3.
    pub dwell_index: Option<u16>,
    /// H4.
    pub last_dwell_of_revisit: Option<bool>,
    /// H5.
    pub mti_report_index: Option<u16>,
    /// H6.
    pub num_target_scatterers: Option<u16>,
    /// H7.
    pub num_range_samples: Option<u16>,
    /// H8.
    pub num_doppler_samples: Option<u16>,
    /// H9 (I8 -dB/4), dB relative to peak scatterer (<= 0).
    pub mean_clutter_power_db: Option<f64>,
    /// H10 (I8 -dB/4), dB relative to peak scatterer (<= 0).
    pub detection_threshold_db: Option<f64>,
    /// H11 (B16 cm), metres; 0 = no statement.
    pub range_resolution_m: Option<f64>,
    /// H12 (B16 cm), metres; 0 = no statement.
    pub range_bin_spacing_m: Option<f64>,
    /// H13 (H32 Hz).
    pub doppler_resolution_hz: Option<f64>,
    /// H14 (H32 Hz).
    pub doppler_bin_spacing_hz: Option<f64>,
    /// H15 (B32 GHz).
    pub center_frequency_ghz: Option<f64>,
    /// H16.
    pub compression_flag: Option<Enumeration>,
    /// H17.
    pub range_weighting: Option<Enumeration>,
    /// H18.
    pub doppler_weighting: Option<Enumeration>,
    /// H19 (B16 dB).
    pub maximum_pixel_power_db: Option<f64>,
    /// H20 (S8 dB/2), dBsm.
    pub maximum_rcs_dbsm: Option<f64>,
    /// H21 (S16 m).
    pub range_of_origin_m: Option<f64>,
    /// H22 (H32 Hz).
    pub doppler_of_origin_hz: Option<f64>,
    /// H23.
    pub hrr_type: Option<Enumeration>,
    /// H24, Figure 2-5.1 flag byte.
    pub processing_mask: Option<u8>,
    /// H24 decoded to the names of the set bits.
    pub processing_applied: Vec<&'static str>,
    /// H25 (1 or 2).
    pub magnitude_bytes: Option<u8>,
    /// H26 (0, 1 or 2).
    pub phase_bytes: Option<u8>,
    /// H27.
    pub range_extent_pixels: Option<u8>,
    /// H28 (I32 cm), metres.
    pub range_to_nearest_edge_m: Option<f64>,
    /// H29.
    pub index_of_zero_velocity_bin: Option<u8>,
    /// H30 (B32 m).
    pub target_radial_electrical_length_m: Option<f64>,
    /// H31 (B32 m).
    pub electrical_length_uncertainty_m: Option<f64>,
    /// H32, Table 2-5.1, as many whole records as the segment holds.
    pub scatterers: Vec<Scatterer>,
}

/// HRR scatterer record, Table 2-5.1.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Scatterer {
    /// H32.1, quarter-dB relative to peak scatterer, as transmitted.
    pub magnitude: u16,
    /// H32.2, quantised rotation as transmitted.
    pub phase: Option<u16>,
    /// H32.2 in degrees (phase x 360 / 2^(8 x H26)).
    pub phase_deg: Option<f64>,
    /// H32.3.
    pub range_index: Option<u16>,
    /// H32.4.
    pub doppler_index: Option<u16>,
}

impl HrrSegment {
    fn parse(c: &mut Cursor<'_>, segment_size: usize) -> Result<Self, Error> {
        let m = c.mask(5)?;
        // Field Hn (n = 2..=31) is mask index n - 2; H32.k is index 29 + k.
        let h = |n: u32| has(m, n - 2);
        let neg_quarter_db = |v: u8| -f64::from(v) / 4.0;
        let revisit_index = opt(h(2), || c.u16())?;
        let dwell_index = opt(h(3), || c.u16())?;
        let last_dwell_of_revisit = opt(h(4), || c.u8().map(|v| v != 0))?;
        let mti_report_index = opt(h(5), || c.u16())?;
        let num_target_scatterers = opt(h(6), || c.u16())?;
        let num_range_samples = opt(h(7), || c.u16())?;
        let num_doppler_samples = opt(h(8), || c.u16())?;
        let mean_clutter_power_db = opt(h(9), || c.u8().map(neg_quarter_db))?;
        let detection_threshold_db = opt(h(10), || c.u8().map(neg_quarter_db))?;
        let cm_to_m = |v: f64| if v == 0.0 { None } else { Some(v / 100.0) };
        let range_resolution_m = opt(h(11), || c.b16())?.and_then(cm_to_m);
        let range_bin_spacing_m = opt(h(12), || c.b16())?.and_then(cm_to_m);
        let doppler_resolution_hz = opt(h(13), || c.h32())?;
        let doppler_bin_spacing_hz = opt(h(14), || c.h32())?;
        let center_frequency_ghz = opt(h(15), || c.b32())?;
        let compression_flag = opt(h(16), || {
            c.u8().map(|v| Enumeration::new(v, enums::compression_name))
        })?;
        let range_weighting = opt(h(17), || {
            c.u8().map(|v| Enumeration::new(v, enums::weighting_name))
        })?;
        let doppler_weighting = opt(h(18), || {
            c.u8().map(|v| Enumeration::new(v, enums::weighting_name))
        })?;
        let maximum_pixel_power_db = opt(h(19), || c.b16())?;
        let maximum_rcs_dbsm = opt(h(20), || c.i8().map(|v| f64::from(v) / 2.0))?;
        let range_of_origin_m = opt(h(21), || c.i16().map(f64::from))?;
        let doppler_of_origin_hz = opt(h(22), || c.h32())?;
        let hrr_type = opt(h(23), || {
            c.u8().map(|v| Enumeration::new(v, enums::hrr_type_name))
        })?;
        let processing_mask = opt(h(24), || c.u8())?;
        let magnitude_bytes = opt(h(25), || c.u8())?;
        let phase_bytes = opt(h(26), || c.u8())?;
        let range_extent_pixels = opt(h(27), || c.u8())?;
        let range_to_nearest_edge_m = opt(h(28), || c.u32().map(|v| f64::from(v) / 100.0))?;
        let index_of_zero_velocity_bin = opt(h(29), || c.u8())?;
        let target_radial_electrical_length_m = opt(h(30), || c.b32())?;
        let electrical_length_uncertainty_m = opt(h(31), || c.b32())?;

        let mut processing_applied = Vec::new();
        if let Some(p) = processing_mask {
            for (bit, name) in [
                (0x80u8, "Clutter Cancellation"),
                (0x40, "Single-Ambiguity Keystoning"),
                (0x20, "Multi-Ambiguity Keystoning"),
            ] {
                if p & bit != 0 {
                    processing_applied.push(name);
                }
            }
        }

        // Scatterer records (Table 2-5.1). H32.1 width per H25 (default 1 when
        // H25 absent); H32.2 per H26; H32.3/H32.4 are two bytes (I16) each.
        let mag_n = usize::from(magnitude_bytes.unwrap_or(1));
        if !(1..=2).contains(&mag_n) {
            return Err(Error::BadSegment(format!(
                "HRR magnitude byte count H25={mag_n} (must be 1 or 2)"
            )));
        }
        let phase_n = if has(m, 31) {
            usize::from(phase_bytes.unwrap_or(0))
        } else {
            0
        };
        if phase_n > 2 {
            return Err(Error::BadSegment(format!(
                "HRR phase byte count H26={phase_n} (must be 0, 1 or 2)"
            )));
        }
        let (has_range, has_doppler) = (has(m, 32), has(m, 33));
        let record_len = if has(m, 30) { mag_n } else { 0 }
            + phase_n
            + if has_range { 2 } else { 0 }
            + if has_doppler { 2 } else { 0 };
        let read_n = |c: &mut Cursor<'_>, n: usize| -> Result<u16, Error> {
            if n == 2 {
                c.u16()
            } else {
                c.u8().map(u16::from)
            }
        };
        let mut scatterers = Vec::new();
        if record_len > 0 {
            while c.remaining() >= record_len {
                let magnitude = if has(m, 30) { read_n(c, mag_n)? } else { 0 };
                let phase = if phase_n > 0 {
                    Some(read_n(c, phase_n)?)
                } else {
                    None
                };
                let range_index = opt(has_range, || c.u16())?;
                let doppler_index = opt(has_doppler, || c.u16())?;
                let full = if phase_n == 2 { 65_536.0 } else { 256.0 };
                scatterers.push(Scatterer {
                    magnitude,
                    phase,
                    phase_deg: phase.map(|p| f64::from(p) * 360.0 / full),
                    range_index,
                    doppler_index,
                });
            }
        }
        Ok(Self {
            segment_size,
            existence_mask: format!("{:010x}", m >> 24),
            revisit_index,
            dwell_index,
            last_dwell_of_revisit,
            mti_report_index,
            num_target_scatterers,
            num_range_samples,
            num_doppler_samples,
            mean_clutter_power_db,
            detection_threshold_db,
            range_resolution_m,
            range_bin_spacing_m,
            doppler_resolution_hz,
            doppler_bin_spacing_hz,
            center_frequency_ghz,
            compression_flag,
            range_weighting,
            doppler_weighting,
            maximum_pixel_power_db,
            maximum_rcs_dbsm,
            range_of_origin_m,
            doppler_of_origin_hz,
            hrr_type,
            processing_mask,
            processing_applied,
            magnitude_bytes,
            phase_bytes,
            range_extent_pixels,
            range_to_nearest_edge_m,
            index_of_zero_velocity_bin,
            target_radial_electrical_length_m,
            electrical_length_uncertainty_m,
            scatterers,
        })
    }
}

/// A four-corner bounding area (J6-J13, R4-R11, A7-A14), clockwise A-D.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BoundingArea {
    /// `[lat, lon]` pairs for points A, B, C, D (lon normalised).
    pub points: [[f64; 2]; 4],
}

impl BoundingArea {
    fn parse(c: &mut Cursor<'_>) -> Result<Self, Error> {
        let mut points = [[0.0; 2]; 4];
        for p in &mut points {
            let lat = c.sa32()?;
            let lon = norm_lon(c.ba32()?);
            *p = [lat, lon];
        }
        Ok(Self { points })
    }
}

/// Job definition segment, Table 2-7 (para 2.7).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JobDefinitionSegment {
    pub segment_size: usize,
    /// J1.
    pub job_id: u32,
    /// J2, Table 2-7.1.
    pub sensor_type: Enumeration,
    /// J3.
    pub sensor_model: String,
    /// J4 flag byte (0 = no filtering).
    pub target_filtering_flag: u8,
    /// J4 bit 0.
    pub area_filtering: bool,
    /// J4 bit 1.
    pub area_blanking: bool,
    /// J4 bit 2.
    pub sector_blanking: bool,
    /// J5 (1 highest .. 99 lowest; 255 = end of job).
    pub radar_priority: u8,
    /// J5 == 255.
    pub end_of_job: bool,
    /// J6-J13.
    pub bounding_area: BoundingArea,
    /// J14, Table 2-7.2.
    pub radar_mode: Enumeration,
    /// J15 (I16 ds), seconds.
    pub nominal_revisit_interval_s: f64,
    /// J16 (I16 dm; 65535 = no statement).
    pub nominal_along_track_uncertainty_m: Option<f64>,
    /// J17 (I16 dm; 65535 = no statement).
    pub nominal_cross_track_uncertainty_m: Option<f64>,
    /// J18 (I16 dm; 65535 = no statement).
    pub nominal_alt_uncertainty_m: Option<f64>,
    /// J19 (I8 deg; 255 = no statement).
    pub nominal_track_heading_uncertainty_deg: Option<f64>,
    /// J20 (I16 mm/s; 65535 = no statement).
    pub nominal_speed_uncertainty_mps: Option<f64>,
    /// J21 (I16 cm; 65535 = no statement).
    pub nominal_slant_range_std_m: Option<f64>,
    /// J22 (BA16; 180.0 = no statement, values >= 180 treated as such).
    pub nominal_cross_range_std_deg: Option<f64>,
    /// J23 (I16 cm/s; 65535 = no statement).
    pub nominal_radial_velocity_std_mps: Option<f64>,
    /// J24 (I8 dm/s; 255 = no statement).
    pub nominal_mdv_mps: Option<f64>,
    /// J25 (I8 percent; 255 = no statement).
    pub nominal_detection_probability_pct: Option<u8>,
    /// J26 (I8 -dB; 255 = no statement): false alarms per m^2 = 10^(-value/10).
    pub nominal_false_alarm_density_neg_db: Option<u8>,
    /// J27, Table 2-7.3.
    pub terrain_elevation_model: Enumeration,
    /// J28, Table 2-7.4.
    pub geoid_model: Enumeration,
}

fn ns16(v: u16, scale: f64) -> Option<f64> {
    (v != 0xFFFF).then(|| f64::from(v) / scale)
}
fn ns8(v: u8, scale: f64) -> Option<f64> {
    (v != 0xFF).then(|| f64::from(v) / scale)
}

impl JobDefinitionSegment {
    fn parse(c: &mut Cursor<'_>, segment_size: usize) -> Result<Self, Error> {
        let job_id = c.u32()?;
        let sensor_type = Enumeration::new(c.u8()?, enums::sensor_type_name);
        let sensor_model = c.text(6)?;
        let target_filtering_flag = c.u8()?;
        let radar_priority = c.u8()?;
        let bounding_area = BoundingArea::parse(c)?;
        let radar_mode = Enumeration::new(c.u8()?, enums::radar_mode_name);
        let nominal_revisit_interval_s = f64::from(c.u16()?) / 10.0;
        let nominal_along_track_uncertainty_m = ns16(c.u16()?, 10.0);
        let nominal_cross_track_uncertainty_m = ns16(c.u16()?, 10.0);
        let nominal_alt_uncertainty_m = ns16(c.u16()?, 10.0);
        let nominal_track_heading_uncertainty_deg = ns8(c.u8()?, 1.0);
        let nominal_speed_uncertainty_mps = ns16(c.u16()?, 1000.0);
        let nominal_slant_range_std_m = ns16(c.u16()?, 100.0);
        let j22 = c.u16()?;
        // Range 0..179.9945; 180.0 (0x8000) is No Statement. Larger values are
        // out of range and treated the same way.
        let nominal_cross_range_std_deg = (j22 < 0x8000).then(|| crate::cursor::ba16(j22));
        let nominal_radial_velocity_std_mps = ns16(c.u16()?, 100.0);
        let nominal_mdv_mps = ns8(c.u8()?, 10.0);
        let j25 = c.u8()?;
        let j26 = c.u8()?;
        let terrain_elevation_model = Enumeration::new(c.u8()?, enums::terrain_model_name);
        let geoid_model = Enumeration::new(c.u8()?, enums::geoid_model_name);
        Ok(Self {
            segment_size,
            job_id,
            sensor_type,
            sensor_model,
            target_filtering_flag,
            area_filtering: target_filtering_flag & 0x01 != 0,
            area_blanking: target_filtering_flag & 0x02 != 0,
            sector_blanking: target_filtering_flag & 0x04 != 0,
            radar_priority,
            end_of_job: radar_priority == 255,
            bounding_area,
            radar_mode,
            nominal_revisit_interval_s,
            nominal_along_track_uncertainty_m,
            nominal_cross_track_uncertainty_m,
            nominal_alt_uncertainty_m,
            nominal_track_heading_uncertainty_deg,
            nominal_speed_uncertainty_mps,
            nominal_slant_range_std_m,
            nominal_cross_range_std_deg,
            nominal_radial_velocity_std_mps,
            nominal_mdv_mps,
            nominal_detection_probability_pct: (j25 != 255).then_some(j25),
            nominal_false_alarm_density_neg_db: (j26 != 255).then_some(j26),
            terrain_elevation_model,
            geoid_model,
        })
    }
}

/// Free text segment, Table 2-8 (para 2.8).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FreeTextSegment {
    pub segment_size: usize,
    /// F1.
    pub originator_id: String,
    /// F2.
    pub recipient_id: String,
    /// F3, the rest of the segment.
    pub text: String,
}

impl FreeTextSegment {
    fn parse(c: &mut Cursor<'_>, segment_size: usize) -> Result<Self, Error> {
        let originator_id = c.text(10)?;
        let recipient_id = c.text(10)?;
        let rest = c.take(c.remaining())?;
        Ok(Self {
            segment_size,
            originator_id,
            recipient_id,
            text: text(rest),
        })
    }
}

/// Test and status segment, Table 2-12 (para 2.12).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TestAndStatusSegment {
    pub segment_size: usize,
    /// T1.
    pub job_id: u32,
    /// T2.
    pub revisit_index: u16,
    /// T3.
    pub dwell_index: u16,
    /// T4, ms after the mission reference day's midnight.
    pub dwell_time_ms: u32,
    /// T5 flag byte (1 = fail).
    pub hardware_status: u8,
    pub antenna_fail: bool,
    pub rf_electronics_fail: bool,
    pub processor_fail: bool,
    pub datalink_fail: bool,
    pub calibration_mode_fail: bool,
    /// T6 flag byte (1 = outside limits).
    pub mode_status: u8,
    pub range_limit_exceeded: bool,
    pub azimuth_limit_exceeded: bool,
    pub elevation_limit_exceeded: bool,
    pub temperature_limit_exceeded: bool,
}

impl TestAndStatusSegment {
    fn parse(c: &mut Cursor<'_>, segment_size: usize) -> Result<Self, Error> {
        let job_id = c.u32()?;
        let revisit_index = c.u16()?;
        let dwell_index = c.u16()?;
        let dwell_time_ms = c.u32()?;
        let hw = c.u8()?;
        let ms = c.u8()?;
        Ok(Self {
            segment_size,
            job_id,
            revisit_index,
            dwell_index,
            dwell_time_ms,
            hardware_status: hw,
            antenna_fail: hw & 0x80 != 0,
            rf_electronics_fail: hw & 0x40 != 0,
            processor_fail: hw & 0x20 != 0,
            datalink_fail: hw & 0x10 != 0,
            calibration_mode_fail: hw & 0x08 != 0,
            mode_status: ms,
            range_limit_exceeded: ms & 0x80 != 0,
            azimuth_limit_exceeded: ms & 0x40 != 0,
            elevation_limit_exceeded: ms & 0x20 != 0,
            temperature_limit_exceeded: ms & 0x10 != 0,
        })
    }
}

/// Processing history segment, Table 2-14 (para 2.14).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProcessingHistorySegment {
    pub segment_size: usize,
    /// C1.
    pub history_count: u8,
    /// C2.
    pub based_on_nationality: String,
    /// C3.
    pub based_on_platform_id: String,
    /// C4.
    pub based_on_mission_id: u32,
    /// C5.
    pub based_on_job_id: u32,
    /// C6, Table 2-14.1 (C1 records).
    pub records: Vec<ProcessingRecord>,
}

/// Processing record, Table 2-14.1.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProcessingRecord {
    /// C6.1.
    pub sequence_number: u8,
    /// C6.2.
    pub nationality: String,
    /// C6.3.
    pub platform_id: String,
    /// C6.4.
    pub mission_id: u32,
    /// C6.5.
    pub job_id: u32,
    /// C6.6, Table 2-14.2 flag field.
    pub processing_performed: u16,
    pub processing_performed_names: Vec<&'static str>,
}

impl ProcessingHistorySegment {
    fn parse(c: &mut Cursor<'_>, segment_size: usize) -> Result<Self, Error> {
        let history_count = c.u8()?;
        let based_on_nationality = c.text(2)?;
        let based_on_platform_id = c.text(10)?;
        let based_on_mission_id = c.u32()?;
        let based_on_job_id = c.u32()?;
        let mut records = Vec::new();
        for _ in 0..history_count {
            let sequence_number = c.u8()?;
            let nationality = c.text(2)?;
            let platform_id = c.text(10)?;
            let mission_id = c.u32()?;
            let job_id = c.u32()?;
            let processing_performed = c.u16()?;
            records.push(ProcessingRecord {
                sequence_number,
                nationality,
                platform_id,
                mission_id,
                job_id,
                processing_performed,
                processing_performed_names: enums::processing_performed_names(processing_performed),
            });
        }
        Ok(Self {
            segment_size,
            history_count,
            based_on_nationality,
            based_on_platform_id,
            based_on_mission_id,
            based_on_job_id,
            records,
        })
    }
}

/// Platform location segment, Table 2-15 (para 2.15).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlatformLocationSegment {
    pub segment_size: usize,
    /// L1, ms after the mission reference day's midnight.
    pub location_time_ms: u32,
    /// L2 (SA32).
    pub lat: f64,
    /// L3 (BA32), normalised.
    pub lon: f64,
    /// L4 (S32 cm), metres.
    pub alt_m: f64,
    /// L5 (BA16).
    pub track_deg: f64,
    /// L6 (I32 mm/s).
    pub speed_mps: f64,
    /// L7 (S8 dm/s).
    pub vertical_velocity_mps: f64,
}

impl PlatformLocationSegment {
    fn parse(c: &mut Cursor<'_>, segment_size: usize) -> Result<Self, Error> {
        Ok(Self {
            segment_size,
            location_time_ms: c.u32()?,
            lat: c.sa32()?,
            lon: norm_lon(c.ba32()?),
            alt_m: f64::from(c.i32()?) / 100.0,
            track_deg: c.ba16()?,
            speed_mps: f64::from(c.u32()?) / 1000.0,
            vertical_velocity_mps: f64::from(c.i8()?) / 10.0,
        })
    }
}

fn datetime(y: u16, mo: u8, d: u8, h: u8, mi: u8, s: u8) -> Option<String> {
    let date = chrono::NaiveDate::from_ymd_opt(i32::from(y), u32::from(mo), u32::from(d))?;
    // A seconds value of 60 (leap second, allowed in A24) is carried as :59 + 1s.
    let (sec, extra) = if s == 60 { (59, 1) } else { (s, 0) };
    let t = date.and_hms_opt(u32::from(h), u32::from(mi), u32::from(sec))?
        + chrono::Duration::seconds(extra);
    Some(
        t.and_utc()
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    )
}

/// Job request segment, Table 3-1 (para 3.1).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JobRequestSegment {
    pub segment_size: usize,
    /// R1.
    pub requestor_id: String,
    /// R2.
    pub requestor_task_id: String,
    /// R3 (0 = default).
    pub requestor_priority: u8,
    /// R4-R11.
    pub bounding_area: BoundingArea,
    /// R12, Table 2-7.2.
    pub radar_mode: Enumeration,
    /// R13 (I16 cm; 0 = don't care), metres.
    pub range_resolution_m: Option<f64>,
    /// R14 (I16 dm; 0 = don't care), metres.
    pub cross_range_resolution_m: Option<f64>,
    /// R15-R20 as raw values.
    pub earliest_start: [u16; 6],
    /// R15-R20 as RFC 3339 UTC, when valid.
    pub earliest_start_time: Option<String>,
    /// R21 (s).
    pub allowed_delay_s: u16,
    /// R22 (s; 0 = continuous).
    pub duration_s: u16,
    /// R23 (ds; 0 = default), seconds.
    pub revisit_interval_s: f64,
    /// R24, Table 2-7.1 (255 = no statement).
    pub sensor_type: Enumeration,
    /// R25 ("None" = no statement).
    pub sensor_model: String,
    /// R26 flag (0 = initial request, 1 = cancel).
    pub request_type: u8,
    pub cancel: bool,
}

impl JobRequestSegment {
    fn parse(c: &mut Cursor<'_>, segment_size: usize) -> Result<Self, Error> {
        let requestor_id = c.text(10)?;
        let requestor_task_id = c.text(10)?;
        let requestor_priority = c.u8()?;
        let bounding_area = BoundingArea::parse(c)?;
        let radar_mode = Enumeration::new(c.u8()?, enums::radar_mode_name);
        let r13 = c.u16()?;
        let r14 = c.u16()?;
        let (y, mo, d, h, mi, s) = (c.u16()?, c.u8()?, c.u8()?, c.u8()?, c.u8()?, c.u8()?);
        let allowed_delay_s = c.u16()?;
        let duration_s = c.u16()?;
        let revisit_interval_s = f64::from(c.u16()?) / 10.0;
        let sensor_type = Enumeration::new(c.u8()?, enums::sensor_type_name);
        let sensor_model = c.text(6)?;
        let request_type = c.u8()?;
        Ok(Self {
            segment_size,
            requestor_id,
            requestor_task_id,
            requestor_priority,
            bounding_area,
            radar_mode,
            range_resolution_m: (r13 != 0).then(|| f64::from(r13) / 100.0),
            cross_range_resolution_m: (r14 != 0).then(|| f64::from(r14) / 10.0),
            earliest_start: [
                y,
                u16::from(mo),
                u16::from(d),
                u16::from(h),
                u16::from(mi),
                u16::from(s),
            ],
            earliest_start_time: datetime(y, mo, d, h, mi, s),
            allowed_delay_s,
            duration_s,
            revisit_interval_s,
            sensor_type,
            sensor_model,
            request_type,
            cancel: request_type & 0x01 != 0,
        })
    }
}

/// Job acknowledge segment, Table 3-2 (para 3.2).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JobAcknowledgeSegment {
    pub segment_size: usize,
    /// A1.
    pub job_id: u32,
    /// A2.
    pub requestor_id: String,
    /// A3.
    pub requestor_task_id: String,
    /// A4, Table 2-7.1.
    pub sensor_type: Enumeration,
    /// A5.
    pub sensor_model: String,
    /// A6.
    pub radar_priority: u8,
    /// A7-A14.
    pub bounding_area: BoundingArea,
    /// A15, Table 2-7.2.
    pub radar_mode: Enumeration,
    /// A16 (s; 0 = continuous).
    pub duration_s: u16,
    /// A17 (ds; 0 = default), seconds.
    pub revisit_interval_s: f64,
    /// A18.
    pub request_status: Enumeration,
    /// A19-A24 as raw values.
    pub start: [u16; 6],
    /// A19-A24 as RFC 3339 UTC, when valid.
    pub start_time: Option<String>,
    /// A25.
    pub requestor_nationality: String,
}

impl JobAcknowledgeSegment {
    fn parse(c: &mut Cursor<'_>, segment_size: usize) -> Result<Self, Error> {
        let job_id = c.u32()?;
        let requestor_id = c.text(10)?;
        let requestor_task_id = c.text(10)?;
        let sensor_type = Enumeration::new(c.u8()?, enums::sensor_type_name);
        let sensor_model = c.text(6)?;
        let radar_priority = c.u8()?;
        let bounding_area = BoundingArea::parse(c)?;
        let radar_mode = Enumeration::new(c.u8()?, enums::radar_mode_name);
        let duration_s = c.u16()?;
        let revisit_interval_s = f64::from(c.u16()?) / 10.0;
        let request_status = Enumeration::new(c.u8()?, enums::request_status_name);
        let (y, mo, d, h, mi, s) = (c.u16()?, c.u8()?, c.u8()?, c.u8()?, c.u8()?, c.u8()?);
        let requestor_nationality = c.text(2)?;
        Ok(Self {
            segment_size,
            job_id,
            requestor_id,
            requestor_task_id,
            sensor_type,
            sensor_model,
            radar_priority,
            bounding_area,
            radar_mode,
            duration_s,
            revisit_interval_s,
            request_status,
            start: [
                y,
                u16::from(mo),
                u16::from(d),
                u16::from(h),
                u16::from(mi),
                u16::from(s),
            ],
            start_time: datetime(y, mo, d, h, mi, s),
            requestor_nationality,
        })
    }
}
