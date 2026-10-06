//! A stage-by-stage record of what a pipeline did with its first few
//! records, for the pipeline designer's live preview.
//!
//! Tracing is opt-in: [`crate::pipeline::Pipeline::process_traced`] fills a
//! [`Trace`]; [`crate::pipeline::Pipeline::process`] takes no trace and
//! neither clones nor allocates for one.
//!
//! A sample is one decoded record, in decode order: the first frame's
//! records first, later frames' only while the trace has room. A frame that
//! does not decode is a sample of its own. Other records are processed as
//! usual (they move the tracker, throttle and counts) but not recorded.

use ot_core::Observation;
use serde::Serialize;
use serde_json::{Value, json};

use crate::frame::Frame;

/// Largest JSON item kept whole; bigger ones are cut to a marker.
pub const MAX_ITEM_BYTES: usize = 64 * 1024;

/// The most records one trace follows.
pub const MAX_SAMPLES: usize = 10;

/// The first records through a pipeline, stage by stage.
#[derive(Debug, Default, Serialize)]
pub struct Trace {
    /// The frames the samples came from, as received, each once.
    pub frames: Vec<FrameView>,
    pub samples: Vec<SampleTrace>,
    #[serde(skip)]
    limit: usize,
    /// The frame being processed, once it is in `frames`.
    #[serde(skip)]
    open: Option<usize>,
}

/// One record and what became of it after each stage.
#[derive(Debug, Serialize)]
pub struct SampleTrace {
    /// Index in [`Trace::frames`] of the frame it came from.
    pub frame: usize,
    /// The pipeline's stages after the transport, in order, only those it has.
    pub stages: Vec<StageTrace>,
    /// What the pipeline emitted from this record, to show as published.
    #[serde(skip)]
    pub emitted: Vec<Observation>,
    /// Detections this record gave the tracker stage.
    #[serde(skip)]
    pub(crate) plots: Vec<Observation>,
}

/// A raw frame, readable: JSON when it parses, else text, else escaped bytes.
#[derive(Debug, Serialize)]
pub struct FrameView {
    /// `json`, `text` or `binary`.
    pub format: &'static str,
    /// Length of the frame in bytes.
    pub bytes: usize,
    /// The parsed JSON, the text, or the bytes escaped (`\xNN`), cut to
    /// [`MAX_ITEM_BYTES`]. JSON too big for that comes as its pretty-printed
    /// text, cut at a line (`truncated` is then set).
    pub content: Value,
    /// Only the start of the frame is in `content`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

/// A record after one stage.
#[derive(Debug, Serialize)]
pub struct StageTrace {
    /// The stage id, as the UI names it (`decode`, `map`, `filter`…).
    pub id: &'static str,
    /// What this stage passed on: the record up to the reject stage, the
    /// mapping's outputs, then observations.
    pub items: Vec<Value>,
    /// Why it (or one of its outputs) was dropped here.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dropped: Vec<String>,
    /// What this stage did with it that is not a drop (a static identity
    /// kept to fill later reports, say): shown at this stage only.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    /// The stage held it back (a tracker waiting for the rest of a scan);
    /// what it made of it later is in `items`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub held: bool,
}

impl Trace {
    /// Follow the first `records` records (at most [`MAX_SAMPLES`]).
    pub fn new(records: usize) -> Self {
        Self {
            limit: records.min(MAX_SAMPLES),
            ..Self::default()
        }
    }

    /// A new frame is being processed.
    pub(crate) fn start_frame(&mut self) {
        self.open = None;
    }

    /// Start tracing a record of `frame` (or the frame itself, when it does
    /// not decode), if the trace still has room.
    pub(crate) fn begin(&mut self, frame: &Frame, stages: &[&'static str]) -> Option<usize> {
        if self.samples.len() >= self.limit {
            return None;
        }
        let at = match self.open {
            Some(at) => at,
            None => {
                self.frames.push(FrameView::of(&frame.bytes));
                let at = self.frames.len() - 1;
                self.open = Some(at);
                at
            }
        };
        self.samples.push(SampleTrace {
            frame: at,
            stages: stages
                .iter()
                .map(|id| StageTrace {
                    id,
                    items: Vec::new(),
                    dropped: Vec::new(),
                    notes: Vec::new(),
                    held: false,
                })
                .collect(),
            emitted: Vec::new(),
            plots: Vec::new(),
        });
        Some(self.samples.len() - 1)
    }
}

impl SampleTrace {
    pub fn stage(&mut self, id: &str) -> Option<&mut StageTrace> {
        self.stages.iter_mut().find(|s| s.id == id)
    }

    /// Record what `id` passed on (built only if the pipeline has that stage).
    pub(crate) fn item(&mut self, id: &str, v: impl FnOnce() -> Value) {
        if let Some(s) = self.stage(id) {
            s.items.push(capped(v()));
        }
    }

    /// Record an observation (or its JSON form) `id` passed on.
    pub(crate) fn obs<T: Serialize>(&mut self, id: &str, o: &T) {
        self.item(id, || serde_json::to_value(o).unwrap_or(Value::Null));
    }

    /// Record a record dropped at `id`.
    pub(crate) fn dropped(&mut self, id: &str, reason: impl Into<String>) {
        if let Some(s) = self.stage(id) {
            s.dropped.push(reason.into());
        }
    }

    /// Note what `id` did with it, when that is not a drop.
    pub(crate) fn note(&mut self, id: &str, text: impl Into<String>) {
        if let Some(s) = self.stage(id) {
            s.notes.push(text.into());
        }
    }
}

impl FrameView {
    fn of(bytes: &[u8]) -> Self {
        let n = bytes.len();
        match std::str::from_utf8(bytes) {
            Ok(text) => match serde_json::from_str::<Value>(text) {
                Ok(v) => {
                    let pretty = serde_json::to_string_pretty(&v).unwrap_or_default();
                    if pretty.len() <= MAX_ITEM_BYTES {
                        Self {
                            format: "json",
                            bytes: n,
                            content: v,
                            truncated: false,
                        }
                    } else {
                        // Readable still: the pretty text, to the last whole line that fits.
                        let end = pretty[..MAX_ITEM_BYTES.min(pretty.len())]
                            .rfind('\n')
                            .unwrap_or(0);
                        Self {
                            format: "json",
                            bytes: n,
                            content: Value::String(pretty[..end].to_owned()),
                            truncated: true,
                        }
                    }
                }
                Err(_) => Self {
                    format: "text",
                    bytes: n,
                    content: Value::String(cut(text, MAX_ITEM_BYTES)),
                    truncated: text.len() > MAX_ITEM_BYTES,
                },
            },
            Err(_) => {
                let mut s = String::new();
                for &b in bytes {
                    if s.len() >= MAX_ITEM_BYTES {
                        s.push_str(" …");
                        break;
                    }
                    s.extend(std::ascii::escape_default(b).map(char::from));
                }
                Self {
                    format: "binary",
                    bytes: n,
                    truncated: s.ends_with(" …"),
                    content: Value::String(s),
                }
            }
        }
    }
}

/// `v`, or a marker with its start if it serialises to more than
/// [`MAX_ITEM_BYTES`].
pub fn capped(v: Value) -> Value {
    let text = v.to_string();
    if text.len() <= MAX_ITEM_BYTES {
        return v;
    }
    json!({
        "truncated": format!("{} bytes, over the {} KiB trace limit", text.len(), MAX_ITEM_BYTES / 1024),
        "start": cut(&text, 4096),
    })
}

/// The first `max` bytes of `s`, on a character boundary, marked if cut.
fn cut(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{} …", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_shown_as_json_text_or_escaped_bytes() {
        let j = FrameView::of(br#"{"a":1}"#);
        assert_eq!((j.format, j.bytes, j.content), ("json", 7, json!({"a": 1})));
        let t = FrameView::of(b"!AIVDM,1");
        assert_eq!((t.format, t.content), ("text", json!("!AIVDM,1")));
        let b = FrameView::of(&[0x01, b'A', 0xff]);
        assert_eq!(
            (b.format, b.bytes, b.content),
            ("binary", 3, json!("\\x01A\\xff"))
        );
    }

    #[test]
    fn a_big_json_frame_is_its_pretty_start_cut_at_a_line() {
        let rows: Vec<Value> = (0..4000)
            .map(|i| json!({"hex": format!("{i:06x}"), "alt": i}))
            .collect();
        let raw = serde_json::to_vec(&json!({"ac": rows})).unwrap();
        let f = FrameView::of(&raw);
        assert_eq!((f.format, f.bytes, f.truncated), ("json", raw.len(), true));
        let text = f.content.as_str().unwrap();
        assert!(text.len() <= MAX_ITEM_BYTES);
        assert!(text.starts_with("{\n  \"ac\": [\n    {"), "{}", &text[..40]);
        assert!(!text.ends_with('\n') && text.lines().all(|l| !l.is_empty()));
        assert!(!FrameView::of(br#"{"a":1}"#).truncated);
    }

    #[test]
    fn big_items_are_cut_to_a_marker() {
        let v = json!({"s": "x".repeat(MAX_ITEM_BYTES)});
        let c = capped(v);
        assert!(c["truncated"].as_str().unwrap().contains("64 KiB"), "{c}");
        assert!(c["start"].as_str().unwrap().len() < 5000);
        assert_eq!(capped(json!([1])), json!([1]));
    }
}
