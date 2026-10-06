//! Flattening of typed segments into JSON records (see the crate docs for the
//! record contract).

use chrono::{DateTime, Duration, NaiveTime, Utc};
use serde_json::{Map, Value, json};

use crate::ground_error::{self, CrossRange, Geo, SigmaSource, Sigmas, sigma};
use crate::segments::{
    DwellSegment, JobDefinitionSegment, MissionSegment, PacketHeader, Segment, TargetReport,
};

pub(crate) struct Context<'a> {
    pub mission: Option<&'a MissionSegment>,
    pub job: Option<&'a JobDefinitionSegment>,
    pub packet: &'a Value,
    pub job_id: u32,
    /// Fallback one-sigma errors from the decoder options.
    pub range_sigma_m: Option<f64>,
    pub cross_range_sigma_deg: Option<f64>,
}

const DAY_MS: i64 = 86_400_000;

pub(crate) fn packet_json(h: &PacketHeader) -> Value {
    json!({
        "version": h.version,
        "nationality": h.nationality,
        "classification": h.classification.name,
        "classification_code": h.classification.code,
        "classification_system": h.classification_system,
        "code": h.code,
        "code_names": h.code_names,
        "exercise": h.is_exercise(),
        "exercise_indicator": h.exercise_indicator.name,
        "exercise_indicator_code": h.exercise_indicator.code,
        "platform_id": h.platform_id,
        "mission_id": h.mission_id,
        "job_id": h.job_id,
    })
}

fn mission_json(m: &MissionSegment) -> Value {
    json!({
        "plan": m.mission_plan,
        "flight_plan": m.flight_plan,
        "platform_type": m.platform_type.name,
        "platform_type_code": m.platform_type.code,
        "platform_config": m.platform_configuration,
        "reference_date": m.reference_date,
    })
}

fn job_json(j: &JobDefinitionSegment) -> Value {
    let mut v = serde_json::to_value(j).unwrap_or(Value::Null);
    if let Some(o) = v.as_object_mut() {
        o.remove("segment_size");
    }
    v
}

/// Resolve a millisecond count from the mission reference day (see the crate
/// docs, "Time basis"). Returns `(RFC 3339 time, basis)`.
pub(crate) fn resolve_time(
    mission: Option<&MissionSegment>,
    ms: u32,
) -> (Option<String>, &'static str) {
    let fmt = |t: DateTime<Utc>| t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    if let Some(day) = mission.and_then(MissionSegment::reference_day) {
        let t = day.and_time(NaiveTime::MIN).and_utc() + Duration::milliseconds(i64::from(ms));
        return (Some(fmt(t)), "mission_reference_date");
    }
    let now = Utc::now();
    let midnight = now.date_naive().and_time(NaiveTime::MIN).and_utc();
    let tod = Duration::milliseconds(i64::from(ms) % DAY_MS);
    let best = [-1i64, 0, 1]
        .into_iter()
        .map(|d| midnight + Duration::days(d) + tod)
        .min_by_key(|t| (*t - now).num_milliseconds().abs())
        .unwrap_or(midnight + tod);
    (Some(fmt(best)), "receipt_clock_time_of_day")
}

/// Remove `null` members (recursively) and objects left empty by that.
fn prune(v: &mut Value) {
    match v {
        Value::Object(o) => {
            for x in o.values_mut() {
                prune(x);
            }
            o.retain(|_, x| !(x.is_null() || x.as_object().is_some_and(Map::is_empty)));
        }
        Value::Array(a) => a.iter_mut().for_each(prune),
        _ => {}
    }
}

fn with_head(record: &str, ctx: &Context<'_>, body: Value) -> Value {
    let mut o = Map::new();
    o.insert("record".into(), Value::from(record));
    if let Value::Object(b) = body {
        o.extend(b);
    }
    o.insert("packet".into(), ctx.packet.clone());
    let mut v = Value::Object(o);
    prune(&mut v);
    v
}

pub(crate) fn segment_record(seg: &Segment, ctx: &Context<'_>) -> Value {
    let name = seg.name();
    let mut body = match seg {
        Segment::Mission(s) => serde_json::to_value(s),
        Segment::Dwell(s) => serde_json::to_value(s),
        Segment::Hrr(s) => serde_json::to_value(s),
        Segment::JobDefinition(s) => serde_json::to_value(s),
        Segment::FreeText(s) => serde_json::to_value(s),
        Segment::TestAndStatus(s) => serde_json::to_value(s),
        Segment::ProcessingHistory(s) => serde_json::to_value(s),
        Segment::PlatformLocation(s) => serde_json::to_value(s),
        Segment::JobRequest(s) => serde_json::to_value(s),
        Segment::JobAcknowledge(s) => serde_json::to_value(s),
        Segment::RangeDoppler(s)
        | Segment::LowReflectivityIndex(s)
        | Segment::Group(s)
        | Segment::AttachedTarget(s)
        | Segment::SystemSpecific(s) => serde_json::to_value(s),
        Segment::Unknown {
            segment_type,
            bytes_len,
        } => Ok(json!({ "segment_type": segment_type, "bytes_len": bytes_len })),
        Segment::Malformed {
            segment_type,
            bytes_len,
            error,
        } => Ok(json!({
            "segment_type": segment_type,
            "segment_name": crate::enums::segment_type_name(*segment_type),
            "bytes_len": bytes_len,
            "error": error,
        })),
    }
    .unwrap_or(Value::Null);
    let ms = match seg {
        Segment::Dwell(s) => Some(s.dwell_time_ms),
        Segment::TestAndStatus(s) => Some(s.dwell_time_ms),
        Segment::PlatformLocation(s) => Some(s.location_time_ms),
        _ => None,
    };
    if let (Some(ms), Some(o)) = (ms, body.as_object_mut()) {
        let (time, basis) = resolve_time(ctx.mission, ms);
        o.insert("time".into(), json!(time));
        o.insert("time_basis".into(), json!(basis));
    }
    with_head(name, ctx, body)
}

/// The sensor platform's own state from a dwell (D6-D17) or platform
/// location (L1-L7) segment, as a `platform` record; None for other segments.
pub(crate) fn platform_record(seg: &Segment, ctx: &Context<'_>) -> Option<Value> {
    let (from, ms, body) = match seg {
        Segment::Dwell(d) => (
            "dwell",
            d.dwell_time_ms,
            json!({
                "lat": d.sensor_lat, "lon": d.sensor_lon, "alt_m": d.sensor_alt_m,
                "track_deg": d.sensor_track_deg, "speed_mps": d.sensor_speed_mps,
                "vertical_velocity_mps": d.sensor_vertical_velocity_mps,
                "uncertainty": {
                    "along_track_m": d.sensor_along_track_uncertainty_m,
                    "cross_track_m": d.sensor_cross_track_uncertainty_m,
                    "alt_m": d.sensor_alt_uncertainty_m,
                    "track_deg": d.sensor_track_uncertainty_deg,
                    "speed_mps": d.sensor_speed_uncertainty_mps,
                },
            }),
        ),
        Segment::PlatformLocation(l) => (
            "platform_location",
            l.location_time_ms,
            json!({
                "lat": l.lat, "lon": l.lon, "alt_m": l.alt_m, "track_deg": l.track_deg,
                "speed_mps": l.speed_mps, "vertical_velocity_mps": l.vertical_velocity_mps,
            }),
        ),
        _ => return None,
    };
    let (time, basis) = resolve_time(ctx.mission, ms);
    let mut body = body;
    let o = body.as_object_mut().expect("object");
    o.insert("from".into(), json!(from));
    o.insert("time".into(), json!(time));
    o.insert("time_basis".into(), json!(basis));
    o.insert("platform_id".into(), ctx.packet["platform_id"].clone());
    o.insert(
        "mission".into(),
        ctx.mission.map_or(Value::Null, mission_json),
    );
    Some(with_head("platform", ctx, body))
}

pub(crate) fn target_record(d: &DwellSegment, t: &TargetReport, ctx: &Context<'_>) -> Value {
    let (time, basis) = resolve_time(ctx.mission, d.dwell_time_ms);

    let classification = t.classification.map(|c| {
        json!({
            "code": c.code,
            "name": c.name,
            "probability": t.classification_probability_pct.map(|p| f64::from(p) / 100.0),
        })
    });

    let has_target_error = t.slant_range_uncertainty_m.is_some()
        || t.cross_range_uncertainty_m.is_some()
        || t.height_uncertainty_m.is_some()
        || t.radial_velocity_uncertainty_mps.is_some();
    let location_error = if has_target_error {
        Some(json!({
            "source": "target_report",
            "slant_range_m": t.slant_range_uncertainty_m,
            "cross_range_m": t.cross_range_uncertainty_m,
            "height_m": t.height_uncertainty_m,
            "radial_velocity_mps": t.radial_velocity_uncertainty_mps,
        }))
    } else {
        ctx.job
            .filter(|j| {
                j.nominal_slant_range_std_m.is_some()
                    || j.nominal_cross_range_std_deg.is_some()
                    || j.nominal_radial_velocity_std_mps.is_some()
            })
            .map(|j| {
                json!({
                    "source": "job_nominal",
                    "slant_range_m": j.nominal_slant_range_std_m,
                    "cross_range_deg": j.nominal_cross_range_std_deg,
                    "radial_velocity_mps": j.nominal_radial_velocity_std_mps,
                })
            })
    };

    let ground_error = target_ground_error(d, t, ctx);

    let truth_tag = (t.truth_tag_application.is_some() || t.truth_tag_entity.is_some())
        .then(|| json!({ "application": t.truth_tag_application, "entity": t.truth_tag_entity }));

    let platform_orientation = json!({
        "heading_deg": d.platform_heading_deg,
        "pitch_deg": d.platform_pitch_deg,
        "roll_deg": d.platform_roll_deg,
    });
    let sensor_orientation = json!({
        "heading_deg": d.sensor_heading_deg,
        "pitch_deg": d.sensor_pitch_deg,
        "roll_deg": d.sensor_roll_deg,
    });

    let mut v = json!({
        "record": "target",
        "time": time,
        "time_basis": basis,
        "lat": t.lat,
        "lon": t.lon,
        "height_m": t.height_m,
        "radial_velocity_mps": t.radial_velocity_mps,
        "wrap_velocity_mps": t.wrap_velocity_mps,
        "snr_db": t.snr_db,
        "rcs_dbsm": t.rcs_dbsm,
        "classification": classification,
        "location_error": location_error,
        "ground_error": ground_error,
        "truth_tag": truth_tag,
        "report_index": t.report_index,
        "mti_report_index": t.mti_report_index,
        "dwell": {
            "revisit_index": d.revisit_index,
            "dwell_index": d.dwell_index,
            "last_dwell_of_revisit": d.last_dwell_of_revisit,
            "time_ms": d.dwell_time_ms,
            "target_count": d.target_report_count,
            "mdv_mps": d.mdv_mps,
            "sensor": {
                "lat": d.sensor_lat,
                "lon": d.sensor_lon,
                "alt_m": d.sensor_alt_m,
                "track_deg": d.sensor_track_deg,
                "speed_mps": d.sensor_speed_mps,
                "vertical_velocity_mps": d.sensor_vertical_velocity_mps,
                "along_track_uncertainty_m": d.sensor_along_track_uncertainty_m,
                "cross_track_uncertainty_m": d.sensor_cross_track_uncertainty_m,
                "alt_uncertainty_m": d.sensor_alt_uncertainty_m,
                "track_uncertainty_deg": d.sensor_track_uncertainty_deg,
                "speed_uncertainty_mps": d.sensor_speed_uncertainty_mps,
                "vertical_velocity_uncertainty_mps": d.sensor_vertical_velocity_uncertainty_mps,
            },
            "area": {
                "center_lat": d.center_lat,
                "center_lon": d.center_lon,
                "range_half_extent_m": d.range_half_extent_m,
                "dwell_angle_half_extent_deg": d.dwell_angle_half_extent_deg,
            },
            "platform_orientation": platform_orientation,
            "sensor_orientation": sensor_orientation,
        },
        "job_id": ctx.job_id,
        "job": ctx.job.map(job_json),
        "mission": ctx.mission.map(mission_json),
        "packet": ctx.packet,
    });
    prune(&mut v);
    v
}

/// The target's error on the ground (see the README, "Ground error"): each
/// sigma from the report, else the job's nominal value, else the options.
fn target_ground_error(d: &DwellSegment, t: &TargetReport, ctx: &Context<'_>) -> Option<Value> {
    let (lat, lon) = (t.lat?, t.lon?);
    let job = ctx.job;
    let range = sigma(t.slant_range_uncertainty_m)
        .map(|v| (v, SigmaSource::TargetReport))
        .or_else(|| {
            sigma(job.and_then(|j| j.nominal_slant_range_std_m))
                .map(|v| (v, SigmaSource::JobNominal))
        })
        .or_else(|| option_sigma(ctx.range_sigma_m).map(|v| (v, SigmaSource::Options)))?;
    let cross = sigma(t.cross_range_uncertainty_m)
        .map(|v| (CrossRange::Metres(v), SigmaSource::TargetReport))
        .or_else(|| {
            sigma(job.and_then(|j| j.nominal_cross_range_std_deg))
                .map(|v| (CrossRange::Deg(v), SigmaSource::JobNominal))
        })
        .or_else(|| {
            option_sigma(ctx.cross_range_sigma_deg)
                .map(|v| (CrossRange::Deg(v), SigmaSource::Options))
        })?;
    let sigmas = Sigmas {
        slant_range_m: range.0,
        cross_range: cross.0,
        source: range.1.max(cross.1),
        radial_velocity_mps: sigma(t.radial_velocity_uncertainty_mps)
            .or_else(|| sigma(job.and_then(|j| j.nominal_radial_velocity_std_mps))),
    };
    let sensor = Geo {
        lat: d.sensor_lat,
        lon: d.sensor_lon,
        h: d.sensor_alt_m,
    };
    let target = Geo {
        lat,
        lon,
        h: t.height_m.unwrap_or(0.0),
    };
    ground_error::ground_error(sensor, target, &sigmas).and_then(|e| serde_json::to_value(e).ok())
}

/// An option sigma is used only when positive.
fn option_sigma(v: Option<f64>) -> Option<f64> {
    v.filter(|v| v.is_finite() && *v > 0.0)
}
