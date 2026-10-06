//! The per-source processing pipeline, free of I/O:
//!
//! frame → codec → records → reject → mapping → static join → registry →
//! affiliation → filter → tracker → throttle → observation.
//!
//! The tracker stage is optional: it turns a detection feed's plots into
//! source tracks (see [`crate::tracker`]).
//!
//! The worker feeds frames in and ships observations out; everything here is
//! deterministic given its inputs, so it can be replayed in tests and probes.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::time::Duration;

use chrono::{DateTime, Utc};
use ot_core::{Identifier, Observation};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::codec::{Codec, CodecConfig};
use crate::expr::{Condition, ValueSpec, as_string};
use crate::frame::Frame;
use crate::mapping::{self, MapOutcome, MappingSpec, RuleKind};
use crate::registry::{RegistryLookup, RegistryStage};
use crate::trace::{SampleTrace, Trace};

/// Everything between the transport and the observation stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PipelineSpec {
    pub codec: CodecConfig,
    pub mapping: MappingSpec,
    #[serde(default)]
    pub static_join: StaticJoinSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Registry resolution settings. Without them the registry still runs
    /// with defaults: identifiers are resolved and graded under
    /// `ext.registry`, and the entity's OTH-GOLD minimum populates the track
    /// (see [`crate::registry::default_links`]).
    pub registry: Option<RegistryStage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub affiliation: Option<AffiliationStage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<FilterSpec>,
    /// Form tracks from detections (GNN or MHT) before they leave the source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracker: Option<crate::tracker::TrackerSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub throttle: Option<ThrottleSpec>,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PipelineError {
    #[error(transparent)]
    Mapping(#[from] mapping::MappingError),
    #[error("{0}")]
    Tracker(String),
    #[error("{0}")]
    Codec(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StaticJoinSpec {
    /// How long cached identity fields stay usable.
    #[serde(default = "week")]
    pub ttl_secs: u64,
}

fn week() -> u64 {
    7 * 24 * 3600
}

impl Default for StaticJoinSpec {
    fn default() -> Self {
        Self { ttl_secs: week() }
    }
}

/// Derive affiliation from a country code, e.g. a registry flag or the
/// ICAO address block. Country lists are configuration, not code.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AffiliationStage {
    /// Value (over the observation) holding an ISO country code.
    pub country: ValueSpec,
    #[serde(default)]
    pub friendly: BTreeSet<String>,
    #[serde(default)]
    pub hostile: BTreeSet<String>,
    #[serde(default)]
    pub neutral: BTreeSet<String>,
    /// Affiliation when a country is known but in no list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub otherwise: Option<String>,
    /// Affiliation when no country is known (null leaves it unset).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unknown_country: Option<String>,
}

impl AffiliationStage {
    fn run(&self, obs: &mut Value) {
        let country = self.country.eval(obs);
        let code = as_string(&country).trim().to_uppercase();
        let aff = if code.is_empty() {
            self.unknown_country.clone()
        } else if self.friendly.contains(&code) {
            Some("friend".to_owned())
        } else if self.hostile.contains(&code) {
            Some("hostile".to_owned())
        } else if self.neutral.contains(&code) {
            Some("neutral".to_owned())
        } else {
            self.otherwise.clone()
        };
        if let Some(a) = aff {
            let path: crate::path::Path = "classification.affiliation".parse().expect("path");
            path.set(obs, Value::String(a));
        }
    }
}

/// Keep or drop mapped observations. Paths address the observation (and
/// `ext.registry.*` once the registry stage has run).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilterSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_if: Option<Condition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drop_if: Option<Condition>,
}

impl FilterSpec {
    /// Why the filter drops `obs`, or `None` to keep it.
    fn verdict(&self, obs: &Value) -> Option<&'static str> {
        if !self.keep_if.as_ref().is_none_or(|c| c.eval(obs)) {
            Some("keep_if did not match")
        } else if self.drop_if.as_ref().is_some_and(|c| c.eval(obs)) {
            Some("drop_if matched")
        } else {
            None
        }
    }
}

/// Per source track: never more often than `min_interval_secs`; between that
/// and `heartbeat_secs` only if it moved at least `min_move_m`; always after
/// the heartbeat.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThrottleSpec {
    #[serde(default)]
    pub min_interval_secs: f64,
    #[serde(default = "ten_minutes")]
    pub heartbeat_secs: f64,
    #[serde(default)]
    pub min_move_m: f64,
}

fn ten_minutes() -> f64 {
    600.0
}

#[derive(Debug, Clone, Copy)]
struct LastWrite {
    at: DateTime<Utc>,
    lat: f64,
    lon: f64,
}

impl ThrottleSpec {
    /// `changed`: a static report changed the track's joined details since
    /// the last write, so it ships as soon as the minimum interval allows.
    fn due(&self, last: Option<&LastWrite>, obs: &Observation, changed: bool) -> bool {
        let Some(last) = last else { return true };
        let dt = (obs.observed_at - last.at).num_milliseconds() as f64 / 1000.0;
        if dt < self.min_interval_secs {
            return false;
        }
        if changed || dt >= self.heartbeat_secs {
            return true;
        }
        self.min_move_m > 0.0
            && distance_m(
                last.lat,
                last.lon,
                obs.position.latitude,
                obs.position.longitude,
            ) >= self.min_move_m
    }

    /// Why `obs` is not due, for a trace.
    fn why_not(&self, last: Option<&LastWrite>, obs: &Observation) -> String {
        let Some(last) = last else {
            return "throttled".into();
        };
        let dt = (obs.observed_at - last.at).num_milliseconds() as f64 / 1000.0;
        if dt < self.min_interval_secs {
            return format!(
                "throttled: {dt:.1} s after the last report, under the {} s minimum",
                self.min_interval_secs
            );
        }
        let moved = distance_m(
            last.lat,
            last.lon,
            obs.position.latitude,
            obs.position.longitude,
        );
        format!(
            "throttled: moved {moved:.0} m in {dt:.0} s, under {} m and before the {} s heartbeat",
            self.min_move_m, self.heartbeat_secs
        )
    }
}

/// Equirectangular distance, good to well under 1% at throttle scales.
pub fn distance_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let dy = (lat2 - lat1) * 111_320.0;
    let dx = (lon2 - lon1) * 111_320.0 * ((lat1 + lat2) / 2.0).to_radians().cos();
    (dx * dx + dy * dy).sqrt()
}

/// Cached identity fields for one source track.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StaticEntry {
    pub fields: Value,
    #[serde(default)]
    pub identifiers: Vec<Identifier>,
    pub updated_at: DateTime<Utc>,
}

/// Counters for one batch, merged into per-source metrics by the worker.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Counts {
    pub frames: u64,
    pub decode_errors: u64,
    pub records: u64,
    pub rejected: HashMap<String, u64>,
    pub unmatched: u64,
    pub statics: u64,
    pub invalid: u64,
    pub filtered: u64,
    /// Detections handed to the tracker, and those too late for their scan.
    pub plots: u64,
    pub late_plots: u64,
    /// Runs of a tracker plugin that failed.
    pub tracker_errors: u64,
    pub throttled: u64,
    pub emitted: u64,
    pub grades: HashMap<&'static str, u64>,
}

impl Counts {
    /// Flatten to `(metric, count)` pairs, skipping zeros.
    pub fn pairs(&self) -> Vec<(String, u64)> {
        let mut out: Vec<(String, u64)> = [
            ("frames", self.frames),
            ("decode_error", self.decode_errors),
            ("records", self.records),
            ("unmatched", self.unmatched),
            ("static", self.statics),
            ("invalid", self.invalid),
            ("filtered", self.filtered),
            ("plots", self.plots),
            ("late_plots", self.late_plots),
            ("tracker_error", self.tracker_errors),
            ("throttled", self.throttled),
            ("emitted", self.emitted),
        ]
        .into_iter()
        .filter(|(_, n)| *n > 0)
        .map(|(k, n)| (k.to_owned(), n))
        .collect();
        let rejected: u64 = self.rejected.values().sum();
        if rejected > 0 {
            out.push(("rejected".into(), rejected));
        }
        for (reason, n) in &self.rejected {
            out.push((format!("rejected:{reason}"), *n));
        }
        for (grade, n) in &self.grades {
            out.push((format!("grade:{grade}"), *n));
        }
        out
    }
}

/// Output of one frame.
#[derive(Debug, Default)]
pub struct Output {
    pub observations: Vec<Observation>,
    /// Static cache entries that changed, for the worker to persist.
    pub statics: Vec<(String, StaticEntry)>,
    /// Entity fields the feed reports differently (track → entity links),
    /// per entity, for the worker to write.
    pub entity_updates: Vec<(String, Vec<(String, Value)>)>,
    /// The last decode or validation error seen, for status reporting.
    pub last_error: Option<String>,
    /// Plots given to the tracker stage, when [`Pipeline::keep_plots`] is on.
    pub plots: Vec<Observation>,
}

pub struct Pipeline {
    source_id: String,
    spec: PipelineSpec,
    codec: Codec,
    statics: HashMap<String, StaticEntry>,
    throttle: HashMap<String, LastWrite>,
    /// Source tracks whose static details changed since their last write.
    details_changed: HashSet<String>,
    /// Extension field types for the mapping's schema version.
    schema: Option<crate::schema::ExtensionSchema>,
    pub counts: Counts,
    /// Used when the spec configures no registry stage.
    default_registry: RegistryStage,
    /// The last value sent to each (entity, field), so a feed repeating a
    /// value writes it once, not until the registry snapshot catches up.
    entity_sent: HashMap<(String, String), Value>,
    tracker: Option<Stage>,
    keep_plots: bool,
}

/// The tracker stage: OpenTrack's own tracker, or a plugin's.
enum Stage {
    Builtin(Box<crate::tracker::Tracker>),
    Plugin {
        tracker: Box<dyn crate::plugin::PluginTracker>,
        /// `<plugin>-<version>`, stamped as `provenance.tracker`.
        version: String,
    },
}

impl Stage {
    fn new(spec: crate::tracker::TrackerSpec) -> Result<Self, String> {
        if spec.algorithm != crate::tracker::Algorithm::Plugin {
            return crate::tracker::Tracker::new(spec).map(|t| Stage::Builtin(Box::new(t)));
        }
        spec.validate()?;
        let name = spec.plugin.as_deref().unwrap_or_default();
        let plugin = crate::plugin::plugin(name).ok_or_else(|| format!("no plugin {name:?}"))?;
        let version = format!("{name}-{}", plugin.manifest().version);
        Ok(Stage::Plugin {
            tracker: plugin.tracker(&spec.options)?,
            version,
        })
    }

    fn push(&mut self, obs: Observation, received_at: DateTime<Utc>) {
        match self {
            Stage::Builtin(t) => t.push(obs, received_at),
            Stage::Plugin { tracker, .. } => tracker.push(obs, received_at),
        }
    }
}

impl Pipeline {
    pub fn new(source_id: impl Into<String>, spec: PipelineSpec) -> Result<Self, PipelineError> {
        spec.mapping.validate()?;
        let tracker = spec
            .tracker
            .clone()
            .map(Stage::new)
            .transpose()
            .map_err(PipelineError::Tracker)?;
        Ok(Self {
            tracker,
            source_id: source_id.into(),
            codec: Codec::new(spec.codec.clone())
                .map_err(|e| PipelineError::Codec(e.to_string()))?,
            spec,
            statics: HashMap::new(),
            throttle: HashMap::new(),
            details_changed: HashSet::new(),
            schema: None,
            counts: Counts::default(),
            default_registry: RegistryStage::default(),
            entity_sent: HashMap::new(),
            keep_plots: false,
        })
    }

    /// Also return the plots the tracker stage is given (benchmarks score
    /// what the sensor saw as well as what the tracker made of it).
    pub fn keep_plots(mut self, on: bool) -> Self {
        self.keep_plots = on;
        self
    }

    /// Enforce this extension schema (types, defaults, required fields).
    pub fn with_schema(mut self, schema: crate::schema::ExtensionSchema) -> Self {
        self.schema = Some(schema);
        self
    }

    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    pub fn spec(&self) -> &PipelineSpec {
        &self.spec
    }

    /// Seed the static cache (e.g. from Redis after a restart).
    pub fn load_static(&mut self, key: String, entry: StaticEntry) {
        self.statics.insert(key, entry);
    }

    /// Take and reset the counters.
    pub fn take_counts(&mut self) -> Counts {
        std::mem::take(&mut self.counts)
    }

    /// Forget throttle and static state older than the static TTL.
    pub fn prune(&mut self, now: DateTime<Utc>) {
        let ttl = chrono::Duration::from_std(Duration::from_secs(self.spec.static_join.ttl_secs))
            .unwrap_or(chrono::Duration::MAX);
        self.statics.retain(|_, e| now - e.updated_at < ttl);
        let heartbeat = self
            .spec
            .throttle
            .as_ref()
            .map_or(0.0, |t| t.heartbeat_secs);
        let keep = chrono::Duration::milliseconds((heartbeat * 1000.0) as i64 * 2);
        self.throttle.retain(|_, w| now - w.at < keep);
    }

    pub fn process(&mut self, frame: &Frame, registry: &dyn RegistryLookup) -> Output {
        self.run_frame(frame, registry, None)
    }

    /// [`Self::process`], recording in `trace` what each stage made of the
    /// frame's records while the trace has room (see [`crate::trace`]).
    pub fn process_traced(
        &mut self,
        frame: &Frame,
        registry: &dyn RegistryLookup,
        trace: &mut Trace,
    ) -> Output {
        self.run_frame(frame, registry, Some(trace))
    }

    /// The stages a trace records, after the transport, in the order they
    /// run, with the ids the UI gives them.
    fn trace_stages(&self) -> Vec<&'static str> {
        let s = &self.spec;
        let mut out = vec!["decode"];
        if !s.mapping.reject.is_empty() {
            out.push("reject");
        }
        out.push("map");
        if self.has_join() {
            out.push("join");
        }
        out.push("registry");
        for (id, on) in [
            ("affiliation", s.affiliation.is_some()),
            ("filter", s.filter.is_some()),
            ("tracker", s.tracker.is_some()),
            ("throttle", s.throttle.is_some()),
        ] {
            if on {
                out.push(id);
            }
        }
        out
    }

    /// Whether static rules feed an identity join.
    fn has_join(&self) -> bool {
        self.spec
            .mapping
            .rules
            .iter()
            .any(|r| r.kind == RuleKind::Static)
    }

    fn run_frame(
        &mut self,
        frame: &Frame,
        registry: &dyn RegistryLookup,
        mut trace: Option<&mut Trace>,
    ) -> Output {
        let mut out = Output::default();
        self.counts.frames += 1;
        let stages = match trace.as_deref_mut() {
            Some(t) => {
                t.start_frame();
                self.trace_stages()
            }
            None => Vec::new(),
        };
        let records = match self.codec.decode(frame) {
            Ok(r) => r,
            Err(e) => {
                self.counts.decode_errors += 1;
                // A frame that does not decode is a sample of its own.
                if let Some(t) = trace.as_deref_mut()
                    && let Some(at) = t.begin(frame, &stages)
                {
                    t.samples[at].dropped("decode", format!("decode error: {e}"));
                }
                out.last_error = Some(e.to_string());
                return out;
            }
        };
        // A tracker timed by the sensor follows what the codec has learnt.
        if let Some(Stage::Builtin(t)) = self.tracker.as_mut()
            && let Some(h) = self.codec.hints()
        {
            t.set_revisit(h.revisit_secs, h.revisit_source);
        }
        for record in records {
            let mut f = match trace.as_deref_mut() {
                Some(t) => t.begin(frame, &stages).map(|at| &mut t.samples[at]),
                None => None,
            };
            if let Some(f) = f.as_deref_mut() {
                f.item("decode", || record.clone());
            }
            self.process_record(&record, frame.received_at, registry, &mut out, f);
        }
        self.run_tracker(frame.received_at, false, &mut out, trace.as_deref_mut());
        // Plots still waiting for the rest of their scan.
        if let Some(Stage::Builtin(t)) = self.tracker.as_ref()
            && let Some(tr) = trace
        {
            for f in &mut tr.samples {
                if !f.plots.is_empty()
                    && t.waiting().any(|w| f.plots.contains(w))
                    && let Some(s) = f.stage("tracker")
                {
                    s.held = true;
                }
            }
        }
        out
    }

    /// The windows a tracker with `auto_timing` is using, once it knows the
    /// sensor's revisit period.
    pub fn tracker_timing(&self) -> Option<&crate::tracker::Timing> {
        match self.tracker.as_ref()? {
            Stage::Builtin(t) => t.timing(),
            Stage::Plugin { .. } => None,
        }
    }

    /// Run the tracker on scans still waiting (plots grouped by time wait
    /// `scan_hold_secs` for more; `force` runs them regardless). The worker
    /// calls this on a timer; a dry run calls it with `force` at the end.
    pub fn flush(&mut self, now: DateTime<Utc>, force: bool) -> Output {
        let mut out = Output::default();
        self.run_tracker(now, force, &mut out, None);
        out
    }

    /// [`Self::flush`], adding the reports to the traced samples whose plots
    /// they came from. With `force` (the end of a dry run), a sample whose
    /// plots gave no report is marked so.
    pub fn flush_traced(&mut self, now: DateTime<Utc>, force: bool, trace: &mut Trace) -> Output {
        let mut out = Output::default();
        self.run_tracker(now, force, &mut out, Some(trace));
        if force {
            let why = match self.tracker {
                Some(Stage::Plugin { .. }) => {
                    "a tracker plugin's reports are not traced back to the records behind them"
                }
                _ => {
                    "no track report: the tracker has not confirmed a track from these plots (or merged them into a nearby plot, or they came too late for their scan)"
                }
            };
            for f in &mut trace.samples {
                let plots = !f.plots.is_empty();
                if let Some(s) = f.stage("tracker")
                    && plots
                    && s.items.is_empty()
                    && s.dropped.is_empty()
                {
                    s.dropped.push(why.to_owned());
                }
            }
        }
        out
    }

    /// Run the tracker stage. When tracing, each report of the built-in
    /// tracker is added to the sample whose detection it came from.
    fn run_tracker(
        &mut self,
        now: DateTime<Utc>,
        force: bool,
        out: &mut Output,
        mut trace: Option<&mut Trace>,
    ) {
        // The detection behind each report, only when tracing.
        let mut dets: Vec<Observation> = Vec::new();
        let reports = match self.tracker.as_mut() {
            None => return,
            Some(Stage::Builtin(t)) => {
                let force = force || t.spec().scans == crate::tracker::ScanGrouping::Frame;
                let reports = t.run(now, force);
                self.counts.late_plots += std::mem::take(&mut t.late);
                if trace.is_some() {
                    dets = reports.iter().map(|(_, d)| d.clone()).collect();
                }
                reports.into_iter().map(|(obs, _)| obs).collect()
            }
            Some(Stage::Plugin { tracker, version }) => match tracker.run(now, force) {
                Ok(tracks) => {
                    let mut tracks: Vec<Observation> = tracks;
                    for t in &mut tracks {
                        t.source_id = self.source_id.clone();
                        t.provenance.tracker = Some(version.clone());
                    }
                    tracks
                }
                Err(e) => {
                    self.counts.tracker_errors += 1;
                    out.last_error = Some(format!("tracker plugin: {e}"));
                    Vec::new()
                }
            },
        };
        for (i, obs) in reports.into_iter().enumerate() {
            let mut f = None;
            if let Some(t) = trace.as_deref_mut() {
                f = dets
                    .get(i)
                    .and_then(|d| t.samples.iter_mut().find(|f| f.plots.contains(d)));
                if let Some(f) = f.as_deref_mut() {
                    f.obs("tracker", &obs);
                }
            }
            self.emit(obs, out, f);
        }
    }

    /// Throttle, then ship. A source track's end always ships.
    fn emit(&mut self, obs: Observation, out: &mut Output, trace: Option<&mut SampleTrace>) {
        if obs.state == Some(ot_core::TrackState::Dropped) {
            self.throttle.remove(&obs.source_track_key);
            self.details_changed.remove(&obs.source_track_key);
        } else if let Some(t) = &self.spec.throttle {
            let key = obs.source_track_key.clone();
            let last = self.throttle.get(&key);
            if !t.due(last, &obs, self.details_changed.contains(&key)) {
                self.counts.throttled += 1;
                if let Some(f) = trace {
                    f.dropped("throttle", t.why_not(last, &obs));
                }
                return;
            }
            self.details_changed.remove(&key);
            self.throttle.insert(
                key,
                LastWrite {
                    at: obs.observed_at,
                    lat: obs.position.latitude,
                    lon: obs.position.longitude,
                },
            );
        }
        self.counts.emitted += 1;
        if let Some(f) = trace {
            f.obs("throttle", &obs);
            f.emitted.push(obs.clone());
        }
        out.observations.push(obs);
    }

    fn process_record(
        &mut self,
        record: &Value,
        received_at: DateTime<Utc>,
        registry: &dyn RegistryLookup,
        out: &mut Output,
        mut trace: Option<&mut SampleTrace>,
    ) {
        self.counts.records += 1;
        let mapped = match self.spec.mapping.apply(record) {
            MapOutcome::Rejected(reason) => {
                if let Some(f) = trace {
                    f.dropped("reject", format!("rejected: {reason}"));
                }
                *self.counts.rejected.entry(reason).or_default() += 1;
                return;
            }
            MapOutcome::Unmatched => {
                if let Some(f) = trace {
                    f.item("reject", || record.clone());
                    f.dropped(
                        "map",
                        "no mapping rule matched it (or the rule's key was empty)",
                    );
                }
                self.counts.unmatched += 1;
                return;
            }
            MapOutcome::Mapped(m) => m,
        };
        // Where a mapped report is finalised into an observation.
        let join = self.has_join();
        let made = if join { "join" } else { "map" };
        if let Some(f) = trace.as_deref_mut() {
            f.item("reject", || record.clone());
            for m in &mapped {
                if m.kind == RuleKind::Static {
                    f.item("map", || mapped_json(m));
                    f.note(
                        "join",
                        format!(
                            "kept as the static identity of {}, to fill its later reports",
                            m.key
                        ),
                    );
                } else if join {
                    // Before the join fills it in.
                    let before = mapping::finalize(
                        &self.source_id,
                        self.spec.mapping.schema_version,
                        m,
                        received_at,
                    );
                    match before {
                        Ok(o) => f.obs("map", &o),
                        Err(_) => f.item("map", || mapped_json(m)),
                    }
                }
            }
        }
        // Static rules first, so a record carrying both (AIS msg 19) joins
        // its own identity fields.
        for m in mapped.iter().filter(|m| m.kind == RuleKind::Static) {
            self.counts.statics += 1;
            let entry = self
                .statics
                .entry(m.key.clone())
                .or_insert_with(|| StaticEntry {
                    fields: Value::Object(Default::default()),
                    identifiers: Vec::new(),
                    updated_at: received_at,
                });
            let before = entry.fields.clone();
            let ids_before = entry.identifiers.len();
            merge_over(&mut entry.fields, &m.fields);
            for id in &m.identifiers {
                if !entry.identifiers.contains(id) {
                    entry.identifiers.push(id.clone());
                }
            }
            entry.updated_at = received_at;
            if entry.fields != before || entry.identifiers.len() != ids_before {
                self.details_changed.insert(m.key.clone());
            }
            out.statics.push((m.key.clone(), entry.clone()));
        }
        for m in mapped.into_iter().filter(|m| m.kind.reports()) {
            let mut m = m;
            if let Some(s) = self.statics.get(&m.key) {
                mapping::fill_missing(&mut m.fields, &s.fields);
                for id in &s.identifiers {
                    if !m.identifiers.contains(id) {
                        m.identifiers.push(id.clone());
                    }
                }
            }
            if let Some(schema) = &self.schema {
                let ext = m
                    .fields
                    .as_object_mut()
                    .expect("mapped fields are an object")
                    .entry("ext")
                    .or_insert_with(|| Value::Object(Default::default()));
                if let Some(ext) = ext.as_object_mut()
                    && let Err(e) = schema.apply(ext)
                {
                    self.counts.invalid += 1;
                    if let Some(f) = trace.as_deref_mut() {
                        f.dropped(made, format!("invalid: {e}"));
                    }
                    out.last_error = Some(e);
                    continue;
                }
            }
            // Stages below work on the observation's JSON form.
            let mut obs = match mapping::finalize(
                &self.source_id,
                self.spec.mapping.schema_version,
                &m,
                received_at,
            )
            .and_then(|o| {
                serde_json::to_value(&o).map_err(|e| mapping::FinalizeError::Invalid(e.to_string()))
            }) {
                Ok(v) => v,
                Err(e) => {
                    self.counts.invalid += 1;
                    if let Some(f) = trace.as_deref_mut() {
                        f.dropped(made, format!("invalid: {e}"));
                    }
                    out.last_error = Some(e.to_string());
                    continue;
                }
            };
            if let Some(f) = trace.as_deref_mut() {
                f.item(made, || obs.clone());
            }
            let stage = self
                .spec
                .registry
                .as_ref()
                .unwrap_or(&self.default_registry);
            let matched = stage.run(&mut obs, registry);
            // Values mapped to the entity go no further than here.
            if let Some(ext) = obs.get_mut("ext").and_then(Value::as_object_mut) {
                ext.remove(crate::registry::ENTITY_EXT);
            }
            if let Some(f) = trace.as_deref_mut() {
                f.item("registry", || obs.clone());
            }
            let grade = matched.as_ref().map_or("none", |m| m.grade.as_str());
            // An affiliation the entity set stands: the entity is the authority
            // over the country lists.
            let entity_affiliation = matched
                .as_ref()
                .is_some_and(|m| m.set.iter().any(|f| f == "classification.affiliation"));
            if let Some(m) = matched {
                let fresh: Vec<(String, Value)> = m
                    .updates
                    .into_iter()
                    .filter(|(field, v)| {
                        self.entity_sent.get(&(m.entity_id.clone(), field.clone())) != Some(v)
                    })
                    .collect();
                if !fresh.is_empty() {
                    if self.entity_sent.len() > 100_000 {
                        self.entity_sent.clear();
                    }
                    for (field, v) in &fresh {
                        self.entity_sent
                            .insert((m.entity_id.clone(), field.clone()), v.clone());
                    }
                    out.entity_updates.push((m.entity_id, fresh));
                }
            }
            *self.counts.grades.entry(grade).or_default() += 1;
            if let Some(stage) = &self.spec.affiliation {
                if !entity_affiliation {
                    stage.run(&mut obs);
                }
                if let Some(f) = trace.as_deref_mut() {
                    f.item("affiliation", || obs.clone());
                    if entity_affiliation {
                        f.note("affiliation", "kept the entity's affiliation");
                    }
                }
            }
            if let Some(filter) = &self.spec.filter {
                if let Some(why) = filter.verdict(&obs) {
                    self.counts.filtered += 1;
                    if let Some(f) = trace.as_deref_mut() {
                        f.dropped("filter", why);
                    }
                    continue;
                }
                if let Some(f) = trace.as_deref_mut() {
                    f.item("filter", || obs.clone());
                }
            }
            let obs: Observation = match serde_json::from_value(obs) {
                Ok(o) => o,
                Err(e) => {
                    self.counts.invalid += 1;
                    if let Some(f) = trace.as_deref_mut() {
                        f.dropped(self.last_json_stage(), format!("invalid: {e}"));
                    }
                    out.last_error = Some(e.to_string());
                    continue;
                }
            };
            if let Some(t) = self.tracker.as_mut() {
                if m.kind == RuleKind::Observation {
                    self.counts.plots += 1;
                    if self.keep_plots {
                        out.plots.push(obs.clone());
                    }
                    if let Some(f) = trace.as_deref_mut() {
                        f.plots.push(obs.clone());
                    }
                    t.push(obs, received_at);
                    continue;
                }
                // A report with its own identity passes the tracker by.
                if let Some(f) = trace.as_deref_mut() {
                    f.obs("tracker", &obs);
                }
            }
            self.emit(obs, out, trace.as_deref_mut());
        }
    }

    /// The last stage working on an observation's JSON form.
    fn last_json_stage(&self) -> &'static str {
        if self.spec.filter.is_some() {
            "filter"
        } else if self.spec.affiliation.is_some() {
            "affiliation"
        } else {
            "registry"
        }
    }
}

/// A mapping rule's output as JSON, for a trace.
fn mapped_json(m: &mapping::Mapped) -> Value {
    serde_json::json!({
        "rule": m.rule,
        "kind": m.kind,
        "key": m.key,
        "identifiers": m.identifiers,
        "fields": m.fields,
    })
}

/// Overwrite `base` with every non-null value in `newer` (static updates).
fn merge_over(base: &mut Value, newer: &Value) {
    let (Value::Object(b), Value::Object(n)) = (base, newer) else {
        return;
    };
    for (k, v) in n {
        match (b.get_mut(k), v) {
            (_, Value::Null) => {}
            (Some(existing @ Value::Object(_)), Value::Object(_)) => merge_over(existing, v),
            _ => {
                b.insert(k.clone(), v.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::RegistryEntry;
    use serde_json::json;
    use std::collections::BTreeMap;

    type Reg = BTreeMap<(String, String), RegistryEntry>;

    fn spec() -> PipelineSpec {
        serde_json::from_value(json!({
            "codec": { "type": "json" },
            "mapping": { "rules": [
                { "name": "static", "kind": "static", "when": { "path": "t", "eq": "static" },
                  "key": "id", "fields": { "name": "name", "platform.flag": "flag",
                                           "entity.destination": "dest" } },
                { "name": "pos", "when": { "path": "t", "eq": "pos" }, "key": "id",
                  "identifiers": [ { "scheme": "mmsi", "value": "id" } ],
                  "fields": { "position.latitude": "lat", "position.longitude": "lon",
                              "observed_at": { "path": "ts", "transforms": ["time"] },
                              "classification.cot_type": { "const": "a-u-S" } } }
            ] },
            "registry": { "markers": ["NAVY"],
                          "apply": { "classification.cot_type": "cot", "platform.flag": "flag" } },
            "affiliation": { "country": "platform.flag", "friendly": ["US"], "hostile": ["XX"] },
            "filter": { "keep_if": { "any": [
                { "path": "ext.registry.entity_id", "exists": true },
                { "path": "classification.cot_type", "matches": "^a-.-S-C" } ] } },
            "throttle": { "min_interval_secs": 30, "heartbeat_secs": 600, "min_move_m": 50 }
        }))
        .unwrap()
    }

    fn reg() -> Reg {
        let e = RegistryEntry {
            entity_id: "e1".into(),
            name: Some("USS EXAMPLE".into()),
            expected_name: Some("EXAMPLE".into()),
            fields: serde_json::from_value(
                json!({"cot_type": "a-u-S-C", "flag": "US", "hull_code": "DDG-1"}),
            )
            .unwrap(),
        };
        [(("mmsi".into(), "1".into()), e)].into()
    }

    fn feed(p: &mut Pipeline, reg: &Reg, rec: Value) -> Output {
        p.process(&Frame::new(rec.to_string().into_bytes()), reg)
    }

    fn pos(id: u32, ts: &str, lat: f64) -> Value {
        json!({"t": "pos", "id": id, "lat": lat, "lon": -117.0, "ts": ts})
    }

    #[test]
    fn an_affiliation_the_entity_set_outranks_the_country_lists() {
        let spec: PipelineSpec = serde_json::from_value(json!({
            "codec": { "type": "json" },
            "mapping": { "rules": [ { "name": "pos", "key": "id",
                "identifiers": [ { "scheme": "icao", "value": "id" } ],
                "fields": { "position.latitude": "lat", "position.longitude": "lon",
                            "callsign": "cs", "platform.flag": "flag" } } ] },
            "registry": { "apply": { "classification.affiliation": "affiliation" } },
            "affiliation": { "country": "platform.flag", "friendly": ["US"], "otherwise": "unknown" }
        }))
        .unwrap();
        let entity = |fields: Value| RegistryEntry {
            entity_id: "e9".into(),
            name: Some("LOST56".into()),
            expected_name: None,
            fields: serde_json::from_value(fields).unwrap(),
        };
        let report =
            json!({"id": "080010", "lat": 42.0, "lon": -121.0, "cs": "LOST56", "flag": "TZ"});
        let run = |e: RegistryEntry| {
            let reg: Reg = [(("icao".into(), "080010".into()), e)].into();
            let mut p = Pipeline::new("adsb", spec.clone()).unwrap();
            feed(&mut p, &reg, report.clone()).observations[0]
                .classification
                .affiliation
        };
        // The entity says friend: the country lists (TZ: unknown) leave it.
        assert_eq!(
            run(entity(json!({"affiliation": "friend"}))),
            Some(ot_core::Affiliation::Friend)
        );
        // No affiliation on the entity: the country lists decide.
        assert_eq!(run(entity(json!({}))), Some(ot_core::Affiliation::Unknown));
    }

    #[test]
    fn join_registry_affiliation_filter_and_throttle() {
        let reg = reg();
        let mut p = Pipeline::new("ais", spec()).unwrap();
        let st = feed(
            &mut p,
            &reg,
            json!({"t": "static", "id": 1, "name": "EXAMPLE", "dest": "LONG BEACH"}),
        );
        assert_eq!(st.statics.len(), 1);

        let out = feed(&mut p, &reg, pos(1, "2026-09-24T12:00:00Z", 32.0));
        assert_eq!(out.observations.len(), 1);
        // The static data's destination updates the entity, once, and is
        // not carried on the observation.
        assert_eq!(
            out.entity_updates,
            [(
                "e1".to_string(),
                vec![("destination".to_string(), json!("LONG BEACH"))]
            )]
        );
        let o = &out.observations[0];
        assert!(!o.ext.contains_key("entity"));
        assert_eq!(o.name.as_deref(), Some("EXAMPLE"), "static join");
        assert_eq!(
            o.classification.cot_type.as_deref(),
            Some("a-u-S-C"),
            "registry applied"
        );
        assert_eq!(
            o.classification.affiliation,
            Some(ot_core::Affiliation::Friend)
        );
        assert_eq!(o.ext["registry"]["grade"], "exact");

        // 10 s later, moved ~111 m: inside the 30 s floor, throttled.
        let out = feed(&mut p, &reg, pos(1, "2026-09-24T12:00:10Z", 32.001));
        assert!(out.observations.is_empty());
        // 60 s later, moved: due.
        assert_eq!(
            feed(&mut p, &reg, pos(1, "2026-09-24T12:01:00Z", 32.002))
                .observations
                .len(),
            1
        );
        // 120 s later, stationary: not due until the heartbeat.
        assert!(
            feed(&mut p, &reg, pos(1, "2026-09-24T12:03:00Z", 32.002))
                .observations
                .is_empty()
        );
        assert_eq!(
            feed(&mut p, &reg, pos(1, "2026-09-24T12:11:00Z", 32.002))
                .observations
                .len(),
            1
        );

        // Not in the registry and not military: filtered out.
        assert!(
            feed(&mut p, &reg, pos(2, "2026-09-24T12:00:00Z", 33.0))
                .observations
                .is_empty()
        );

        let c = p.take_counts();
        assert_eq!(c.emitted, 3);
        assert_eq!(c.throttled, 2);
        assert_eq!(c.filtered, 1);
        assert_eq!(c.statics, 1);
        assert_eq!(c.grades.get("exact"), Some(&5));
        assert_eq!(c.grades.get("none"), Some(&1));
    }

    #[test]
    fn changed_static_details_ship_before_the_heartbeat() {
        let reg = reg();
        let mut p = Pipeline::new("ais", spec()).unwrap();
        feed(
            &mut p,
            &reg,
            json!({"t": "static", "id": 1, "name": "EXAMPLE"}),
        );
        assert_eq!(
            feed(&mut p, &reg, pos(1, "2026-09-24T12:00:00Z", 32.0))
                .observations
                .len(),
            1
        );
        // The same static report again changes nothing: stationary, not due.
        feed(
            &mut p,
            &reg,
            json!({"t": "static", "id": 1, "name": "EXAMPLE"}),
        );
        assert!(
            feed(&mut p, &reg, pos(1, "2026-09-24T12:02:00Z", 32.0))
                .observations
                .is_empty()
        );
        // A new destination: the next report ships, once.
        feed(
            &mut p,
            &reg,
            json!({"t": "static", "id": 1, "name": "EXAMPLE", "dest": "OAKLAND"}),
        );
        assert_eq!(
            feed(&mut p, &reg, pos(1, "2026-09-24T12:02:20Z", 32.0))
                .observations
                .len(),
            1
        );
        assert!(
            feed(&mut p, &reg, pos(1, "2026-09-24T12:03:20Z", 32.0))
                .observations
                .is_empty()
        );
        // Another change inside the 30 s floor waits for it.
        feed(
            &mut p,
            &reg,
            json!({"t": "static", "id": 1, "name": "EXAMPLE", "dest": "SEATTLE"}),
        );
        assert_eq!(
            feed(&mut p, &reg, pos(1, "2026-09-24T12:03:40Z", 32.0))
                .observations
                .len(),
            1
        );
        feed(
            &mut p,
            &reg,
            json!({"t": "static", "id": 1, "name": "EXAMPLE", "dest": "OAKLAND"}),
        );
        assert!(
            feed(&mut p, &reg, pos(1, "2026-09-24T12:03:50Z", 32.0))
                .observations
                .is_empty()
        );
        assert_eq!(
            feed(&mut p, &reg, pos(1, "2026-09-24T12:04:10Z", 32.0))
                .observations
                .len(),
            1
        );
    }

    #[test]
    fn decode_errors_and_invalid_records_are_counted() {
        let reg = reg();
        let mut p = Pipeline::new("ais", spec()).unwrap();
        let out = p.process(&Frame::new(&b"not json"[..]), &reg);
        assert!(out.last_error.is_some());
        feed(&mut p, &reg, pos(1, "2026-09-24T12:00:00Z", 95.0));
        feed(&mut p, &reg, json!({"t": "other"}));
        let c = p.take_counts();
        assert_eq!((c.decode_errors, c.invalid, c.unmatched), (1, 1, 1));
    }

    #[test]
    fn a_tracker_stage_turns_plots_into_anonymous_tracks() {
        let spec: PipelineSpec = serde_json::from_value(json!({
            "codec": { "type": "json", "records": "plots" },
            "mapping": { "rules": [
                { "name": "plot", "key": "n",
                  "identifiers": [ { "scheme": "mmsi", "value": "mmsi" } ],
                  "fields": { "position.latitude": "lat", "position.longitude": "lon", "name": "label",
                              "observed_at": { "path": "t", "transforms": ["time"] } } }
            ] },
            "tracker": { "algorithm": "gnn", "confirm_hits": 2, "domain": "surface" }
        }))
        .unwrap();
        let mut p = Pipeline::new("radar", spec).unwrap();
        let mut out = Vec::new();
        for s in 0..4 {
            // Two boats 1 km apart, each scan one frame.
            let t = format!("2026-09-25T10:00:0{s}Z");
            let body = json!({"plots": [
                {"n": 1, "lat": 63.44, "lon": 10.40 + s as f64 * 1e-4, "t": t, "mmsi": "366", "label": "KNOWN"},
                {"n": 2, "lat": 63.45, "lon": 10.40, "t": t}
            ]});
            let frame = Frame::new(serde_json::to_vec(&body).unwrap());
            out.extend(p.process(&frame, &Reg::new()).observations);
        }
        // Confirmed on the second scan: two tracks, three reports each.
        let keys: BTreeSet<&str> = out.iter().map(|o| o.source_track_key.as_str()).collect();
        assert_eq!(keys.len(), 2);
        assert!(
            keys.iter()
                .all(|k| k.starts_with('T') && (k.ends_with("-1") || k.ends_with("-2"))),
            "{keys:?}"
        );
        assert_eq!(out.len(), 6);
        for o in &out {
            assert!(o.identifiers.is_empty() && o.name.is_none(), "{o:?}");
            assert_eq!(o.classification.cot_type_or_derived(), "a-u-S");
        }
        assert_eq!(p.counts.plots, 8);
        assert_eq!(p.counts.emitted, 6);
        // Nothing waits: frames are scans.
        assert!(p.flush(Utc::now(), true).observations.is_empty());
    }

    fn stage<'a>(f: &'a crate::trace::SampleTrace, id: &str) -> &'a crate::trace::StageTrace {
        f.stages.iter().find(|s| s.id == id).expect(id)
    }

    fn radar(scans: &str) -> PipelineSpec {
        serde_json::from_value(json!({
            "codec": { "type": "json", "records": "plots" },
            "mapping": {
                "reject": [ { "reason": "no_position", "when": { "path": "lat", "exists": false } } ],
                "rules": [
                { "name": "plot", "key": "n",
                  "fields": { "position.latitude": "lat", "position.longitude": "lon",
                              "observed_at": { "path": "t", "transforms": ["time"] } } }
            ] },
            "tracker": { "algorithm": "gnn", "confirm_hits": 2, "domain": "surface",
                         "scans": scans, "scan_hold_secs": 60 }
        }))
        .unwrap()
    }

    #[test]
    fn a_trace_follows_each_record_through_its_stages() {
        let reg = reg();
        let mut spec = spec();
        spec.throttle = None;
        spec.filter = Some(FilterSpec {
            keep_if: None,
            drop_if: Some(
                serde_json::from_value(json!({"path": "position.latitude", "gt": 33})).unwrap(),
            ),
        });
        let mut p = Pipeline::new("ais", spec).unwrap();
        let mut trace = Trace::new(5);
        let frame = |rec: Value| Frame::new(rec.to_string().into_bytes());
        for rec in [
            json!({"t": "static", "id": 1, "name": "EXAMPLE"}),
            json!({"t": "pos", "id": 1, "lat": 32.0, "lon": -117.0, "ts": "2026-09-24T12:00:00Z"}),
            json!({"t": "pos", "id": 3, "lat": 34.0, "lon": -117.0, "ts": "2026-09-24T12:00:00Z"}),
        ] {
            p.process_traced(&frame(rec), &reg, &mut trace);
        }
        p.process_traced(&Frame::new(&b"not json"[..]), &reg, &mut trace);
        // One record per frame: a sample per frame, each frame kept once.
        let f = &trace.samples;
        assert_eq!((f.len(), trace.frames.len()), (4, 4));
        assert_eq!(f.iter().map(|s| s.frame).collect::<Vec<_>>(), [0, 1, 2, 3]);
        let ids: Vec<&str> = f[0].stages.iter().map(|s| s.id).collect();
        assert_eq!(
            ids,
            ["decode", "map", "join", "registry", "affiliation", "filter"],
            "only the stages the pipeline has"
        );
        assert_eq!(trace.frames[0].format, "json");
        // A static report is kept by the join.
        assert_eq!(stage(&f[0], "map").items[0]["kind"], "static");
        let join = stage(&f[0], "join");
        assert!(
            join.dropped.is_empty(),
            "keeping a static identity is not a drop"
        );
        assert!(join.notes[0].starts_with("kept as the static identity of 1"));
        // A position report: joined, graded, published.
        assert!(
            stage(&f[1], "map").items[0].get("name").is_none(),
            "before the join"
        );
        assert_eq!(stage(&f[1], "join").items[0]["name"], "EXAMPLE");
        assert_eq!(
            stage(&f[1], "registry").items[0]["ext"]["registry"]["grade"],
            "exact"
        );
        assert_eq!(stage(&f[1], "filter").items.len(), 1);
        assert_eq!(f[1].emitted.len(), 1);
        // The filter drops the third, with the reason.
        assert_eq!(stage(&f[2], "registry").items.len(), 1);
        assert!(stage(&f[2], "filter").items.is_empty());
        assert_eq!(stage(&f[2], "filter").dropped, ["drop_if matched"]);
        assert!(f[2].emitted.is_empty());
        // A frame that does not decode is a sample of its own.
        assert_eq!(trace.frames[3].format, "text");
        assert!(stage(&f[3], "decode").dropped[0].starts_with("decode error"));
    }

    #[test]
    fn each_record_of_a_frame_is_a_sample_of_its_own() {
        let body = json!({"plots": [
            {"n": 1, "lat": 63.44, "lon": 10.40, "t": "2026-09-25T10:00:00Z"},
            {"n": 2, "lon": 10.40, "t": "2026-09-25T10:00:00Z"},
            {"n": 3, "lat": 63.45, "lon": 10.40, "t": "2026-09-25T10:00:00Z"}
        ]});
        let frame = Frame::new(body.to_string().into_bytes());
        let mut p = Pipeline::new("radar", radar("frame")).unwrap();
        let mut trace = Trace::new(5);
        p.process_traced(&frame, &Reg::new(), &mut trace);
        // Three samples from one frame, which is kept once.
        let s = &trace.samples;
        assert_eq!((s.len(), trace.frames.len()), (3, 1));
        assert!(s.iter().all(|s| s.frame == 0));
        assert_eq!(trace.frames[0].content["plots"][2]["n"], 3);
        for (i, sample) in s.iter().enumerate() {
            let decoded = &stage(sample, "decode").items;
            assert_eq!(decoded.len(), 1);
            assert_eq!(decoded[0]["n"], i + 1, "decode order");
        }
        assert_eq!(stage(&s[0], "map").items.len(), 1);
        assert!(stage(&s[1], "reject").items.is_empty());
        assert_eq!(stage(&s[1], "reject").dropped, ["rejected: no_position"]);
        assert!(stage(&s[1], "map").items.is_empty());
        assert_eq!(stage(&s[2], "map").items[0]["source_track_key"], "3");
        // First scan: nothing confirmed yet, and nothing waits (frames are scans).
        assert!(!stage(&s[0], "tracker").held);
        p.flush_traced(Utc::now(), true, &mut trace);
        let s = &trace.samples;
        assert!(stage(&s[0], "tracker").dropped[0].starts_with("no track report"));
        assert!(
            stage(&s[1], "tracker").dropped.is_empty(),
            "no plot of its own"
        );

        // Fewer samples than records: the rest are processed, not recorded.
        let mut p = Pipeline::new("radar", radar("frame")).unwrap();
        let mut trace = Trace::new(2);
        p.process_traced(&frame, &Reg::new(), &mut trace);
        p.process_traced(&frame, &Reg::new(), &mut trace);
        assert_eq!((trace.samples.len(), trace.frames.len()), (2, 1));
        assert_eq!(p.counts.records, 6);
    }

    #[test]
    fn a_tracker_holding_plots_for_their_scan_is_shown() {
        let mut p = Pipeline::new("radar", radar("time")).unwrap();
        let mut trace = Trace::new(5);
        for s in 0..2 {
            let body = json!({"plots": [
                {"n": 1, "lat": 63.44, "lon": 10.40 + s as f64 * 1e-4, "t": format!("2026-09-25T10:00:0{s}Z")}
            ]});
            p.process_traced(
                &Frame::new(body.to_string().into_bytes()),
                &Reg::new(),
                &mut trace,
            );
        }
        // Each frame's scan waits for more plots of its time.
        assert_eq!(trace.samples.len(), 2);
        assert!(trace.samples.iter().all(|f| stage(f, "tracker").held));
        let out = p.flush_traced(Utc::now(), true, &mut trace);
        assert_eq!(out.observations.len(), 1, "confirmed on the second scan");
        let (a, b) = (&trace.samples[0], &trace.samples[1]);
        assert!(stage(a, "tracker").items.is_empty());
        assert!(stage(a, "tracker").dropped[0].starts_with("no track report"));
        assert_eq!(
            stage(b, "tracker").items.len(),
            1,
            "the report goes to its plot's sample"
        );
        assert_eq!(b.emitted.len(), 1);
        assert_eq!(
            stage(b, "tracker").items[0]["source_track_key"],
            json!(out.observations[0].source_track_key)
        );
    }

    #[test]
    fn tracing_is_off_unless_asked_and_changes_nothing() {
        let reg = reg();
        let frames: Vec<Frame> = (0..4)
            .map(|i| {
                Frame::new(
                    pos(
                        1,
                        &format!("2026-09-24T12:0{i}:00Z"),
                        32.0 + i as f64 * 0.01,
                    )
                    .to_string()
                    .into_bytes(),
                )
            })
            .collect();
        let mut plain = Pipeline::new("ais", spec()).unwrap();
        let mut traced = Pipeline::new("ais", spec()).unwrap();
        let mut trace = Trace::new(2);
        let mut none = Trace::new(0);
        for f in &frames {
            let a = plain.process(f, &reg).observations;
            let b = traced.process_traced(f, &reg, &mut trace).observations;
            assert_eq!(a, b);
        }
        for f in &frames {
            traced.process_traced(f, &reg, &mut none);
        }
        assert!(none.samples.is_empty(), "a zero trace records nothing");
        assert_eq!(trace.samples.len(), 2, "only the first records");
    }
}
