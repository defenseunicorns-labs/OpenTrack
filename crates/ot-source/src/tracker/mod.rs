//! The optional tracker stage: turns a source's detections (anonymous plots)
//! into source tracks before correlation, with global nearest neighbour
//! (GNN) or multiple hypothesis tracking (MHT) association.
//!
//! Both share one Kalman filter ([`filter`]) and one track lifecycle: a track
//! is confirmed after `confirm_hits` plots within `confirm_within_secs`, and
//! dropped after going `drop_tentative_secs` (unconfirmed) or
//! `drop_confirmed_secs` without one. Only confirmed tracks are reported,
//! each time a plot updates them, and once more with `state: dropped` when
//! the tracker gives up on them (so correlation can let them go).
//!
//! A tracker knows where something is and how it moves, not what it is. Its
//! tracks carry no identity, and at most an unknown affiliation and a domain
//! (see [`tracker_classification`]): every tracker's output goes through it.

pub mod assign;
pub mod filter;
mod gnn;
mod mht;

use chrono::{DateTime, Utc};
use ot_core::{
    Affiliation, Classification, Domain, Kinematics, Observation, Position, Uncertainty,
};
use serde::{Deserialize, Serialize};

use filter::Kf;

/// Versions of the trackers' behaviour, stamped on every report as
/// `provenance.tracker`. Bump one whenever what that tracker outputs for the
/// same plots changes, and record it in docs/algorithms.md with its scores.
pub const GNN_VERSION: &str = "gnn-2";
pub const MHT_VERSION: &str = "mht-2";

/// Settings of the tracker stage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrackerSpec {
    pub algorithm: Algorithm,
    /// Plot standard deviation per axis (m), when a detection reports no uncertainty.
    #[serde(default = "d_sigma")]
    pub measurement_sigma_m: f64,
    /// White-acceleration process noise (m/s²): how hard targets manoeuvre.
    #[serde(default = "d_q")]
    pub process_noise_mps2: f64,
    /// Velocity standard deviation of a new track (m/s).
    #[serde(default = "d_v0")]
    pub initial_speed_sigma_mps: f64,
    /// Chi-square gate (2 degrees of freedom; 13.8 keeps 99.9% of true plots).
    #[serde(default = "d_gate")]
    pub gate: f64,
    /// Fewest plots a track needs before it can be confirmed.
    #[serde(default = "d_confirm_hits")]
    pub confirm_hits: usize,
    /// Superseded by `confirm_probability` (gnn-2, mht-2); still accepted,
    /// and still sized by `auto_timing`, but no longer used.
    #[serde(default = "d_confirm_secs")]
    pub confirm_within_secs: f64,
    /// Chance the sensor detects a target it looks at.
    #[serde(default = "d_pd")]
    pub detection_probability: f64,
    /// False plots per square metre per look.
    #[serde(default = "d_clutter")]
    pub clutter_density: f64,
    /// New targets per square metre per look.
    #[serde(default = "d_birth")]
    pub birth_density: f64,
    /// How long a target lasts on average (s): a track's existence decays at
    /// this rate between looks, so misses can end even a long-lived track.
    #[serde(default = "d_lifetime")]
    pub target_lifetime_secs: f64,
    /// A track is confirmed once the probability that it is a real target
    /// reaches this (and it has `confirm_hits` plots).
    #[serde(default = "d_confirm_p")]
    pub confirm_probability: f64,
    /// A track is dropped once that probability falls to this.
    #[serde(default = "d_drop_p")]
    pub drop_probability: f64,
    #[serde(default = "d_drop_tentative")]
    pub drop_tentative_secs: f64,
    #[serde(default = "d_drop_confirmed")]
    pub drop_confirmed_secs: f64,
    /// The domain of everything this sensor sees (a surface radar: surface).
    /// Without it, a track takes the domain most of its plots report, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<TrackerDomain>,
    /// What makes a scan: the plots of one frame, or plots with the same time.
    #[serde(default)]
    pub scans: ScanGrouping,
    /// With `scans: time`, how long to wait for more plots of the newest scan (s).
    #[serde(default = "d_hold")]
    pub scan_hold_secs: f64,
    /// Merge a scan's plots closer than this (m) into one, for sensors that
    /// return several points off one large object (lidar on a ship). 0: off.
    #[serde(default)]
    pub cluster_m: f64,
    /// Track keys are this prefix, a tag for the tracker's run and a number (`T3f2a-12`).
    #[serde(default = "d_prefix")]
    pub key_prefix: String,
    #[serde(default)]
    pub mht: MhtSpec,
    /// Set `confirm_within_secs` and the drop times from the sensor's revisit
    /// period, as the source's codec measures it (STANAG 4607), instead of
    /// the fixed values; those apply until the codec knows the period.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_timing: Option<AutoTiming>,
    /// The sensor's revisit period once `auto_timing` knows it: a track is
    /// counted as missed once per revisit without a plot, not once per scan
    /// (a scan may be one dwell that never looked at it).
    #[serde(skip)]
    pub revisit_secs: Option<f64>,
    /// The tracker profile these settings started from (Settings are the
    /// profile's until changed; this only records where they came from).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// With `algorithm: plugin`: the plugin, by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin: Option<String>,
    /// With `algorithm: plugin`: the plugin's options.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub options: serde_json::Value,
}

impl TrackerSpec {
    /// The detection model, `(detection probability, clutter density, birth
    /// density)`; MHT settings from before gnn-2 override it when present.
    pub fn detection_model(&self) -> (f64, f64, f64) {
        (
            self.mht
                .detection_probability
                .unwrap_or(self.detection_probability),
            self.mht.clutter_density.unwrap_or(self.clutter_density),
            self.mht.birth_density.unwrap_or(self.birth_density),
        )
    }
}

/// Tracker windows as multiples of the sensor's revisit period, never
/// shorter than a floor: a sensor that revisits every couple of seconds still
/// misses a target for longer than that (terrain, a stop below the minimum
/// detectable velocity). The default floors suit GMTI of ground movers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutoTiming {
    /// `confirm_hits` plots within this many revisits confirm a track.
    #[serde(default = "d_confirm_revisits")]
    pub confirm_revisits: f64,
    /// An unconfirmed track is dropped this many revisits after its last plot
    /// (above 1, or none survives to the next revisit).
    #[serde(default = "d_drop_tentative_revisits")]
    pub drop_tentative_revisits: f64,
    /// A confirmed track is dropped this many revisits after its last plot.
    #[serde(default = "d_drop_confirmed_revisits")]
    pub drop_confirmed_revisits: f64,
    #[serde(default = "d_min_confirm")]
    pub min_confirm_secs: f64,
    #[serde(default = "d_min_drop_tentative")]
    pub min_drop_tentative_secs: f64,
    #[serde(default = "d_min_drop_confirmed")]
    pub min_drop_confirmed_secs: f64,
}

impl Default for AutoTiming {
    fn default() -> Self {
        serde_json::from_value(serde_json::json!({})).expect("defaults")
    }
}

/// The windows a tracker with `auto_timing` is using, for status.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Timing {
    pub revisit_secs: f64,
    pub revisit_source: &'static str,
    pub confirm_within_secs: f64,
    pub drop_tentative_secs: f64,
    pub drop_confirmed_secs: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Algorithm {
    /// Global nearest neighbour: each scan, the least-cost assignment of plots to tracks.
    Gnn,
    /// Multiple hypothesis tracking: competing assignments kept for a few scans.
    Mht,
    /// A tracker plugin (`plugin`, with its `options`); the other settings
    /// here do not apply to it.
    Plugin,
}

/// The domains a tracker may assign.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackerDomain {
    Ground,
    Air,
    Surface,
    Subsurface,
}

impl TrackerDomain {
    const ALL: [TrackerDomain; 4] = [
        TrackerDomain::Ground,
        TrackerDomain::Air,
        TrackerDomain::Surface,
        TrackerDomain::Subsurface,
    ];

    pub fn domain(self) -> Domain {
        match self {
            TrackerDomain::Ground => Domain::Ground,
            TrackerDomain::Air => Domain::Air,
            TrackerDomain::Surface => Domain::Surface,
            TrackerDomain::Subsurface => Domain::Subsurface,
        }
    }

    fn of(d: Domain) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.domain() == d)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanGrouping {
    /// Each frame is a scan (plots with different times in it are separate scans).
    #[default]
    Frame,
    /// Plots with the same time are a scan, across frames.
    Time,
}

/// MHT settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MhtSpec {
    /// Before gnn-2 and mht-2 the detection model was MHT-only and lived
    /// here; set, these override the stage's `detection_probability`,
    /// `clutter_density` and `birth_density` (sources saved before keep their
    /// meaning).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detection_probability: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clutter_density: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub birth_density: Option<f64>,
    /// Scans before an association is final.
    #[serde(default = "d_n_scan")]
    pub n_scan: usize,
    /// Hypotheses kept per possible target.
    #[serde(default = "d_branches")]
    pub max_branches: usize,
}

impl Default for MhtSpec {
    fn default() -> Self {
        serde_json::from_value(serde_json::json!({})).expect("defaults")
    }
}

fn d_sigma() -> f64 {
    10.0
}
fn d_q() -> f64 {
    0.5
}
fn d_v0() -> f64 {
    5.0
}
fn d_gate() -> f64 {
    13.8
}
fn d_confirm_hits() -> usize {
    3
}
fn d_confirm_secs() -> f64 {
    5.0
}
fn d_drop_tentative() -> f64 {
    3.0
}
fn d_drop_confirmed() -> f64 {
    8.0
}
fn d_confirm_revisits() -> f64 {
    3.5
}
fn d_drop_tentative_revisits() -> f64 {
    1.3
}
fn d_drop_confirmed_revisits() -> f64 {
    2.5
}
fn d_min_confirm() -> f64 {
    20.0
}
fn d_min_drop_tentative() -> f64 {
    8.0
}
fn d_min_drop_confirmed() -> f64 {
    30.0
}
fn d_lifetime() -> f64 {
    600.0
}
fn d_confirm_p() -> f64 {
    0.95
}
fn d_drop_p() -> f64 {
    0.02
}
fn d_hold() -> f64 {
    0.5
}
fn d_prefix() -> String {
    "T".into()
}
fn d_pd() -> f64 {
    0.9
}
fn d_clutter() -> f64 {
    1e-6
}
fn d_birth() -> f64 {
    1e-7
}
fn d_n_scan() -> usize {
    3
}
fn d_branches() -> usize {
    20
}

impl TrackerSpec {
    pub fn validate(&self) -> Result<(), String> {
        if self.algorithm == Algorithm::Plugin {
            return match self.plugin.as_deref() {
                Some(p) if !p.is_empty() => Ok(()),
                _ => Err("algorithm plugin needs `plugin`: the tracker plugin's name".into()),
            };
        }
        let positive = [
            ("measurement_sigma_m", self.measurement_sigma_m),
            ("process_noise_mps2", self.process_noise_mps2),
            ("initial_speed_sigma_mps", self.initial_speed_sigma_mps),
            ("gate", self.gate),
            ("confirm_within_secs", self.confirm_within_secs),
            ("drop_tentative_secs", self.drop_tentative_secs),
            ("drop_confirmed_secs", self.drop_confirmed_secs),
            ("clutter_density", self.detection_model().1),
            ("birth_density", self.detection_model().2),
            ("target_lifetime_secs", self.target_lifetime_secs),
        ];
        for (name, v) in positive {
            if !(v.is_finite() && v > 0.0) {
                return Err(format!("tracker {name} must be a positive number"));
            }
        }
        let (pd, _, _) = self.detection_model();
        if !(pd > 0.0 && pd < 1.0) {
            return Err("tracker detection_probability must be between 0 and 1".into());
        }
        let (c, d) = (self.confirm_probability, self.drop_probability);
        if !(d > 0.0 && d < c && c < 1.0) {
            return Err(
                "tracker probabilities must satisfy 0 < drop_probability < confirm_probability < 1"
                    .into(),
            );
        }
        if self.confirm_hits == 0 || self.mht.n_scan == 0 || self.mht.max_branches == 0 {
            return Err(
                "tracker confirm_hits, mht.n_scan and mht.max_branches must be at least 1".into(),
            );
        }
        if !(self.scan_hold_secs >= 0.0 && self.cluster_m >= 0.0) {
            return Err("tracker scan_hold_secs and cluster_m must not be negative".into());
        }
        if let Some(a) = &self.auto_timing {
            let ok = |v: f64| v.is_finite() && v > 1.0;
            if !(ok(a.confirm_revisits)
                && ok(a.drop_tentative_revisits)
                && ok(a.drop_confirmed_revisits))
            {
                return Err(
                    "tracker auto_timing multiples must be above 1 (a track must outlive a revisit)"
                        .into(),
                );
            }
            let floors = [
                a.min_confirm_secs,
                a.min_drop_tentative_secs,
                a.min_drop_confirmed_secs,
            ];
            if !floors.iter().all(|v| v.is_finite() && *v >= 0.0) {
                return Err("tracker auto_timing floors must not be negative".into());
            }
        }
        Ok(())
    }
}

/// The only classification any tracker gives a track: unknown affiliation,
/// and a ground, air, surface or subsurface domain when it has one. Never an
/// identity, a platform type or a more specific symbol.
pub fn tracker_classification(domain: Option<Domain>) -> Classification {
    Classification {
        affiliation: Some(Affiliation::Unknown),
        domain: domain
            .and_then(TrackerDomain::of)
            .map(TrackerDomain::domain),
        cot_type: None,
        sidc: None,
    }
}

/// One detection as the trackers see it.
#[derive(Debug, Clone, Copy)]
pub struct Plot {
    pub lat: f64,
    pub lon: f64,
    /// Position covariance `[nn, ne, ee]` (m²): the detection's own error
    /// ellipse, else its circular error, else `measurement_sigma_m`.
    pub r: [f64; 3],
    pub domain: Option<TrackerDomain>,
}

/// A confirmed track updated by a plot in this scan, or dropped.
#[derive(Debug, Clone)]
pub struct Report {
    pub id: u64,
    pub kf: Kf,
    pub domain: Option<TrackerDomain>,
    /// Index of the plot that updated it (a dropped track: any plot of the scan).
    pub plot: usize,
    /// The tracker gave up on it: reported once, with `state: dropped`.
    pub dropped: bool,
    /// The probability that it is a real target.
    pub existence: f64,
}

/// A track's existence, confirmation, age and the domains its plots
/// reported.
///
/// Existence is the probability that the track is a real target rather than
/// clutter (IPDA, integrated probabilistic data association, with the
/// assigned plot only): a new track starts at the prior `birth / (birth +
/// clutter)`; between looks it decays with the target's lifetime; a plot
/// multiplies its odds by `(Λ + 1 − Pd)` with `Λ = Pd·g/λ` (the plot's
/// Gaussian likelihood against the clutter density); a look without one by
/// `1 − Pd`. The track is confirmed at `confirm_probability` and dropped at
/// `drop_probability` (or after the drop time without a plot).
#[derive(Debug, Clone)]
struct Life {
    hits: usize,
    last_hit: DateTime<Utc>,
    /// When existence was last brought up to date by a look.
    last_look: DateTime<Utc>,
    existence: f64,
    confirmed: bool,
    domains: [u32; 4],
}

impl Life {
    fn new(t: DateTime<Utc>, plot: &Plot, spec: &TrackerSpec) -> Self {
        let (_, clutter, birth) = spec.detection_model();
        let mut l = Self {
            hits: 1,
            last_hit: t,
            last_look: t,
            existence: birth / (birth + clutter),
            confirmed: false,
            domains: [0; 4],
        };
        l.count_domain(plot);
        l.confirm(spec);
        l
    }

    /// Decay existence over `dt` seconds of the target's lifetime.
    fn survive(&mut self, dt: f64, spec: &TrackerSpec) {
        self.existence *= (-dt.max(0.0) / spec.target_lifetime_secs).exp();
    }

    /// A plot updated the track.
    fn hit(&mut self, t: DateTime<Utc>, plot: &Plot, inn: &filter::Innovation, spec: &TrackerSpec) {
        let (pd, clutter, _) = spec.detection_model();
        self.survive(seconds(t - self.last_look), spec);
        let lambda = pd * inn.ln_likelihood().exp() / clutter;
        let p = self.existence;
        let num = p * (lambda + 1.0 - pd);
        self.existence = num / (num + 1.0 - p);
        self.hits += 1;
        self.last_hit = t;
        self.last_look = t;
        self.count_domain(plot);
        self.confirm(spec);
    }

    /// A scan passed with no plot for the track. With a known revisit period,
    /// a miss counts once per revisit since the last look, not per scan.
    fn miss(&mut self, t: DateTime<Utc>, spec: &TrackerSpec) {
        let (pd, _, _) = spec.detection_model();
        let looks = match spec.revisit_secs {
            Some(r) => {
                let n = (seconds(t - self.last_look) / r).floor();
                if n < 1.0 {
                    return;
                }
                self.survive(n * r, spec);
                self.last_look += secs(n * r);
                n as i32
            }
            None => {
                self.survive(seconds(t - self.last_look), spec);
                self.last_look = t;
                1
            }
        };
        for _ in 0..looks {
            let p = self.existence;
            self.existence = p * (1.0 - pd) / (p * (1.0 - pd) + 1.0 - p);
        }
    }

    fn confirm(&mut self, spec: &TrackerSpec) {
        self.confirmed |=
            self.hits >= spec.confirm_hits && self.existence >= spec.confirm_probability;
    }

    fn count_domain(&mut self, plot: &Plot) {
        if let Some(d) = plot.domain {
            let i = TrackerDomain::ALL
                .iter()
                .position(|x| *x == d)
                .expect("listed");
            self.domains[i] += 1;
        }
    }

    fn alive(&self, t: DateTime<Utc>, spec: &TrackerSpec) -> bool {
        let limit = if self.confirmed {
            spec.drop_confirmed_secs
        } else {
            spec.drop_tentative_secs
        };
        self.existence > spec.drop_probability && t - self.last_hit <= secs(limit)
    }

    /// The configured domain, else the one most plots reported.
    fn domain(&self, spec: &TrackerSpec) -> Option<TrackerDomain> {
        spec.domain.or_else(|| {
            let (i, n) = self
                .domains
                .iter()
                .enumerate()
                .max_by_key(|(_, n)| **n)
                .expect("four domains");
            (*n > 0).then_some(TrackerDomain::ALL[i])
        })
    }
}

fn seconds(d: chrono::Duration) -> f64 {
    d.num_microseconds().unwrap_or(i64::MAX) as f64 / 1e6
}

fn secs(s: f64) -> chrono::Duration {
    chrono::Duration::microseconds((s * 1e6) as i64)
}

/// Scan-by-scan association: GNN or MHT.
trait Associate: Send {
    fn scan(&mut self, t: DateTime<Utc>, plots: &[Plot]) -> Vec<Report>;
    /// The settings it runs with, to retime (`auto_timing`).
    fn spec_mut(&mut self) -> &mut TrackerSpec;
}

/// The tracker stage of one source's pipeline.
pub struct Tracker {
    spec: TrackerSpec,
    inner: Box<dyn Associate>,
    /// Plots waiting for their scan to complete, with when they arrived.
    buffer: Vec<(Observation, DateTime<Utc>)>,
    last_scan: Option<DateTime<Utc>>,
    /// Plots older than the last processed scan, dropped.
    pub late: u64,
    /// Tags this tracker's keys (`R3f2a-7`): a restarted tracker numbers from
    /// 1 again and must not continue another run's tracks.
    run: String,
    /// With `auto_timing`, the windows in use since the revisit period was known.
    timing: Option<Timing>,
}

impl Tracker {
    pub fn new(spec: TrackerSpec) -> Result<Self, String> {
        spec.validate()?;
        let inner: Box<dyn Associate> = match spec.algorithm {
            Algorithm::Gnn => Box::new(gnn::Gnn::new(spec.clone())),
            Algorithm::Mht => Box::new(mht::Mht::new(spec.clone())),
            Algorithm::Plugin => {
                return Err("a plugin tracker is started from its plugin".into());
            }
        };
        Ok(Self {
            spec,
            inner,
            buffer: Vec::new(),
            last_scan: None,
            late: 0,
            timing: None,
            run: format!("{:04x}", Utc::now().timestamp() & 0xffff),
        })
    }

    pub fn spec(&self) -> &TrackerSpec {
        &self.spec
    }

    /// The running algorithm's version (`gnn-1`, `mht-1`).
    pub fn version(&self) -> &'static str {
        match self.spec.algorithm {
            Algorithm::Gnn => GNN_VERSION,
            Algorithm::Mht => MHT_VERSION,
            Algorithm::Plugin => "plugin",
        }
    }

    /// With `auto_timing`, size the windows to the sensor's revisit period
    /// (a no-op without it, or for a change within 5%: the windows follow a
    /// new job, not every revisit's jitter).
    pub fn set_revisit(&mut self, revisit_secs: f64, source: &'static str) {
        let Some(a) = &self.spec.auto_timing else {
            return;
        };
        if !(revisit_secs.is_finite() && revisit_secs > 0.0) {
            return;
        }
        if let Some(t) = &self.timing
            && t.revisit_source == source
            && (t.revisit_secs - revisit_secs).abs() <= 0.05 * t.revisit_secs
        {
            return;
        }
        let timing = Timing {
            revisit_secs,
            revisit_source: source,
            confirm_within_secs: (a.confirm_revisits * revisit_secs).max(a.min_confirm_secs),
            drop_tentative_secs: (a.drop_tentative_revisits * revisit_secs)
                .max(a.min_drop_tentative_secs),
            drop_confirmed_secs: (a.drop_confirmed_revisits * revisit_secs)
                .max(a.min_drop_confirmed_secs),
        };
        for spec in [&mut self.spec, self.inner.spec_mut()] {
            spec.revisit_secs = Some(revisit_secs);
            spec.confirm_within_secs = timing.confirm_within_secs;
            spec.drop_tentative_secs = timing.drop_tentative_secs;
            spec.drop_confirmed_secs = timing.drop_confirmed_secs;
        }
        self.timing = Some(timing);
    }

    /// The windows `auto_timing` chose, once it has a revisit period.
    pub fn timing(&self) -> Option<&Timing> {
        self.timing.as_ref()
    }

    /// Detections waiting for their scan to complete.
    pub fn waiting(&self) -> impl Iterator<Item = &Observation> {
        self.buffer.iter().map(|(o, _)| o)
    }

    /// Queue a detection.
    pub fn push(&mut self, obs: Observation, received_at: DateTime<Utc>) {
        self.buffer.push((obs, received_at));
    }

    /// Run every complete scan (and with `force`, the newest one too),
    /// returning track reports and the detection behind each.
    pub fn run(&mut self, now: DateTime<Utc>, force: bool) -> Vec<(Observation, Observation)> {
        if self.buffer.is_empty() {
            return Vec::new();
        }
        let mut buffer = std::mem::take(&mut self.buffer);
        buffer.sort_by_key(|(o, _)| o.observed_at);
        let newest = buffer
            .last()
            .map(|(o, _)| o.observed_at)
            .expect("non-empty");
        let hold = self.spec.scans == ScanGrouping::Time && !force && {
            let arrived = buffer
                .iter()
                .filter(|(o, _)| o.observed_at == newest)
                .map(|(_, r)| *r)
                .max()
                .expect("non-empty");
            now - arrived < secs(self.spec.scan_hold_secs)
        };
        let mut out = Vec::new();
        let mut i = 0;
        while i < buffer.len() {
            let t = buffer[i].0.observed_at;
            let j = i + buffer[i..]
                .iter()
                .take_while(|(o, _)| o.observed_at == t)
                .count();
            if hold && t == newest {
                self.buffer.extend(buffer.drain(i..));
                break;
            }
            let scan: Vec<Observation> = buffer[i..j].iter().map(|(o, _)| o.clone()).collect();
            i = j;
            if self.last_scan.is_some_and(|l| t <= l) {
                self.late += scan.len() as u64;
                continue;
            }
            self.last_scan = Some(t);
            let (plots, first) = self.plots(&scan);
            for r in self.inner.scan(t, &plots) {
                let det = &scan[first[r.plot]];
                out.push((self.report(&r, det), det.clone()));
            }
        }
        out
    }

    /// A scan's plots, merged within `cluster_m` (at their centroid, with the
    /// smallest sigma), and for each the first detection in it.
    fn plots(&self, scan: &[Observation]) -> (Vec<Plot>, Vec<usize>) {
        let mut groups: Vec<(Vec<usize>, f64, f64)> = Vec::new();
        for (i, o) in scan.iter().enumerate() {
            let (lat, lon) = (o.position.latitude, o.position.longitude);
            let near = if self.spec.cluster_m > 0.0 {
                groups.iter().position(|(_, glat, glon)| {
                    let (dn, de) = filter::offset_m(*glat, *glon, lat, lon);
                    dn.hypot(de) < self.spec.cluster_m
                })
            } else {
                None
            };
            match near {
                Some(g) => {
                    let (members, glat, glon) = &mut groups[g];
                    let n = members.len() as f64;
                    *glat = (*glat * n + lat) / (n + 1.0);
                    *glon = (*glon * n + lon) / (n + 1.0);
                    members.push(i);
                }
                None => groups.push((vec![i], lat, lon)),
            }
        }
        groups
            .into_iter()
            .map(|(members, lat, lon)| {
                let each: Vec<Plot> = members.iter().map(|&i| self.plot(&scan[i])).collect();
                // The sharpest member's error (smallest area).
                let r = each
                    .iter()
                    .map(|p| p.r)
                    .min_by(|a, b| {
                        (a[0] * a[2] - a[1] * a[1]).total_cmp(&(b[0] * b[2] - b[1] * b[1]))
                    })
                    .expect("a group has members");
                let domain = each.iter().find_map(|p| p.domain);
                (
                    Plot {
                        lat,
                        lon,
                        r,
                        domain,
                    },
                    members[0],
                )
            })
            .unzip()
    }

    fn plot(&self, o: &Observation) -> Plot {
        Plot {
            lat: o.position.latitude,
            lon: o.position.longitude,
            r: o.uncertainty
                .and_then(|u| u.position_covariance())
                .unwrap_or_else(|| {
                    let v = self.spec.measurement_sigma_m.powi(2);
                    [v, 0.0, v]
                }),
            domain: o
                .classification
                .effective_domain()
                .and_then(TrackerDomain::of),
        }
    }

    /// A track report as an observation: position and motion from the
    /// filter, the source and sensor details of the detection, and nothing
    /// that says what the object is beyond [`tracker_classification`].
    fn report(&self, r: &Report, det: &Observation) -> Observation {
        let (course, speed) = r.kf.course_speed();
        Observation {
            schema_version: det.schema_version,
            source_id: det.source_id.clone(),
            source_track_key: format!("{}{}-{}", self.spec.key_prefix, self.run, r.id),
            identifiers: Vec::new(),
            name: None,
            callsign: None,
            observed_at: if r.dropped { det.observed_at } else { r.kf.t },
            received_at: det.received_at,
            position: Position {
                latitude: r.kf.lat,
                longitude: r.kf.lon,
                altitude_hae_m: det.position.altitude_hae_m,
            },
            uncertainty: Some(Uncertainty {
                ellipse: Some(round_ellipse(ot_core::Ellipse::from_covariance(
                    r.kf.position_cov(),
                ))),
                circular_error_m: Some((r.kf.cep_m() * 10.0).round() / 10.0),
                vertical_error_m: None,
                covariance: Some(r.kf.covariance()),
            }),
            kinematics: Kinematics {
                course_deg: Some((course * 10.0).round() / 10.0),
                speed_mps: Some((speed * 100.0).round() / 100.0),
                ..Default::default()
            },
            classification: tracker_classification(r.domain.map(TrackerDomain::domain)),
            platform: Default::default(),
            provenance: ot_core::Provenance {
                tracker: Some(self.version().to_owned()),
                // The probability the track is a real target.
                confidence: Some((r.existence * 1e4).round() / 1e4),
                ..det.provenance.clone()
            },
            security: None,
            state: r.dropped.then_some(ot_core::TrackState::Dropped),
            track_type: det.track_type,
            ext: Default::default(),
            origin: None,
            geometry: None,
        }
    }
}

fn round_ellipse(e: ot_core::Ellipse) -> ot_core::Ellipse {
    let r = |x: f64| (x * 10.0).round() / 10.0;
    ot_core::Ellipse {
        semi_major_m: r(e.semi_major_m),
        semi_minor_m: r(e.semi_minor_m).min(r(e.semi_major_m)),
        orientation_deg: r(e.orientation_deg) % 180.0,
    }
}

#[cfg(test)]
mod tests;
