//! The STANAG 4607 example source end to end on a recorded GMTI file: the
//! stream framed by the packet size field, decoded by the codec plugin and
//! formed into ground tracks by its tracker. Needs the sample recordings
//! (`$OT_GMTI_SAMPLES`, default `~/data/gmti/STANAG4607`), which are not in
//! the repository; without them the test says so and passes.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use bytes::BytesMut;
use ot_source::frame::{Frame, Framer};
use ot_source::pipeline::Pipeline;
use ot_source::registry::RegistryEntry;
use ot_source::source::SourceSpec;
use ot_source::transport::TransportConfig;

fn sample(name: &str) -> Option<PathBuf> {
    let dir = std::env::var_os("OT_GMTI_SAMPLES")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("data/gmti/STANAG4607")
        });
    let found = walk(&dir).into_iter().find(|p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.contains(name))
    });
    if found.is_none() {
        eprintln!(
            "no GMTI sample matching {name} under {}; skipped",
            dir.display()
        );
    }
    found
}

fn walk(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out.sort();
    out
}

/// The example's codec options, less `shift_to_now`.
fn replay_options() -> serde_json::Value {
    let spec: SourceSpec =
        serde_json::from_str(include_str!("../../../docs/examples/stanag4607.json")).unwrap();
    let mut options = serde_json::to_value(&spec.pipeline.codec).unwrap()["options"].clone();
    assert_eq!(options["range_sigma_m"], 20.0);
    assert_eq!(options["cross_range_sigma_deg"], 0.2);
    options.as_object_mut().unwrap().remove("shift_to_now");
    options
}

/// A recording through the example source's pipeline, framed as it would
/// arrive over TCP: every report, the pipeline, and the packet and dwell counts.
fn replay(path: &std::path::Path) -> (Vec<ot_core::Observation>, Pipeline, usize, usize) {
    let mut spec: SourceSpec =
        serde_json::from_str(include_str!("../../../docs/examples/stanag4607.json")).unwrap();
    spec.validate().unwrap();
    let TransportConfig::TcpClient { framing, .. } = &spec.transport else {
        panic!("the example streams over TCP");
    };
    let mut framer = Framer::new(framing.clone()).unwrap();
    // Keep the recording's own times, so the test does not depend on the clock.
    spec.pipeline.codec = serde_json::from_value(serde_json::json!(
        { "type": "plugin", "plugin": "stanag4607", "options": replay_options() }
    ))
    .unwrap();
    let mut pipeline = Pipeline::new(&spec.id, spec.pipeline.clone()).unwrap();
    let registry = BTreeMap::<(String, String), RegistryEntry>::new();

    let mut buf = BytesMut::from(&std::fs::read(path).unwrap()[..]);
    let mut out = Vec::new();
    let (mut packets, mut dwells) = (0, 0);
    while let Some(bytes) = framer.next(&mut buf).unwrap() {
        packets += 1;
        // One segment per packet in these recordings; type 2 is a dwell.
        dwells += usize::from(bytes.get(32) == Some(&2));
        let o = pipeline.process(&Frame::new(bytes), &registry);
        assert_eq!(o.last_error, None);
        out.extend(o.observations);
    }
    assert!(buf.is_empty(), "the file is whole packets");
    let last = out.last().map(|o| o.observed_at).unwrap();
    out.extend(
        pipeline
            .flush(last + chrono::TimeDelta::hours(1), true)
            .observations,
    );
    (out, pipeline, packets, dwells)
}

/// How long each track ran (s).
fn spans(out: &[ot_core::Observation]) -> Vec<i64> {
    let mut span = BTreeMap::new();
    for o in out {
        let e = span
            .entry(o.source_track_key.as_str())
            .or_insert((o.observed_at, o.observed_at));
        e.1 = o.observed_at;
    }
    span.values()
        .map(|(a, b)| (*b - *a).num_seconds())
        .collect()
}

#[test]
fn a_gmti_recording_becomes_ground_tracks() {
    let Some(path) = sample("StuartHwy_48") else {
        return;
    };
    let (out, mut pipeline, packets, dwells) = replay(&path);
    // The tracker timed itself by the measured revisit (7 dwells, 2.56 s).
    let timing = pipeline.tracker_timing().unwrap().clone();
    assert_eq!(timing.revisit_source, "measured");
    assert!((timing.revisit_secs - 2.56).abs() < 0.1, "{timing:?}");
    // Above the floors' revisit: the floors (confirm 20 s, drop 8 s / 30 s).
    assert_eq!(
        (
            timing.confirm_within_secs,
            timing.drop_tentative_secs,
            timing.drop_confirmed_secs
        ),
        (20.0, 8.0, 30.0)
    );

    // The sensor platform: its own friendly air track, straight past the tracker.
    let (platform, out): (Vec<_>, Vec<_>) =
        out.into_iter().partition(|o| o.source_track_key == "DEAP");
    assert_eq!(platform.len(), dwells, "one per dwell");
    for o in &platform {
        assert_eq!(o.callsign.as_deref(), Some("DEAP"));
        // The recordings' mission segments name no platform type: friendly air.
        assert_eq!(o.classification.cot_type_or_derived(), "a-f-A");
        assert_eq!(
            o.classification.effective_domain(),
            Some(ot_core::Domain::Air)
        );
        assert_eq!(o.platform.class, None);
        assert_eq!(o.platform.flag.as_deref(), Some("UK"));
        assert!(
            o.position.altitude_hae_m.is_some_and(|a| a > 1000.0),
            "{o:?}"
        );
        assert!(o.kinematics.speed_mps.is_some_and(|v| v > 30.0), "{o:?}");
        assert_eq!(o.provenance.tracker, None);
    }

    let counts = pipeline.take_counts();
    let tracks: BTreeSet<&str> = out.iter().map(|o| o.source_track_key.as_str()).collect();
    eprintln!(
        "{packets} packets, {} plots, {} reports, {} tracks",
        counts.plots,
        out.len(),
        tracks.len()
    );
    assert!(counts.plots > 1000, "{counts:?}");
    let long = spans(&out).into_iter().filter(|s| *s >= 30).count();
    assert!(
        long * 10 >= tracks.len() * 8,
        "{long} of {} tracks ran 30 s",
        tracks.len()
    );
    assert!(tracks.len() >= 5, "{tracks:?}");
    assert!(tracks.iter().all(|k| k.starts_with('G')));
    for o in &out {
        // Tracker output: anonymous, unknown affiliation, ground at most.
        assert!(o.identifiers.is_empty() && o.name.is_none());
        assert_eq!(o.classification.cot_type_or_derived(), "a-u-G");
        assert_eq!(o.track_type, Some(ot_core::TrackType::LiveTraining));
        // Over Adelaide.
        assert!((-36.0..-33.0).contains(&o.position.latitude), "{o:?}");
        assert!((137.0..140.0).contains(&o.position.longitude), "{o:?}");
    }
    // Some tracks run for a while, not only fragments.
    let long = spans(&out).into_iter().filter(|s| *s >= 30).count();
    eprintln!("{long} tracks of 30 s or more");
    assert!(long >= 3);
}

/// A sensor that comes back only every 12.8 s: with windows set by hand for
/// a fast revisit nothing would confirm; timed by the revisit, tracks do.
#[test]
fn a_slow_revisit_recording_tunes_the_tracker_itself() {
    let Some(path) = sample("StuartHwy_115") else {
        return;
    };
    let (out, pipeline, _, _) = replay(&path);
    let timing = pipeline.tracker_timing().unwrap().clone();
    assert_eq!(timing.revisit_source, "measured");
    // The first whole revisit's length; later ones are within 5%, not retimed.
    assert!((timing.revisit_secs - 12.8).abs() < 0.64, "{timing:?}");
    // Multiples of a slow revisit are above the floors.
    assert!((timing.drop_tentative_secs - 1.3 * timing.revisit_secs).abs() < 1e-9);
    assert!((timing.drop_confirmed_secs - 2.5 * timing.revisit_secs).abs() < 1e-9);
    let ground: Vec<_> = out
        .into_iter()
        .filter(|o| o.source_track_key != "DEAP")
        .collect();
    let spans = spans(&ground);
    eprintln!(
        "{} reports, {} tracks, {} of 30 s or more",
        ground.len(),
        spans.len(),
        spans.iter().filter(|s| **s >= 30).count()
    );
    // gmti-wide-area (clutter density 2e-7): 8 tracks, 5 of them long.
    assert!(spans.len() >= 5, "{spans:?}");
    assert!(spans.iter().filter(|s| **s >= 30).count() >= 5, "{spans:?}");
}

/// Every symbol the example gives a platform type is a CoT type, with the
/// domain that type implies.
#[test]
fn platform_types_have_valid_symbols() {
    let spec: SourceSpec =
        serde_json::from_str(include_str!("../../../docs/examples/stanag4607.json")).unwrap();
    let json = serde_json::to_value(&spec.pipeline.mapping).unwrap();
    let rule = json["rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "platform")
        .unwrap();
    let cot = &rule["fields"]["classification.cot_type"];
    let domain = |t: &str| {
        ot_core::Classification {
            cot_type: Some(t.into()),
            ..Default::default()
        }
        .effective_domain()
    };
    for (code, t) in cot["table"].as_object().unwrap() {
        let t = t.as_str().unwrap();
        let sidc = ot_core::Sidc::parse(t).unwrap_or_else(|| panic!("{code}: {t}"));
        assert_eq!(sidc.code, t);
        let want = match code.as_str() {
            "10" => ot_core::Domain::Space,
            "13" | "14" | "15" | "33" => ot_core::Domain::Ground,
            _ => ot_core::Domain::Air,
        };
        assert_eq!(domain(t), Some(want), "{code}: {t}");
    }
    assert_eq!(cot["table_default"], "a-f-A");
}

/// The tracker's input: every located target report the example maps is a
/// plot with the ground error ellipse the decoder propagated from the sigma
/// options (the recordings state no accuracy of their own).
#[test]
fn gmti_plots_carry_ground_error_ellipses() {
    let Some(path) = sample("StuartHwy_48") else {
        return;
    };
    let spec: SourceSpec =
        serde_json::from_str(include_str!("../../../docs/examples/stanag4607.json")).unwrap();
    let mut decoder = ot_source::plugin::plugin("stanag4607")
        .unwrap()
        .decoder(&replay_options())
        .unwrap();
    let records = decoder
        .decode(&std::fs::read(&path).unwrap(), chrono::Utc::now())
        .unwrap();
    let received = chrono::Utc::now();
    let (mut plots, mut located) = (0, 0);
    for r in &records {
        if r["record"] != "target" {
            continue;
        }
        located += usize::from(r.get("lat").is_some());
        let ot_source::mapping::MapOutcome::Mapped(mapped) = spec.pipeline.mapping.apply(r) else {
            continue;
        };
        for m in mapped.iter().filter(|m| m.rule == "target") {
            let o = ot_source::mapping::finalize(&spec.id, 2, m, received).unwrap();
            let u = o.uncertainty.unwrap_or_else(|| panic!("{r}"));
            let e = u.ellipse.unwrap_or_else(|| panic!("{r}"));
            let g = &r["ground_error"];
            assert_eq!(g["source"], "options");
            assert_eq!(Some(e.semi_major_m), g["semi_major_m"].as_f64());
            assert_eq!(Some(e.semi_minor_m), g["semi_minor_m"].as_f64());
            assert_eq!(Some(e.orientation_deg), g["orientation_deg"].as_f64());
            // The 20 m range sigma only grows when projected to the ground.
            assert!(e.semi_major_m >= 20.0, "{e:?}");
            assert!(
                e.semi_minor_m > 0.0 && e.semi_minor_m <= e.semi_major_m,
                "{e:?}"
            );
            assert_eq!(u.circular_error_m, None);
            plots += 1;
        }
    }
    eprintln!("{plots} plots with ellipses of {located} located targets");
    assert!(plots > 1000);
    assert_eq!(plots, located);
}
