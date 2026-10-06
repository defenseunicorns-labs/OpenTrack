//! Plugins: code OpenTrack runs through a common interface rather than
//! its own. A plugin provides any of three kinds:
//!
//! - codec: a frame's bytes to JSON records (`"codec": {"type": "plugin",
//!   "plugin": "stanag4607", "options": {..}}`), keeping whatever state its
//!   format needs across the frames of one stream
//! - tracker: plots in, tracks out, as a source's tracker stage
//!   (`"tracker": {"algorithm": "plugin", "plugin": ..., "options": {..}}`)
//! - scorer: evidence for the engine's pairing test (correlation settings'
//!   `scorer`)
//!
//! Some are built in ([`stanag4607`]); the rest are loaded while running
//! (WebAssembly components, or external programs over a socket: the
//! `ot-plugin` crate) and [`install`]ed here. Either way a source's
//! configuration names a plugin the same way. The interface is `wit/plugin.wit`.

use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock, RwLock};

use chrono::{DateTime, Utc};
use ot_core::Observation;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::frame::Framing;

pub mod sapient;
pub mod stanag4607;

/// What a plugin provides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Codec,
    Tracker,
    Scorer,
}

/// What a plugin says about itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    /// The name sources and settings refer to it by.
    pub name: String,
    /// Version of its output (records, tracks or scores for the same input);
    /// recorded with what it produces.
    pub version: String,
    #[serde(default)]
    pub description: String,
    pub kinds: Vec<Kind>,
    /// The options it takes, for the UI; [`default_options`] are what a new
    /// source starts with.
    #[serde(default)]
    pub options: Vec<PluginOption>,
    /// How to split a byte stream of its format into frames (codecs: TCP, files).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub framing: Option<Framing>,
}

impl Manifest {
    pub fn validate(&self) -> Result<(), String> {
        let ok = !self.name.is_empty()
            && self.name.len() <= 64
            && self
                .name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
        if !ok {
            return Err(format!(
                "plugin name {:?}: lowercase letters, digits, - and _ (at most 64)",
                self.name
            ));
        }
        if self.version.is_empty() {
            return Err(format!("plugin {}: no version", self.name));
        }
        if self.kinds.is_empty() {
            return Err(format!("plugin {}: provides no kind", self.name));
        }
        Ok(())
    }

    pub fn provides(&self, kind: Kind) -> bool {
        self.kinds.contains(&kind)
    }
}

/// A plugin.
pub trait Plugin: Send + Sync {
    fn manifest(&self) -> &Manifest;

    /// A decoder for one stream, with these options (checked here).
    fn decoder(&self, _options: &Value) -> Result<Box<dyn PluginDecoder>, String> {
        Err(format!("plugin {} is not a codec", self.manifest().name))
    }

    /// A tracker for one source, with these options.
    fn tracker(&self, _options: &Value) -> Result<Box<dyn PluginTracker>, String> {
        Err(format!("plugin {} is not a tracker", self.manifest().name))
    }

    /// A pairing scorer, with these options.
    fn scorer(&self, _options: &Value) -> Result<Box<dyn PluginScorer>, String> {
        Err(format!("plugin {} is not a scorer", self.manifest().name))
    }
}

/// One option a plugin takes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginOption {
    pub name: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub help: String,
    #[serde(flatten)]
    pub kind: OptionKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OptionKind {
    Bool {
        #[serde(default)]
        default: bool,
    },
    Choice {
        choices: Vec<String>,
        default: String,
    },
    /// A number; absent from the options when left empty.
    Number {
        #[serde(default)]
        default: Option<f64>,
        /// Unit shown next to the input, e.g. `m`.
        #[serde(default)]
        unit: String,
        /// Smallest value allowed (the plugin says whether it is exclusive).
        #[serde(default)]
        min: Option<f64>,
    },
    /// Free text; absent from the options when left empty.
    Text {
        #[serde(default)]
        default: Option<String>,
    },
}

/// A plugin's options at their defaults (numbers and text only when they have one).
pub fn default_options(m: &Manifest) -> Value {
    let mut out = Map::new();
    for o in &m.options {
        let v = match &o.kind {
            OptionKind::Bool { default } => Value::from(*default),
            OptionKind::Choice { default, .. } => Value::from(default.clone()),
            OptionKind::Number { default, .. } => match default {
                Some(d) => Value::from(*d),
                None => continue,
            },
            OptionKind::Text { default } => match default {
                Some(d) => Value::from(d.clone()),
                None => continue,
            },
        };
        out.insert(o.name.clone(), v);
    }
    Value::Object(out)
}

/// A plugin as the API lists it.
pub fn describe(p: &dyn Plugin) -> Value {
    let m = p.manifest();
    let mut v = serde_json::to_value(m).unwrap_or(Value::Null);
    v["default_options"] = default_options(m);
    v["builtin"] = Value::from(is_builtin(&m.name));
    v
}

/// Decodes one stream's frames, keeping state between them.
pub trait PluginDecoder: Send {
    fn decode(&mut self, bytes: &[u8], received_at: DateTime<Utc>) -> Result<Vec<Value>, String>;

    /// What the stream has shown about the sensor so far, for stages that
    /// adapt to it (a tracker's `auto_timing`).
    fn hints(&self) -> Option<StreamHints> {
        None
    }
}

/// What a decoder has learnt about its sensor from the stream.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StreamHints {
    /// How often the sensor comes back to the same ground (s).
    pub revisit_secs: f64,
    /// How the decoder knows, e.g. `measured` or `nominal`.
    pub revisit_source: &'static str,
}

impl StreamHints {
    /// Hints a loaded plugin reported as JSON ({"revisit_secs", "revisit_source"}).
    pub fn from_json(v: &Value) -> Option<Self> {
        let revisit_secs = v.get("revisit_secs")?.as_f64().filter(|s| *s > 0.0)?;
        let revisit_source = match v.get("revisit_source").and_then(Value::as_str) {
            Some("measured") => "measured",
            Some("nominal") => "nominal",
            _ => "plugin",
        };
        Some(Self {
            revisit_secs,
            revisit_source,
        })
    }
}

/// A source's tracker stage, provided by a plugin.
pub trait PluginTracker: Send {
    /// A plot, at the time it arrived.
    fn push(&mut self, plot: Observation, received_at: DateTime<Utc>);
    /// Tracks to report now. Called after every frame and about once a
    /// second; `force` at the end of a stream.
    fn run(&mut self, now: DateTime<Utc>, force: bool) -> Result<Vec<Observation>, String>;
}

/// A candidate the engine asks a scorer about.
#[derive(Debug, Clone, Serialize)]
pub struct ScoreCandidate {
    /// The system track as it stands.
    pub view: Value,
    /// OpenTrack's own kinematic comparison of the report with it.
    pub kinematic: Value,
}

/// A scorer's answer for one candidate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Score {
    /// ln of the likelihood ratio: the same object against another nearby.
    pub ln_lr: f64,
    /// Inside the gate: a comparison that counts.
    pub pass: bool,
    #[serde(default)]
    pub evidence: Value,
}

/// Evidence for the engine's pairing test, provided by a plugin.
pub trait PluginScorer: Send {
    /// One score per candidate, in order.
    fn score(
        &mut self,
        report: &Observation,
        candidates: &[ScoreCandidate],
    ) -> Result<Vec<Score>, String>;
}

static BUILTIN: LazyLock<Vec<Arc<dyn Plugin>>> = LazyLock::new(|| {
    vec![
        Arc::new(stanag4607::Stanag4607::new()) as Arc<dyn Plugin>,
        Arc::new(sapient::Sapient::new()) as Arc<dyn Plugin>,
    ]
});

static LOADED: LazyLock<RwLock<BTreeMap<String, Arc<dyn Plugin>>>> =
    LazyLock::new(|| RwLock::new(BTreeMap::new()));

/// Whether a plugin of this name is built in.
pub fn is_builtin(name: &str) -> bool {
    BUILTIN.iter().any(|p| p.manifest().name == name)
}

/// Every plugin this process has: the built-in ones, then those installed.
pub fn plugins() -> Vec<Arc<dyn Plugin>> {
    let mut out: Vec<Arc<dyn Plugin>> = BUILTIN.clone();
    if let Ok(loaded) = LOADED.read() {
        out.extend(loaded.values().cloned());
    }
    out
}

pub fn plugin(name: &str) -> Option<Arc<dyn Plugin>> {
    if let Some(p) = BUILTIN.iter().find(|p| p.manifest().name == name) {
        return Some(p.clone());
    }
    LOADED.read().ok()?.get(name).cloned()
}

/// Make a loaded plugin available by its name (replacing one loaded before;
/// never a built-in one). Sources and settings pick it up when they next
/// start a decoder, tracker or scorer.
pub fn install(p: Arc<dyn Plugin>) -> Result<(), String> {
    let m = p.manifest();
    m.validate()?;
    if is_builtin(&m.name) {
        return Err(format!("{} is a built-in plugin", m.name));
    }
    let name = m.name.clone();
    LOADED
        .write()
        .map_err(|_| "plugin registry poisoned".to_owned())?
        .insert(name, p);
    Ok(())
}

/// Forget a loaded plugin; what is already running keeps its instance.
pub fn uninstall(name: &str) {
    if let Ok(mut l) = LOADED.write() {
        l.remove(name);
    }
}

/// The names of the plugins installed (not built in).
pub fn installed() -> Vec<String> {
    LOADED
        .read()
        .map(|l| l.keys().cloned().collect())
        .unwrap_or_default()
}
