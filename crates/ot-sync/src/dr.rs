//! Dead reckoning: a reporter sends a track when the other nodes'
//! prediction of it (the last report, carried forward at its course and
//! speed) has drifted from its own estimate by more than a threshold, and
//! otherwise every [`HEARTBEAT_MS`]. A ship on a steady course costs one
//! report every 12 s; a manoeuvring aircraft, as many as it needs.

use ot_core::{Domain, TrackState};

use crate::r2::HEARTBEAT_MS;

/// What was last sent about a track.
#[derive(Debug, Clone, PartialEq)]
pub struct Sent {
    pub time_ms: i64,
    pub lat: f64,
    pub lon: f64,
    pub course_deg: Option<f64>,
    pub speed_mps: Option<f64>,
    pub state: TrackState,
}

/// Drift allowed before a report is due.
pub fn threshold_m(domain: Option<Domain>) -> f64 {
    match domain {
        Some(Domain::Air) | Some(Domain::Space) => 200.0,
        _ => 50.0,
    }
}

const R: f64 = 6_371_008.8;

/// Where the receivers think the track is at `time_ms`.
pub fn predict(s: &Sent, time_ms: i64) -> (f64, f64) {
    let (Some(c), Some(v)) = (s.course_deg, s.speed_mps) else {
        return (s.lat, s.lon);
    };
    let d = v * ((time_ms - s.time_ms) as f64 / 1000.0).max(0.0);
    let (n, e) = (d * c.to_radians().cos(), d * c.to_radians().sin());
    let lat = s.lat + (n / R).to_degrees();
    let lon = s.lon + (e / (R * s.lat.to_radians().cos().max(1e-6))).to_degrees();
    (lat, lon)
}

/// Distance in metres (equirectangular; fine at report spacings).
pub fn distance_m(a: (f64, f64), b: (f64, f64)) -> f64 {
    let x = (b.1 - a.1).to_radians() * ((a.0 + b.0) / 2.0).to_radians().cos();
    let y = (b.0 - a.0).to_radians();
    R * (x * x + y * y).sqrt()
}

/// How urgently `now` (this node's estimate) must be sent, given what was
/// sent; none: not due. A drift within the track's own position error
/// (`error_m`, the error ellipse's semi-major axis) is noise the receivers
/// already allow for, so it is not worth a report.
pub fn urgency(
    sent: Option<&Sent>,
    now: &Sent,
    domain: Option<Domain>,
    error_m: f64,
) -> Option<f64> {
    let Some(s) = sent else {
        return Some(URGENT);
    };
    if s.state != now.state {
        return Some(URGENT);
    }
    let allowed = threshold_m(domain).max(if error_m.is_finite() { error_m } else { 0.0 });
    let drift = distance_m(predict(s, now.time_ms), (now.lat, now.lon)) / allowed;
    if drift > 1.0 {
        return Some(drift);
    }
    (now.time_ms - s.time_ms >= HEARTBEAT_MS).then_some(HEARTBEAT)
}

/// Urgency of a new track or a state change, and of a heartbeat.
pub const URGENT: f64 = 1000.0;
pub const HEARTBEAT: f64 = 0.5;

/// Whether `now` must be sent (see [`urgency`]).
pub fn due(sent: Option<&Sent>, now: &Sent, domain: Option<Domain>) -> bool {
    urgency(sent, now, domain, 0.0).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(t: i64, lat: f64, lon: f64) -> Sent {
        Sent {
            time_ms: t,
            lat,
            lon,
            course_deg: Some(90.0),
            speed_mps: Some(10.0),
            state: TrackState::Confirmed,
        }
    }

    #[test]
    fn a_steady_track_waits_for_its_heartbeat() {
        let first = at(0, 50.0, -1.0);
        // 10 s east at 10 m/s: exactly where it was predicted.
        let (lat, lon) = predict(&first, 10_000);
        assert!((distance_m((50.0, -1.0), (lat, lon)) - 100.0).abs() < 0.5);
        assert!(!due(
            Some(&first),
            &at(10_000, lat, lon),
            Some(Domain::Surface)
        ));
        assert!(due(
            Some(&first),
            &at(12_000, lat, lon),
            Some(Domain::Surface)
        ));
        assert!(due(None, &first, None));
    }

    #[test]
    fn noise_within_the_tracks_own_error_is_not_sent() {
        let first = at(0, 50.0, -1.0);
        let (lat, lon) = predict(&first, 5_000);
        // 80 m off the prediction: past the 50 m threshold, but within a
        // radar track's 100 m error.
        let off = at(5_000, lat + (80.0 / R).to_degrees(), lon);
        assert!(urgency(Some(&first), &off, Some(Domain::Surface), 100.0).is_none());
        assert!(urgency(Some(&first), &off, Some(Domain::Surface), 10.0).unwrap() > 1.0);
        assert_eq!(urgency(None, &off, None, 100.0), Some(URGENT));
        let late = at(12_000, lat, lon);
        assert_eq!(urgency(Some(&first), &late, None, 100.0), Some(HEARTBEAT));
    }

    #[test]
    fn a_turn_is_sent_once_it_drifts_past_the_threshold() {
        let first = at(0, 50.0, -1.0);
        // It turned north instead: 5 s later it is ~70 m from the prediction.
        let turned = at(5_000, 50.0 + (50.0 / R).to_degrees(), -1.0);
        assert!(due(Some(&first), &turned, Some(Domain::Surface)));
        assert!(
            !due(Some(&first), &turned, Some(Domain::Air)),
            "an aircraft allows 200 m"
        );
        let mut lost = at(1_000, 50.0, -1.0 + 0.000_14);
        lost.state = TrackState::Lost;
        assert!(due(Some(&first), &lost, None));
    }
}
