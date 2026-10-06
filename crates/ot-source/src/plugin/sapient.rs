//! SAPIENT (BSI Flex 335 v2.0) as a codec plugin, on the
//! `ot-sapient` decoder. SAPIENT is a NATO-standard protobuf schema for
//! sensor data exchange. Messages are framed on TCP with a 4-byte little-endian
//! length prefix (matching `sapient-rs`'s `utils::send`/`utils::read` helpers);
//! over MQTT each published message carries the raw protobuf payload.
//!
//! See [`ot_sapient`] for the underlying decoder.

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{Kind, Manifest, Plugin, PluginDecoder};
use crate::frame::{DEFAULT_MAX_FRAME, Endian, Framing, PrefixWidth};

pub struct Sapient {
    manifest: Manifest,
}

impl Sapient {
    pub fn new() -> Self {
        let framing = Some(Framing::LengthPrefix {
            width: PrefixWidth::U32,
            endian: Endian::Little,
            max_len: DEFAULT_MAX_FRAME,
        });
        let manifest = Manifest {
            name: "sapient".into(),
            version: ot_sapient::VERSION.into(),
            description: "SAPIENT (BSI Flex 335 v2.0): protobuf messages decoded via prost-reflect"
                .into(),
            kinds: vec![Kind::Codec],
            options: vec![],
            framing,
        };
        Self { manifest }
    }
}

impl Default for Sapient {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for Sapient {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn decoder(&self, options: &Value) -> Result<Box<dyn PluginDecoder>, String> {
        if !options.is_null() {
            return Err(format!("sapient takes no options, got {}", options));
        }
        let inner = ot_sapient::Decoder::new().map_err(|e| format!("sapient init: {e}"))?;
        Ok(Box::new(Decoder { inner }))
    }
}

struct Decoder {
    inner: ot_sapient::Decoder,
}

impl PluginDecoder for Decoder {
    fn decode(&mut self, bytes: &[u8], _received_at: DateTime<Utc>) -> Result<Vec<Value>, String> {
        self.inner.decode(bytes).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_is_valid() {
        let s = Sapient::new();
        s.manifest().validate().unwrap();
        assert_eq!(s.manifest().name, "sapient");
        assert!(s.manifest().provides(Kind::Codec));
        assert!(!s.manifest().provides(Kind::Tracker));
    }

    #[test]
    fn decoder_rejects_non_null_options() {
        let s = Sapient::new();
        assert!(s.decoder(&Value::Null).is_ok());
        let e = s
            .decoder(&Value::Object(serde_json::Map::new()))
            .err()
            .unwrap();
        assert!(e.contains("takes no options"), "{e}");
    }
}
