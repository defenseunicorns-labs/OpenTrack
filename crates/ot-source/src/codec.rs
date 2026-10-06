//! Codecs: frames to records.
//!
//! A record is a decoded tree (`serde_json::Value`) that the mapping stage
//! reads with paths. Codecs never interpret fields; that is the mapping's job.

use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::frame::Frame;
use crate::path::Path;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CodecConfig {
    /// Any JSON object or array.
    Json {
        /// Path to the array of records inside each frame (e.g. `ac` for
        /// adsb.lol). Without it the frame itself is the record, or each
        /// element if the frame is an array.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        records: Option<Path>,
        /// Frame-level fields copied into every record under `_frame`
        /// (e.g. adsb.lol's snapshot time `now`), keyed by their path.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        context: Vec<Path>,
    },
    /// MITRE Cursor-on-Target XML: one record per `<event>`.
    CotXml,
    /// Generic XML: one record per `record_element`.
    Xml { record_element: String },
    /// Protobuf: one message per frame, of type `message`, decoded with the
    /// producer's own `.proto` files (uploaded with the source; see
    /// [`crate::proto`]). Over gRPC, `message` is the method's response
    /// (or, for a gRPC server, request) type.
    Protobuf {
        /// The producer's `.proto` files, name → contents. They may import
        /// each other and the `google/protobuf/*.proto` well-known types.
        files: std::collections::BTreeMap<String, String>,
        /// Full name of the message each frame holds, e.g.
        /// `acme.tracks.v1.TrackBatch`.
        message: String,
        /// Path to the repeated field holding the records (e.g. `tracks`);
        /// without it the message itself is the record.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        records: Option<Path>,
        /// Message-level fields copied into every record under `_frame`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        context: Vec<Path>,
    },
    /// A codec plugin (see [`crate::plugin`]), e.g. STANAG 4607.
    Plugin {
        plugin: String,
        #[serde(default, skip_serializing_if = "Value::is_null")]
        options: Value,
    },
}

impl CodecConfig {
    pub fn name(&self) -> &'static str {
        match self {
            CodecConfig::Json { .. } => "json",
            CodecConfig::CotXml => "cot_xml",
            CodecConfig::Xml { .. } => "xml",
            CodecConfig::Protobuf { .. } => "protobuf",
            CodecConfig::Plugin { .. } => "plugin",
        }
    }

    /// The compiled `.proto` files of a protobuf codec.
    pub fn proto(&self) -> Result<Option<std::sync::Arc<crate::proto::ProtoSet>>, CodecError> {
        match self {
            CodecConfig::Protobuf { files, .. } => Ok(Some(
                crate::proto::ProtoSet::compile(files)
                    .map_err(|e| CodecError::Protobuf(e.to_string()))?,
            )),
            _ => Ok(None),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("record path {0} not found in frame")]
    MissingRecords(String),
    #[error("record path {0} is not an array")]
    NotArray(String),
    #[error("invalid XML: {0}")]
    Xml(String),
    #[error("{0}")]
    Plugin(String),
    #[error("protobuf: {0}")]
    Protobuf(String),
}

pub struct Codec {
    config: CodecConfig,
    /// The plugin's decoder for this stream, for a plugin codec.
    plugin: Option<Box<dyn crate::plugin::PluginDecoder>>,
    /// The compiled schema and the message each frame holds, for protobuf.
    proto: Option<(
        std::sync::Arc<crate::proto::ProtoSet>,
        prost_reflect::MessageDescriptor,
    )>,
}

impl Codec {
    /// A codec for one stream (a plugin's decoder keeps state across frames).
    pub fn new(config: CodecConfig) -> Result<Self, CodecError> {
        let plugin = match &config {
            CodecConfig::Plugin { plugin, options } => Some(
                crate::plugin::plugin(plugin)
                    .ok_or_else(|| CodecError::Plugin(format!("no plugin {plugin:?}")))?
                    .decoder(options)
                    .map_err(CodecError::Plugin)?,
            ),
            _ => None,
        };
        let proto = match (&config, config.proto()?) {
            (CodecConfig::Protobuf { message, .. }, Some(set)) => {
                let desc = set
                    .message(message)
                    .map_err(|e| CodecError::Protobuf(e.to_string()))?;
                Some((set, desc))
            }
            _ => None,
        };
        Ok(Self {
            config,
            plugin,
            proto,
        })
    }

    pub fn config(&self) -> &CodecConfig {
        &self.config
    }

    /// What a plugin decoder has learnt about the sensor so far.
    pub fn hints(&self) -> Option<crate::plugin::StreamHints> {
        self.plugin.as_ref()?.hints()
    }

    /// Decode one frame into zero or more records. Transport metadata on the
    /// frame is added to every record under `_frame`.
    pub fn decode(&mut self, frame: &Frame) -> Result<Vec<Value>, CodecError> {
        let mut records = match &self.config {
            CodecConfig::Plugin { .. } => self
                .plugin
                .as_mut()
                .expect("a plugin codec has a decoder")
                .decode(&frame.bytes, frame.received_at)
                .map_err(CodecError::Plugin),
            CodecConfig::Json { records, context } => {
                decode_json(&frame.bytes, records.as_ref(), context)
            }
            CodecConfig::Protobuf {
                records, context, ..
            } => {
                let (set, desc) = self
                    .proto
                    .as_ref()
                    .expect("a protobuf codec has its schema");
                let value = set
                    .decode(desc, &frame.bytes)
                    .map_err(|e| CodecError::Protobuf(e.to_string()))?;
                split_with_context(value, records.as_ref(), context)
            }
            CodecConfig::CotXml => decode_xml(&frame.bytes, "event"),
            CodecConfig::Xml { record_element } => decode_xml(&frame.bytes, record_element),
        }?;
        if !frame.meta.is_empty() {
            for rec in &mut records {
                if let Value::Object(map) = rec {
                    let ctx = map
                        .entry("_frame")
                        .or_insert_with(|| Value::Object(Map::new()));
                    if let Value::Object(ctx) = ctx {
                        for (k, v) in &frame.meta {
                            ctx.insert(k.clone(), v.clone());
                        }
                    }
                }
            }
        }
        Ok(records)
    }
}

fn decode_json(
    bytes: &[u8],
    records: Option<&Path>,
    context: &[Path],
) -> Result<Vec<Value>, CodecError> {
    split_with_context(serde_json::from_slice(bytes)?, records, context)
}

/// Split a decoded tree into records, copying `context` fields into each.
fn split_with_context(
    value: Value,
    records: Option<&Path>,
    context: &[Path],
) -> Result<Vec<Value>, CodecError> {
    let mut out = split_json(value.clone(), records)?;
    if !context.is_empty() {
        let mut frame = Map::new();
        for p in context {
            if let Some(v) = p.get(&value) {
                frame.insert(p.to_string(), v.clone());
            }
        }
        for rec in &mut out {
            if let Value::Object(map) = rec {
                map.insert("_frame".into(), Value::Object(frame.clone()));
            }
        }
    }
    Ok(out)
}

fn split_json(value: Value, records: Option<&Path>) -> Result<Vec<Value>, CodecError> {
    match records {
        Some(path) => match path.get(&value) {
            Some(Value::Array(items)) => Ok(items.clone()),
            // An explicit null means "no records this time" (polled APIs).
            Some(Value::Null) => Ok(Vec::new()),
            Some(_) => Err(CodecError::NotArray(path.to_string())),
            None => Err(CodecError::MissingRecords(path.to_string())),
        },
        None => Ok(match value {
            Value::Array(items) => items,
            other => vec![other],
        }),
    }
}

/// Parse XML into one tree per `record_element`, each as
/// `{ "<record_element>": { "@attr": "..", "child": {..}, "#text": ".." } }`.
/// Child elements that recur become arrays; attribute values stay strings.
fn decode_xml(bytes: &[u8], record_element: &str) -> Result<Vec<Value>, CodecError> {
    let mut reader = Reader::from_reader(bytes);
    // Text is kept untrimmed while an element is open, because entity
    // references arrive as separate events ("two ", "&amp;", " three");
    // it is trimmed once when the element closes.
    let mut out = Vec::new();
    // Stack of (element name, element object) under construction.
    let mut stack: Vec<(String, Map<String, Value>)> = Vec::new();
    let mut buf = Vec::new();
    loop {
        let ev = reader
            .read_event_into(&mut buf)
            .map_err(|e| CodecError::Xml(e.to_string()))?;
        match ev {
            Event::Start(start) => {
                let (name, obj) = open(&start)?;
                stack.push((name, obj));
            }
            Event::Empty(start) => {
                let (name, obj) = open(&start)?;
                close(&mut stack, &mut out, record_element, name, obj);
            }
            Event::End(_) => {
                let Some((name, obj)) = stack.pop() else {
                    return Err(CodecError::Xml("unbalanced end tag".into()));
                };
                close(&mut stack, &mut out, record_element, name, obj);
            }
            Event::Text(t) => {
                if let Some((_, obj)) = stack.last_mut() {
                    append_text(obj, &t.xml_content(XmlVersion::Implicit1_0));
                }
            }
            Event::CData(t) => {
                if let Some((_, obj)) = stack.last_mut() {
                    append_text(obj, &t);
                }
            }
            Event::GeneralRef(r) => {
                if let Some((_, obj)) = stack.last_mut() {
                    let resolved = match r.resolve_char_ref() {
                        Ok(Some(c)) => c.to_string(),
                        _ => match &*r {
                            "amp" => "&".into(),
                            "lt" => "<".into(),
                            "gt" => ">".into(),
                            "quot" => "\"".into(),
                            "apos" => "'".into(),
                            other => format!("&{other};"),
                        },
                    };
                    append_text(obj, &resolved);
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    if !stack.is_empty() {
        return Err(CodecError::Xml("unexpected end of document".into()));
    }
    Ok(out)
}

fn append_text(obj: &mut Map<String, Value>, text: &str) {
    if text.is_empty() {
        return;
    }
    if !obj.contains_key("#text") && text.trim().is_empty() {
        return;
    }
    match obj.get_mut("#text") {
        Some(Value::String(existing)) => existing.push_str(text),
        _ => {
            obj.insert("#text".into(), Value::String(text.to_owned()));
        }
    }
}

fn open(start: &BytesStart<'_>) -> Result<(String, Map<String, Value>), CodecError> {
    let name = start.name().as_ref().to_owned();
    let mut obj = Map::new();
    for attr in start.attributes() {
        let attr = attr.map_err(|e| CodecError::Xml(e.to_string()))?;
        let key = format!("@{}", attr.key.as_ref() as &str);
        let value = attr
            .normalized_value(XmlVersion::Implicit1_0)
            .map_err(|e| CodecError::Xml(e.to_string()))?;
        obj.insert(key, Value::String(value.into_owned()));
    }
    Ok((name, obj))
}

fn close(
    stack: &mut [(String, Map<String, Value>)],
    out: &mut Vec<Value>,
    record_element: &str,
    name: String,
    mut obj: Map<String, Value>,
) {
    match obj.get("#text").and_then(Value::as_str).map(str::trim) {
        Some("") => {
            obj.remove("#text");
        }
        Some(t) => {
            let t = t.to_owned();
            obj.insert("#text".into(), Value::String(t));
        }
        None => {}
    }
    let value = Value::Object(obj);
    if name == record_element {
        let mut rec = Map::new();
        rec.insert(name, value);
        out.push(Value::Object(rec));
        return;
    }
    let Some((_, parent)) = stack.last_mut() else {
        return;
    };
    match parent.get_mut(&name) {
        Some(Value::Array(items)) => items.push(value),
        Some(existing) => {
            let first = existing.take();
            *existing = Value::Array(vec![first, value]);
        }
        None => {
            parent.insert(name, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn decode(config: CodecConfig, s: &str) -> Vec<Value> {
        Codec::new(config)
            .unwrap()
            .decode(&Frame::new(s.as_bytes().to_vec()))
            .unwrap()
    }

    #[test]
    fn json_object_array_and_record_path() {
        let plain = CodecConfig::Json {
            records: None,
            context: vec![],
        };
        assert_eq!(decode(plain.clone(), r#"{"a":1}"#), [json!({"a":1})]);
        assert_eq!(decode(plain, r#"[{"a":1},{"a":2}]"#).len(), 2);

        let ac = CodecConfig::Json {
            records: Some("ac".parse().unwrap()),
            context: vec!["now".parse().unwrap()],
        };
        let recs = decode(ac.clone(), r#"{"now":1,"ac":[{"hex":"a"},{"hex":"b"}]}"#);
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[1]["_frame"]["now"], 1);
        assert!(decode(ac.clone(), r#"{"now":1,"ac":null}"#).is_empty());
        let err = Codec::new(ac)
            .unwrap()
            .decode(&Frame::new(&b"{\"now\":1}"[..]))
            .unwrap_err();
        assert!(matches!(err, CodecError::MissingRecords(_)));
    }

    #[test]
    fn cot_event_becomes_a_tree() {
        let xml = r#"<?xml version="1.0"?>
            <event version="2.0" uid="ANDROID-1" type="a-f-G-U-C" time="2026-09-24T12:00:00Z"
                   start="2026-09-24T12:00:00Z" stale="2026-09-24T12:05:00Z" how="m-g">
              <point lat="32.7" lon="-117.2" hae="10" ce="5" le="9999999"/>
              <detail>
                <contact callsign="VIPER 1"/>
                <__group name="Cyan" role="Team Member"/>
                <remarks>two &amp; three</remarks>
                <link uid="a"/><link uid="b"/>
              </detail>
            </event>"#;
        let recs = decode(CodecConfig::CotXml, xml);
        assert_eq!(recs.len(), 1);
        let e = &recs[0]["event"];
        assert_eq!(e["@uid"], "ANDROID-1");
        assert_eq!(e["point"]["@lat"], "32.7");
        assert_eq!(e["detail"]["contact"]["@callsign"], "VIPER 1");
        assert_eq!(e["detail"]["remarks"]["#text"], "two & three");
        assert_eq!(e["detail"]["link"][1]["@uid"], "b");
    }

    #[test]
    fn frame_meta_reaches_every_record() {
        let mut meta = serde_json::Map::new();
        meta.insert("topic".into(), json!("ais/366123456/pos"));
        let frame = Frame::new(&br#"{"now":1,"ac":[{"hex":"a"},{"hex":"b"}]}"#[..]).with_meta(meta);
        let mut codec = Codec::new(CodecConfig::Json {
            records: Some("ac".parse().unwrap()),
            context: vec!["now".parse().unwrap()],
        })
        .unwrap();
        let recs = codec.decode(&frame).unwrap();
        assert_eq!(recs.len(), 2);
        // Merged with the JSON codec's own frame context.
        assert_eq!(
            recs[1]["_frame"],
            json!({"now": 1, "topic": "ais/366123456/pos"})
        );
        let cot = Codec::new(CodecConfig::CotXml)
            .unwrap()
            .decode(
                &Frame::new(&br#"<event uid="x" type="a-f-G"/>"#[..]).with_meta(
                    serde_json::Map::from_iter([("topic".to_string(), json!("cot/x"))]),
                ),
            )
            .unwrap();
        assert_eq!(cot[0]["_frame"]["topic"], "cot/x");
    }

    #[test]
    fn several_events_in_one_frame_and_bad_xml() {
        let recs = decode(
            CodecConfig::CotXml,
            r#"<event uid="a"/><event uid="b"></event>"#,
        );
        assert_eq!(recs.len(), 2);
        assert!(
            Codec::new(CodecConfig::CotXml)
                .unwrap()
                .decode(&Frame::new(&b"<event><point></event>"[..]))
                .is_err()
        );
    }
}
