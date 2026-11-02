//! The per-source processing pipeline, free of I/O: a frame is decoded into
//! records, and each record follows the source's flow of steps
//! ([`crate::flow`]) to its Output (system tracks) and NATS topics.
//!
//! A Tracker step turns a detection feed's plots into source tracks (see
//! [`crate::tracker`]).
//!
//! The worker feeds frames in and ships observations out; everything here is
//! deterministic given its inputs, so it can be replayed in tests and probes.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::time::Duration;

use chrono::{DateTime, Utc};
use ot_core::{Identifier, Observation};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::codec::Codec;
use crate::expr::{Condition, ValueSpec, as_string};
use crate::frame::Frame;
use crate::mapping::{self, MapOutcome, MappingSpec, RuleKind};
use crate::registry::{RegistryLookup, RegistryStage};
use crate::trace::{SampleTrace, Trace};

pub use crate::flow::PipelineSpec;
use crate::flow::{INPUT, Step};

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PipelineError {
    #[error(transparent)]
    Mapping(#[from] mapping::MappingError),
    #[error("{0}")]
    Tracker(String),
    #[error("{0}")]
    Codec(String),
    /// The flow's shape cannot run (a loop, a step fed the wrong data).
    #[error("{0}")]
    Flow(String),
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
    /// Observations sent to the Output, and items published on NATS topics.
    pub emitted: u64,
    pub published: u64,
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
            ("published", self.published),
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
    /// Plots given to a Tracker step, when [`Pipeline::keep_plots`] is on.
    pub plots: Vec<Observation>,
    /// What reached each NATS topic step: its subject and the item as JSON.
    pub published: Vec<(String, Value)>,
}

pub struct Pipeline {
    source_id: String,
    spec: PipelineSpec,
    codec: Codec,
    /// Each step's state, in `spec.nodes` order.
    steps: Vec<Run>,
    /// Each step's children, by index.
    children: Vec<Vec<usize>>,
    /// Whether an Entity links step comes before each step.
    linked: Vec<bool>,
    input: usize,
    /// The Map whose static cache is stored under the plain track key.
    primary_map: Option<usize>,
    /// The steps a trace records, input first, in the order they are met.
    order: Vec<String>,
    /// Extension field types for the mappings' schema version.
    schema: Option<crate::schema::ExtensionSchema>,
    pub counts: Counts,
    /// What each step took and passed on, in `spec.nodes` order.
    step_counts: Vec<StepCount>,
    /// Run before the Output when no Entity links step comes before it.
    default_registry: RegistryStage,
    /// The last value sent to each (entity, field), so a feed repeating a
    /// value writes it once, not until the registry snapshot catches up.
    entity_sent: HashMap<(String, String), Value>,
    keep_plots: bool,
}

/// A step's settings and state.
enum Run {
    Input,
    Reject(Vec<mapping::RejectRule>),
    Map {
        spec: MappingSpec,
        ttl_secs: u64,
        statics: HashMap<String, StaticEntry>,
        /// Tracks whose static details changed since their last report.
        changed: HashSet<String>,
    },
    Filter(FilterSpec),
    Registry(RegistryStage),
    Affiliation(AffiliationStage),
    Tracker(Stage),
    Throttle {
        spec: ThrottleSpec,
        last: HashMap<String, LastWrite>,
        /// Tracks whose details changed since they last passed.
        pending: HashSet<String>,
    },
    Output,
    Nats(String),
}

/// What one step took in and passed on: frames and records for the input,
/// records or observations for the rest, and what an output delivered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct StepCount {
    #[serde(rename = "in")]
    pub taken: u64,
    #[serde(rename = "out")]
    pub passed: u64,
}

/// What passes between steps: a record, or an observation's JSON form.
#[derive(Clone)]
struct Item {
    value: Value,
    /// The mapping rule's kind, for observations.
    kind: RuleKind,
    /// Its track's static details changed since its last report.
    changed: bool,
    /// The entity set its affiliation (the country lists then keep out).
    entity_affiliation: bool,
    received_at: DateTime<Utc>,
    /// The traced sample it belongs to.
    sample: Option<usize>,
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

/// Separates a Map's id from a track key in a stored static cache key; the
/// primary Map's keys are plain.
const STATIC_KEY_SEP: char = '\u{1f}';

impl Pipeline {
    pub fn new(source_id: impl Into<String>, spec: PipelineSpec) -> Result<Self, PipelineError> {
        if let Some(p) = spec.shape_problems().into_iter().find(|p| p.invalid) {
            return Err(PipelineError::Flow(match p.node {
                Some(n) => format!("step {n}: {}", p.message),
                None => p.message,
            }));
        }
        let index: HashMap<&str, usize> = spec
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.id.as_str(), i))
            .collect();
        let mut steps = Vec::with_capacity(spec.nodes.len());
        for n in &spec.nodes {
            steps.push(match &n.step {
                Step::Input => Run::Input,
                Step::Reject(r) => Run::Reject(r.clone()),
                Step::Map(m) => {
                    m.mapping.validate()?;
                    Run::Map {
                        spec: m.mapping.clone(),
                        ttl_secs: m.static_join.ttl_secs,
                        statics: HashMap::new(),
                        changed: HashSet::new(),
                    }
                }
                Step::Filter(f) => Run::Filter(f.clone()),
                Step::Registry(r) => Run::Registry(r.clone()),
                Step::Affiliation(a) => Run::Affiliation(a.clone()),
                Step::Tracker(t) => {
                    Run::Tracker(Stage::new(t.clone()).map_err(PipelineError::Tracker)?)
                }
                Step::Throttle(t) => Run::Throttle {
                    spec: t.clone(),
                    last: HashMap::new(),
                    pending: HashSet::new(),
                },
                Step::Output => Run::Output,
                Step::Nats(n) => Run::Nats(n.subject.clone()),
            });
        }
        let mut children = vec![Vec::new(); spec.nodes.len()];
        for e in &spec.edges {
            if let (Some(&from), Some(&to)) =
                (index.get(e.source.as_str()), index.get(e.target.as_str()))
            {
                children[from].push(to);
            }
        }
        let input = index[INPUT];
        // Breadth first from the input: the order steps are met in, and
        // whether an Entity links step comes before each.
        let mut order = Vec::new();
        let mut linked = vec![false; spec.nodes.len()];
        let mut queue = std::collections::VecDeque::from([input]);
        let mut seen = HashSet::new();
        while let Some(at) = queue.pop_front() {
            if !seen.insert(at) {
                continue;
            }
            order.push(spec.nodes[at].id.clone());
            let here = linked[at] || matches!(spec.nodes[at].step, Step::Registry(_));
            for &c in &children[at] {
                linked[c] = here;
                queue.push_back(c);
            }
        }
        let primary_map = index
            .get("map")
            .copied()
            .filter(|&i| matches!(steps[i], Run::Map { .. }))
            .or_else(|| steps.iter().position(|s| matches!(s, Run::Map { .. })));
        let spec_len = spec.nodes.len();
        Ok(Self {
            source_id: source_id.into(),
            codec: Codec::new(spec.codec.clone())
                .map_err(|e| PipelineError::Codec(e.to_string()))?,
            spec,
            steps,
            children,
            linked,
            input,
            primary_map,
            order,
            schema: None,
            counts: Counts::default(),
            step_counts: vec![StepCount::default(); spec_len],
            default_registry: RegistryStage::default(),
            entity_sent: HashMap::new(),
            keep_plots: false,
        })
    }

    /// Also return the plots the Tracker steps are given (benchmarks score
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

    /// Whether it has a Tracker step (the worker then runs it on a timer).
    pub fn has_tracker(&self) -> bool {
        self.steps.iter().any(|s| matches!(s, Run::Tracker(_)))
    }

    /// The key a Map's static cache entry is stored under.
    fn static_key(&self, map: usize, key: &str) -> String {
        if Some(map) == self.primary_map {
            key.to_owned()
        } else {
            format!("{}{STATIC_KEY_SEP}{key}", self.spec.nodes[map].id)
        }
    }

    /// Seed a Map's static cache (e.g. from Redis after a restart), by the
    /// key [`Output::statics`] gave it.
    pub fn load_static(&mut self, key: String, entry: StaticEntry) {
        let (map, key) = match key.split_once(STATIC_KEY_SEP) {
            Some((id, k)) => match self.spec.nodes.iter().position(|n| n.id == id) {
                Some(i) => (Some(i), k.to_owned()),
                None => return,
            },
            None => (self.primary_map, key),
        };
        if let Some(Run::Map { statics, .. }) = map.and_then(|i| self.steps.get_mut(i)) {
            statics.insert(key, entry);
        }
    }

    /// The longest any Map keeps its static details.
    pub fn static_ttl(&self) -> Duration {
        let secs = self
            .steps
            .iter()
            .filter_map(|s| match s {
                Run::Map { ttl_secs, .. } => Some(*ttl_secs),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        Duration::from_secs(secs)
    }

    /// What each step took in and passed on so far, by step id (for a dry
    /// run: the counts on the editor's steps and connections).
    pub fn step_counts(&self) -> std::collections::BTreeMap<String, StepCount> {
        self.spec
            .nodes
            .iter()
            .zip(&self.step_counts)
            .map(|(n, c)| (n.id.clone(), *c))
            .collect()
    }

    /// Take and reset the counters.
    pub fn take_counts(&mut self) -> Counts {
        std::mem::take(&mut self.counts)
    }

    /// Forget throttle and static state older than they are kept.
    pub fn prune(&mut self, now: DateTime<Utc>) {
        for s in &mut self.steps {
            match s {
                Run::Map {
                    ttl_secs, statics, ..
                } => {
                    let ttl = chrono::Duration::from_std(Duration::from_secs(*ttl_secs))
                        .unwrap_or(chrono::Duration::MAX);
                    statics.retain(|_, e| now - e.updated_at < ttl);
                }
                Run::Throttle { spec, last, .. } => {
                    let keep =
                        chrono::Duration::milliseconds((spec.heartbeat_secs * 1000.0) as i64 * 2);
                    last.retain(|_, w| now - w.at < keep);
                }
                _ => {}
            }
        }
    }

    pub fn process(&mut self, frame: &Frame, registry: &dyn RegistryLookup) -> Output {
        self.run_frame(frame, registry, None)
    }

    /// [`Self::process`], recording in `trace` what each step made of the
    /// frame's records while the trace has room (see [`crate::trace`]).
    pub fn process_traced(
        &mut self,
        frame: &Frame,
        registry: &dyn RegistryLookup,
        trace: &mut Trace,
    ) -> Output {
        self.run_frame(frame, registry, Some(trace))
    }

    fn run_frame(
        &mut self,
        frame: &Frame,
        registry: &dyn RegistryLookup,
        mut trace: Option<&mut Trace>,
    ) -> Output {
        let mut out = Output::default();
        self.counts.frames += 1;
        self.step_counts[self.input].taken += 1;
        if let Some(t) = trace.as_deref_mut() {
            t.start_frame();
        }
        let records = match self.codec.decode(frame) {
            Ok(r) => r,
            Err(e) => {
                self.counts.decode_errors += 1;
                // A frame that does not decode is a sample of its own.
                if let Some(t) = trace.as_deref_mut()
                    && let Some(at) = t.begin(frame, &self.order)
                {
                    t.samples[at].dropped(INPUT, format!("decode error: {e}"));
                }
                out.last_error = Some(e.to_string());
                return out;
            }
        };
        // A tracker timed by the sensor follows what the codec has learnt.
        if let Some(h) = self.codec.hints() {
            for s in &mut self.steps {
                if let Run::Tracker(Stage::Builtin(t)) = s {
                    t.set_revisit(h.revisit_secs, h.revisit_source);
                }
            }
        }
        for record in records {
            self.counts.records += 1;
            let sample = trace
                .as_deref_mut()
                .and_then(|t| t.begin(frame, &self.order));
            if let (Some(t), Some(at)) = (trace.as_deref_mut(), sample) {
                t.samples[at].item(INPUT, || record.clone());
            }
            let item = Item {
                value: record,
                kind: RuleKind::Observation,
                changed: false,
                entity_affiliation: false,
                received_at: frame.received_at,
                sample,
            };
            self.pass_on(self.input, item, registry, &mut out, &mut trace);
        }
        self.run_trackers(frame.received_at, false, registry, &mut out, &mut trace);
        // Plots still waiting for the rest of their scan.
        if let Some(tr) = trace {
            for (i, s) in self.steps.iter().enumerate() {
                let Run::Tracker(Stage::Builtin(t)) = s else {
                    continue;
                };
                let id = &self.spec.nodes[i].id;
                for f in &mut tr.samples {
                    let mine: Vec<&Observation> = f
                        .plots
                        .iter()
                        .filter(|(n, _)| n == id)
                        .map(|(_, o)| o)
                        .collect();
                    if !mine.is_empty()
                        && t.waiting().any(|w| mine.contains(&w))
                        && let Some(st) = f.stage(id)
                    {
                        st.held = true;
                    }
                }
            }
        }
        out
    }

    /// Give `item` to each of `at`'s children.
    fn pass_on(
        &mut self,
        at: usize,
        item: Item,
        registry: &dyn RegistryLookup,
        out: &mut Output,
        trace: &mut Option<&mut Trace>,
    ) {
        self.step_counts[at].passed += 1;
        let n = self.children[at].len();
        for i in 0..n {
            let c = self.children[at][i];
            if i + 1 < n {
                self.step(c, item.clone(), registry, out, trace);
            } else {
                self.step(c, item, registry, out, trace);
                return;
            }
        }
    }

    /// One step's work on one item.
    fn step(
        &mut self,
        at: usize,
        mut item: Item,
        registry: &dyn RegistryLookup,
        out: &mut Output,
        trace: &mut Option<&mut Trace>,
    ) {
        self.step_counts[at].taken += 1;
        let id = self.spec.nodes[at].id.clone();
        let mut sample = match (trace.as_deref_mut(), item.sample) {
            (Some(t), Some(s)) => t.samples.get_mut(s),
            _ => None,
        };
        match &mut self.steps[at] {
            Run::Input => {}
            Run::Reject(rules) => {
                if let Some(r) = rules.iter().find(|r| r.when.eval(&item.value)) {
                    if let Some(f) = sample {
                        f.dropped(&id, format!("rejected: {}", r.reason));
                    }
                    *self.counts.rejected.entry(r.reason.clone()).or_default() += 1;
                    return;
                }
                if let Some(f) = sample {
                    f.item(&id, || item.value.clone());
                }
            }
            Run::Map { .. } => return self.map(at, &id, item, registry, out, trace),
            Run::Filter(filter) => {
                if let Some(why) = filter.verdict(&item.value) {
                    self.counts.filtered += 1;
                    if let Some(f) = sample {
                        f.dropped(&id, why);
                    }
                    return;
                }
                if let Some(f) = sample {
                    f.item(&id, || item.value.clone());
                }
            }
            Run::Registry(stage) => {
                let stage = stage.clone();
                self.link(&stage, &mut item, registry, out);
                if let Some(f) = sample {
                    f.item(&id, || item.value.clone());
                }
            }
            Run::Affiliation(stage) => {
                if !item.entity_affiliation {
                    stage.run(&mut item.value);
                }
                if let Some(f) = sample {
                    f.item(&id, || item.value.clone());
                    if item.entity_affiliation {
                        f.note(&id, "kept the entity's affiliation");
                    }
                }
            }
            Run::Tracker(_) => {
                let Some(obs) = self.typed(&id, &item, out, sample.as_deref_mut()) else {
                    return;
                };
                if item.kind == RuleKind::Observation {
                    self.counts.plots += 1;
                    if self.keep_plots {
                        out.plots.push(obs.clone());
                    }
                    if let Some(f) = sample {
                        f.plots.push((id.clone(), obs.clone()));
                    }
                    if let Run::Tracker(t) = &mut self.steps[at] {
                        t.push(obs, item.received_at);
                    }
                    return;
                }
                // A report with its own identity passes the tracker by.
                if let Some(f) = sample {
                    f.obs(&id, &obs);
                }
            }
            Run::Throttle { .. } => {
                let Some(obs) = self.typed(&id, &item, out, sample.as_deref_mut()) else {
                    return;
                };
                let Run::Throttle {
                    spec,
                    last,
                    pending,
                } = &mut self.steps[at]
                else {
                    unreachable!()
                };
                let key = obs.source_track_key.clone();
                if obs.state == Some(ot_core::TrackState::Dropped) {
                    // A source track's end always passes.
                    last.remove(&key);
                    pending.remove(&key);
                } else {
                    if item.changed {
                        pending.insert(key.clone());
                    }
                    let prev = last.get(&key);
                    if !spec.due(prev, &obs, pending.contains(&key)) {
                        self.counts.throttled += 1;
                        if let Some(f) = sample {
                            f.dropped(&id, spec.why_not(prev, &obs));
                        }
                        return;
                    }
                    pending.remove(&key);
                    last.insert(
                        key,
                        LastWrite {
                            at: obs.observed_at,
                            lat: obs.position.latitude,
                            lon: obs.position.longitude,
                        },
                    );
                }
                if let Some(f) = sample {
                    f.obs(&id, &obs);
                }
            }
            Run::Output => {
                // Tracks reach correlation with their entity looked up.
                if !self.linked[at] {
                    let stage = self.default_registry.clone();
                    self.link(&stage, &mut item, registry, out);
                }
                let Some(obs) = self.typed(&id, &item, out, sample.as_deref_mut()) else {
                    return;
                };
                self.counts.emitted += 1;
                self.step_counts[at].passed += 1;
                if let Some(f) = sample {
                    f.obs(&id, &obs);
                    f.emitted.push(obs.clone());
                }
                out.observations.push(obs);
                return;
            }
            Run::Nats(subject) => {
                self.counts.published += 1;
                self.step_counts[at].passed += 1;
                if let Some(f) = sample {
                    f.item(&id, || item.value.clone());
                }
                out.published.push((subject.clone(), item.value));
                return;
            }
        }
        self.pass_on(at, item, registry, out, trace);
    }

    /// An item as an observation, or why not (counted, and noted on the step).
    fn typed(
        &mut self,
        id: &str,
        item: &Item,
        out: &mut Output,
        sample: Option<&mut SampleTrace>,
    ) -> Option<Observation> {
        match serde_json::from_value::<Observation>(item.value.clone()) {
            Ok(o) => Some(o),
            Err(e) => {
                self.counts.invalid += 1;
                if let Some(f) = sample {
                    f.dropped(id, format!("invalid: {e}"));
                }
                out.last_error = Some(e.to_string());
                None
            }
        }
    }

    /// Look up the item's entity with these Entity links: grades it, passes
    /// fields between entity and track, and queues the entity's updates.
    fn link(
        &mut self,
        stage: &RegistryStage,
        item: &mut Item,
        registry: &dyn RegistryLookup,
        out: &mut Output,
    ) {
        let matched = stage.run(&mut item.value, registry);
        // Values mapped to the entity go no further than here.
        if let Some(ext) = item.value.get_mut("ext").and_then(Value::as_object_mut) {
            ext.remove(crate::registry::ENTITY_EXT);
        }
        let grade = matched.as_ref().map_or("none", |m| m.grade.as_str());
        // An affiliation the entity set stands: the entity is the authority
        // over the country lists.
        item.entity_affiliation = matched
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
    }

    /// A Map step: the record's rules, static details kept and joined, each
    /// report finalised into an observation.
    fn map(
        &mut self,
        at: usize,
        id: &str,
        item: Item,
        registry: &dyn RegistryLookup,
        out: &mut Output,
        trace: &mut Option<&mut Trace>,
    ) {
        let Run::Map { spec, .. } = &self.steps[at] else {
            return;
        };
        let schema_version = spec.schema_version;
        let outcome = spec.apply(&item.value);
        let mut sample = match (trace.as_deref_mut(), item.sample) {
            (Some(t), Some(s)) => t.samples.get_mut(s),
            _ => None,
        };
        let mapped = match outcome {
            MapOutcome::Rejected(reason) => {
                if let Some(f) = sample {
                    f.dropped(id, format!("rejected: {reason}"));
                }
                *self.counts.rejected.entry(reason).or_default() += 1;
                return;
            }
            MapOutcome::Unmatched => {
                if let Some(f) = sample {
                    f.dropped(
                        id,
                        "no mapping rule matched it (or the rule's key was empty)",
                    );
                }
                self.counts.unmatched += 1;
                return;
            }
            MapOutcome::Mapped(m) => m,
        };
        // Static rules first, so a record carrying both (AIS msg 19) joins
        // its own identity fields.
        for m in mapped.iter().filter(|m| m.kind == RuleKind::Static) {
            self.counts.statics += 1;
            if let Some(f) = sample.as_deref_mut() {
                f.item(id, || mapped_json(m));
                f.note(
                    id,
                    format!(
                        "kept the static identity of {}, to fill its later reports",
                        m.key
                    ),
                );
            }
            let stored = self.static_key(at, &m.key);
            let Run::Map {
                statics, changed, ..
            } = &mut self.steps[at]
            else {
                return;
            };
            let entry = statics.entry(m.key.clone()).or_insert_with(|| StaticEntry {
                fields: Value::Object(Default::default()),
                identifiers: Vec::new(),
                updated_at: item.received_at,
            });
            let before = entry.fields.clone();
            let ids_before = entry.identifiers.len();
            merge_over(&mut entry.fields, &m.fields);
            for i in &m.identifiers {
                if !entry.identifiers.contains(i) {
                    entry.identifiers.push(i.clone());
                }
            }
            entry.updated_at = item.received_at;
            if entry.fields != before || entry.identifiers.len() != ids_before {
                changed.insert(m.key.clone());
            }
            out.statics.push((stored, entry.clone()));
        }
        for mut m in mapped.into_iter().filter(|m| m.kind.reports()) {
            let Run::Map {
                statics, changed, ..
            } = &mut self.steps[at]
            else {
                return;
            };
            if let Some(s) = statics.get(&m.key) {
                mapping::fill_missing(&mut m.fields, &s.fields);
                for i in &s.identifiers {
                    if !m.identifiers.contains(i) {
                        m.identifiers.push(i.clone());
                    }
                }
            }
            let details_changed = changed.remove(&m.key);
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
                    if let Some(f) = sample.as_deref_mut() {
                        f.dropped(id, format!("invalid: {e}"));
                    }
                    out.last_error = Some(e);
                    continue;
                }
            }
            // Later steps work on the observation's JSON form.
            let obs = match mapping::finalize(&self.source_id, schema_version, &m, item.received_at)
                .and_then(|o| {
                    serde_json::to_value(&o)
                        .map_err(|e| mapping::FinalizeError::Invalid(e.to_string()))
                }) {
                Ok(v) => v,
                Err(e) => {
                    self.counts.invalid += 1;
                    if let Some(f) = sample.as_deref_mut() {
                        f.dropped(id, format!("invalid: {e}"));
                    }
                    out.last_error = Some(e.to_string());
                    continue;
                }
            };
            if let Some(f) = sample.as_deref_mut() {
                f.item(id, || obs.clone());
            }
            let next = Item {
                value: obs,
                kind: m.kind,
                changed: details_changed,
                entity_affiliation: false,
                received_at: item.received_at,
                sample: item.sample,
            };
            self.pass_on(at, next, registry, out, trace);
            sample = match (trace.as_deref_mut(), item.sample) {
                (Some(t), Some(s)) => t.samples.get_mut(s),
                _ => None,
            };
        }
    }

    /// The windows a Tracker step with `auto_timing` is using, once it knows
    /// the sensor's revisit period (the first such step's).
    pub fn tracker_timing(&self) -> Option<&crate::tracker::Timing> {
        self.steps.iter().find_map(|s| match s {
            Run::Tracker(Stage::Builtin(t)) => t.timing(),
            _ => None,
        })
    }

    /// Run the Tracker steps on scans still waiting (plots grouped by time
    /// wait `scan_hold_secs` for more; `force` runs them regardless). The
    /// worker calls this on a timer; a dry run calls it with `force` at the
    /// end.
    pub fn flush(
        &mut self,
        now: DateTime<Utc>,
        force: bool,
        registry: &dyn RegistryLookup,
    ) -> Output {
        let mut out = Output::default();
        self.run_trackers(now, force, registry, &mut out, &mut None);
        out
    }

    /// [`Self::flush`], adding the reports to the traced samples whose plots
    /// they came from. With `force` (the end of a dry run), a sample whose
    /// plots gave no report is marked so.
    pub fn flush_traced(
        &mut self,
        now: DateTime<Utc>,
        force: bool,
        registry: &dyn RegistryLookup,
        trace: &mut Trace,
    ) -> Output {
        let mut out = Output::default();
        let mut t = Some(&mut *trace);
        self.run_trackers(now, force, registry, &mut out, &mut t);
        if force {
            for (i, s) in self.steps.iter().enumerate() {
                let Run::Tracker(stage) = s else { continue };
                let why = match stage {
                    Stage::Plugin { .. } => {
                        "a tracker plugin's reports are not traced back to the records behind them"
                    }
                    Stage::Builtin(_) => {
                        "no track report: the tracker has not confirmed a track from these plots (or merged them into a nearby plot, or they came too late for their scan)"
                    }
                };
                let id = &self.spec.nodes[i].id;
                for f in &mut trace.samples {
                    let plots = f.plots.iter().any(|(n, _)| n == id);
                    if let Some(st) = f.stage(id)
                        && plots
                        && st.items.is_empty()
                        && st.dropped.is_empty()
                    {
                        st.dropped.push(why.to_owned());
                    }
                }
            }
        }
        out
    }

    /// Run each Tracker step, and pass its reports on. When tracing, each
    /// report of the built-in tracker joins the sample its detection came
    /// from.
    fn run_trackers(
        &mut self,
        now: DateTime<Utc>,
        force: bool,
        registry: &dyn RegistryLookup,
        out: &mut Output,
        trace: &mut Option<&mut Trace>,
    ) {
        for at in 0..self.steps.len() {
            let tracing = trace.is_some();
            // The detection behind each report, only when tracing.
            let mut dets: Vec<Observation> = Vec::new();
            let reports: Vec<Observation> = match &mut self.steps[at] {
                Run::Tracker(Stage::Builtin(t)) => {
                    let force = force || t.spec().scans == crate::tracker::ScanGrouping::Frame;
                    let reports = t.run(now, force);
                    self.counts.late_plots += std::mem::take(&mut t.late);
                    if tracing {
                        dets = reports.iter().map(|(_, d)| d.clone()).collect();
                    }
                    reports.into_iter().map(|(obs, _)| obs).collect()
                }
                Run::Tracker(Stage::Plugin { tracker, version }) => match tracker.run(now, force) {
                    Ok(mut tracks) => {
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
                _ => continue,
            };
            let id = self.spec.nodes[at].id.clone();
            for (i, obs) in reports.into_iter().enumerate() {
                let mut sample = None;
                if let Some(t) = trace.as_deref_mut() {
                    sample = dets.get(i).and_then(|d| {
                        t.samples
                            .iter()
                            .position(|f| f.plots.iter().any(|(n, p)| n == &id && p == d))
                    });
                    if let Some(f) = sample.and_then(|s| t.samples.get_mut(s)) {
                        f.obs(&id, &obs);
                    }
                }
                let item = Item {
                    value: serde_json::to_value(&obs).unwrap_or(Value::Null),
                    kind: RuleKind::Track,
                    changed: false,
                    entity_affiliation: false,
                    received_at: now,
                    sample,
                };
                self.pass_on(at, item, registry, out, trace);
            }
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
        serde_json::from_value(spec_json()).unwrap()
    }

    fn spec_json() -> Value {
        json!({
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
        })
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

    /// A flow from steps and connections, after the input.
    fn flow(nodes: Value, edges: Value) -> PipelineSpec {
        serde_json::from_value(json!({"codec": {"type": "json"}, "nodes": nodes, "edges": edges}))
            .unwrap()
    }

    fn pos_map() -> Value {
        json!({"mapping": {"rules": [{"name": "pos", "key": "id",
            "identifiers": [{"scheme": "mmsi", "value": "id"}],
            "fields": {"position.latitude": "lat", "position.longitude": "lon", "name": "name",
                       "observed_at": {"path": "ts", "transforms": ["time"]}}}]}})
    }

    #[test]
    fn a_flow_branches_to_its_output_and_nats_topics() {
        // input → map → { filter → filter → output, nats "all" }, map → filter → nats "north"
        let spec = flow(
            json!([
                {"id": "m", "type": "map", "config": pos_map()},
                {"id": "named", "type": "filter", "config": {"keep_if": {"path": "name", "exists": true}}},
                {"id": "south", "type": "filter", "config": {"keep_if": {"path": "position.latitude", "lt": 33}}},
                {"id": "out", "type": "output"},
                {"id": "all", "type": "nats", "config": {"subject": "feed.all"}},
                {"id": "north", "type": "filter", "config": {"keep_if": {"path": "position.latitude", "gte": 33}}},
                {"id": "pub", "type": "nats", "config": {"subject": "feed.north"}}
            ]),
            json!([
                {"source": "input", "target": "m"},
                {"source": "m", "target": "named"}, {"source": "named", "target": "south"},
                {"source": "south", "target": "out"},
                {"source": "m", "target": "all"},
                {"source": "m", "target": "north"}, {"source": "north", "target": "pub"}
            ]),
        );
        let mut p = Pipeline::new("ais", spec).unwrap();
        let reg = reg();
        let rec = |id: u32, lat: f64, name: Option<&str>| {
            let mut r = json!({"id": id, "lat": lat, "lon": -117.0, "ts": "2026-09-24T12:00:00Z"});
            if let Some(n) = name {
                r["name"] = json!(n);
            }
            r
        };
        let a = feed(&mut p, &reg, rec(1, 32.0, Some("ALPHA")));
        assert_eq!(a.observations.len(), 1, "named and south: to correlation");
        assert_eq!(
            a.published
                .iter()
                .map(|(s, _)| s.as_str())
                .collect::<Vec<_>>(),
            ["feed.all"]
        );
        // The Output looks the entity up, with no Entity links step before it.
        assert_eq!(a.observations[0].ext["registry"]["entity_id"], "e1");
        assert!(
            a.published[0].1["ext"].get("registry").is_none(),
            "a topic gets the mapped data"
        );
        let b = feed(&mut p, &reg, rec(2, 34.0, None));
        assert!(b.observations.is_empty(), "unnamed, and north");
        assert_eq!(
            b.published
                .iter()
                .map(|(s, _)| s.as_str())
                .collect::<Vec<_>>(),
            ["feed.all", "feed.north"]
        );
        assert_eq!(b.published[1].1["position"]["latitude"], 34.0);
        assert_eq!(
            (p.counts.emitted, p.counts.published, p.counts.filtered),
            (1, 3, 2)
        );
    }

    #[test]
    fn each_step_counts_what_it_took_and_passed_on() {
        let spec = flow(
            json!([
                {"id": "m", "type": "map", "config": pos_map()},
                {"id": "f", "type": "filter", "config": {"drop_if": {"path": "position.latitude", "gt": 33}}},
                {"id": "out", "type": "output"},
                {"id": "raw", "type": "nats", "config": {"subject": "raw"}}
            ]),
            json!([
                {"source": "input", "target": "m"}, {"source": "m", "target": "f"},
                {"source": "f", "target": "out"}, {"source": "m", "target": "raw"}
            ]),
        );
        let mut p = Pipeline::new("ais", spec).unwrap();
        let body = json!([{"id": 1, "lat": 32.0, "lon": 1.0, "ts": "2026-09-24T12:00:00Z"},
                          {"id": 2, "lat": 40.0, "lon": 1.0, "ts": "2026-09-24T12:00:00Z"},
                          {"nothing": true}]);
        p.process(&Frame::new(body.to_string().into_bytes()), &Reg::new());
        let c = p.step_counts();
        let at = |id: &str| (c[id].taken, c[id].passed);
        assert_eq!(at("input"), (1, 3), "one frame, three records");
        assert_eq!(at("m"), (3, 2), "one record matched no rule");
        assert_eq!(at("f"), (2, 1));
        assert_eq!(at("out"), (1, 1));
        assert_eq!(at("raw"), (2, 2));
    }

    #[test]
    fn a_trace_follows_each_branch() {
        let spec = flow(
            json!([
                {"id": "m", "type": "map", "config": pos_map()},
                {"id": "f", "type": "filter", "config": {"drop_if": {"path": "position.latitude", "gt": 33}}},
                {"id": "out", "type": "output"},
                {"id": "raw", "type": "nats", "config": {"subject": "raw"}}
            ]),
            json!([
                {"source": "input", "target": "m"}, {"source": "m", "target": "f"},
                {"source": "f", "target": "out"}, {"source": "m", "target": "raw"}
            ]),
        );
        let mut p = Pipeline::new("ais", spec).unwrap();
        let mut trace = Trace::new(5);
        let body = json!([{"id": 1, "lat": 32.0, "lon": 1.0, "ts": "2026-09-24T12:00:00Z"},
                          {"id": 2, "lat": 40.0, "lon": 1.0, "ts": "2026-09-24T12:00:00Z"}]);
        p.process_traced(
            &Frame::new(body.to_string().into_bytes()),
            &Reg::new(),
            &mut trace,
        );
        let ids: Vec<&str> = trace.samples[0]
            .stages
            .iter()
            .map(|s| s.id.as_str())
            .collect();
        assert_eq!(ids, ["input", "m", "f", "raw", "out"]);
        let (a, b) = (&trace.samples[0], &trace.samples[1]);
        assert_eq!(
            (stage(a, "out").items.len(), stage(a, "raw").items.len()),
            (1, 1)
        );
        assert_eq!(stage(b, "f").dropped, ["drop_if matched"]);
        assert!(stage(b, "out").items.is_empty());
        assert_eq!(
            stage(b, "raw").items.len(),
            1,
            "the other branch still has it"
        );
    }

    #[test]
    fn a_loop_or_a_step_fed_the_wrong_data_cannot_run() {
        let spec = flow(
            json!([{"id": "f", "type": "filter", "config": {}}, {"id": "out", "type": "output"}]),
            json!([{"source": "input", "target": "f"}, {"source": "f", "target": "out"}]),
        );
        assert!(
            matches!(Pipeline::new("x", spec), Err(PipelineError::Flow(m)) if m.contains("put a Map before it"))
        );
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
        assert!(
            p.flush(Utc::now(), true, &Reg::new())
                .observations
                .is_empty()
        );
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
        let mut v = spec_json();
        v.as_object_mut().unwrap().remove("throttle");
        v["filter"] = json!({"drop_if": {"path": "position.latitude", "gt": 33}});
        let spec: PipelineSpec = serde_json::from_value(v).unwrap();
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
        let ids: Vec<&str> = f[0].stages.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "input",
                "map",
                "registry",
                "affiliation",
                "filter",
                "output"
            ],
            "the flow's steps, in the order they are met"
        );
        assert_eq!(trace.frames[0].format, "json");
        // A static report is kept by the map, to join later reports.
        let map = stage(&f[0], "map");
        assert_eq!(map.items[0]["kind"], "static");
        assert!(
            map.dropped.is_empty(),
            "keeping a static identity is not a drop"
        );
        assert!(map.notes[0].starts_with("kept the static identity of 1"));
        // A position report: joined, graded, published.
        assert_eq!(stage(&f[1], "map").items[0]["name"], "EXAMPLE");
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
        assert!(stage(&f[3], "input").dropped[0].starts_with("decode error"));
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
            let decoded = &stage(sample, "input").items;
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
        p.flush_traced(Utc::now(), true, &Reg::new(), &mut trace);
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
        let out = p.flush_traced(Utc::now(), true, &Reg::new(), &mut trace);
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
