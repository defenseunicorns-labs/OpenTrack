//! Tests over the real sample corpus. The samples are not in the repository:
//! set `OT_GMTI_SAMPLES` (default `$HOME/data/gmti/STANAG4607`); the tests
//! print a message and pass when the directory is absent.

use std::collections::{BTreeMap, BTreeSet};
mod common;

use common::samples;
use ot_codec_stanag4607::{Decoder, Emit, Options, Segment, packet_len, parse_packet};
use serde_json::Value;

#[test]
fn every_sample_decodes_in_every_mode() {
    let Some(files) = samples() else { return };
    for (path, bytes) in &files {
        for emit in [Emit::Targets, Emit::Segments, Emit::All] {
            let recs = Decoder::new(Options {
                emit,
                ..Default::default()
            })
            .decode(bytes)
            .unwrap_or_else(|e| panic!("{}: {emit:?}: {e}", path.display()));
            let warnings: Vec<_> = recs.iter().filter(|r| r["record"] == "warning").collect();
            assert!(warnings.is_empty(), "{}: {warnings:?}", path.display());
            if emit == Emit::Targets {
                assert!(recs.iter().all(|r| r["record"] == "target"));
            }
        }
    }
}

#[test]
fn sample_sanity() {
    let Some(files) = samples() else { return };
    for (path, bytes) in &files {
        let name = path.display();
        // Structure: sizes add up, packet_len agrees, counts agree.
        let mut rest = bytes.as_slice();
        let mut dwell_targets = 0usize;
        while !rest.is_empty() {
            let (pkt, used) = parse_packet(rest).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(packet_len(rest), Some(used), "{name}");
            assert_eq!(pkt.header.packet_size as usize, used);
            let seg_sum: usize = pkt.segments.iter().map(Segment::segment_size).sum();
            assert_eq!(32 + seg_sum, used, "{name}: segment sizes vs packet size");
            for s in &pkt.segments {
                assert!(!matches!(s, Segment::Malformed { .. }), "{name}: {s:?}");
                if let Segment::Dwell(d) = s {
                    assert_eq!(usize::from(d.target_report_count), d.targets.len());
                    dwell_targets += d.targets.len();
                }
            }
            rest = &rest[used..];
        }

        // Records: one per target report, positions in range, times in the mission day.
        let recs = Decoder::new(Options::default()).decode(bytes).unwrap();
        assert_eq!(recs.len(), dwell_targets, "{name}");
        for r in &recs {
            // Targets with the (-90, 180) "no location" placeholder carry no lat/lon.
            let (Some(lat), Some(lon)) = (r["lat"].as_f64(), r["lon"].as_f64()) else {
                assert!(r["lat"].is_null() && r["lon"].is_null(), "{name}");
                continue;
            };
            assert!((-90.0..=90.0).contains(&lat), "{name}: lat {lat}");
            assert!((-180.0..180.0).contains(&lon), "{name}: lon {lon}");
            assert_eq!(r["time_basis"], "mission_reference_date", "{name}");
            // Dwell time counts from the mission reference day's midnight and
            // may run past it (para 2.4.6): check time == reference day + ms and
            // that the dwell lies within the day after the reference day at most.
            let day = r["mission"]["reference_date"].as_str().unwrap();
            let ms = r["dwell"]["time_ms"].as_u64().unwrap();
            assert!(
                ms < 2 * 86_400_000,
                "{name}: dwell {ms} ms after reference day"
            );
            let t0 = chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").unwrap();
            let expect = t0.and_hms_opt(0, 0, 0).unwrap().and_utc()
                + chrono::Duration::milliseconds(ms as i64);
            let time = chrono::DateTime::parse_from_rfc3339(r["time"].as_str().unwrap()).unwrap();
            assert_eq!(time, expect, "{name}");
            // Target near the dwell centre (within the dwell's range extent, generously).
            let clat = r["dwell"]["area"]["center_lat"].as_f64().unwrap();
            assert!(
                (lat - clat).abs() < 2.0,
                "{name}: target far from dwell centre"
            );
        }
    }
}

/// The corpus states no measurement accuracy (no D32.12/13, J21/J22 No
/// Statement): without the sigma options no target has a ground error; with
/// them every located target has a finite one.
#[test]
fn ground_error_needs_the_sigma_options_on_the_corpus() {
    let Some(files) = samples() else { return };
    let mut with = 0usize;
    for (path, bytes) in &files {
        let name = path.display();
        let recs = Decoder::new(Options::default()).decode(bytes).unwrap();
        assert!(
            recs.iter().all(|r| r.get("ground_error").is_none()),
            "{name}"
        );
        let recs = Decoder::new(Options {
            range_sigma_m: Some(20.0),
            cross_range_sigma_deg: Some(0.2),
            ..Default::default()
        })
        .decode(bytes)
        .unwrap();
        for r in &recs {
            let g = &r["ground_error"];
            if r.get("lat").is_none() {
                assert!(g.is_null(), "{name}");
                continue;
            }
            with += 1;
            assert_eq!(g["source"], "options", "{name}: {g}");
            for k in [
                "semi_major_m",
                "semi_minor_m",
                "orientation_deg",
                "nn_m2",
                "ne_m2",
                "ee_m2",
                "range_sigma_m",
                "cross_range_sigma_m",
                "slant_range_m",
                "ground_range_m",
            ] {
                let v = g[k]
                    .as_f64()
                    .unwrap_or_else(|| panic!("{name}: {k} in {g}"));
                assert!(v.is_finite(), "{name}: {k} in {g}");
            }
            let (a, b) = (
                g["semi_major_m"].as_f64().unwrap(),
                g["semi_minor_m"].as_f64().unwrap(),
            );
            assert!(0.0 < b && b <= a, "{name}: {g}");
            // The ground range error is at least the slant range error.
            assert!(g["range_sigma_m"].as_f64().unwrap() >= 20.0, "{name}: {g}");
            assert!((0.0..180.0).contains(&g["orientation_deg"].as_f64().unwrap()));
        }
    }
    eprintln!("{with} located targets with a ground error");
}

/// Prints a summary of the corpus (run with `--nocapture`).
#[test]
fn corpus_summary() {
    let Some(files) = samples() else { return };
    let mut seg_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut versions = BTreeSet::new();
    let mut platforms = BTreeSet::new();
    let mut target_fields: BTreeMap<String, usize> = BTreeMap::new();
    let mut dwell_masks = BTreeSet::new();
    let mut other = BTreeSet::new();
    let (mut packets, mut targets) = (0usize, 0usize);
    let (mut tmin, mut tmax): (Option<String>, Option<String>) = (None, None);
    for (_, bytes) in &files {
        let recs = Decoder::new(Options {
            emit: Emit::All,
            ..Default::default()
        })
        .decode(bytes)
        .unwrap();
        for r in &recs {
            let kind = r["record"].as_str().unwrap_or_default().to_string();
            *seg_counts.entry(kind.clone()).or_default() += 1;
            let p = &r["packet"];
            versions.insert(p["version"].to_string());
            platforms.insert(format!(
                "{} {} {} ex={}",
                p["nationality"], p["platform_id"], p["classification"], p["exercise_indicator"]
            ));
            match kind.as_str() {
                "target" => {
                    targets += 1;
                    if r["lat"].is_null() {
                        *seg_counts
                            .entry("target_without_location".into())
                            .or_default() += 1;
                    }
                    if let Value::Object(o) = r {
                        for k in o.keys() {
                            *target_fields.entry(k.clone()).or_default() += 1;
                        }
                    }
                    let t = r["time"].as_str().unwrap().to_string();
                    if tmin.as_ref().is_none_or(|m| &t < m) {
                        tmin = Some(t.clone());
                    }
                    if tmax.as_ref().is_none_or(|m| &t > m) {
                        tmax = Some(t);
                    }
                }
                "dwell" => {
                    dwell_masks.insert(r["existence_mask"].to_string());
                }
                "mission" => {
                    other.insert(format!(
                        "mission plan={} fp={} type={} cfg={} date={}",
                        r["mission_plan"],
                        r["flight_plan"],
                        r["platform_type"]["name"],
                        r["platform_configuration"],
                        r["reference_date"]
                    ));
                }
                "job_definition" => {
                    other.insert(format!(
                        "job sensor={} model={} mode={} filt={} prio={} slant={} xr={} vel={} terrain={} geoid={}",
                        r["sensor_type"]["name"], r["sensor_model"], r["radar_mode"]["name"],
                        r["target_filtering_flag"], r["radar_priority"],
                        r["nominal_slant_range_std_m"], r["nominal_cross_range_std_deg"],
                        r["nominal_radial_velocity_std_mps"], r["terrain_elevation_model"]["name"],
                        r["geoid_model"]["name"]
                    ));
                }
                _ => {}
            }
        }
        let mut rest = bytes.as_slice();
        while let Ok((_, used)) = parse_packet(rest) {
            packets += 1;
            rest = &rest[used..];
            if rest.is_empty() {
                break;
            }
        }
    }
    eprintln!(
        "files={} packets={packets} target_reports={targets}",
        files.len()
    );
    eprintln!("record counts: {seg_counts:?}");
    eprintln!("versions: {versions:?}");
    eprintln!("platforms: {platforms:?}");
    eprintln!("dwell existence masks: {dwell_masks:?}");
    eprintln!("target record keys: {target_fields:?}");
    eprintln!("time span: {tmin:?} .. {tmax:?}");
    for o in other.iter().take(20) {
        eprintln!("{o}");
    }
    eprintln!("distinct mission/job variants: {}", other.len());
}

/// The revisit period is timed from the dwells, not taken from the job
/// definitions (which all say 6 s): 7 dwells of ~366 ms on one file, 25
/// dwells over 12.8 s on another.
#[test]
fn revisit_periods_are_measured_from_dwells() {
    let Some(files) = samples() else { return };
    let mut seen = 0;
    for (name, want) in [("StuartHwy_48", 2.56), ("StuartHwy_115", 12.8)] {
        let Some((_, bytes)) = files
            .iter()
            .find(|(p, _)| p.to_string_lossy().contains(name))
        else {
            continue;
        };
        seen += 1;
        let mut d = Decoder::new(Options::default());
        let mut first = None;
        for chunk in split(bytes) {
            d.decode(chunk).unwrap();
            if first.is_none() {
                first = d.revisit();
            }
        }
        // Before a whole revisit: the job's nominal interval.
        let first = first.unwrap();
        assert_eq!(first.source, ot_codec_stanag4607::RevisitSource::Nominal);
        assert_eq!(first.period_s, 6.0);
        let r = d.revisit().unwrap();
        assert_eq!(r.source, ot_codec_stanag4607::RevisitSource::Measured);
        assert!(r.samples >= 5, "{name}: {r:?}");
        assert!((r.period_s - want).abs() < 0.1, "{name}: {r:?}");
    }
    assert!(seen > 0 || files.is_empty());
}

fn split(bytes: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let n = packet_len(&bytes[i..]).unwrap();
        out.push(&bytes[i..i + n]);
        i += n;
    }
    out
}
