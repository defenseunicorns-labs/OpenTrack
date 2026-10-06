//! Correlation: deciding which source tracks report the same object, and
//! building a system track's view from its contributors.
//!
//! Pure functions over observations, so every rule is unit-tested without
//! Redis or SQLite. The engine calls them and records each decision with the
//! evidence these functions return.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use ot_core::{Domain, Observation};
use serde::{Deserialize, Serialize};

const EARTH_RADIUS_M: f64 = 6_371_008.8;

/// Version of the correlation engine's behaviour: pairing and merging rules,
/// best-source selection, detection association, the lifecycle and the
/// publish rule. Stamped on every engine decision and published message.
/// Bump it whenever the same inputs would give different system tracks, and
/// record it in docs/algorithms.md with its scores.
pub const VERSION: &str = "correlation-6";

/// Keys under which an observation claims an identity: each identifier as
/// `<scheme>:<value>` (lowercase), and `entity:<id>` when the registry
/// resolved it to an entity with corroboration. Two tracks sharing a key
/// are the same object, subject to the sanity gate.
pub fn identity_keys(obs: &Observation) -> Vec<String> {
    let mut out: Vec<String> = obs
        .identifiers
        .iter()
        .filter(|i| !i.value.trim().is_empty())
        .filter(|i| !is_evidence_scheme(&i.scheme))
        .map(|i| {
            format!(
                "{}:{}",
                i.scheme.trim().to_lowercase(),
                i.value.trim().to_lowercase()
            )
        })
        .collect();
    if let Some(id) = trusted_entity(obs) {
        out.push(format!("entity:{id}"));
    }
    out.sort();
    out.dedup();
    out
}

/// Identifier schemes that name a kind of thing, not one thing: many
/// emitters share an ELNOT (every boat with the same radar model), and one
/// platform can carry several. They never make identity, never veto a
/// pairing, and a shared value is evidence for one (see
/// [`EVIDENCE_LN_LR`]).
pub const EVIDENCE_SCHEMES: &[&str] = &["elnot"];

/// Whether a scheme is evidence only ([`EVIDENCE_SCHEMES`]).
pub fn is_evidence_scheme(scheme: &str) -> bool {
    let s = scheme.trim();
    EVIDENCE_SCHEMES.iter().any(|e| e.eq_ignore_ascii_case(s))
}

/// What a shared evidence identifier (an ELNOT) adds to a comparison's ln
/// likelihood ratio: 10 to 1 for the same object. A different one adds
/// nothing either way.
pub const EVIDENCE_LN_LR: f64 = std::f64::consts::LN_10;

/// Whether two observations share an evidence identifier's value.
pub fn share_evidence(a: &Observation, b: &Observation) -> bool {
    a.identifiers
        .iter()
        .filter(|i| is_evidence_scheme(&i.scheme))
        .any(|x| {
            b.identifiers.iter().any(|y| {
                y.scheme.trim().eq_ignore_ascii_case(x.scheme.trim())
                    && y.value.trim().eq_ignore_ascii_case(x.value.trim())
            })
        })
}

/// The registry entity an observation resolved to, when the match is
/// trustworthy: corroborated (`applied` on older records) and without
/// conflicting identifiers. The same rule decides whether entity links apply.
pub fn trusted_entity(obs: &Observation) -> Option<String> {
    let r = obs.ext.get("registry")?;
    let trusted = r
        .get("corroborated")
        .or_else(|| r.get("applied"))
        .and_then(|v| v.as_bool())
        == Some(true);
    let conflicted = r
        .get("conflicts")
        .and_then(|c| c.as_array())
        .is_some_and(|c| !c.is_empty());
    (trusted && !conflicted)
        .then(|| {
            r.get("entity_id")
                .and_then(|v| v.as_str())
                .map(str::to_owned)
        })
        .flatten()
}

/// Great-circle distance in metres.
pub fn distance_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dp = p2 - p1;
    let dl = (lon2 - lon1).to_radians();
    let a = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_M * a.sqrt().asin()
}

/// Where `obs` would be at `at`, moving at its reported course and speed
/// (for at most `max_secs`; unchanged when it reports neither).
pub fn dead_reckon(obs: &Observation, at: DateTime<Utc>, max_secs: f64) -> (f64, f64) {
    let (lat, lon) = (obs.position.latitude, obs.position.longitude);
    let (Some(course), Some(speed)) = (obs.kinematics.course_deg, obs.kinematics.speed_mps) else {
        return (lat, lon);
    };
    let dt = ((at - obs.observed_at).num_milliseconds() as f64 / 1000.0).clamp(-max_secs, max_secs);
    let d = speed * dt;
    let (c, lat_r) = (course.to_radians(), lat.to_radians());
    let dlat = d * c.cos() / EARTH_RADIUS_M;
    let dlon = d * c.sin() / (EARTH_RADIUS_M * lat_r.cos().max(1e-6));
    (lat + dlat.to_degrees(), lon + dlon.to_degrees())
}

/// The fastest a platform of this domain plausibly moves (m/s), for the
/// sanity gate's allowance over time.
fn max_speed(domain: Option<Domain>) -> f64 {
    match domain {
        Some(Domain::Surface) | Some(Domain::Subsurface) => 40.0,
        Some(Domain::Ground) => 70.0,
        Some(Domain::Space) => 8_000.0,
        Some(Domain::Air) | None => 400.0,
    }
}

/// Settings of the kinematic sanity gate on identifier matches.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GateSettings {
    /// Allowed distance at zero time difference (reporting error, lag).
    pub base_m: f64,
    /// Longest dead-reckoning extrapolation.
    pub max_extrapolation_secs: f64,
}

impl Default for GateSettings {
    fn default() -> Self {
        Self {
            base_m: 10_000.0,
            max_extrapolation_secs: 600.0,
        }
    }
}

/// Whether a track reported at `obs` can be the object a system track shows
/// in `view`: the distance after dead-reckoning the view to the observation's
/// time must be within `base + max speed × time apart`. Stops a reassigned or
/// spoofed identifier from pulling two distant objects together.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Gate {
    pub distance_m: f64,
    pub dt_s: f64,
    pub gate_m: f64,
    pub pass: bool,
}

pub fn sanity_gate(obs: &Observation, view: &Observation, s: &GateSettings) -> Gate {
    let (lat, lon) = dead_reckon(view, obs.observed_at, s.max_extrapolation_secs);
    let distance_m = distance_m(obs.position.latitude, obs.position.longitude, lat, lon);
    let dt_s = ((obs.observed_at - view.observed_at).num_milliseconds() as f64 / 1000.0).abs();
    let domain = obs
        .classification
        .effective_domain()
        .or(view.classification.effective_domain());
    let gate_m = s.base_m + max_speed(domain) * dt_s;
    Gate {
        distance_m: distance_m.round(),
        dt_s: (dt_s * 10.0).round() / 10.0,
        gate_m: gate_m.round(),
        pass: distance_m <= gate_m,
    }
}

/// Settings of kinematic pairing (tracks with no shared identity) and of
/// detection association.
///
/// Two reports are compared by propagating the older one's state and
/// covariance to the newer one's time (constant velocity, white-acceleration
/// process noise) and testing the difference against the sum of both
/// covariances: position, and velocity when both report motion (2 or 4
/// degrees of freedom). Each comparison is a likelihood ratio, "the same
/// object" against "another object nearby"; a pair's probability of being the
/// same object is the prior updated by the ratios of its recent comparisons.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct KinematicSettings {
    /// Share of true matches the gate keeps (chi-square, 2 or 4 degrees of
    /// freedom); a comparison outside it counts against the pair.
    pub gate_probability: f64,
    /// Floor on a report's position standard deviation per axis, for
    /// sources that claim more precision than they have (or report none).
    pub min_sigma_m: f64,
    /// White-acceleration process noise (m/s²) for propagating a report's
    /// covariance to another's time.
    pub process_noise_mps2: f64,
    /// Standard deviation of a reported velocity (per axis, m/s) when the
    /// report gives course and speed but no covariance.
    pub speed_sigma_mps: f64,
    /// How many other objects per km² a report could be, and how spread their
    /// velocities are (m/s): the "another object nearby" of each comparison.
    pub object_density_per_km2: f64,
    /// Count the live tracks within this radius of a report (metres) and use
    /// their density when it is higher than `object_density_per_km2`: in a
    /// crowded harbour a close approach is weaker evidence than at sea.
    /// 0 turns it off.
    pub local_density_radius_m: f64,
    pub velocity_spread_mps: f64,
    /// The velocity spread when either report is an aircraft: air traffic's
    /// velocities differ by far more than surface traffic's.
    pub air_velocity_spread_mps: f64,
    /// Probability that two unrelated tracks are the same object before any
    /// comparison.
    pub prior_probability: f64,
    /// Pair (or propose the pairing) at this probability...
    pub pair_probability: f64,
    /// ...with at least `m` comparisons, from the last `n`...
    pub m: usize,
    pub n: usize,
    /// ...within this many seconds, at least this far apart (consecutive
    /// reports are not independent evidence).
    pub window_secs: f64,
    pub min_interval_secs: f64,
    /// Views older than this are not compared.
    pub max_age_secs: f64,
    /// A source's track counts as live on a system track this long after its
    /// last report: while it does, another track of the same source cannot
    /// join (a sensor's two tracks are two objects).
    pub source_live_secs: f64,
    /// Compare a new report with the other side's report already used, its
    /// evidence weighted by the new report's share of the uncertainty.
    pub reuse_views: bool,
    /// ...at least this long after the last counted comparison of the pair:
    /// a tracker's consecutive reports are smoothed, not independent.
    pub reuse_interval_secs: f64,
    /// How fast a stopped target's position may drift (m/s): a view that
    /// reports no motion is propagated with this, not the manoeuvre noise.
    pub stopped_drift_mps: f64,
    /// Settings from before correlation-3, still read: a chi-square gate
    /// (2 degrees of freedom) becomes the gate probability; the flat drift
    /// allowance is replaced by `process_noise_mps2`.
    #[serde(skip_serializing)]
    pub chi2_gate: Option<f64>,
    #[serde(skip_serializing)]
    pub drift_mps: Option<f64>,
}

impl Default for KinematicSettings {
    fn default() -> Self {
        Self {
            gate_probability: 0.99,
            min_sigma_m: 5.0,
            process_noise_mps2: 0.3,
            speed_sigma_mps: 1.0,
            object_density_per_km2: 1.0,
            local_density_radius_m: 500.0,
            velocity_spread_mps: 15.0,
            air_velocity_spread_mps: 60.0,
            prior_probability: 0.01,
            pair_probability: 0.99,
            m: 3,
            n: 5,
            window_secs: 30.0,
            min_interval_secs: 3.0,
            max_age_secs: 180.0,
            source_live_secs: 30.0,
            stopped_drift_mps: 0.5,
            reuse_views: true,
            reuse_interval_secs: 10.0,
            chi2_gate: None,
            drift_mps: None,
        }
    }
}

impl KinematicSettings {
    /// The gate probability, from a pre-correlation-3 chi-square gate if set.
    pub fn gate(&self) -> f64 {
        self.chi2_gate
            .map(|g| 1.0 - (-g / 2.0).exp())
            .unwrap_or(self.gate_probability)
    }
}

/// How pairing decides, beyond shared identifiers (which always pair).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Approach {
    /// Only shared identifiers pair tracks.
    Identifiers,
    /// Kinematic agreement over time, whatever else the tracks report.
    Kinematics,
    /// Kinematic agreement, vetoed by conflicting identifiers or domains.
    #[default]
    KinematicsMetadata,
}

/// Whether kinematic pairings are made or proposed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// The engine pairs tracks that agree kinematically.
    #[default]
    Automatic,
    /// The engine proposes those pairings; an operator accepts or rejects.
    /// (Shared identifiers always pair.)
    Suggest,
}

/// Decorrelation: a source track that stops agreeing with the rest of its
/// system track (a sensor track that followed the wrong vessel through a
/// crossing, a wrongly shared identifier). A paired source track keeps a
/// probability of being the same object as the rest, starting at the pairing
/// threshold and updated by every comparison (like pairing, see
/// [`KinematicSettings`]); it is its confidence on the track.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SplitSettings {
    /// Propose splits for an operator to accept or reject.
    pub propose: bool,
    /// Split without asking.
    pub automatic: bool,
    /// Split (or propose it) when the probability falls to this...
    pub split_probability: f64,
    /// ...and `m` of the last `n` comparisons fall outside a gate keeping this
    /// share of true matches: a constant-velocity model understates the error
    /// of a manoeuvring target, so one bad stretch alone does not split.
    pub gate_probability: f64,
    pub m: usize,
    pub n: usize,
    /// How far back those comparisons may reach (seconds). A slow feed (AIS
    /// every 10 s, a moored vessel every few minutes) gives one comparison
    /// per report, so this must hold `n` of them.
    pub window_secs: f64,
    /// Before correlation-3: a chi-square distance counted as a miss. Read,
    /// no longer used.
    #[serde(skip_serializing)]
    pub chi2_gate: Option<f64>,
}

impl Default for SplitSettings {
    fn default() -> Self {
        Self {
            propose: true,
            automatic: true,
            split_probability: 0.001,
            gate_probability: 0.9999,
            m: 5,
            n: 6,
            window_secs: 300.0,
            chi2_gate: None,
        }
    }
}

/// What OpenTrack publishes: a track that fails the filter stays inside
/// OpenTrack (marked filtered), and a published track that stops passing is
/// withdrawn (deleted downstream) until it passes again. Empty: everything.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OutputFilter {
    /// Bounding boxes: with any `include` area a track must be inside one,
    /// and it must be outside every `exclude` area.
    pub areas: Vec<Area>,
    /// Allowed values, each list empty for any: affiliation (`hostile`,
    /// `unknown`...), domain (`air`, `surface`...), track type
    /// (`tactical`, `live_training`...).
    pub affiliations: Vec<String>,
    pub domains: Vec<String>,
    pub track_types: Vec<String>,
    /// Lowest track confidence (0 to 1) to publish.
    pub min_confidence: f64,
    /// Anything else, as a rule over the track's view (the observation
    /// fields) plus `confidence` and `state`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<ot_source::expr::Condition>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Area {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub exclude: bool,
    pub min_lat: f64,
    pub min_lon: f64,
    pub max_lat: f64,
    pub max_lon: f64,
}

impl Area {
    fn contains(&self, lat: f64, lon: f64) -> bool {
        let lon_in = if self.min_lon <= self.max_lon {
            (self.min_lon..=self.max_lon).contains(&lon)
        } else {
            // Across the antimeridian.
            lon >= self.min_lon || lon <= self.max_lon
        };
        (self.min_lat..=self.max_lat).contains(&lat) && lon_in
    }
}

impl OutputFilter {
    /// Why a track may not be published, or None when it may.
    pub fn rejects(&self, t: &ot_core::SystemTrack) -> Option<String> {
        let v = &t.view;
        let (lat, lon) = (v.position.latitude, v.position.longitude);
        let label = |a: &Area| {
            if a.name.is_empty() {
                "an area".to_owned()
            } else {
                format!("area {}", a.name)
            }
        };
        if let Some(a) = self
            .areas
            .iter()
            .find(|a| a.exclude && a.contains(lat, lon))
        {
            return Some(format!("inside excluded {}", label(a)));
        }
        let includes: Vec<&Area> = self.areas.iter().filter(|a| !a.exclude).collect();
        if !includes.is_empty() && !includes.iter().any(|a| a.contains(lat, lon)) {
            return Some("outside every included area".into());
        }
        // As published: no affiliation or domain is "unknown".
        let text = |x: Option<String>| x.unwrap_or_else(|| "unknown".into());
        let affiliation = text(
            v.classification
                .effective_affiliation()
                .and_then(|a| serde_json::to_value(a).ok())
                .and_then(|a| a.as_str().map(str::to_owned)),
        );
        let domain = text(
            v.classification
                .effective_domain()
                .and_then(|d| serde_json::to_value(d).ok())
                .and_then(|d| d.as_str().map(str::to_owned)),
        );
        let track_type = text(
            serde_json::to_value(v.track_type.unwrap_or_default())
                .ok()
                .and_then(|x| x.as_str().map(str::to_owned)),
        );
        for (what, allowed, value) in [
            ("affiliation", &self.affiliations, &affiliation),
            ("domain", &self.domains, &domain),
            ("track type", &self.track_types, &track_type),
        ] {
            if !allowed.is_empty() && !allowed.iter().any(|a| a.eq_ignore_ascii_case(value)) {
                return Some(format!("{what} {value} is not published"));
            }
        }
        let confidence = t.confidence();
        if confidence < self.min_confidence {
            return Some(format!(
                "confidence {confidence:.2} below {:.2}",
                self.min_confidence
            ));
        }
        if let Some(rule) = &self.rule {
            let mut doc = serde_json::to_value(v).unwrap_or_default();
            if let Some(o) = doc.as_object_mut() {
                o.insert("confidence".into(), serde_json::json!(confidence));
                o.insert("state".into(), serde_json::json!(t.state));
            }
            if !rule.eval(&doc) {
                return Some("the output rule does not hold".into());
            }
        }
        None
    }

    fn validate(&self) -> Result<(), String> {
        for a in &self.areas {
            let ok = [a.min_lat, a.max_lat]
                .iter()
                .all(|x| (-90.0..=90.0).contains(x))
                && [a.min_lon, a.max_lon]
                    .iter()
                    .all(|x| (-180.0..=180.0).contains(x))
                && a.min_lat <= a.max_lat;
            if !ok {
                return Err(format!(
                    "output area {:?}: latitudes -90 to 90 (min ≤ max), longitudes -180 to 180",
                    a.name
                ));
            }
        }
        if !(0.0..=1.0).contains(&self.min_confidence) {
            return Err("output.min_confidence must be between 0 and 1".into());
        }
        Ok(())
    }
}

/// Everything about correlation an operator can change while it runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CorrelationSettings {
    pub approach: Approach,
    pub mode: Mode,
    pub kinematic: KinematicSettings,
    /// Sanity gate on identifier matches.
    pub gate: GateSettings,
    /// Reports this close to a system track's newest one compete on quality
    /// for its position (best-source selection).
    pub freshness_secs: f64,
    pub split: SplitSettings,
    /// What gets published.
    pub output: OutputFilter,
    /// A scorer plugin: its evidence (ln likelihood ratio and gate) takes the
    /// place of the kinematic comparison's in the pairing test. The test,
    /// its thresholds and every decision stay the engine's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scorer: Option<ScorerRef>,
    /// How a fused track's security label is chosen.
    pub labels: LabelSettings,
}

/// A fused track's security label: the highest classification of its
/// sources' in this order, every restriction, and only the releasability
/// they share (see `ot_core::SecurityLabel::combine`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LabelSettings {
    /// Lowest first; case does not matter and U, C, S and TS stand for
    /// their names. One not listed ranks above them all.
    pub classification_order: Vec<String>,
}

impl Default for LabelSettings {
    fn default() -> Self {
        Self {
            classification_order: ot_core::DEFAULT_CLASSIFICATION_ORDER
                .map(String::from)
                .to_vec(),
        }
    }
}

impl LabelSettings {
    /// The label of a track from its sources' labels (highest priority
    /// first); a classification not in the order is logged once.
    pub fn combine<'a>(
        &self,
        labels: impl IntoIterator<Item = &'a ot_core::SecurityLabel>,
    ) -> Option<ot_core::SecurityLabel> {
        let labels: Vec<&ot_core::SecurityLabel> = labels.into_iter().collect();
        for l in &labels {
            if ot_core::classification_rank(&l.classification, &self.classification_order).is_none()
            {
                warn_unknown(&l.classification);
            }
        }
        ot_core::SecurityLabel::combine(labels, &self.classification_order)
    }
}

/// Log a classification the order does not know, once per value.
fn warn_unknown(c: &str) {
    use std::sync::{Mutex, OnceLock};
    static SEEN: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    let seen = SEEN.get_or_init(Default::default);
    if let Ok(mut s) = seen.lock()
        && s.len() < 1000
        && s.insert(c.to_owned())
    {
        tracing::warn!(
            classification = c,
            "a security label's classification is not in the classification order: it ranks above every known one"
        );
    }
}

/// A scorer plugin, by name, with its options.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScorerRef {
    pub plugin: String,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub options: serde_json::Value,
}

impl Default for CorrelationSettings {
    fn default() -> Self {
        Self {
            approach: Approach::default(),
            mode: Mode::default(),
            kinematic: KinematicSettings::default(),
            gate: GateSettings::default(),
            freshness_secs: 60.0,
            split: SplitSettings::default(),
            output: OutputFilter::default(),
            scorer: None,
            labels: LabelSettings::default(),
        }
    }
}

impl CorrelationSettings {
    pub fn validate(&self) -> Result<(), String> {
        if let Some(s) = &self.scorer {
            let p = ot_source::plugin::plugin(&s.plugin)
                .ok_or_else(|| format!("scorer: no plugin {:?}", s.plugin))?;
            if !p.manifest().provides(ot_source::plugin::Kind::Scorer) {
                return Err(format!("scorer: plugin {} is not a scorer", s.plugin));
            }
        }
        let k = &self.kinematic;
        let positive = [
            ("kinematic.min_sigma_m", k.min_sigma_m),
            ("kinematic.process_noise_mps2", k.process_noise_mps2),
            ("kinematic.speed_sigma_mps", k.speed_sigma_mps),
            ("kinematic.object_density_per_km2", k.object_density_per_km2),
            ("kinematic.velocity_spread_mps", k.velocity_spread_mps),
            ("kinematic.window_secs", k.window_secs),
            ("kinematic.max_age_secs", k.max_age_secs),
            ("split.window_secs", self.split.window_secs),
            ("gate.base_m", self.gate.base_m),
            (
                "gate.max_extrapolation_secs",
                self.gate.max_extrapolation_secs,
            ),
            ("freshness_secs", self.freshness_secs),
        ];
        for (name, v) in positive {
            if !(v.is_finite() && v > 0.0) {
                return Err(format!("{name} must be a positive number"));
            }
        }
        let unit = |v: f64| v > 0.0 && v < 1.0;
        for (name, v) in [
            ("kinematic.gate_probability", k.gate()),
            ("kinematic.prior_probability", k.prior_probability),
            ("kinematic.pair_probability", k.pair_probability),
            ("split.split_probability", self.split.split_probability),
            ("split.gate_probability", self.split.gate_probability),
        ] {
            if !unit(v) {
                return Err(format!("{name} must be between 0 and 1"));
            }
        }
        if self.split.split_probability >= k.pair_probability {
            return Err("split.split_probability must be below kinematic.pair_probability".into());
        }
        self.output.validate()?;
        let order = &self.labels.classification_order;
        if order.is_empty() || order.len() > 32 {
            return Err("labels.classification_order: 1 to 32 classifications".into());
        }
        let mut seen = std::collections::HashSet::new();
        for c in order {
            let n = ot_core::normalize_classification(c);
            if n.is_empty() || !seen.insert(n) {
                return Err(format!(
                    "labels.classification_order: {c:?} is empty or listed twice"
                ));
            }
        }
        for (name, v) in [
            ("kinematic.min_interval_secs", k.min_interval_secs),
            ("kinematic.local_density_radius_m", k.local_density_radius_m),
        ] {
            if !(v.is_finite() && v >= 0.0) {
                return Err(format!("{name} must not be negative"));
            }
        }
        for (name, m, n) in [
            ("kinematic", k.m, k.n),
            ("split", self.split.m, self.split.n),
        ] {
            if m == 0 || m > n {
                return Err(format!("{name}.m must be between 1 and {name}.n"));
            }
        }
        Ok(())
    }
}

type M4 = [[f64; 4]; 4];

/// A report's kinematic state with its covariance: north/east metres and m/s
/// at the report's position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Estimate {
    pub lat: f64,
    pub lon: f64,
    /// North and east velocity, when the report gives course and speed.
    pub v: Option<[f64; 2]>,
    /// Covariance of (north, east, v north, v east); the velocity rows only
    /// mean something with `v`.
    pub p: M4,
    pub t: DateTime<Utc>,
    domain: Option<Domain>,
    /// It reports no motion (a speed within `speed_sigma_mps`): propagated
    /// as a slow drift, not a manoeuvring target.
    pub stopped: bool,
}

/// A report as an [`Estimate`]: its full covariance when it has one (a
/// tracker's), else its ellipse or circular error (floored at
/// `min_sigma_m`) and `speed_sigma_mps` on a reported velocity.
pub fn estimate(obs: &Observation, s: &KinematicSettings) -> Estimate {
    let floor = s.min_sigma_m.powi(2);
    let pos = obs
        .uncertainty
        .and_then(|u| u.position_covariance())
        .unwrap_or([floor, 0.0, floor]);
    // Raise the smaller principal variance to the floor.
    let mid = (pos[0] + pos[2]) / 2.0;
    let r = (((pos[0] - pos[2]) / 2.0).powi(2) + pos[1] * pos[1]).sqrt();
    let add = (floor - (mid - r)).max(0.0);
    let mut p = [[0.0; 4]; 4];
    p[0][0] = pos[0] + add;
    p[0][1] = pos[1];
    p[1][0] = pos[1];
    p[1][1] = pos[2] + add;
    let v = match (obs.kinematics.course_deg, obs.kinematics.speed_mps) {
        (Some(c), Some(sp)) if c.is_finite() && sp.is_finite() => {
            let (sn, cs) = c.to_radians().sin_cos();
            Some([sp * cs, sp * sn])
        }
        // Stopped: no course (a still GPS reports none), but the velocity is
        // known to be about zero.
        (None, Some(sp)) if sp.is_finite() && sp.abs() <= s.speed_sigma_mps => Some([0.0, 0.0]),
        _ => None,
    };
    let cov = obs.uncertainty.and_then(|u| u.covariance);
    match cov.and_then(|c| c.velocity) {
        Some([a, b, c]) if v.is_some() => {
            let fl = s.speed_sigma_mps.powi(2) * 0.01;
            p[2][2] = a.max(fl);
            p[2][3] = b;
            p[3][2] = b;
            p[3][3] = c.max(fl);
            if let Some([nvn, nve, evn, eve]) = cov.and_then(|c| c.cross) {
                (p[0][2], p[2][0], p[0][3], p[3][0]) = (nvn, nvn, nve, nve);
                (p[1][2], p[2][1], p[1][3], p[3][1]) = (evn, evn, eve, eve);
            }
        }
        _ => {
            // Unknown motion: a spread wide enough for the domain.
            let sigma = if v.is_some() {
                s.speed_sigma_mps
            } else {
                max_speed(obs.classification.effective_domain()) / 3.0
            };
            p[2][2] = sigma * sigma;
            p[3][3] = sigma * sigma;
        }
    }
    let stopped = obs
        .kinematics
        .speed_mps
        .is_some_and(|sp| sp.is_finite() && sp.abs() <= s.speed_sigma_mps);
    Estimate {
        lat: obs.position.latitude,
        lon: obs.position.longitude,
        v,
        p,
        t: obs.observed_at,
        domain: obs.classification.effective_domain(),
        stopped,
    }
}

impl Estimate {
    /// The state at `at` (at most `max_secs` away), moving at constant
    /// velocity (still, without one) with white-acceleration noise `q`.
    /// With `drift`, a stopped target (`stopped`) instead drifts: its
    /// position variance grows by `(drift × dt)²`, without the manoeuvre terms.
    pub fn predict_with(
        &self,
        at: DateTime<Utc>,
        q: f64,
        max_secs: f64,
        drift: Option<f64>,
    ) -> Self {
        let dt = ((at - self.t).num_milliseconds() as f64 / 1000.0).clamp(-max_secs, max_secs);
        let [vn, ve] = self.v.unwrap_or([0.0, 0.0]);
        let (lat, lon) = offset(self.lat, self.lon, vn * dt, ve * dt);
        let mut f = identity4();
        f[0][2] = dt;
        f[1][3] = dt;
        let mut p = mul4(&mul4(&f, &self.p), &transpose4(&f));
        if let Some(d) = drift.filter(|_| self.stopped) {
            let grow = (d * dt).powi(2);
            p[0][0] += grow;
            p[1][1] += grow;
            return Self {
                lat,
                lon,
                p,
                t: at,
                ..*self
            };
        }
        let (a, b, c) = (
            dt.powi(4) / 4.0,
            dt.abs().powi(3) / 2.0 * dt.signum(),
            dt * dt,
        );
        let q2 = q * q;
        for (i, j) in [(0, 2), (1, 3)] {
            p[i][i] += a * q2;
            p[i][j] += b * q2;
            p[j][i] += b * q2;
            p[j][j] += c * q2;
        }
        Self {
            lat,
            lon,
            p,
            t: at,
            ..*self
        }
    }
}

/// How well a report agrees with a view: the view's state and covariance
/// propagated to the report's time, and their difference against the sum of
/// both covariances.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Kinematic {
    pub distance_m: f64,
    pub dt_s: f64,
    /// Combined position standard deviation (RMS of the two axes).
    pub sigma_m: f64,
    /// The report's and the (propagated) view's own position standard
    /// deviations: how much of the combined uncertainty each brings.
    pub sigma_report_m: f64,
    pub sigma_view_m: f64,
    /// Difference in speed, when both report motion.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed_diff_mps: Option<f64>,
    /// Squared Mahalanobis distance and its degrees of freedom (2: position,
    /// 4: position and velocity).
    pub d2: f64,
    pub dof: usize,
    /// ln of the likelihood ratio: the same object against another nearby.
    pub ln_lr: f64,
    pub pass: bool,
}

pub fn kinematic(obs: &Observation, view: &Observation, s: &KinematicSettings) -> Kinematic {
    let a = estimate(obs, s);
    let b = estimate(view, s).predict_with(
        obs.observed_at,
        s.process_noise_mps2,
        s.max_age_secs,
        Some(s.stopped_drift_mps),
    );
    let dt = (obs.observed_at - view.observed_at).num_milliseconds() as f64 / 1000.0;
    let mut k = Kinematic {
        dt_s: (dt.abs() * 10.0).round() / 10.0,
        ..compare(&a, &b, s)
    };
    // A shared ELNOT: the same kind of emitter, evidence for the same one.
    if share_evidence(obs, view) {
        k.ln_lr += EVIDENCE_LN_LR;
    }
    k
}

/// Compare two estimates at the same time (`dt_s` is left 0).
#[allow(clippy::needless_range_loop)] // matrix maths reads clearest by index
pub fn compare(a: &Estimate, b: &Estimate, s: &KinematicSettings) -> Kinematic {
    let (dn, de) = local(b.lat, b.lon, a.lat, a.lon);
    let mut sum = [[0.0; 4]; 4];
    for i in 0..4 {
        for j in 0..4 {
            sum[i][j] = a.p[i][j] + b.p[i][j];
        }
    }
    let (dof, v) = match (a.v, b.v) {
        (Some(x), Some(y)) => (4, [dn, de, x[0] - y[0], x[1] - y[1]]),
        _ => (2, [dn, de, 0.0, 0.0]),
    };
    let (d2, ln_det) = mahalanobis(&v, &sum, dof).unwrap_or((f64::INFINITY, 0.0));
    let ln_g = -0.5 * d2 - 0.5 * dof as f64 * (2.0 * std::f64::consts::PI).ln() - 0.5 * ln_det;
    // "Another object": uniform over the area it could be in, and over a
    // velocity spread when velocities are compared.
    let mut ln_other = (s.object_density_per_km2 / 1e6).ln();
    if dof == 4 {
        let spread = if a.domain == Some(Domain::Air) || b.domain == Some(Domain::Air) {
            s.air_velocity_spread_mps.max(s.velocity_spread_mps)
        } else {
            s.velocity_spread_mps
        };
        ln_other -= (std::f64::consts::PI * spread.powi(2)).ln();
    }
    let round = |x: f64, k: f64| (x * k).round() / k;
    let pass = d2 <= chi2_quantile(s.gate(), dof);
    // Outside the gate, a true match is that rare: the comparison counts at
    // most (1 − gate probability) : 1 for "the same object".
    let mut ln_lr = ln_g - ln_other;
    if !pass {
        ln_lr = ln_lr.min((1.0 - s.gate()).ln());
    }
    Kinematic {
        distance_m: round(dn.hypot(de), 10.0),
        dt_s: 0.0,
        sigma_m: round(((sum[0][0] + sum[1][1]) / 2.0).sqrt(), 10.0),
        sigma_report_m: round(((a.p[0][0] + a.p[1][1]) / 2.0).sqrt(), 10.0),
        sigma_view_m: round(((b.p[0][0] + b.p[1][1]) / 2.0).sqrt(), 10.0),
        speed_diff_mps: match (a.v, b.v) {
            (Some(x), Some(y)) => Some(round(x[0].hypot(x[1]) - y[0].hypot(y[1]), 100.0)),
            _ => None,
        },
        d2: round(d2.min(1e9), 100.0),
        dof,
        ln_lr: round(ln_lr.max(-50.0), 1000.0),
        pass,
    }
}

/// The chi-square value below which a share `p` of the distribution lies,
/// for 2 or 4 degrees of freedom.
pub fn chi2_quantile(p: f64, dof: usize) -> f64 {
    let two = -2.0 * (1.0 - p).ln();
    if dof == 2 {
        return two;
    }
    // 4 degrees of freedom: 1 − e^(−x/2)(1 + x/2) = p, by bisection.
    let (mut lo, mut hi) = (two, two * 4.0 + 10.0);
    for _ in 0..100 {
        let x = (lo + hi) / 2.0;
        if 1.0 - (-x / 2.0).exp() * (1.0 + x / 2.0) < p {
            lo = x;
        } else {
            hi = x;
        }
    }
    (lo + hi) / 2.0
}

/// vᵀ S⁻¹ v and ln |S| over the first `n` dimensions (Cholesky); None when S
/// is not positive definite.
#[allow(clippy::needless_range_loop)] // matrix maths reads clearest by index
fn mahalanobis(v: &[f64; 4], s: &M4, n: usize) -> Option<(f64, f64)> {
    let mut l = [[0.0; 4]; 4];
    for i in 0..n {
        for j in 0..=i {
            let sum: f64 = (0..j).map(|k| l[i][k] * l[j][k]).sum();
            if i == j {
                let d = s[i][i] - sum;
                if !(d > 0.0 && d.is_finite()) {
                    return None;
                }
                l[i][i] = d.sqrt();
            } else {
                l[i][j] = (s[i][j] - sum) / l[j][j];
            }
        }
    }
    // Solve L y = v; d2 = |y|².
    let mut y = [0.0; 4];
    for i in 0..n {
        let sum: f64 = (0..i).map(|k| l[i][k] * y[k]).sum();
        y[i] = (v[i] - sum) / l[i][i];
    }
    let d2 = y[..n].iter().map(|x| x * x).sum();
    let ln_det = 2.0 * (0..n).map(|i| l[i][i].ln()).sum::<f64>();
    Some((d2, ln_det))
}

fn identity4() -> M4 {
    let mut m = [[0.0; 4]; 4];
    for (i, row) in m.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    m
}

#[allow(clippy::needless_range_loop)] // matrix maths reads clearest by index
fn mul4(a: &M4, b: &M4) -> M4 {
    let mut m = [[0.0; 4]; 4];
    for i in 0..4 {
        for j in 0..4 {
            m[i][j] = (0..4).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    m
}

#[allow(clippy::needless_range_loop)] // matrix maths reads clearest by index
fn transpose4(a: &M4) -> M4 {
    let mut m = [[0.0; 4]; 4];
    for i in 0..4 {
        for j in 0..4 {
            m[i][j] = a[j][i];
        }
    }
    m
}

/// North and east metres from (lat1, lon1) to (lat2, lon2), locally flat.
fn local(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> (f64, f64) {
    let n = (lat2 - lat1).to_radians() * EARTH_RADIUS_M;
    let e = (lon2 - lon1).to_radians() * EARTH_RADIUS_M * lat1.to_radians().cos();
    (n, e)
}

/// The point `dn` metres north and `de` east of (lat, lon).
fn offset(lat: f64, lon: f64, dn: f64, de: f64) -> (f64, f64) {
    let lat2 = lat + (dn / EARTH_RADIUS_M).to_degrees();
    let lon2 = lon + (de / (EARTH_RADIUS_M * lat.to_radians().cos().max(1e-6))).to_degrees();
    (lat2, lon2)
}

/// The probability of "the same object" from a prior and the summed ln
/// likelihood ratios of the evidence.
pub fn posterior(prior: f64, ln_lr: f64) -> f64 {
    let lo = (prior / (1.0 - prior)).ln() + ln_lr;
    1.0 / (1.0 + (-lo).exp())
}

/// Why two tracks cannot be the same object whatever their kinematics:
/// different values for the same identifier scheme, or different domains.
pub fn veto(a: &Observation, b: &Observation) -> Option<String> {
    for x in a
        .identifiers
        .iter()
        .filter(|i| !is_evidence_scheme(&i.scheme))
    {
        let scheme = x.scheme.trim().to_lowercase();
        for y in &b.identifiers {
            if y.scheme.trim().to_lowercase() == scheme
                && !x.value.trim().eq_ignore_ascii_case(y.value.trim())
            {
                return Some(format!("{scheme} {} vs {}", x.value.trim(), y.value.trim()));
            }
        }
    }
    match (
        a.classification.effective_domain(),
        b.classification.effective_domain(),
    ) {
        (Some(x), Some(y)) if x != y => Some(format!("domain {x:?} vs {y:?}")),
        _ => None,
    }
}

/// The recent comparisons of one pair of tracks (or of a source track with
/// the rest of its system track): their ln likelihood ratios, at most `n`,
/// within the window, at least `min_interval_secs` apart.
#[derive(Debug, Clone, Default)]
pub struct Evidence {
    results: std::collections::VecDeque<(DateTime<Utc>, f64, bool)>,
    /// Each side's report in the last counted comparison: comparing a new
    /// report with the other side's same old one again is not new evidence.
    used: BTreeMap<String, DateTime<Utc>>,
}

/// What a pair's recent comparisons add up to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tally {
    /// Summed ln likelihood ratio.
    pub ln_lr: f64,
    pub comparisons: usize,
    /// How many fell outside the gate they were recorded against.
    pub outside: usize,
}

impl Evidence {
    /// Record a comparison of `side`'s report at `at` with `other`'s report of
    /// `other_at` (and whether it fell outside the gate that matters to the
    /// caller); returns the tally, or None when it is not new evidence: too
    /// soon after the last one, or with either side's report no newer than
    /// the one it gave the last counted comparison.
    #[cfg(test)]
    pub fn record(
        &mut self,
        side: (&str, DateTime<Utc>),
        other: (&str, DateTime<Utc>),
        ln_lr: f64,
        outside: bool,
        ns: (usize, &KinematicSettings),
    ) -> Option<Tally> {
        self.record_reusing(side, other, ln_lr, outside, ns, None)
    }

    /// As [`Self::record`], but a new report of `side` may also be compared
    /// with `other`'s report used before, when `reuse` gives the weight of
    /// such a comparison: the share of the combined uncertainty that is the
    /// new report's own (a precise view reused against a noisy report is
    /// nearly independent evidence each time; two equally noisy ones are not).
    pub fn record_reusing(
        &mut self,
        (side, at): (&str, DateTime<Utc>),
        (other, other_at): (&str, DateTime<Utc>),
        ln_lr: f64,
        outside: bool,
        (n, s): (usize, &KinematicSettings),
        reuse: Option<f64>,
    ) -> Option<Tally> {
        let gap = chrono::Duration::milliseconds((s.min_interval_secs * 1000.0) as i64);
        let stale = |who: &str, t: DateTime<Utc>| self.used.get(who).is_some_and(|u| t <= *u);
        if self.results.back().is_some_and(|l| (at - l.0).abs() < gap) || stale(side, at) {
            return None;
        }
        let reuse_gap = chrono::Duration::milliseconds((s.reuse_interval_secs * 1000.0) as i64);
        let weight = if stale(other, other_at) {
            if self
                .results
                .back()
                .is_some_and(|l| (at - l.0).abs() < reuse_gap)
            {
                return None;
            }
            match reuse {
                Some(w) if w > 0.0 => w.min(1.0),
                _ => return None,
            }
        } else {
            1.0
        };
        self.used.insert(side.to_owned(), at);
        self.used.insert(other.to_owned(), other_at);
        self.results.push_back((at, ln_lr * weight, outside));
        let window = chrono::Duration::milliseconds((s.window_secs * 1000.0) as i64);
        let newest = self.results.iter().map(|r| r.0).max().unwrap_or(at);
        self.results.retain(|r| newest - r.0 <= window);
        while self.results.len() > n {
            self.results.pop_front();
        }
        Some(Tally {
            ln_lr: self.results.iter().map(|r| r.1).sum(),
            comparisons: self.results.len(),
            outside: self.results.iter().filter(|r| r.2).count(),
        })
    }

    /// The newest comparison, to forget pairs that stopped being compared.
    pub fn last(&self) -> Option<DateTime<Utc>> {
        self.results.back().map(|r| r.0)
    }
}

/// A coarse spatial index of system tracks (0.05° cells, about 5 km of
/// latitude), so pairing and association compare against neighbours only.
#[derive(Debug)]
pub struct Grid<K> {
    cell_of: std::collections::HashMap<K, (i32, i32)>,
    cells: std::collections::HashMap<(i32, i32), Vec<K>>,
}

impl<K> Default for Grid<K> {
    fn default() -> Self {
        Self {
            cell_of: Default::default(),
            cells: Default::default(),
        }
    }
}

const CELL_DEG: f64 = 0.05;

fn cell(lat: f64, lon: f64) -> (i32, i32) {
    (
        (lat / CELL_DEG).floor() as i32,
        (lon / CELL_DEG).floor() as i32,
    )
}

impl<K: Copy + Eq + std::hash::Hash> Grid<K> {
    pub fn put(&mut self, k: K, lat: f64, lon: f64) {
        let c = cell(lat, lon);
        if self.cell_of.get(&k) == Some(&c) {
            return;
        }
        self.remove(k);
        self.cell_of.insert(k, c);
        self.cells.entry(c).or_default().push(k);
    }

    pub fn remove(&mut self, k: K) {
        if let Some(c) = self.cell_of.remove(&k)
            && let Some(v) = self.cells.get_mut(&c)
        {
            v.retain(|x| *x != k);
            if v.is_empty() {
                self.cells.remove(&c);
            }
        }
    }

    /// Everything in the cell of (lat, lon) and the eight around it.
    pub fn near(&self, lat: f64, lon: f64) -> Vec<K> {
        let (a, b) = cell(lat, lon);
        let mut out = Vec::new();
        for i in a - 1..=a + 1 {
            for j in b - 1..=b + 1 {
                if let Some(v) = self.cells.get(&(i, j)) {
                    out.extend(v.iter().copied());
                }
            }
        }
        out
    }
}

/// One contributor's latest report, for best-source selection.
#[derive(Debug, Clone, Copy)]
pub struct Contribution<'a> {
    pub obs: &'a Observation,
    /// The source's priority; higher wins.
    pub priority: i64,
}

impl Contribution<'_> {
    fn key(&self) -> String {
        format!("{}/{}", self.obs.source_id, self.obs.source_track_key)
    }

    fn corroborated(&self) -> bool {
        trusted_entity(self.obs).is_some()
    }
}

/// Specificity of a CoT type: its number of atoms (`a-f-S-C-L` beats `a-f-S`).
fn cot_specificity(cot: Option<&str>) -> usize {
    cot.map(|c| c.split('-').filter(|a| !a.is_empty()).count())
        .unwrap_or(0)
}

/// The system track's view from its contributors' latest reports, and which
/// contributor supplied each field group:
///
/// - `position` (position, kinematics, uncertainty, time): among reports
///   within `window_secs` of the newest, the smallest uncertainty, then the
///   highest priority, then the newest.
/// - `kinematics`: when that report has no course and speed (a plot), the
///   newest report in the window that has them.
/// - `identity` (name, callsign, platform): a registry-corroborated report,
///   else the highest priority, else the newest; gaps filled from the others
///   in the same order.
/// - `classification`: a registry-corroborated report's, else the most
///   specific CoT type, then priority.
/// - extension fields: each from the highest-priority report that has it.
/// - identifiers: every contributor's.
/// - security label: the highest classification of any contributor's, with
///   every restriction and the releasability they share
///   ([`LabelSettings::combine`]).
pub fn best_view(
    contribs: &[Contribution<'_>],
    window_secs: f64,
    labels: &LabelSettings,
) -> (Observation, BTreeMap<String, String>) {
    assert!(
        !contribs.is_empty(),
        "a system track has at least one contributor"
    );
    let mut provenance = BTreeMap::new();
    if contribs.len() == 1 {
        return (contribs[0].obs.clone(), provenance);
    }
    let newest = contribs
        .iter()
        .map(|c| c.obs.observed_at)
        .max()
        .expect("non-empty");
    let window = chrono::Duration::milliseconds((window_secs * 1000.0) as i64);
    let cep = |c: &Contribution<'_>| {
        c.obs
            .uncertainty
            .and_then(|u| u.cep_m())
            .unwrap_or(f64::INFINITY)
    };

    let position = contribs
        .iter()
        .filter(|c| newest - c.obs.observed_at <= window)
        .min_by(|a, b| {
            cep(a)
                .total_cmp(&cep(b))
                .then(b.priority.cmp(&a.priority))
                .then(b.obs.observed_at.cmp(&a.obs.observed_at))
        })
        .expect("the newest report is within the window");
    let mut view = position.obs.clone();
    provenance.insert("position".into(), position.key());
    // A position without motion (a plot) takes it from the newest report
    // that has it: a view without velocity cannot be propagated in time.
    let moving =
        |o: &Observation| o.kinematics.course_deg.is_some() && o.kinematics.speed_mps.is_some();
    if !moving(&view)
        && let Some(m) = contribs
            .iter()
            .filter(|c| moving(c.obs) && newest - c.obs.observed_at <= window)
            .max_by_key(|c| c.obs.observed_at)
    {
        view.kinematics = m.obs.kinematics;
        provenance.insert("kinematics".into(), m.key());
    }

    // Identity: ordered by preference, the first supplies, the rest fill gaps.
    let mut by_identity: Vec<&Contribution<'_>> = contribs.iter().collect();
    by_identity.sort_by(|a, b| {
        b.corroborated()
            .cmp(&a.corroborated())
            .then(b.priority.cmp(&a.priority))
            .then(b.obs.observed_at.cmp(&a.obs.observed_at))
    });
    let lead = by_identity[0];
    view.name = by_identity.iter().find_map(|c| c.obs.name.clone());
    view.callsign = by_identity.iter().find_map(|c| c.obs.callsign.clone());
    let pick =
        |f: fn(&Observation) -> &Option<String>| by_identity.iter().find_map(|c| f(c.obs).clone());
    view.platform.name = pick(|o| &o.platform.name);
    view.platform.class = pick(|o| &o.platform.class);
    view.platform.type_code = pick(|o| &o.platform.type_code);
    view.platform.flag = pick(|o| &o.platform.flag);
    view.platform.hull = pick(|o| &o.platform.hull);
    provenance.insert("identity".into(), lead.key());

    let class = contribs
        .iter()
        .max_by(|a, b| {
            a.corroborated()
                .cmp(&b.corroborated())
                .then(
                    cot_specificity(a.obs.classification.cot_type.as_deref())
                        .cmp(&cot_specificity(b.obs.classification.cot_type.as_deref())),
                )
                .then(a.priority.cmp(&b.priority))
        })
        .expect("non-empty");
    view.classification = class.obs.classification.clone();
    provenance.insert("classification".into(), class.key());

    let mut by_priority: Vec<&Contribution<'_>> = contribs.iter().collect();
    by_priority.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then(b.obs.observed_at.cmp(&a.obs.observed_at))
    });
    // Extension values: a source whose entity match is trusted first (its
    // entity links are the authority), then by priority.
    let mut by_entity = by_priority.clone();
    by_entity.sort_by_key(|c| std::cmp::Reverse(c.corroborated()));
    let mut ext = serde_json::Map::new();
    for c in &by_entity {
        for (k, v) in &c.obs.ext {
            if !v.is_null() && !ext.contains_key(k) {
                ext.insert(k.clone(), v.clone());
            }
        }
    }
    view.ext = ext;

    let mut identifiers = Vec::new();
    for c in &by_priority {
        for id in &c.obs.identifiers {
            if !identifiers.contains(id) {
                identifiers.push(id.clone());
            }
        }
    }
    view.identifiers = identifiers;
    // Never marked below what went into it.
    view.security = labels.combine(by_priority.iter().filter_map(|c| c.obs.security.as_ref()));
    (view, provenance)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ot_core::{Identifier, Uncertainty};
    use serde_json::json;

    /// An observation with no kinematics, identifiers, classification or extensions.
    fn sample() -> Observation {
        serde_json::from_value(json!({
            "schema_version": 1,
            "source_id": "s",
            "source_track_key": "k",
            "observed_at": "2026-09-24T12:00:00Z",
            "received_at": "2026-09-24T12:00:00Z",
            "position": {"latitude": 0.0, "longitude": 0.0}
        }))
        .expect("minimal observation")
    }

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_790_000_000 + secs, 0).unwrap()
    }

    fn obs(source: &str, key: &str, t: i64, lat: f64, lon: f64) -> Observation {
        let mut o = sample();
        o.source_id = source.into();
        o.source_track_key = key.into();
        o.observed_at = at(t);
        o.position.latitude = lat;
        o.position.longitude = lon;
        o.kinematics.course_deg = None;
        o.kinematics.speed_mps = None;
        o.ext = Default::default();
        o
    }

    #[test]
    fn identity_keys_normalise_and_include_corroborated_entities() {
        let mut o = obs("ais", "1", 0, 0.0, 0.0);
        o.identifiers = vec![
            Identifier::new("MMSI", " 366123456 "),
            Identifier::new("imo", ""),
        ];
        o.ext.insert(
            "registry".into(),
            json!({"entity_id": "ent-1", "corroborated": true}),
        );
        assert_eq!(identity_keys(&o), ["entity:ent-1", "mmsi:366123456"]);
        o.ext.insert(
            "registry".into(),
            json!({"entity_id": "ent-1", "corroborated": false}),
        );
        assert_eq!(identity_keys(&o), ["mmsi:366123456"]);
    }

    #[test]
    fn dead_reckoning_moves_along_course() {
        let mut o = obs("ais", "1", 0, 0.0, 0.0);
        o.kinematics.course_deg = Some(90.0);
        o.kinematics.speed_mps = Some(10.0);
        let (lat, lon) = dead_reckon(&o, at(100), 600.0);
        assert!(lat.abs() < 1e-9);
        assert!((distance_m(0.0, 0.0, lat, lon) - 1000.0).abs() < 1.0);
    }

    #[test]
    fn sanity_gate_grows_with_time_and_domain() {
        let view = obs("ais", "1", 0, 32.0, -117.0);
        // 20 km away at the same moment: too far for any domain.
        let far = obs("tak", "x", 0, 32.18, -117.0);
        assert!(!sanity_gate(&far, &view, &GateSettings::default()).pass);
        // The same 20 km five minutes later: an aircraft could, a ship could not.
        let mut later = obs("tak", "x", 300, 32.18, -117.0);
        later.classification.domain = Some(Domain::Air);
        assert!(sanity_gate(&later, &view, &GateSettings::default()).pass);
        later.classification.domain = Some(Domain::Surface);
        let g = sanity_gate(&later, &view, &GateSettings::default());
        assert!(g.pass, "{g:?}: 10 km + 40 m/s × 300 s = 22 km");
        let mut far_later = obs("tak", "x", 60, 32.18, -117.0);
        far_later.classification.domain = Some(Domain::Surface);
        assert!(!sanity_gate(&far_later, &view, &GateSettings::default()).pass);
    }

    #[test]
    fn best_view_takes_each_group_from_the_best_contributor() {
        let mut ais = obs("ais", "366", 0, 32.0, -117.0);
        ais.uncertainty = Some(Uncertainty {
            circular_error_m: Some(10.0),
            ..Default::default()
        });
        ais.name = Some("TED STEVENS".into());
        ais.classification.cot_type = Some("a-f-S".into());
        ais.identifiers = vec![Identifier::new("mmsi", "366")];
        ais.ext.insert("imo".into(), json!("9876543"));

        let mut radar = obs("radar", "t7", 5, 32.001, -117.0);
        radar.uncertainty = Some(Uncertainty {
            circular_error_m: Some(150.0),
            ..Default::default()
        });
        radar.classification.cot_type = Some("a-f-S-C-L-D-D".into());
        radar.identifiers = vec![Identifier::new("track", "t7")];
        radar.ext.insert("imo".into(), json!("0000000"));
        radar.ext.insert("rcs".into(), json!(12));

        let (v, p) = best_view(
            &[
                Contribution {
                    obs: &ais,
                    priority: 50,
                },
                Contribution {
                    obs: &radar,
                    priority: 100,
                },
            ],
            60.0,
            &LabelSettings::default(),
        );
        // Both within the window: AIS has the smaller uncertainty.
        assert_eq!(p["position"], "ais/366");
        assert_eq!(v.position.latitude, 32.0);
        // Radar has the higher priority, but no name: AIS fills it.
        assert_eq!(p["identity"], "radar/t7");
        assert_eq!(v.name.as_deref(), Some("TED STEVENS"));
        assert_eq!(p["classification"], "radar/t7");
        assert_eq!(v.classification.cot_type.as_deref(), Some("a-f-S-C-L-D-D"));
        assert_eq!(v.ext["imo"], "0000000");
        assert_eq!(v.ext["rcs"], 12);
        assert_eq!(v.identifiers.len(), 2);

        // AIS a minute and a half older: outside the window, radar supplies position.
        let mut old = ais.clone();
        old.observed_at = at(-90);
        let (_, p) = best_view(
            &[
                Contribution {
                    obs: &old,
                    priority: 50,
                },
                Contribution {
                    obs: &radar,
                    priority: 100,
                },
            ],
            60.0,
            &LabelSettings::default(),
        );
        assert_eq!(p["position"], "radar/t7");

        // A registry-corroborated report supplies identity and classification.
        let mut reg = ais.clone();
        reg.ext.insert(
            "registry".into(),
            json!({"entity_id": "ent-1", "corroborated": true}),
        );
        let (_, p) = best_view(
            &[
                Contribution {
                    obs: &reg,
                    priority: 50,
                },
                Contribution {
                    obs: &radar,
                    priority: 100,
                },
            ],
            60.0,
            &LabelSettings::default(),
        );
        assert_eq!(p["identity"], "ais/366");
        assert_eq!(p["classification"], "ais/366");
    }

    #[test]
    fn kinematic_gate_normalises_by_uncertainty_and_time() {
        let s = KinematicSettings::default();
        let mut view = obs("track", "1", 0, 63.44, 10.40);
        view.uncertainty = Some(Uncertainty {
            circular_error_m: Some(10.0),
            ..Default::default()
        });
        // 20 m apart, both about 8.5 m sigma: d2 = 400 / 144 < 9.21.
        let mut radar = obs("radar", "r", 0, 63.44018, 10.40);
        radar.uncertainty = view.uncertainty;
        let k = kinematic(&radar, &view, &s);
        assert!(k.pass, "{k:?}");
        // 60 m apart: outside.
        radar.position.latitude = 63.44054;
        assert!(!kinematic(&radar, &view, &s).pass);
        // The same 60 m after 60 s without kinematics: the drift allowance admits it.
        radar.observed_at = at(60);
        assert!(kinematic(&radar, &view, &s).pass);
    }

    #[test]
    fn vetoes_conflicting_identifiers_and_domains() {
        let mut a = obs("ais", "1", 0, 0.0, 0.0);
        let mut b = obs("tak", "2", 0, 0.0, 0.0);
        assert_eq!(veto(&a, &b), None);
        a.identifiers = vec![Identifier::new("mmsi", "366")];
        b.identifiers = vec![Identifier::new("MMSI", "367")];
        assert_eq!(veto(&a, &b).as_deref(), Some("mmsi 366 vs 367"));
        b.identifiers = vec![Identifier::new("hull", "DDG-51")];
        assert_eq!(veto(&a, &b), None);
        a.classification.domain = Some(Domain::Surface);
        b.classification.domain = Some(Domain::Air);
        assert!(veto(&a, &b).is_some());
    }

    #[test]
    fn evidence_sums_spaced_comparisons_within_the_window() {
        let s = KinematicSettings::default();
        let mut e = Evidence::default();
        let tally = |ln_lr, comparisons, outside| {
            Some(Tally {
                ln_lr,
                comparisons,
                outside,
            })
        };
        assert_eq!(
            e.record(("a", at(0)), ("b", at(0)), 2.0, false, (5, &s)),
            tally(2.0, 1, 0)
        );
        // Too soon after the last one to be new evidence.
        assert_eq!(
            e.record(("a", at(1)), ("b", at(1)), 9.0, false, (5, &s)),
            None
        );
        // A new report, but against the other side's same old one.
        assert_eq!(
            e.record(("a", at(40)), ("b", at(0)), 9.0, false, (5, &s)),
            None
        );
        assert_eq!(
            e.record(("a", at(3)), ("b", at(3)), -1.0, true, (5, &s)),
            tally(1.0, 2, 1)
        );
        for t in [6, 9, 12] {
            e.record(("a", at(t)), ("b", at(t)), 1.0, false, (5, &s));
        }
        // Only the last five count: -1 + 1 + 1 + 1 + 1.
        assert_eq!(
            e.record(("a", at(15)), ("b", at(15)), 1.0, false, (5, &s)),
            tally(3.0, 5, 1)
        );
        // Older than the window: gone.
        assert_eq!(
            e.record(("a", at(60)), ("b", at(60)), 0.5, false, (5, &s)),
            tally(0.5, 1, 0)
        );
        assert!((posterior(0.5, 0.0) - 0.5).abs() < 1e-12);
        assert!(posterior(0.01, 10.0) > 0.99);
    }

    #[test]
    fn the_output_filter_checks_areas_attributes_confidence_and_a_rule() {
        let mut o = obs("ais", "1", 0, 10.0, 179.5);
        o.classification.domain = Some(Domain::Surface);
        let t = ot_core::SystemTrack::from_first_observation("OTK000000001".parse().unwrap(), o);
        let f = |v: serde_json::Value| -> OutputFilter { serde_json::from_value(v).unwrap() };
        assert_eq!(OutputFilter::default().rejects(&t), None);
        // An area across the antimeridian holds it; an excluded one refuses it.
        let pacific = json!({"min_lat": 0, "min_lon": 170, "max_lat": 20, "max_lon": -170});
        assert_eq!(f(json!({"areas": [pacific]})).rejects(&t), None);
        let mut ex = pacific.clone();
        ex["exclude"] = json!(true);
        ex["name"] = json!("range");
        assert_eq!(
            f(json!({"areas": [ex]})).rejects(&t).as_deref(),
            Some("inside excluded area range")
        );
        assert!(
            f(json!({"domains": ["air"]}))
                .rejects(&t)
                .unwrap()
                .contains("domain surface")
        );
        assert_eq!(
            f(json!({"domains": ["Surface"], "affiliations": ["unknown"]})).rejects(&t),
            None
        );
        // A lone track feed report is taken as real: confidence 1.
        assert_eq!(f(json!({"min_confidence": 0.9})).rejects(&t), None);
        let rule = f(json!({"rule": {"path": "confidence", "lt": 0.5}}));
        assert_eq!(
            rule.rejects(&t).as_deref(),
            Some("the output rule does not hold")
        );
        assert!(
            f(json!({"areas": [{"min_lat": 5, "min_lon": 0, "max_lat": 1, "max_lon": 1}]}))
                .validate()
                .is_err()
        );
    }

    #[test]
    fn a_track_carries_the_highest_classification_of_its_sources() {
        let label = |c: &str| ot_core::SecurityLabel {
            classification: c.into(),
            restrictions: vec!["NOFORN".into()],
            sharing: Some("USA".into()),
        };
        let mut high = obs("gmti", "G1", 0, 32.0, -117.0);
        high.security = Some(label("SECRET"));
        let mut low = obs("gps", "TM01", 0, 32.0, -117.0);
        low.security = Some(label("UNCLASSIFIED"));
        let plain = obs("ais", "366", 0, 32.0, -117.0);
        // The unclassified source has the higher priority, but the track
        // is marked with the highest classification that went into it.
        let (view, _) = best_view(
            &[
                Contribution {
                    obs: &low,
                    priority: 150,
                },
                Contribution {
                    obs: &plain,
                    priority: 200,
                },
                Contribution {
                    obs: &high,
                    priority: 100,
                },
            ],
            60.0,
            &LabelSettings::default(),
        );
        assert_eq!(
            view.security.as_ref().map(|l| l.classification.as_str()),
            Some("SECRET")
        );
        let mut t = ot_core::SystemTrack::from_first_observation(
            "OTK000000001".parse().unwrap(),
            plain.clone(),
        );
        t.view = view;
        let ctx = ot_core::wire::PublishContext {
            node_id: "n".into(),
            version: "v".into(),
            correlation: String::new(),
        };
        let m = serde_json::to_value(ot_core::wire::to_message(&t, &ctx, at(0))).unwrap();
        assert_eq!(
            m["security"],
            json!({"classification": "SECRET", "restrictions": ["NOFORN"], "sharing": "USA"})
        );
        // Unlabelled: no security field at all.
        t.view = plain;
        let m = serde_json::to_value(ot_core::wire::to_message(&t, &ctx, at(0))).unwrap();
        assert!(m.get("security").is_none());
        assert!(label(" ").validate().is_err());
    }

    #[test]
    fn chi_square_quantiles() {
        assert!((chi2_quantile(0.99, 2) - 9.2103).abs() < 1e-3);
        assert!((chi2_quantile(0.99, 4) - 13.2767).abs() < 1e-3);
        assert!((chi2_quantile(0.95, 4) - 9.4877).abs() < 1e-3);
        let old: KinematicSettings = serde_json::from_value(json!({"chi2_gate": 9.21})).unwrap();
        assert!(
            (old.gate() - 0.99).abs() < 1e-4,
            "a chi-square gate from before"
        );
    }

    #[test]
    fn propagation_grows_the_older_reports_covariance() {
        let s = KinematicSettings::default();
        let mut o = obs("ais", "1", 0, 0.0, 0.0);
        o.kinematics.course_deg = Some(90.0);
        o.kinematics.speed_mps = Some(10.0);
        let e = estimate(&o, &s);
        let later = e.predict_with(at(10), s.process_noise_mps2, s.max_age_secs, None);
        assert!((distance_m(0.0, 0.0, later.lat, later.lon) - 100.0).abs() < 0.5);
        // Position variance: σ² + σv²·t² + q²t⁴/4.
        let want = 25.0 + 1.0f64.powi(2) * 100.0 + 0.09 * 1e4 / 4.0;
        assert!(
            (later.p[1][1] - want).abs() < 1e-6,
            "{} vs {want}",
            later.p[1][1]
        );
        // No motion reported: a still estimate with a wide velocity spread.
        let still = estimate(&obs("ais", "2", 0, 0.0, 0.0), &s);
        assert!(still.v.is_none() && still.p[2][2] > 100.0);
    }

    /// correlation-4: a moored vessel's AIS report, three minutes old, is
    /// still a position to within a few hundred metres, not kilometres.
    #[test]
    fn a_stopped_target_drifts_instead_of_manoeuvring() {
        let s = KinematicSettings::default();
        let mut o = obs("ais", "1", 0, 0.0, 0.0);
        o.kinematics.speed_mps = Some(0.1);
        let e = estimate(&o, &s);
        assert!(e.stopped);
        let manoeuvring = e.predict_with(at(180), s.process_noise_mps2, s.max_age_secs, None);
        let drifting = e.predict_with(
            at(180),
            s.process_noise_mps2,
            s.max_age_secs,
            Some(s.stopped_drift_mps),
        );
        let sd = |p: &M4| ((p[0][0] + p[1][1]) / 2.0).sqrt();
        assert!(sd(&manoeuvring.p) > 2000.0, "{}", sd(&manoeuvring.p));
        assert!(sd(&drifting.p) < 300.0, "{}", sd(&drifting.p));
        // A moving target still manoeuvres.
        let mut m = obs("ais", "2", 0, 0.0, 0.0);
        m.kinematics.speed_mps = Some(8.0);
        m.kinematics.course_deg = Some(90.0);
        let moving = estimate(&m, &s).predict_with(
            at(180),
            s.process_noise_mps2,
            s.max_age_secs,
            Some(s.stopped_drift_mps),
        );
        assert!(sd(&moving.p) > 2000.0);
    }

    /// correlation-4: a new report against a view already used counts, at
    /// its weight, no sooner than `reuse_interval_secs` after the last.
    #[test]
    fn a_reused_view_counts_at_its_weight_and_not_too_often() {
        let s = KinematicSettings::default();
        let mut e = Evidence::default();
        let r = |e: &mut Evidence, a: i64, b: i64, reuse: Option<f64>| {
            e.record_reusing(("a", at(a)), ("b", at(b)), 2.0, false, (5, &s), reuse)
        };
        assert_eq!(r(&mut e, 0, 0, Some(0.9)).map(|t| t.comparisons), Some(1));
        // The same view again, 3 s later: too soon for a reused view.
        assert_eq!(r(&mut e, 3, 0, Some(0.9)), None);
        // 10 s later it counts, weighted.
        let t = r(&mut e, 10, 0, Some(0.9)).unwrap();
        assert_eq!(t.comparisons, 2);
        assert!((t.ln_lr - (2.0 + 1.8)).abs() < 1e-9, "{t:?}");
        // Without reuse (the split test's use) it does not count.
        assert_eq!(r(&mut e, 30, 0, None), None);
        // Both sides new: full weight (the first comparison, at 0 s, has
        // left the 30 s window).
        assert!((r(&mut e, 33, 33, None).unwrap().ln_lr - (1.8 + 2.0)).abs() < 1e-9);
    }

    /// A GMTI track passing a parked GPS vehicle at 56 m (Garden Island,
    /// 13 Oct 2015): close enough in position, but one is moving at 6 m/s and
    /// the other is still. Before correlation-3 only positions were compared
    /// and the two paired.
    #[test]
    fn a_moving_track_does_not_pair_with_a_still_one_nearby() {
        let s = KinematicSettings::default();
        // Parked: speed 0 and no course, as a still GPS reports it.
        let mut gps = obs("gps", "TM01", 0, -34.80595, 138.54036);
        gps.kinematics.speed_mps = Some(0.0);
        gps.uncertainty = Some(Uncertainty {
            circular_error_m: Some(5.0),
            ..Default::default()
        });
        let (lat, lon) = (-34.80595 + 56.0 / 110_540.0, 138.54036);
        let mut gmti = obs("gmti", "G1", 0, lat, lon);
        gmti.kinematics.course_deg = Some(40.0);
        gmti.kinematics.speed_mps = Some(6.0);
        gmti.uncertainty = Some(Uncertainty {
            ellipse: Some(ot_core::Ellipse {
                semi_major_m: 60.0,
                semi_minor_m: 20.0,
                orientation_deg: 30.0,
            }),
            covariance: Some(ot_core::Covariance {
                position: ot_core::Ellipse {
                    semi_major_m: 60.0,
                    semi_minor_m: 20.0,
                    orientation_deg: 30.0,
                }
                .covariance(),
                velocity: Some([1.0, 0.0, 1.0]),
                cross: None,
            }),
            ..Default::default()
        });
        let k = kinematic(&gmti, &gps, &s);
        assert_eq!(k.dof, 4);
        assert!(!k.pass, "{k:?}");
        assert!(k.ln_lr <= (0.01f64).ln() + 1e-3, "{k:?}");
        // Positions alone would have passed.
        let mut still = gmti.clone();
        still.kinematics = Default::default();
        let k = kinematic(&still, &gps, &s);
        assert_eq!(k.dof, 2);
        assert!(k.pass, "{k:?}");
        // The same vehicle moving the same way pairs quickly.
        gps.kinematics.course_deg = Some(40.0);
        gps.kinematics.speed_mps = Some(6.3);
        let k = kinematic(&gmti, &gps, &s);
        assert!(k.pass && k.ln_lr > 2.0, "{k:?}");
    }

    #[test]
    fn correlation_settings_take_partial_json_and_are_checked() {
        let s: CorrelationSettings =
            serde_json::from_value(json!({"mode": "suggest", "kinematic": {"m": 3}})).unwrap();
        assert_eq!(s.mode, Mode::Suggest);
        assert_eq!(s.kinematic.m, 3);
        assert_eq!(s.kinematic.n, 5, "unset fields keep their defaults");
        s.validate().unwrap();
        let bad: CorrelationSettings = serde_json::from_value(json!({"split": {"m": 7}})).unwrap();
        assert!(bad.validate().unwrap_err().contains("split.m"));
        for (bad, name) in [
            (json!({"split": {"window_secs": 0.0}}), "split.window_secs"),
            (
                json!({"kinematic": {"local_density_radius_m": -1.0}}),
                "kinematic.local_density_radius_m",
            ),
        ] {
            let bad: CorrelationSettings = serde_json::from_value(bad).unwrap();
            assert!(bad.validate().unwrap_err().contains(name));
        }
        // 0 turns the local density off.
        let off: CorrelationSettings =
            serde_json::from_value(json!({"kinematic": {"local_density_radius_m": 0.0}})).unwrap();
        off.validate().unwrap();
        assert!(serde_json::from_value::<CorrelationSettings>(json!({"nope": 1})).is_err());
    }

    #[test]
    fn grid_finds_neighbours_across_cells() {
        let mut g = Grid::default();
        g.put(1, 63.449, 10.449);
        g.put(2, 63.451, 10.451);
        g.put(3, 64.0, 10.4);
        let mut near = g.near(63.45, 10.45);
        near.sort();
        assert_eq!(near, [1, 2]);
        g.put(2, 64.0, 10.4);
        assert_eq!(g.near(63.45, 10.45), [1]);
        g.remove(1);
        assert!(g.near(63.45, 10.45).is_empty());
    }
}
