//! Write OpenTrack plugins in Rust. A plugin is a WebAssembly component
//! (`wit/plugin.wit`) that provides any of:
//!
//! - a [`Codec`]: a frame's bytes to JSON records a source's mapping reads
//! - a [`Tracker`]: plots in, tracks out, as a source's tracker stage
//! - a [`Scorer`]: evidence whether a report and a system track are the
//!   same object, for the engine's pairing test
//!
//! Implement the kinds you provide and name them in [`plugin!`]:
//!
//! ```ignore
//! use opentrack_plugin::{Observation, Tracker, json, plugin};
//!
//! struct Nearest { /* ... */ }
//!
//! impl Tracker for Nearest {
//!     fn open(options: serde_json::Value) -> Result<Self, String> { /* ... */ }
//!     fn push(&mut self, plot: Observation, received_at_ms: i64) { /* ... */ }
//!     fn run(&mut self, now_ms: i64, force: bool) -> Result<Vec<Observation>, String> { /* ... */ }
//! }
//!
//! plugin! {
//!     manifest: json!({"name": "nearest", "version": "1", "description": "..."}),
//!     tracker: Nearest,
//! }
//! ```
//!
//! Build with `cargo build --release --target wasm32-wasip2` and add the
//! `.wasm` in Settings → Plugins (or `opentrack plugin check` it first).

use std::cell::RefCell;

pub use ot_core::Observation;
pub use serde_json::{self, Value, json};

#[doc(hidden)]
pub mod bindings {
    wit_bindgen::generate!({
        path: "../../../wit",
        world: "plugin",
        pub_export_macro: true,
        export_macro_name: "export_bindings",
        default_bindings_module: "opentrack_plugin::bindings",
    });
}

use bindings::exports::opentrack::plugin::{codec, scorer, tracker};

/// A codec: one instance per stream, so state carries across its frames.
pub trait Codec: Sized + 'static {
    /// Start a stream's decoder with the source's options.
    fn open(options: Value) -> Result<Self, String>;
    /// The JSON records in one frame.
    fn decode(&mut self, frame: &[u8], received_at_ms: i64) -> Result<Vec<Value>, String>;
    /// What the stream has shown about the sensor
    /// (`{"revisit_secs": .., "revisit_source": "measured"}`), once known.
    fn hints(&self) -> Option<Value> {
        None
    }
}

/// A tracker stage: one instance per source.
pub trait Tracker: Sized + 'static {
    fn open(options: Value) -> Result<Self, String>;
    /// A plot, at the time it arrived.
    fn push(&mut self, plot: Observation, received_at_ms: i64);
    /// Tracks to report now: observations whose `source_track_key` names the
    /// track (OpenTrack fills in the source). Called after every frame and
    /// about once a second; `force` at the end of a stream.
    fn run(&mut self, now_ms: i64, force: bool) -> Result<Vec<Observation>, String>;
}

/// A pairing scorer.
pub trait Scorer: Sized + 'static {
    fn open(options: Value) -> Result<Self, String>;
    /// One [`Score`] per candidate, in order.
    fn score(&mut self, report: &Observation, candidates: &[Candidate]) -> Result<Vec<Score>, String>;
}

/// A system track the engine asks a scorer about.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Candidate {
    /// The system track as it stands.
    pub view: Observation,
    /// OpenTrack's own kinematic comparison: `distance_m`, `dt_s`,
    /// `sigma_m`, `d2`, `dof`, `ln_lr`, `pass`...
    pub kinematic: Value,
}

/// A scorer's answer for one candidate.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Score {
    /// ln of the likelihood ratio: the same object against another nearby.
    pub ln_lr: f64,
    /// Inside the gate: a comparison that counts toward pairing.
    pub pass: bool,
    /// Recorded with the engine's decision.
    pub evidence: Value,
}

/// For the kinds a plugin does not provide.
pub struct Unsupported;

impl Codec for Unsupported {
    fn open(_: Value) -> Result<Self, String> {
        Err("this plugin is not a codec".into())
    }
    fn decode(&mut self, _: &[u8], _: i64) -> Result<Vec<Value>, String> {
        Ok(Vec::new())
    }
}

impl Tracker for Unsupported {
    fn open(_: Value) -> Result<Self, String> {
        Err("this plugin is not a tracker".into())
    }
    fn push(&mut self, _: Observation, _: i64) {}
    fn run(&mut self, _: i64, _: bool) -> Result<Vec<Observation>, String> {
        Ok(Vec::new())
    }
}

impl Scorer for Unsupported {
    fn open(_: Value) -> Result<Self, String> {
        Err("this plugin is not a scorer".into())
    }
    fn score(&mut self, _: &Observation, _: &[Candidate]) -> Result<Vec<Score>, String> {
        Ok(Vec::new())
    }
}

/// Lines in OpenTrack's log, tagged with the plugin's name.
pub mod log {
    use super::bindings::opentrack::plugin::host::{Level, log};

    pub fn debug(message: &str) {
        log(Level::Debug, message);
    }
    pub fn info(message: &str) {
        log(Level::Info, message);
    }
    pub fn warn(message: &str) {
        log(Level::Warn, message);
    }
    pub fn error(message: &str) {
        log(Level::Error, message);
    }
}

fn options(raw: &str) -> Result<Value, String> {
    if raw.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(raw).map_err(|e| format!("options: {e}"))
}

fn to_json<T: serde::Serialize>(v: &T) -> Result<String, String> {
    serde_json::to_string(v).map_err(|e| e.to_string())
}

// The resources behind each kind: generic over the plugin's own types.

#[doc(hidden)]
pub struct DecoderOf<C: Codec>(RefCell<C>);

impl<C: Codec> codec::GuestDecoder for DecoderOf<C> {
    fn decode(&self, frame: Vec<u8>, received_at_ms: i64) -> Result<Vec<String>, String> {
        let records = self.0.borrow_mut().decode(&frame, received_at_ms)?;
        records.iter().map(to_json).collect()
    }

    fn hints(&self) -> Option<String> {
        self.0.borrow().hints().map(|h| h.to_string())
    }
}

#[doc(hidden)]
pub struct TrackerOf<T: Tracker>(RefCell<T>);

impl<T: Tracker> tracker::GuestTracker for TrackerOf<T> {
    fn push(&self, plot: String, received_at_ms: i64) {
        match serde_json::from_str::<Observation>(&plot) {
            Ok(p) => self.0.borrow_mut().push(p, received_at_ms),
            Err(e) => log::warn(&format!("a plot is not an observation: {e}")),
        }
    }

    fn run(&self, now_ms: i64, force: bool) -> Result<Vec<String>, String> {
        let tracks = self.0.borrow_mut().run(now_ms, force)?;
        tracks.iter().map(to_json).collect()
    }
}

#[doc(hidden)]
pub struct ScorerOf<S: Scorer>(RefCell<S>);

impl<S: Scorer> scorer::GuestScorer for ScorerOf<S> {
    fn score(&self, report: String, candidates: Vec<String>) -> Result<Vec<String>, String> {
        let report: Observation =
            serde_json::from_str(&report).map_err(|e| format!("the report: {e}"))?;
        let candidates: Vec<Candidate> = candidates
            .iter()
            .map(|c| serde_json::from_str(c).map_err(|e| format!("a candidate: {e}")))
            .collect::<Result<_, _>>()?;
        let scores = self.0.borrow_mut().score(&report, &candidates)?;
        scores.iter().map(to_json).collect()
    }
}

#[doc(hidden)]
pub fn open_decoder<C: Codec>(raw: String) -> Result<codec::Decoder, String> {
    Ok(codec::Decoder::new(DecoderOf(RefCell::new(C::open(options(&raw)?)?))))
}

#[doc(hidden)]
pub fn open_tracker<T: Tracker>(raw: String) -> Result<tracker::Tracker, String> {
    Ok(tracker::Tracker::new(TrackerOf(RefCell::new(T::open(options(&raw)?)?))))
}

#[doc(hidden)]
pub fn open_scorer<S: Scorer>(raw: String) -> Result<scorer::Scorer, String> {
    Ok(scorer::Scorer::new(ScorerOf(RefCell::new(S::open(options(&raw)?)?))))
}

/// The manifest as JSON, with `kinds` filled in from what the plugin provides.
#[doc(hidden)]
pub fn describe(mut manifest: Value, kinds: &[(&str, bool)]) -> String {
    if manifest.get("kinds").is_none() {
        let provided: Vec<&str> = kinds.iter().filter(|(_, on)| *on).map(|(k, _)| *k).collect();
        manifest["kinds"] = json!(provided);
    }
    manifest.to_string()
}

#[doc(hidden)]
#[macro_export]
macro_rules! __kind {
    () => {
        $crate::Unsupported
    };
    ($t:ty) => {
        $t
    };
}

/// Export a plugin: its manifest (JSON: `name`, `version`, `description`,
/// `options`, and for a codec `framing`) and the types that provide each
/// kind. Kinds left out are reported as not provided.
#[macro_export]
macro_rules! plugin {
    (
        manifest: $manifest:expr
        $(, codec: $codec:ty)?
        $(, tracker: $tracker:ty)?
        $(, scorer: $scorer:ty)?
        $(,)?
    ) => {
        struct __OpenTrackPlugin;

        impl $crate::bindings::exports::opentrack::plugin::meta::Guest for __OpenTrackPlugin {
            fn describe() -> String {
                $crate::describe(
                    $manifest,
                    &[
                        ("codec", false $(|| { let _ = ::core::marker::PhantomData::<$codec>; true })?),
                        ("tracker", false $(|| { let _ = ::core::marker::PhantomData::<$tracker>; true })?),
                        ("scorer", false $(|| { let _ = ::core::marker::PhantomData::<$scorer>; true })?),
                    ],
                )
            }
        }

        impl $crate::bindings::exports::opentrack::plugin::codec::Guest for __OpenTrackPlugin {
            type Decoder = $crate::DecoderOf<$crate::__kind!($($codec)?)>;
            fn open_decoder(
                options: String,
            ) -> Result<$crate::bindings::exports::opentrack::plugin::codec::Decoder, String> {
                $crate::open_decoder::<$crate::__kind!($($codec)?)>(options)
            }
        }

        impl $crate::bindings::exports::opentrack::plugin::tracker::Guest for __OpenTrackPlugin {
            type Tracker = $crate::TrackerOf<$crate::__kind!($($tracker)?)>;
            fn open_tracker(
                options: String,
            ) -> Result<$crate::bindings::exports::opentrack::plugin::tracker::Tracker, String> {
                $crate::open_tracker::<$crate::__kind!($($tracker)?)>(options)
            }
        }

        impl $crate::bindings::exports::opentrack::plugin::scorer::Guest for __OpenTrackPlugin {
            type Scorer = $crate::ScorerOf<$crate::__kind!($($scorer)?)>;
            fn open_scorer(
                options: String,
            ) -> Result<$crate::bindings::exports::opentrack::plugin::scorer::Scorer, String> {
                $crate::open_scorer::<$crate::__kind!($($scorer)?)>(options)
            }
        }

        $crate::bindings::export_bindings!(__OpenTrackPlugin with_types_in $crate::bindings);
    };
}
