//! Protobuf a producer defines: its `.proto` files, uploaded with the
//! source, compiled here at runtime (no `protoc`, no rebuild), and its
//! messages decoded by reflection into the records the mapping reads.
//!
//! Decoded records follow the proto3 JSON mapping, with two changes that
//! suit mapping better: field names stay as written in the `.proto`
//! (`track_id`, not `trackId`), and 64-bit integers are numbers, not
//! strings. Fields left at their default (0, false, "") are included, so a
//! latitude of 0 is a latitude, not a missing field. Well-known types read
//! naturally: a `google.protobuf.Timestamp` is an RFC 3339 string, a
//! wrapper its value.

use std::collections::BTreeMap;
use std::sync::Arc;

use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor, SerializeOptions};
use protox::file::{ChainFileResolver, File, FileResolver, GoogleFileResolver};
use serde::Serialize;
use serde_json::Value;

/// Most files and bytes a source's schema may have.
pub const MAX_FILES: usize = 64;
pub const MAX_BYTES: usize = 2 << 20;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ProtoError {
    #[error("{0}")]
    Compile(String),
    #[error("no message {0:?} in the .proto files")]
    NoMessage(String),
    #[error("no method {0:?} in the .proto files (give it as package.Service/Method)")]
    NoMethod(String),
    #[error("{message}: {error}")]
    Decode { message: String, error: String },
    #[error("{message}: {error}")]
    Encode { message: String, error: String },
}

/// The uploaded files, name → source.
struct Uploaded(BTreeMap<String, String>);

impl FileResolver for Uploaded {
    fn open_file(&self, name: &str) -> Result<File, protox::Error> {
        match self.0.get(name) {
            Some(source) => File::from_source(name, source),
            None => Err(protox::Error::file_not_found(name)),
        }
    }
}

/// A compiled schema.
#[derive(Debug, Clone)]
pub struct ProtoSet {
    pool: DescriptorPool,
}

/// A method a schema defines.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Method {
    /// `package.Service/Method`.
    pub name: String,
    pub input: String,
    pub output: String,
    pub client_streaming: bool,
    pub server_streaming: bool,
}

/// A field of a message, for the source editor.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FieldInfo {
    pub name: String,
    /// `double`, `string`, `message pkg.Type`, `enum pkg.Kind`, ...
    #[serde(rename = "type")]
    pub kind: String,
    pub repeated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MessageInfo {
    pub name: String,
    pub fields: Vec<FieldInfo>,
}

/// What a schema holds, for the source editor.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Description {
    pub methods: Vec<Method>,
    pub messages: Vec<MessageInfo>,
}

impl ProtoSet {
    /// Compile the uploaded files (each may import the others, and the
    /// `google/protobuf/*.proto` well-known types).
    pub fn compile(files: &BTreeMap<String, String>) -> Result<Arc<Self>, ProtoError> {
        if files.is_empty() {
            return Err(ProtoError::Compile("no .proto files".into()));
        }
        if files.len() > MAX_FILES {
            return Err(ProtoError::Compile(format!(
                "{} .proto files: at most {MAX_FILES}",
                files.len()
            )));
        }
        let bytes: usize = files.values().map(String::len).sum();
        if bytes > MAX_BYTES {
            return Err(ProtoError::Compile(format!(
                ".proto files total {bytes} bytes: at most {MAX_BYTES}"
            )));
        }
        let mut resolver = ChainFileResolver::new();
        resolver.add(Uploaded(files.clone()));
        resolver.add(GoogleFileResolver::new());
        let mut compiler = protox::Compiler::with_file_resolver(resolver);
        compiler.include_imports(true);
        compiler
            .open_files(files.keys())
            .map_err(|e| ProtoError::Compile(explain(&e, files)))?;
        Ok(Arc::new(Self {
            pool: compiler.descriptor_pool(),
        }))
    }

    pub fn message(&self, name: &str) -> Result<MessageDescriptor, ProtoError> {
        let name = name.trim_start_matches('.');
        self.pool
            .get_message_by_name(name)
            .ok_or_else(|| ProtoError::NoMessage(name.to_owned()))
    }

    /// A method by `package.Service/Method` (a leading `/` is allowed).
    pub fn method(&self, name: &str) -> Result<Method, ProtoError> {
        let wanted = name.trim_start_matches('/');
        self.methods()
            .into_iter()
            .find(|m| m.name == wanted)
            .ok_or_else(|| ProtoError::NoMethod(wanted.to_owned()))
    }

    pub fn methods(&self) -> Vec<Method> {
        self.pool
            .services()
            .flat_map(|s| {
                s.methods()
                    .map(|m| Method {
                        name: format!("{}/{}", s.full_name(), m.name()),
                        input: m.input().full_name().to_owned(),
                        output: m.output().full_name().to_owned(),
                        client_streaming: m.is_client_streaming(),
                        server_streaming: m.is_server_streaming(),
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// Every service method and message the uploaded files define (not the
    /// well-known types they import).
    pub fn describe(&self) -> Description {
        let messages = self
            .pool
            .all_messages()
            .filter(|m| !m.full_name().starts_with("google.protobuf."))
            .filter(|m| !m.is_map_entry())
            .map(|m| MessageInfo {
                name: m.full_name().to_owned(),
                fields: m
                    .fields()
                    .map(|f| FieldInfo {
                        name: f.name().to_owned(),
                        kind: kind_name(&f.kind()),
                        repeated: f.is_list() || f.is_map(),
                    })
                    .collect(),
            })
            .collect();
        Description {
            methods: self.methods(),
            messages,
        }
    }

    /// Decode one message into a record.
    pub fn decode(&self, message: &MessageDescriptor, bytes: &[u8]) -> Result<Value, ProtoError> {
        let m = DynamicMessage::decode(message.clone(), bytes).map_err(|e| ProtoError::Decode {
            message: message.full_name().to_owned(),
            error: e.to_string(),
        })?;
        let options = SerializeOptions::new()
            .use_proto_field_name(true)
            .stringify_64_bit_integers(false)
            .skip_default_fields(false);
        m.serialize_with_options(serde_json::value::Serializer, &options)
            .map_err(|e| ProtoError::Decode {
                message: message.full_name().to_owned(),
                error: e.to_string(),
            })
    }

    /// Encode a message from its JSON form (the proto3 JSON mapping; field
    /// names as written or in lowerCamelCase).
    pub fn encode(
        &self,
        message: &MessageDescriptor,
        value: &Value,
    ) -> Result<Vec<u8>, ProtoError> {
        let m = DynamicMessage::deserialize(message.clone(), value).map_err(|e| {
            ProtoError::Encode {
                message: message.full_name().to_owned(),
                error: e.to_string(),
            }
        })?;
        Ok(prost::Message::encode_to_vec(&m))
    }
}

/// A compile error as `file:line: what`, found through its diagnostic spans
/// (a parse error keeps them in its related diagnostics).
fn explain(e: &protox::Error, files: &BTreeMap<String, String>) -> String {
    use miette::Diagnostic;
    let line = |offset: usize| {
        let src = e.file().and_then(|f| files.get(f))?;
        Some(src.get(..offset.min(src.len()))?.matches('\n').count() + 1)
    };
    let mut offset = e.labels().and_then(|mut l| l.next()).map(|l| l.offset());
    if offset.is_none()
        && let Some(mut related) = e.related()
        && let Some(r) = related.next()
    {
        offset = r.labels().and_then(|mut l| l.next()).map(|l| l.offset());
    }
    let related = e
        .related()
        .and_then(|mut r| r.next())
        .map(|r| r.to_string());
    let what = related.unwrap_or_else(|| e.to_string());
    match (e.file(), offset.and_then(line)) {
        (Some(f), Some(n)) => format!("{f}:{n}: {what}"),
        (Some(f), None) => format!("{f}: {what}"),
        _ => what,
    }
}

fn kind_name(k: &prost_reflect::Kind) -> String {
    use prost_reflect::Kind;
    match k {
        Kind::Double => "double".into(),
        Kind::Float => "float".into(),
        Kind::Int32 => "int32".into(),
        Kind::Int64 => "int64".into(),
        Kind::Uint32 => "uint32".into(),
        Kind::Uint64 => "uint64".into(),
        Kind::Sint32 => "sint32".into(),
        Kind::Sint64 => "sint64".into(),
        Kind::Fixed32 => "fixed32".into(),
        Kind::Fixed64 => "fixed64".into(),
        Kind::Sfixed32 => "sfixed32".into(),
        Kind::Sfixed64 => "sfixed64".into(),
        Kind::Bool => "bool".into(),
        Kind::String => "string".into(),
        Kind::Bytes => "bytes".into(),
        Kind::Message(m) => format!("message {}", m.full_name()),
        Kind::Enum(e) => format!("enum {}", e.full_name()),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use serde_json::json;

    use super::*;

    /// A producer's schema, as one would upload it: two files, one
    /// importing the other and a well-known type.
    pub fn files() -> BTreeMap<String, String> {
        BTreeMap::from([
            (
                "common.proto".into(),
                r#"syntax = "proto3";
package acme.common;
message Position { double lat = 1; double lon = 2; optional double alt_m = 3; }
enum Kind { KIND_UNKNOWN = 0; KIND_VESSEL = 1; KIND_AIRCRAFT = 2; }
"#
                .into(),
            ),
            (
                "tracks.proto".into(),
                r#"syntax = "proto3";
package acme.tracks.v1;
import "common.proto";
import "google/protobuf/timestamp.proto";
message Track {
  string track_id = 1;
  acme.common.Position position = 2;
  google.protobuf.Timestamp time = 3;
  acme.common.Kind kind = 4;
  uint64 mmsi = 5;
  repeated string tags = 6;
}
message TrackBatch { repeated Track tracks = 1; }
message Subscribe { string area = 1; uint32 max_rate_hz = 2; }
message Ack {}
service TrackFeed {
  rpc Stream(Subscribe) returns (stream TrackBatch);
  rpc Push(stream TrackBatch) returns (Ack);
}
"#
                .into(),
            ),
        ])
    }

    #[test]
    fn a_producers_schema_compiles_and_describes_itself() {
        let p = ProtoSet::compile(&files()).unwrap();
        let m = p.method("acme.tracks.v1.TrackFeed/Stream").unwrap();
        assert_eq!(m.output, "acme.tracks.v1.TrackBatch");
        assert!(m.server_streaming && !m.client_streaming);
        assert!(
            p.method("/acme.tracks.v1.TrackFeed/Push")
                .unwrap()
                .client_streaming
        );
        let d = p.describe();
        assert_eq!(d.methods.len(), 2);
        let track = d
            .messages
            .iter()
            .find(|m| m.name == "acme.tracks.v1.Track")
            .unwrap();
        let kinds: Vec<(&str, &str)> = track
            .fields
            .iter()
            .map(|f| (f.name.as_str(), f.kind.as_str()))
            .collect();
        assert_eq!(kinds[1], ("position", "message acme.common.Position"));
        assert_eq!(kinds[2], ("time", "message google.protobuf.Timestamp"));
        assert!(track.fields[5].repeated);
        assert!(d.messages.iter().all(|m| !m.name.starts_with("google.")));
    }

    #[test]
    fn a_message_decodes_into_a_record_the_mapping_can_read() {
        let p = ProtoSet::compile(&files()).unwrap();
        let batch = p.message("acme.tracks.v1.TrackBatch").unwrap();
        let bytes = p
            .encode(
                &batch,
                &json!({"tracks": [{
                    "track_id": "T-1", "position": {"lat": 0.0, "lon": -1.25},
                    "time": "2026-09-27T12:00:00Z", "kind": "KIND_VESSEL",
                    "mmsi": "235009876", "tags": ["a"]
                }]}),
            )
            .unwrap();
        let v = p.decode(&batch, &bytes).unwrap();
        let t = &v["tracks"][0];
        assert_eq!(t["track_id"], "T-1");
        // A zero latitude is a latitude.
        assert_eq!(t["position"]["lat"], 0.0);
        assert_eq!(t["position"]["lon"], -1.25);
        assert!(
            t["position"].get("alt_m").is_none(),
            "an unset optional stays absent"
        );
        assert_eq!(t["time"], "2026-09-27T12:00:00Z");
        assert_eq!(t["kind"], "KIND_VESSEL");
        assert_eq!(t["mmsi"], 235009876u64, "64-bit integers are numbers");
    }

    #[test]
    fn bad_schemas_and_bad_bytes_are_explained() {
        let mut f = files();
        f.insert(
            "broken.proto".into(),
            "syntax = \"proto3\"; message X { int32 = 1; }".into(),
        );
        let e = ProtoSet::compile(&f).unwrap_err().to_string();
        assert!(e.starts_with("broken.proto:1: "), "{e}");
        let mut f = files();
        f.remove("common.proto");
        let e = ProtoSet::compile(&f).unwrap_err().to_string();
        assert!(e.contains("common.proto"), "a missing import is named: {e}");
        let p = ProtoSet::compile(&files()).unwrap();
        assert!(matches!(
            p.message("acme.Nope"),
            Err(ProtoError::NoMessage(_))
        ));
        assert!(matches!(
            p.method("acme.tracks.v1.TrackFeed/Nope"),
            Err(ProtoError::NoMethod(_))
        ));
        let batch = p.message("acme.tracks.v1.TrackBatch").unwrap();
        assert!(p.decode(&batch, &[0x0a, 0xff, 0xff]).is_err());
    }
}
