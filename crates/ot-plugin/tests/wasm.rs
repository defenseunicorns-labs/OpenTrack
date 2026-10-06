//! The example plugins in sdk/rust, loaded as WebAssembly components. Built
//! here when the wasm32-wasip2 target is installed; skipped otherwise.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use chrono::{TimeZone, Utc};
use ot_core::Observation;
use ot_plugin::{Grants, Source};
use ot_source::plugin::{Kind, ScoreCandidate};
use serde_json::json;

fn examples() -> Option<&'static Path> {
    static BUILT: OnceLock<Option<PathBuf>> = OnceLock::new();
    BUILT
        .get_or_init(|| {
            let sdk = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sdk/rust");
            let ok = std::process::Command::new(env!("CARGO"))
                .args([
                    "build",
                    "--release",
                    "--target",
                    "wasm32-wasip2",
                    "--manifest-path",
                ])
                .arg(sdk.join("Cargo.toml"))
                .status()
                .is_ok_and(|s| s.success());
            if !ok {
                eprintln!(
                    "skipped: the example plugins did not build (rustup target add wasm32-wasip2)"
                );
                return None;
            }
            Some(sdk.join("target/wasm32-wasip2/release"))
        })
        .as_deref()
}

fn load(name: &str) -> Option<std::sync::Arc<dyn ot_source::plugin::Plugin>> {
    let bytes = std::fs::read(examples()?.join(format!("{name}.wasm"))).unwrap();
    Some(ot_plugin::load(&Source::Wasm(bytes), &Grants::default()).unwrap())
}

fn plot(key: &str, secs: i64, lat: f64, lon: f64) -> Observation {
    let at = Utc.timestamp_opt(1_700_000_000 + secs, 0).unwrap();
    serde_json::from_value(json!({
        "schema_version": 2, "source_id": "radar", "source_track_key": key,
        "observed_at": at, "received_at": at,
        "position": {"latitude": lat, "longitude": lon}
    }))
    .unwrap()
}

#[test]
fn a_codec_plugin_decodes_frames() {
    let Some(p) = load("csv_codec") else { return };
    let m = p.manifest();
    assert_eq!(m.name, "csv-reports");
    assert_eq!(m.kinds, vec![Kind::Codec]);
    assert!(m.framing.is_some());
    let mut d = p.decoder(&json!({})).unwrap();
    let records = d
        .decode(
            b"key,time,lat,lon\nA1,2026-09-26T12:00:00Z,36.9,-76.2,90,5\nbad line\n",
            Utc::now(),
        )
        .unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["key"], "A1");
    assert_eq!(records[0]["speed"], 5.0);
    assert!(p.decoder(&json!({"delimiter": "::"})).is_err());
    assert!(p.tracker(&json!({})).is_err(), "not a tracker");
    let mut strict = p.decoder(&json!({"strict": true})).unwrap();
    assert!(strict.decode(b"A1,0,x,y\n", Utc::now()).is_err());
}

#[test]
fn a_tracker_plugin_forms_tracks_from_plots() {
    let Some(p) = load("alpha_beta_tracker") else {
        return;
    };
    assert_eq!(p.manifest().kinds, vec![Kind::Tracker]);
    let mut t = p.tracker(&json!({"gate_m": 200.0})).unwrap();
    let mut reported = Vec::new();
    for s in 0..6 {
        // One target moving north about 10 m/s, and a far clutter plot each scan.
        t.push(
            plot(&format!("p{s}"), s * 2, 36.9 + s as f64 * 0.00018, -76.2),
            Utc::now(),
        );
        t.push(
            plot(&format!("c{s}"), s * 2, 37.0 + s as f64 * 0.01, -76.0),
            Utc::now(),
        );
        let now = Utc.timestamp_opt(1_700_000_000 + s * 2, 0).unwrap();
        reported.extend(t.run(now, false).unwrap());
    }
    let keys: std::collections::BTreeSet<_> = reported
        .iter()
        .map(|o| o.source_track_key.clone())
        .collect();
    assert_eq!(keys.len(), 1, "one confirmed track: {keys:?}");
    let last = reported.last().unwrap();
    let speed = last.kinematics.speed_mps.unwrap();
    assert!((speed - 10.0).abs() < 2.0, "{speed}");
    assert!(
        last.kinematics.course_deg.unwrap() < 5.0 || last.kinematics.course_deg.unwrap() > 355.0
    );
    // Quiet for longer than drop_secs: the track ends.
    let later = Utc.timestamp_opt(1_700_000_100, 0).unwrap();
    let end = t.run(later, true).unwrap();
    assert_eq!(end.len(), 1);
    assert_eq!(end[0].state, Some(ot_core::TrackState::Dropped));
}

#[test]
fn a_scorer_plugin_scores_candidates() {
    let Some(p) = load("speed_scorer") else {
        return;
    };
    let mut s = p.scorer(&json!({"max_speed_diff_mps": 5.0})).unwrap();
    let mut report = plot("a", 0, 36.9, -76.2);
    report.kinematics.speed_mps = Some(10.0);
    let mut slow = plot("b", 0, 36.9, -76.2);
    slow.kinematics.speed_mps = Some(1.0);
    let mut same = slow.clone();
    same.kinematics.speed_mps = Some(11.0);
    let k = json!({"ln_lr": 2.5, "pass": true});
    let scores = s
        .score(
            &report,
            &[
                ScoreCandidate {
                    view: serde_json::to_value(&slow).unwrap(),
                    kinematic: k.clone(),
                },
                ScoreCandidate {
                    view: serde_json::to_value(&same).unwrap(),
                    kinematic: k,
                },
            ],
        )
        .unwrap();
    assert!(!scores[0].pass && scores[0].ln_lr < 0.0, "{scores:?}");
    assert!(scores[1].pass && (scores[1].ln_lr - 2.5).abs() < 1e-9);
    assert_eq!(scores[0].evidence["veto"], "speed");
}

#[test]
fn a_plugin_is_held_to_its_memory_grant() {
    let Some(dir) = examples() else { return };
    let bytes = std::fs::read(dir.join("alpha_beta_tracker.wasm")).unwrap();
    let tiny = Grants {
        memory_mb: 1,
        ..Grants::default()
    };
    // A megabyte is less than the component's own memory: it cannot start.
    assert!(ot_plugin::load(&Source::Wasm(bytes), &tiny).is_err());
}

#[test]
fn a_file_that_is_not_a_component_is_refused() {
    let e = ot_plugin::load(&Source::Wasm(b"not wasm".to_vec()), &Grants::default())
        .err()
        .unwrap();
    assert!(
        format!("{e:#}").contains("not a WebAssembly component"),
        "{e:#}"
    );
}
