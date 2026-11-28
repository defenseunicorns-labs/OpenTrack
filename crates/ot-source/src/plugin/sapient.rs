//! SAPIENT (BSI Flex 335 v2.0) as a codec plugin, on the
//! `ot-sapient` decoder. SAPIENT is a NATO-standard protobuf schema for
//! sensor data exchange. Messages are framed on TCP with a 4-byte little-endian
//! length prefix (matching `sapient-rs`'s `utils::send`/`utils::read` helpers);
//! over MQTT each published message carries the raw protobuf payload.
//!
//! See [`ot_sapient`] for the underlying decoder.

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{Kind, Manifest, OptionKind, Plugin, PluginDecoder, PluginOption};
use crate::frame::{DEFAULT_MAX_FRAME, Endian, Framing, PrefixWidth};

mod origin;

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
            options: vec![
                PluginOption {
                    name: "resolve_bearing_origin".into(),
                    label: "Resolve bearing origin from status reports".into(),
                    help: "Match each bearing to the reporting node's position at detection time.".into(),
                    kind: OptionKind::Bool { default: false },
                },
                PluginOption {
                    name: "max_position_gap_secs".into(),
                    label: "Maximum position interval".into(),
                    help: "Interpolate between positions up to this interval apart. Zero requires matching timestamps.".into(),
                    kind: OptionKind::Number {
                        default: Some(0.0),
                        unit: "s".into(),
                        min: Some(0.0),
                    },
                },
            ],
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
        let options: origin::Options = if options.is_null() {
            origin::Options::default()
        } else {
            serde_json::from_value(options.clone()).map_err(|e| format!("sapient options: {e}"))?
        };
        if options.max_position_gap_secs > 300 {
            return Err("sapient max_position_gap_secs must be at most 300".into());
        }
        let inner = ot_sapient::Decoder::new().map_err(|e| format!("sapient init: {e}"))?;
        let origins = options
            .resolve_bearing_origin
            .then(|| origin::Resolver::new(&options));
        Ok(Box::new(Decoder { inner, origins }))
    }
}

struct Decoder {
    inner: ot_sapient::Decoder,
    origins: Option<origin::Resolver>,
}

impl PluginDecoder for Decoder {
    fn decode(&mut self, bytes: &[u8], received_at: DateTime<Utc>) -> Result<Vec<Value>, String> {
        let records = self.inner.decode(bytes).map_err(|e| e.to_string())?;
        let Some(origins) = self.origins.as_mut() else {
            return Ok(records);
        };
        origins.expire(received_at, false);
        let mut output = Vec::new();
        for record in records {
            origins.record(record, received_at, &mut output);
        }
        Ok(output)
    }

    fn original_record(&self, bytes: &[u8]) -> Option<Value> {
        if self.origins.is_some() {
            return None;
        }
        self.inner.decode(bytes).ok()?.pop()
    }

    fn take_rejections(&mut self, now: DateTime<Utc>, force: bool) -> Option<(&'static str, u64)> {
        let origins = self.origins.as_mut()?;
        origins.expire(now, force);
        let unresolved = origins.take_unresolved();
        (unresolved > 0).then_some(("sapient_bearing_origin_unresolved", unresolved))
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
    fn only_a_stateless_decoder_confirms_original_frame_evidence() {
        let plugin = Sapient::new();
        let bytes = [0x12, 0x01, b'x'];
        let direct = plugin.decoder(&Value::Null).unwrap();
        assert_eq!(direct.original_record(&bytes).unwrap()["node_id"], "x");
        let stateful = plugin
            .decoder(&serde_json::json!({"resolve_bearing_origin": true}))
            .unwrap();
        assert!(stateful.original_record(&bytes).is_none());
    }

    #[test]
    fn decoder_accepts_defaults_and_rejects_unknown_options() {
        let s = Sapient::new();
        assert!(s.decoder(&Value::Null).is_ok());
        let e = s
            .decoder(&serde_json::json!({"unknown": true}))
            .err()
            .unwrap();
        assert!(e.contains("unknown field"), "{e}");
        assert!(s.decoder(&serde_json::json!({})).is_ok());
    }
}
