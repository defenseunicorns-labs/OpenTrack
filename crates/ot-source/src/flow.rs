//! A source's processing as a flow of steps (kineticlogic-io/OpenTrack#104):
//! one input (the transport and its codec), then steps the user connects,
//! each taking one input and feeding any number of others, ending at the
//! source's one Output (system tracks, to correlation) and any NATS topics
//! (its mapped data, published as JSON without correlation).
//!
//! Records flow from the input through Reject steps to a Map, which turns
//! them into observations; Filter, Entity links, Affiliation, Tracker and
//! Throttle steps work on observations; Output and NATS topics take them.
//!
//! A spec in the older fixed form (`mapping`, `filter`, `tracker`… under
//! `pipeline`) still reads: it becomes the same steps in a line.

use std::collections::{BTreeSet, HashMap, HashSet};

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::codec::CodecConfig;
use crate::mapping::{MappingSpec, RejectRule};
use crate::pipeline::{AffiliationStage, FilterSpec, StaticJoinSpec, ThrottleSpec};
use crate::registry::RegistryStage;
use crate::tracker::TrackerSpec;

/// The input step's id: the transport and codec, where every flow starts.
pub const INPUT: &str = "input";

/// The most steps a flow may have.
pub const MAX_NODES: usize = 200;

/// Everything between the transport and the source's outputs.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PipelineSpec {
    /// How the input's frames become records.
    pub codec: CodecConfig,
    pub nodes: Vec<FlowNode>,
    pub edges: Vec<FlowEdge>,
}

/// One step of a flow.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowNode {
    /// 1-64 of A-Z, a-z, 0-9, `-` and `_`; the input is `input`.
    pub id: String,
    #[serde(flatten)]
    pub step: Step,
    /// Where the editor draws it; nothing runs on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<Position>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Position {
    pub x: f64,
    pub y: f64,
}

/// A connection: `target` takes what `source` passes on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowEdge {
    pub source: String,
    pub target: String,
}

/// What a step does, with its settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "config", rename_all = "snake_case")]
pub enum Step {
    /// The transport and codec (set on the source); only `input`.
    Input,
    /// Drop records by condition, each rule with a reason.
    Reject(Vec<RejectRule>),
    /// Turn records into observations.
    Map(MapStep),
    /// Keep or drop observations.
    Filter(FilterSpec),
    /// Find each track's entity by its identifiers (Entity links).
    Registry(RegistryStage),
    Affiliation(AffiliationStage),
    /// Form tracks from detections.
    Tracker(TrackerSpec),
    Throttle(ThrottleSpec),
    /// The source's system tracks, to correlation.
    Output,
    /// Publish what reaches it, as JSON, on a NATS subject, without
    /// correlation.
    Nats(NatsStep),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MapStep {
    pub mapping: MappingSpec,
    /// How long identity fields from static rules are kept, to fill the
    /// same track's later reports.
    #[serde(default)]
    pub static_join: StaticJoinSpec,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NatsStep {
    pub subject: String,
}

/// What passes along a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Data {
    /// Decoded records, before a Map.
    Records,
    /// Observations, from a Map on.
    Observations,
}

impl Step {
    /// What it takes; `None`: nothing (the input).
    pub fn takes(&self) -> Option<Data> {
        match self {
            Step::Input => None,
            Step::Reject(_) | Step::Map(_) => Some(Data::Records),
            _ => Some(Data::Observations),
        }
    }

    /// What it passes on; `None`: nothing (an output).
    pub fn gives(&self) -> Option<Data> {
        match self {
            Step::Input | Step::Reject(_) => Some(Data::Records),
            Step::Output | Step::Nats(_) => None,
            _ => Some(Data::Observations),
        }
    }

    /// The word for it in messages.
    pub fn name(&self) -> &'static str {
        match self {
            Step::Input => "input",
            Step::Reject(_) => "reject",
            Step::Map(_) => "map",
            Step::Filter(_) => "filter",
            Step::Registry(_) => "entity links",
            Step::Affiliation(_) => "affiliation",
            Step::Tracker(_) => "tracker",
            Step::Throttle(_) => "throttle",
            Step::Output => "output",
            Step::Nats(_) => "NATS topic",
        }
    }
}

/// A problem with a flow's shape, on the step it concerns (`None`: the
/// flow as a whole).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowProblem {
    /// It cannot be saved (rather than only not run yet).
    pub invalid: bool,
    pub node: Option<String>,
    pub message: String,
}

impl PipelineSpec {
    /// A step by id.
    pub fn node(&self, id: &str) -> Option<&FlowNode> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// Every Map's mapping, in step order.
    pub fn mappings(&self) -> impl Iterator<Item = &MappingSpec> {
        self.nodes.iter().filter_map(|n| match &n.step {
            Step::Map(m) => Some(&m.mapping),
            _ => None,
        })
    }

    /// The extension schema version the flow's mappings target (they share
    /// one; see [`Self::shape_problems`]); 1 without a Map.
    pub fn schema_version(&self) -> u32 {
        self.mappings().next().map_or(1, |m| m.schema_version)
    }

    /// The Output step, if the flow has one.
    pub fn output(&self) -> Option<&FlowNode> {
        self.nodes.iter().find(|n| matches!(n.step, Step::Output))
    }

    /// The steps from the input to `id`, input first (empty if `id` is not
    /// reached from the input).
    pub fn path_to(&self, id: &str) -> Vec<&FlowNode> {
        let parent: HashMap<&str, &str> = self
            .edges
            .iter()
            .map(|e| (e.target.as_str(), e.source.as_str()))
            .collect();
        let mut out = Vec::new();
        let mut at = id;
        let mut seen = HashSet::new();
        loop {
            if !seen.insert(at) {
                return Vec::new();
            }
            match self.node(at) {
                Some(n) => out.push(n),
                None if at == INPUT => {}
                None => return Vec::new(),
            }
            if at == INPUT {
                break;
            }
            match parent.get(at) {
                Some(p) => at = p,
                None => return Vec::new(),
            }
        }
        out.reverse();
        out
    }

    /// The Entity links settings the source's system tracks pass through:
    /// the last such step before the Output, else the defaults.
    pub fn output_registry(&self) -> RegistryStage {
        self.output()
            .map(|o| self.path_to(&o.id))
            .unwrap_or_default()
            .into_iter()
            .rev()
            .find_map(|n| match &n.step {
                Step::Registry(r) => Some(r.clone()),
                _ => None,
            })
            .unwrap_or_default()
    }

    /// Whether the source's system tracks come through a Tracker.
    pub fn output_tracked(&self) -> bool {
        self.output().is_some_and(|o| {
            self.path_to(&o.id)
                .iter()
                .any(|n| matches!(n.step, Step::Tracker(_)))
        })
    }

    /// The steps' children, by id.
    pub fn children(&self) -> HashMap<&str, Vec<&str>> {
        let mut out: HashMap<&str, Vec<&str>> = HashMap::new();
        for e in &self.edges {
            out.entry(e.source.as_str())
                .or_default()
                .push(e.target.as_str());
        }
        out
    }

    /// What is wrong with the flow's shape: ids, connections, what each
    /// step takes and gives, and whether it leads to an output. Problems
    /// with a step's own settings are the step's (see `SourceSpec::problems`).
    pub fn shape_problems(&self) -> Vec<FlowProblem> {
        let mut out = Vec::new();
        let invalid = |node: Option<&str>, m: String| FlowProblem {
            invalid: true,
            node: node.map(str::to_owned),
            message: m,
        };
        let incomplete = |node: Option<&str>, m: String| FlowProblem {
            invalid: false,
            node: node.map(str::to_owned),
            message: m,
        };
        if self.nodes.len() > MAX_NODES {
            out.push(invalid(
                None,
                format!("a flow has at most {MAX_NODES} steps"),
            ));
        }
        let mut ids = HashSet::new();
        for n in &self.nodes {
            let ok = !n.id.is_empty()
                && n.id.len() <= 64
                && n.id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
            if !ok {
                out.push(invalid(
                    Some(&n.id),
                    format!("step id {:?}: 1-64 of letters, digits, - and _", n.id),
                ));
            }
            if !ids.insert(n.id.as_str()) {
                out.push(invalid(
                    Some(&n.id),
                    format!("two steps are called {:?}", n.id),
                ));
            }
            if (n.id == INPUT) != matches!(n.step, Step::Input) {
                out.push(invalid(
                    Some(&n.id),
                    "only the step called \"input\" is the input, and it is the input".into(),
                ));
            }
        }
        let outputs = self
            .nodes
            .iter()
            .filter(|n| matches!(n.step, Step::Output))
            .count();
        if outputs > 1 {
            out.push(invalid(None, "a source has one Output".into()));
        }
        let gives = |id: &str| match self.node(id) {
            Some(n) => n.step.gives(),
            None if id == INPUT => Some(Data::Records),
            None => None,
        };
        let mut incoming: HashMap<&str, usize> = HashMap::new();
        let mut pairs = HashSet::new();
        for e in &self.edges {
            let (from, to) = (self.node(&e.source), self.node(&e.target));
            if from.is_none() && e.source != INPUT {
                out.push(invalid(
                    None,
                    format!("a connection from {:?}, which is not a step", e.source),
                ));
                continue;
            }
            let Some(to) = to else {
                out.push(invalid(
                    None,
                    format!("a connection to {:?}, which is not a step", e.target),
                ));
                continue;
            };
            if !pairs.insert((e.source.as_str(), e.target.as_str())) {
                out.push(invalid(
                    Some(&to.id),
                    "it is connected twice from one step".into(),
                ));
                continue;
            }
            *incoming.entry(e.target.as_str()).or_default() += 1;
            match (gives(&e.source), to.step.takes()) {
                (_, None) => out.push(invalid(
                    Some(&to.id),
                    "nothing connects into the input".into(),
                )),
                (None, _) => out.push(invalid(
                    Some(&e.source),
                    format!(
                        "a {} passes nothing on",
                        from.map_or("step", |f| f.step.name())
                    ),
                )),
                (Some(Data::Records), Some(Data::Observations)) => out.push(invalid(
                    Some(&to.id),
                    format!(
                        "a {} takes observations: put a Map before it",
                        to.step.name()
                    ),
                )),
                (Some(Data::Observations), Some(Data::Records)) => out.push(invalid(
                    Some(&to.id),
                    format!("a {} takes records, before a Map", to.step.name()),
                )),
                _ => {}
            }
        }
        for (id, n) in &incoming {
            if *n > 1 {
                out.push(invalid(Some(id), "a step takes one input".into()));
            }
        }
        if !out.is_empty() {
            return out;
        }
        // Every step is reached from the input, and leads to an output.
        let children = self.children();
        let mut reached = HashSet::new();
        let mut stack = vec![INPUT];
        while let Some(at) = stack.pop() {
            if reached.insert(at) {
                stack.extend(children.get(at).into_iter().flatten().copied());
            }
        }
        // A loop is a step with one input that the input never reaches.
        let mut sinks = 0;
        for n in &self.nodes {
            if n.id == INPUT {
                continue;
            }
            if !reached.contains(n.id.as_str()) {
                if incoming.contains_key(n.id.as_str()) {
                    let looped = self.in_loop(&n.id);
                    if looped {
                        out.push(invalid(Some(&n.id), "the flow loops back on itself".into()));
                        continue;
                    }
                }
                out.push(incomplete(Some(&n.id), "this step has no input".into()));
                continue;
            }
            if n.step.gives().is_none() {
                sinks += 1;
            } else if !children.contains_key(n.id.as_str()) {
                out.push(incomplete(Some(&n.id), "this step leads nowhere".into()));
            }
        }
        if out.iter().any(|p| p.invalid) {
            return out;
        }
        if !children.contains_key(INPUT) {
            out.push(incomplete(Some(INPUT), "the input leads nowhere".into()));
        }
        if sinks == 0 {
            out.push(incomplete(
                None,
                "a flow needs an Output or a NATS topic".into(),
            ));
        }
        let versions: BTreeSet<u32> = self.mappings().map(|m| m.schema_version).collect();
        if versions.len() > 1 {
            out.push(incomplete(
                None,
                format!("the Maps target different schema versions ({versions:?}); use one"),
            ));
        }
        out
    }

    /// Whether following inputs back from `id` comes round to it.
    fn in_loop(&self, id: &str) -> bool {
        let parent: HashMap<&str, &str> = self
            .edges
            .iter()
            .map(|e| (e.target.as_str(), e.source.as_str()))
            .collect();
        let mut at = id;
        let mut seen = HashSet::new();
        while let Some(p) = parent.get(at) {
            if *p == id {
                return true;
            }
            if !seen.insert(*p) {
                return false;
            }
            at = p;
        }
        false
    }
}

/// The older fixed form: one of each stage, in a fixed order.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FixedPipeline {
    codec: CodecConfig,
    mapping: MappingSpec,
    #[serde(default)]
    static_join: StaticJoinSpec,
    #[serde(default)]
    registry: Option<RegistryStage>,
    #[serde(default)]
    affiliation: Option<AffiliationStage>,
    #[serde(default)]
    filter: Option<FilterSpec>,
    #[serde(default)]
    tracker: Option<TrackerSpec>,
    #[serde(default)]
    throttle: Option<ThrottleSpec>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GraphPipeline {
    codec: CodecConfig,
    #[serde(default)]
    nodes: Vec<FlowNode>,
    #[serde(default)]
    edges: Vec<FlowEdge>,
}

impl From<FixedPipeline> for PipelineSpec {
    /// The same stages, in a line, with the ids the stages had. The entity
    /// stage, which always ran, is a step of its own.
    fn from(p: FixedPipeline) -> Self {
        let mut mapping = p.mapping;
        let reject = std::mem::take(&mut mapping.reject);
        let mut nodes = vec![node(INPUT, Step::Input)];
        if !reject.is_empty() {
            nodes.push(node("reject", Step::Reject(reject)));
        }
        nodes.push(node(
            "map",
            Step::Map(MapStep {
                mapping,
                static_join: p.static_join,
            }),
        ));
        nodes.push(node(
            "registry",
            Step::Registry(p.registry.unwrap_or_default()),
        ));
        if let Some(a) = p.affiliation {
            nodes.push(node("affiliation", Step::Affiliation(a)));
        }
        if let Some(f) = p.filter {
            nodes.push(node("filter", Step::Filter(f)));
        }
        if let Some(t) = p.tracker {
            nodes.push(node("tracker", Step::Tracker(t)));
        }
        if let Some(t) = p.throttle {
            nodes.push(node("throttle", Step::Throttle(t)));
        }
        nodes.push(node("output", Step::Output));
        let edges = nodes
            .windows(2)
            .map(|w| FlowEdge {
                source: w[0].id.clone(),
                target: w[1].id.clone(),
            })
            .collect();
        PipelineSpec {
            codec: p.codec,
            nodes,
            edges,
        }
    }
}

fn node(id: &str, step: Step) -> FlowNode {
    FlowNode {
        id: id.to_owned(),
        step,
        position: None,
    }
}

impl<'de> Deserialize<'de> for PipelineSpec {
    /// A flow (`nodes` and `edges`), or the older fixed form, read as the
    /// same stages in a line. The input step is added if left out.
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        let graph = v.get("nodes").is_some() || v.get("edges").is_some();
        let mut spec: PipelineSpec = if graph {
            let g: GraphPipeline = serde_json::from_value(v).map_err(serde::de::Error::custom)?;
            PipelineSpec {
                codec: g.codec,
                nodes: g.nodes,
                edges: g.edges,
            }
        } else {
            serde_json::from_value::<FixedPipeline>(v)
                .map_err(serde::de::Error::custom)?
                .into()
        };
        if spec.node(INPUT).is_none() {
            spec.nodes.insert(0, node(INPUT, Step::Input));
        }
        Ok(spec)
    }
}

impl PipelineSpec {
    /// A flow in a line through these steps: input, then each, ending at an
    /// Output. For tests and simple sources.
    pub fn line(codec: CodecConfig, steps: Vec<(&str, Step)>) -> Self {
        let mut nodes = vec![node(INPUT, Step::Input)];
        nodes.extend(steps.into_iter().map(|(id, s)| node(id, s)));
        let edges = nodes
            .windows(2)
            .map(|w| FlowEdge {
                source: w[0].id.clone(),
                target: w[1].id.clone(),
            })
            .collect();
        PipelineSpec {
            codec,
            nodes,
            edges,
        }
    }
}

impl PipelineSpec {
    /// Add a NATS topic step publishing on `subject` what the Output takes
    /// (fed by the Output's step before it), as the raw output of a source
    /// from before flows did. Returns its id; `None` if there is no Output
    /// fed by a step.
    pub fn add_output_topic(&mut self, subject: &str) -> Option<String> {
        let out = self.output()?.id.clone();
        let parent = self.edges.iter().find(|e| e.target == out)?.source.clone();
        let mut id = "raw".to_owned();
        let mut n = 1;
        while self.node(&id).is_some() {
            n += 1;
            id = format!("raw-{n}");
        }
        self.nodes.push(node(
            &id,
            Step::Nats(NatsStep {
                subject: subject.to_owned(),
            }),
        ));
        self.edges.push(FlowEdge {
            source: parent,
            target: id.clone(),
        });
        Some(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn spec(v: Value) -> PipelineSpec {
        serde_json::from_value(v).unwrap()
    }

    fn map() -> Value {
        json!({"rules": [{"name": "r", "key": "id",
            "fields": {"position.latitude": "lat", "position.longitude": "lon"}}]})
    }

    #[test]
    fn the_fixed_form_reads_as_its_stages_in_a_line() {
        let p = spec(json!({
            "codec": {"type": "json"},
            "mapping": {"reject": [{"reason": "x", "when": {"path": "a", "exists": false}}], "rules": map()["rules"]},
            "filter": {"drop_if": {"path": "name", "eq": "N"}},
            "throttle": {"min_interval_secs": 5}
        }));
        let ids: Vec<&str> = p.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "input", "reject", "map", "registry", "filter", "throttle", "output"
            ]
        );
        assert_eq!(p.edges.len(), 6);
        assert!(p.shape_problems().is_empty(), "{:?}", p.shape_problems());
        let Step::Map(m) = &p.node("map").unwrap().step else {
            panic!()
        };
        assert!(
            m.mapping.reject.is_empty(),
            "the rejects moved to their own step"
        );
        // Saved, it reads back the same.
        let again: PipelineSpec =
            serde_json::from_value(serde_json::to_value(&p).unwrap()).unwrap();
        assert_eq!(again, p);
        assert!(p.output_registry() == RegistryStage::default() && !p.output_tracked());
    }

    #[test]
    fn a_flow_branches_to_an_output_and_nats_topics() {
        let p = spec(json!({
            "codec": {"type": "json"},
            "nodes": [
                {"id": "m", "type": "map", "config": {"mapping": map()}},
                {"id": "f1", "type": "filter", "config": {"keep_if": {"path": "name", "exists": true}}},
                {"id": "f2", "type": "filter", "config": {}},
                {"id": "out", "type": "output"},
                {"id": "raw", "type": "nats", "config": {"subject": "raw.feed"}}
            ],
            "edges": [
                {"source": "input", "target": "m"}, {"source": "m", "target": "f1"},
                {"source": "f1", "target": "f2"}, {"source": "f2", "target": "out"},
                {"source": "m", "target": "raw"}
            ]
        }));
        assert_eq!(p.nodes[0].id, "input", "the input is added");
        assert!(p.shape_problems().is_empty(), "{:?}", p.shape_problems());
        let path: Vec<&str> = p.path_to("out").iter().map(|n| n.id.as_str()).collect();
        assert_eq!(path, ["input", "m", "f1", "f2", "out"]);
    }

    #[test]
    fn shape_problems_name_their_step() {
        let base = || {
            json!({"codec": {"type": "json"}, "nodes": [
            {"id": "m", "type": "map", "config": {"mapping": map()}},
            {"id": "f", "type": "filter", "config": {}},
            {"id": "out", "type": "output"}
        ], "edges": [{"source": "input", "target": "m"}, {"source": "m", "target": "f"}, {"source": "f", "target": "out"}]})
        };
        let problems = |v: Value| -> Vec<(bool, Option<String>)> {
            spec(v)
                .shape_problems()
                .into_iter()
                .map(|p| (p.invalid, p.node))
                .collect()
        };
        // A filter before the map.
        let mut v = base();
        v["edges"] = json!([{"source": "input", "target": "f"}, {"source": "f", "target": "m"}, {"source": "m", "target": "out"}]);
        assert_eq!(
            problems(v),
            [(true, Some("f".into())), (true, Some("m".into()))]
        );
        // Two inputs into one step.
        let mut v = base();
        v["edges"]
            .as_array_mut()
            .unwrap()
            .push(json!({"source": "input", "target": "f"}));
        assert!(problems(v).contains(&(true, Some("f".into()))));
        // A loop.
        let mut v = base();
        v["nodes"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id": "g", "type": "filter", "config": {}}));
        v["nodes"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id": "h", "type": "filter", "config": {}}));
        v["edges"].as_array_mut().unwrap().extend([
            json!({"source": "g", "target": "h"}),
            json!({"source": "h", "target": "g"}),
        ]);
        assert!(problems(v).contains(&(true, Some("g".into()))));
        // Two outputs.
        let mut v = base();
        v["nodes"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id": "out2", "type": "output"}));
        assert!(problems(v).contains(&(true, None)));
        // Loose ends are only incomplete: a draft.
        let mut v = base();
        v["nodes"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id": "t", "type": "throttle", "config": {}}));
        v["edges"]
            .as_array_mut()
            .unwrap()
            .push(json!({"source": "f", "target": "t"}));
        assert_eq!(problems(v), [(false, Some("t".into()))]);
        let mut v = base();
        v["edges"] = json!([{"source": "input", "target": "m"}]);
        assert_eq!(
            problems(v),
            [
                (false, Some("m".into())),
                (false, Some("f".into())),
                (false, Some("out".into())),
                (false, None)
            ]
        );
    }
}
