//! An example tracker plugin: nearest-neighbour association with an
//! alpha-beta filter, the kind of tracker inside many radars. Plots within
//! `gate_m` of a track's prediction update it (nearest first); the rest
//! start tentative tracks. A track is reported once it has `confirm_hits`
//! plots, and ends `drop_secs` after its last one.
//!
//! Options: `gate_m` (100), `alpha` (0.5), `beta` (0.2), `confirm_hits` (3),
//! `drop_secs` (20).

use opentrack_plugin::{Observation, Tracker, Value, json, log, plugin};

struct Track {
    id: u64,
    last: Observation,
    t: f64,
    x: f64,
    y: f64,
    vx: f64,
    vy: f64,
    hits: u32,
    reported: bool,
}

struct AlphaBeta {
    gate_m: f64,
    alpha: f64,
    beta: f64,
    confirm_hits: u32,
    drop_secs: f64,
    /// Local frame origin (the first plot): metres east and north of it.
    origin: Option<(f64, f64)>,
    tracks: Vec<Track>,
    pending: Vec<Observation>,
    next: u64,
}

const R: f64 = 6_378_137.0;

impl AlphaBeta {
    fn xy(&mut self, lat: f64, lon: f64) -> (f64, f64) {
        let (lat0, lon0) = *self.origin.get_or_insert((lat, lon));
        (
            (lon - lon0).to_radians() * R * lat0.to_radians().cos(),
            (lat - lat0).to_radians() * R,
        )
    }

    fn latlon(&self, x: f64, y: f64) -> (f64, f64) {
        let (lat0, lon0) = self.origin.unwrap_or_default();
        (
            lat0 + (y / R).to_degrees(),
            lon0 + (x / (R * lat0.to_radians().cos())).to_degrees(),
        )
    }

    fn report(&self, t: &Track) -> Observation {
        let mut o = t.last.clone();
        let (lat, lon) = self.latlon(t.x, t.y);
        o.source_track_key = format!("AB{}", t.id);
        o.position.latitude = lat;
        o.position.longitude = lon;
        let speed = t.vx.hypot(t.vy);
        o.kinematics.speed_mps = Some((speed * 100.0).round() / 100.0);
        o.kinematics.course_deg = (speed > 0.5).then(|| t.vx.atan2(t.vy).to_degrees().rem_euclid(360.0));
        o
    }
}

fn number(options: &Value, key: &str, default: f64) -> Result<f64, String> {
    match options.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v
            .as_f64()
            .filter(|n| *n > 0.0)
            .ok_or_else(|| format!("{key}: a positive number")),
    }
}

impl Tracker for AlphaBeta {
    fn open(options: Value) -> Result<Self, String> {
        let t = Self {
            gate_m: number(&options, "gate_m", 100.0)?,
            alpha: number(&options, "alpha", 0.5)?,
            beta: number(&options, "beta", 0.2)?,
            confirm_hits: number(&options, "confirm_hits", 3.0)? as u32,
            drop_secs: number(&options, "drop_secs", 20.0)?,
            origin: None,
            tracks: Vec::new(),
            pending: Vec::new(),
            next: 1,
        };
        log::info(&format!("alpha-beta tracker: gate {} m", t.gate_m));
        Ok(t)
    }

    fn push(&mut self, plot: Observation, _received_at_ms: i64) {
        self.pending.push(plot);
    }

    fn run(&mut self, now_ms: i64, _force: bool) -> Result<Vec<Observation>, String> {
        let mut plots = std::mem::take(&mut self.pending);
        plots.sort_by_key(|p| p.observed_at);
        let mut out = Vec::new();
        for p in plots {
            let t = p.observed_at.timestamp_millis() as f64 / 1000.0;
            let (x, y) = self.xy(p.position.latitude, p.position.longitude);
            // The nearest prediction inside the gate.
            let best = self
                .tracks
                .iter()
                .enumerate()
                .map(|(i, tr)| {
                    let dt = (t - tr.t).max(0.0);
                    (i, (tr.x + tr.vx * dt - x).hypot(tr.y + tr.vy * dt - y))
                })
                .filter(|(_, d)| *d <= self.gate_m)
                .min_by(|a, b| a.1.total_cmp(&b.1));
            match best {
                Some((i, _)) => {
                    let (alpha, beta) = (self.alpha, self.beta);
                    let tr = &mut self.tracks[i];
                    let dt = (t - tr.t).max(1e-3);
                    let (px, py) = (tr.x + tr.vx * dt, tr.y + tr.vy * dt);
                    let (rx, ry) = (x - px, y - py);
                    if tr.hits == 1 {
                        (tr.vx, tr.vy) = ((x - tr.x) / dt, (y - tr.y) / dt);
                        (tr.x, tr.y) = (x, y);
                    } else {
                        (tr.x, tr.y) = (px + alpha * rx, py + alpha * ry);
                        tr.vx += beta * rx / dt;
                        tr.vy += beta * ry / dt;
                    }
                    tr.t = t;
                    tr.hits += 1;
                    tr.last = p;
                    if tr.hits >= self.confirm_hits {
                        tr.reported = true;
                        let tr = &self.tracks[i];
                        out.push(self.report(tr));
                    }
                }
                None => {
                    self.tracks.push(Track {
                        id: self.next,
                        last: p,
                        t,
                        x,
                        y,
                        vx: 0.0,
                        vy: 0.0,
                        hits: 1,
                        reported: false,
                    });
                    self.next += 1;
                }
            }
        }
        // End tracks that went quiet; a reported one says so.
        let now = now_ms as f64 / 1000.0;
        let (gone, kept): (Vec<Track>, Vec<Track>) = std::mem::take(&mut self.tracks)
            .into_iter()
            .partition(|tr| now - tr.t > self.drop_secs);
        self.tracks = kept;
        for tr in gone.iter().filter(|tr| tr.reported) {
            let mut o = self.report(tr);
            o.state = Some(opentrack_plugin::serde_json::from_value(json!("dropped")).map_err(|e| e.to_string())?);
            out.push(o);
        }
        Ok(out)
    }
}

plugin! {
    manifest: json!({
        "name": "alpha-beta",
        "version": "1",
        "description": "Example tracker: nearest-neighbour association with an alpha-beta filter",
        "options": [
            {"name": "gate_m", "label": "Gate (m)", "type": "number", "default": 100.0, "unit": "m", "min": 0.0,
             "help": "Plots further than this from a track's prediction start a new track"},
            {"name": "alpha", "label": "Alpha", "type": "number", "default": 0.5, "min": 0.0,
             "help": "Position gain: how far a track moves toward each plot"},
            {"name": "beta", "label": "Beta", "type": "number", "default": 0.2, "min": 0.0,
             "help": "Velocity gain"},
            {"name": "confirm_hits", "label": "Confirm after", "type": "number", "default": 3.0, "min": 1.0,
             "help": "Plots before a track is reported"},
            {"name": "drop_secs", "label": "Drop after (s)", "type": "number", "default": 20.0, "unit": "s", "min": 0.0,
             "help": "Seconds without a plot before a track ends"}
        ]
    }),
    tracker: AlphaBeta,
}
