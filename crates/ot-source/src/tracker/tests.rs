use std::collections::HashMap;

use chrono::{DateTime, Utc};
use ot_core::{Affiliation, Domain, Identifier, Observation};
use serde_json::json;

use super::filter::moved;
use super::*;

fn at(ms: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(1_790_000_000_000 + ms).unwrap()
}

fn spec(algorithm: &str) -> TrackerSpec {
    serde_json::from_value(json!({ "algorithm": algorithm })).unwrap()
}

/// A detection `s` seconds in of a boat moving 4 m/s east, claiming far
/// more than a tracker may pass on.
fn loud_plot(s: i64) -> Observation {
    let (lat, lon) = moved(63.44, 10.40, 0.0, 4.0 * s as f64);
    let mut o: Observation = serde_json::from_value(json!({
        "schema_version": 1, "source_id": "radar", "source_track_key": format!("p{s}"),
        "observed_at": at(s * 1000), "received_at": at(s * 1000),
        "position": {"latitude": lat, "longitude": lon},
        "uncertainty": {"circular_error_m": 5.0},
        "name": "NOT FROM A TRACKER", "callsign": "NOPE",
        "classification": {"cot_type": "a-h-S-C-L-D-D", "affiliation": "hostile", "sidc": "SHSPCLDD--*****"},
        "platform": {"class": "destroyer", "type_code": "DDG"},
        "provenance": {"sensor_code": "R1"},
        "ext": {"rcs": 30}
    }))
    .unwrap();
    o.identifiers = vec![Identifier::new("mmsi", "366999999")];
    o
}

fn run_all(t: &mut Tracker, plots: Vec<Observation>) -> Vec<Observation> {
    let mut out = Vec::new();
    for p in plots {
        let r = p.received_at;
        t.push(p, r);
        out.extend(t.run(r, true).into_iter().map(|(o, _)| o));
    }
    out
}

#[test]
fn every_tracker_classifies_no_higher_than_unknown_and_a_domain() {
    for algorithm in ["gnn", "mht"] {
        let mut t = Tracker::new(spec(algorithm)).unwrap();
        let out = run_all(&mut t, (0..8).map(loud_plot).collect());
        assert!(!out.is_empty(), "{algorithm}: the boat is tracked");
        for o in &out {
            assert_eq!(
                o.classification.affiliation,
                Some(Affiliation::Unknown),
                "{algorithm}"
            );
            // The plots' own symbol says surface: the track may know that much.
            assert_eq!(
                o.classification.domain,
                Some(Domain::Surface),
                "{algorithm}"
            );
            assert_eq!(o.classification.cot_type, None);
            assert_eq!(o.classification.sidc, None);
            assert_eq!(o.classification.cot_type_or_derived(), "a-u-S");
            assert!(o.identifiers.is_empty() && o.name.is_none() && o.callsign.is_none());
            assert_eq!(o.platform, Default::default());
            assert!(o.ext.is_empty());
            // Where the report came from is kept, and which tracker made it.
            assert_eq!(o.provenance.sensor_code.as_deref(), Some("R1"));
            assert_eq!(o.provenance.tracker.as_deref(), Some(t.version()));
            assert!(t.version().starts_with(algorithm));
            assert!(o.source_track_key.starts_with('T'));
            ot_core::Observation::validate(o).unwrap();
        }
        let last = out.last().unwrap();
        assert!(
            (last.kinematics.speed_mps.unwrap() - 4.0).abs() < 1.0,
            "{algorithm}: {last:?}"
        );
    }
}

#[test]
fn domain_comes_from_the_spec_or_the_plots_and_never_space() {
    let mut s = spec("gnn");
    s.domain = Some(TrackerDomain::Subsurface);
    let mut t = Tracker::new(s).unwrap();
    let out = run_all(&mut t, (0..4).map(loud_plot).collect());
    assert_eq!(out[0].classification.domain, Some(Domain::Subsurface));

    // Plots that say space: a tracker leaves the domain unset.
    let mut t = Tracker::new(spec("gnn")).unwrap();
    let plots: Vec<Observation> = (0..4)
        .map(|s| {
            let mut o = loud_plot(s);
            o.classification = Default::default();
            o.classification.domain = Some(Domain::Space);
            o
        })
        .collect();
    let out = run_all(&mut t, plots);
    assert_eq!(out[0].classification.domain, None);
    assert_eq!(tracker_classification(Some(Domain::Space)).domain, None);
}

#[test]
fn time_scans_wait_for_their_plots_and_late_ones_are_dropped() {
    let mut s = spec("gnn");
    s.scans = ScanGrouping::Time;
    s.confirm_hits = 1;
    // Confirm on a first plot: its existence is the birth prior, ~0.09.
    s.confirm_probability = 0.05;
    let mut t = Tracker::new(s).unwrap();
    // Two plots of one scan in separate frames.
    let (a, mut b) = (loud_plot(0), loud_plot(0));
    b.position.latitude += 0.01;
    t.push(a, at(0));
    assert!(t.run(at(100), false).is_empty(), "held for more plots");
    t.push(b, at(200));
    assert_eq!(
        t.run(at(800), false).len(),
        2,
        "both reported once the hold passed"
    );
    // A plot from before that scan arrives late.
    let mut late = loud_plot(0);
    late.observed_at = at(-500);
    t.push(late, at(900));
    assert!(t.run(at(900), true).is_empty());
    assert_eq!(t.late, 1);
}

#[test]
fn clustering_merges_one_objects_plots() {
    let mut s = spec("gnn");
    s.confirm_hits = 1;
    s.cluster_m = 10.0;
    // Confirm on a first plot: its existence is the birth prior, ~0.09.
    s.confirm_probability = 0.05;
    let mut t = Tracker::new(s).unwrap();
    // Two points 6 m apart on one hull, and one 80 m away.
    let a = loud_plot(0);
    let mut b = loud_plot(0);
    let (lat, lon) = moved(a.position.latitude, a.position.longitude, 6.0, 0.0);
    b.position.latitude = lat;
    b.position.longitude = lon;
    let mut c = loud_plot(0);
    c.position.latitude += 80.0 / 111_320.0;
    for p in [a, b, c] {
        t.push(p, at(0));
    }
    let out = t.run(at(0), true);
    assert_eq!(out.len(), 2);
    let (lat, _) = moved(63.44, 10.40, 3.0, 0.0);
    assert!(
        out.iter()
            .any(|(o, _)| (o.position.latitude - lat).abs() < 1e-6),
        "at the centroid"
    );
}

#[test]
fn every_tracker_says_when_it_drops_a_track() {
    for algorithm in ["gnn", "mht"] {
        let mut t = Tracker::new(spec(algorithm)).unwrap();
        let mut out = run_all(&mut t, (0..6).map(loud_plot).collect());
        // The boat vanishes; the sensor keeps scanning (one plot far away).
        for s in 6..20 {
            let mut far = loud_plot(s);
            far.position.latitude += 0.05;
            out.extend(run_all(&mut t, vec![far]));
        }
        let ends: Vec<&Observation> = out
            .iter()
            .filter(|o| o.state == Some(ot_core::TrackState::Dropped))
            .collect();
        assert_eq!(ends.len(), 1, "{algorithm}: {ends:?}");
        let key = &out[0].source_track_key;
        assert_eq!(&ends[0].source_track_key, key, "{algorithm}");
        assert_eq!(
            ends[0].classification.affiliation,
            Some(Affiliation::Unknown)
        );
        // Nothing more under that key after its end.
        let after = out.iter().skip_while(|o| o.state.is_none()).skip(1);
        assert!(
            after.filter(|o| &o.source_track_key == key).count() == 0,
            "{algorithm}"
        );
    }
}

#[test]
fn rejects_nonsense_settings() {
    let mut s = spec("mht");
    s.mht.detection_probability = Some(1.0);
    assert!(Tracker::new(s).is_err());
    let mut s = spec("gnn");
    s.detection_probability = 1.0;
    assert!(Tracker::new(s).is_err());
    let mut s = spec("gnn");
    s.drop_probability = 0.99;
    assert!(Tracker::new(s).is_err());
    let mut s = spec("gnn");
    s.gate = 0.0;
    assert!(Tracker::new(s).is_err());
    assert!(
        serde_json::from_value::<TrackerSpec>(json!({"algorithm": "gnn", "domain": "space"}))
            .is_err()
    );
}

// --- Autoferry: real lidar and radar detections ---

#[derive(serde::Deserialize)]
struct Recorded {
    feed: String,
    truth: Option<u64>,
    obs: Observation,
}

/// Per target: the reports of its best track and that track's purity; and
/// how many confirmed tracks follow clutter.
fn score(
    algorithm: &str,
    scenario: u32,
    feed: &str,
    spec_patch: serde_json::Value,
) -> (Vec<(usize, f64)>, usize) {
    let path = format!(
        "{}/../ot-server/tests/data/autoferry/scenario{scenario}-detections.jsonl",
        env!("CARGO_MANIFEST_DIR")
    );
    let rows: Vec<Recorded> = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .filter(|r: &Recorded| r.feed == feed)
        .collect();
    let label: HashMap<String, Option<u64>> = rows
        .iter()
        .map(|r| (r.obs.source_track_key.clone(), r.truth))
        .collect();
    let mut s = spec(algorithm);
    let mut v = serde_json::to_value(&s).unwrap();
    for (k, x) in spec_patch.as_object().unwrap() {
        v[k] = x.clone();
    }
    s = serde_json::from_value(v).unwrap();
    let mut t = Tracker::new(s).unwrap();
    // Each scan as one frame, as the dataset has them.
    let mut tracks: HashMap<String, HashMap<Option<u64>, usize>> = HashMap::new();
    let mut i = 0;
    while i < rows.len() {
        let when = rows[i].obs.observed_at;
        while i < rows.len() && rows[i].obs.observed_at == when {
            t.push(rows[i].obs.clone(), when);
            i += 1;
        }
        for (o, det) in t.run(when, true) {
            *tracks
                .entry(o.source_track_key)
                .or_default()
                .entry(label[&det.source_track_key])
                .or_default() += 1;
        }
    }
    let mut best = Vec::new();
    for k in [1, 2] {
        let b = tracks
            .values()
            .filter_map(|c| {
                let total: usize = c.values().sum();
                let n = *c.get(&Some(k)).unwrap_or(&0);
                (n * 2 > total).then_some((n, n as f64 / total as f64))
            })
            .max_by_key(|x| x.0)
            .unwrap_or((0, 0.0));
        best.push(b);
    }
    let false_tracks = tracks
        .values()
        .filter(|c| {
            let total: usize = c.values().sum();
            *c.get(&None).unwrap_or(&0) * 2 > total
        })
        .count();
    eprintln!(
        "scenario {scenario} {feed} {algorithm}: best track per target (reports, purity) {best:?}; {false_tracks} clutter tracks; {} total",
        tracks.len()
    );
    (best, false_tracks)
}

#[test]
fn autoferry_detections_become_clean_tracks() {
    // Both vessels get a long, pure track from each sensor, by both algorithms.
    let lidar = json!({"measurement_sigma_m": 4.0, "domain": "surface", "cluster_m": 10.0,
                       "detection_probability": 0.8});
    let radar = json!({"measurement_sigma_m": 6.0, "domain": "surface",
                       "detection_probability": 0.9, "clutter_density": 4e-6,
                       "birth_density": 1e-7, "mht": {"n_scan": 3, "max_branches": 20}});
    for algorithm in ["gnn", "mht"] {
        for scenario in [2, 16] {
            for (feed, patch, min_reports) in [("lidar-det", &lidar, 60), ("radar-det", &radar, 30)]
            {
                let (best, _) = score(algorithm, scenario, feed, patch.clone());
                for (target, (n, purity)) in best.iter().enumerate() {
                    assert!(
                        *n >= min_reports && *purity >= 0.95,
                        "{algorithm} scenario {scenario} {feed} target {}: {n} reports, purity {purity:.2}",
                        target + 1
                    );
                }
            }
        }
    }
}

#[test]
fn auto_timing_sizes_windows_to_the_revisit() {
    // Without auto_timing the revisit is ignored.
    let mut t = Tracker::new(spec("gnn")).unwrap();
    t.set_revisit(12.0, "measured");
    assert!(t.timing().is_none());
    assert_eq!(t.spec().confirm_within_secs, 5.0);

    let auto = |a: serde_json::Value| -> TrackerSpec {
        serde_json::from_value(json!({ "algorithm": "gnn", "auto_timing": a })).unwrap()
    };
    assert!(
        auto(json!({ "drop_tentative_revisits": 1.0 }))
            .validate()
            .is_err()
    );
    assert!(
        auto(json!({ "min_confirm_secs": -1.0 }))
            .validate()
            .is_err()
    );

    for algorithm in ["gnn", "mht"] {
        let mut s = auto(json!({}));
        s.algorithm = spec(algorithm).algorithm;
        let mut t = Tracker::new(s).unwrap();
        // A fast revisit: the floors.
        t.set_revisit(2.5, "measured");
        let w = t.timing().unwrap().clone();
        assert_eq!(
            (
                w.confirm_within_secs,
                w.drop_tentative_secs,
                w.drop_confirmed_secs
            ),
            (20.0, 8.0, 30.0)
        );
        // A slow one: multiples of it. Jitter within 5% changes nothing.
        t.set_revisit(12.0, "measured");
        t.set_revisit(12.4, "measured");
        let w = t.timing().unwrap().clone();
        assert_eq!(w.revisit_secs, 12.0);
        assert!((w.confirm_within_secs - 42.0).abs() < 1e-9);
        assert!((w.drop_tentative_secs - 15.6).abs() < 1e-9);
        assert!((w.drop_confirmed_secs - 30.0).abs() < 1e-9);
        assert_eq!(t.spec().drop_tentative_secs, w.drop_tentative_secs);

        // A target seen once every 12 s confirms and is kept (the default
        // windows, 5 s and 3 s, would drop it before it is seen again).
        let mut out = Vec::new();
        for s in [0, 12, 24, 36] {
            t.push(loud_plot(s), at(s * 1000));
            out.extend(t.run(at(s * 1000), true));
        }
        let keys: std::collections::BTreeSet<_> = out
            .iter()
            .map(|(o, _)| o.source_track_key.clone())
            .collect();
        assert_eq!(keys.len(), 1, "{algorithm}: {keys:?}");
        assert!(out.len() >= 2, "{algorithm}: {}", out.len());
    }
}

#[test]
fn reports_carry_existence_and_the_full_covariance() {
    for algorithm in ["gnn", "mht"] {
        let mut t = Tracker::new(spec(algorithm)).unwrap();
        let out = run_all(&mut t, (0..8).map(loud_plot).collect());
        assert!(!out.is_empty(), "{algorithm}");
        let mut last = 0.0;
        for o in &out {
            let p = o.provenance.confidence.unwrap();
            assert!(
                (0.95..=1.0).contains(&p),
                "{algorithm}: confirmed at 0.95, got {p}"
            );
            assert!(p >= last - 1e-9, "{algorithm}: plot after plot raises it");
            last = p;
            let u = o.uncertainty.unwrap();
            let c = u.covariance.unwrap();
            assert!(c.velocity.is_some() && c.cross.is_some());
            let e = u.ellipse.unwrap();
            assert!(e.semi_major_m >= e.semi_minor_m && e.semi_minor_m > 0.0);
            o.validate().unwrap();
        }
    }

    // Misses lower it: a target that stops being seen is dropped by its
    // existence well before the 8 s drop time (one scan per second, Pd 0.9).
    let mut t = Tracker::new(spec("gnn")).unwrap();
    let mut out = run_all(&mut t, (0..6).map(loud_plot).collect());
    for s in 6..12 {
        // Another target far away keeps the scans coming.
        let mut other = loud_plot(s);
        other.position.latitude += 0.05;
        t.push(other, at(s * 1000));
        out.extend(t.run(at(s * 1000), true).into_iter().map(|(o, _)| o));
    }
    let dropped: Vec<_> = out
        .iter()
        .filter(|o| o.state == Some(ot_core::TrackState::Dropped))
        .collect();
    assert_eq!(dropped.len(), 1, "{out:#?}");
    // Last plot at 5 s: the 8 s drop time would end it at 13 s.
    assert!(
        dropped[0].observed_at < at(13_000),
        "dropped by existence, not the 8 s drop time: {:?}",
        dropped[0].observed_at
    );
}

#[test]
fn a_miss_counts_once_per_revisit_when_the_revisit_is_known() {
    let mut s = spec("gnn");
    s.revisit_secs = Some(10.0);
    let plot = Plot {
        lat: 0.0,
        lon: 0.0,
        r: [100.0, 0.0, 100.0],
        domain: None,
    };
    let mut l = Life::new(at(0), &plot, &s);
    l.existence = 0.99;
    // Scans every second: within a revisit, nothing changes.
    for k in 1..10 {
        l.miss(at(k * 1000), &s);
    }
    assert!(l.existence > 0.98, "{}", l.existence);
    // A whole revisit without a plot: one miss.
    l.miss(at(10_000), &s);
    // The revisit's decay over the target's lifetime, then one miss (Pd 0.9).
    let before = 0.99 * (-10.0f64 / 600.0).exp();
    let one = before * 0.1 / (before * 0.1 + 1.0 - before);
    assert!((l.existence - one).abs() < 1e-9, "{} vs {one}", l.existence);
}
