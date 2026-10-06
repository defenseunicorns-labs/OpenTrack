//! STANAG 4607 (NATO GMTI format) as a codec plugin, on the
//! `ot-codec-stanag4607` decoder. Options are the decoder's (`emit`, and the
//! fallback measurement errors `range_sigma_m` and `cross_range_sigma_deg`
//! behind each target's `ground_error`), plus
//! `publish_platform`: also emit the sensor platform's own position from
//! dwell and platform location segments as `platform` records (the example
//! source maps them with a `track` rule, a friendly track that bypasses the
//! tracker), and
//! `shift_to_now`: move every record's `time` so the stream's first record is
//! now, to replay recordings as if live (the rest keep their spacing). When a
//! shifted time lands more than [`REANCHOR`] from the clock (a replay that
//! skips a gap in the recording, or starts over), the shift is taken again.

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use super::{Kind, Manifest, OptionKind, Plugin, PluginDecoder, PluginOption, StreamHints};
use crate::frame::{Endian, Framing, PrefixWidth};

pub struct Stanag4607 {
    manifest: Manifest,
}

/// How far a shifted time may be from the clock before the shift is retaken.
const REANCHOR: chrono::TimeDelta = chrono::TimeDelta::seconds(10);

impl Stanag4607 {
    pub fn new() -> Self {
        // The packet header's size field: a big-endian u32 at byte 2, counting the whole packet.
        let framing = Some(Framing::LengthField {
            offset: 2,
            width: PrefixWidth::U32,
            endian: Endian::Big,
            adjust: 0,
            max_len: crate::frame::DEFAULT_MAX_FRAME,
        });
        let options = {
            vec![
            PluginOption {
                name: "emit".into(),
                label: "Records".into(),
                help: "targets: one per dwell target report; segments: one per segment; all: both".into(),
                kind: OptionKind::Choice {
                    choices: ["targets", "segments", "all"].map(String::from).to_vec(),
                    default: "targets".into(),
                },
            },
            PluginOption {
                name: "publish_platform".into(),
                label: "Publish platform as track".into(),
                help: "The sensor platform's position from dwell and platform location segments, as `platform` records for a track rule (a friendly track that bypasses the tracker)".into(),
                kind: OptionKind::Bool { default: false },
            },
            PluginOption {
                name: "range_sigma_m".into(),
                label: "Range error σ (m)".into(),
                help: "One-sigma slant range error, for each target's ground error ellipse. Applies only when the target reports and job definitions give no accuracy".into(),
                kind: OptionKind::Number {
                    default: Some(20.0),
                    unit: "m".into(),
                    min: Some(0.0),
                },
            },
            PluginOption {
                name: "cross_range_sigma_deg".into(),
                label: "Cross-range error σ (°)".into(),
                help: "One-sigma cross-range (azimuth) error, for each target's ground error ellipse. Applies only when the target reports and job definitions give no accuracy".into(),
                kind: OptionKind::Number {
                    default: Some(0.2),
                    unit: "°".into(),
                    min: Some(0.0),
                },
            },
            PluginOption {
                name: "shift_to_now".into(),
                label: "Replay as live".into(),
                help: "Shift record times so the stream starts now (for recordings)".into(),
                kind: OptionKind::Bool { default: false },
            },
        ]
        };
        Self {
            manifest: Manifest {
                name: "stanag4607".into(),
                version: ot_codec_stanag4607::VERSION.into(),
                description: "STANAG 4607 GMTI (Edition 3): one record per dwell target report, or per segment".into(),
                kinds: vec![Kind::Codec],
                options,
                framing,
            },
        }
    }
}

impl Default for Stanag4607 {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for Stanag4607 {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn decoder(&self, options: &Value) -> Result<Box<dyn PluginDecoder>, String> {
        let mut opts = match options {
            Value::Null => serde_json::Map::new(),
            Value::Object(m) => m.clone(),
            other => return Err(format!("stanag4607 options must be an object, got {other}")),
        };
        let shift = match opts.remove("shift_to_now") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(b)) => b,
            Some(other) => return Err(format!("shift_to_now must be true or false, got {other}")),
        };
        let platform = match opts.remove("publish_platform") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(b)) => b,
            Some(other) => {
                return Err(format!(
                    "publish_platform must be true or false, got {other}"
                ));
            }
        };
        let range_sigma = positive(&mut opts, "range_sigma_m")?;
        let cross_range_sigma = positive(&mut opts, "cross_range_sigma_deg")?;
        if opts.contains_key("platform") {
            return Err("use publish_platform, not platform".into());
        }
        let mut options: ot_codec_stanag4607::Options = serde_json::from_value(Value::Object(opts))
            .map_err(|e| format!("stanag4607 options: {e}"))?;
        options.platform = platform;
        options.range_sigma_m = range_sigma;
        options.cross_range_sigma_deg = cross_range_sigma;
        Ok(Box::new(Decoder {
            inner: ot_codec_stanag4607::Decoder::new(options),
            shift,
            offset: None,
        }))
    }
}

/// A numeric option that must be a positive number when given.
fn positive(opts: &mut serde_json::Map<String, Value>, name: &str) -> Result<Option<f64>, String> {
    match opts.remove(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => match n.as_f64() {
            Some(v) if v.is_finite() && v > 0.0 => Ok(Some(v)),
            _ => Err(format!("{name} must be a positive number, got {n}")),
        },
        Some(other) => Err(format!("{name} must be a positive number, got {other}")),
    }
}

struct Decoder {
    inner: ot_codec_stanag4607::Decoder,
    shift: bool,
    /// Now minus the first record's time, once seen.
    offset: Option<chrono::Duration>,
}

impl PluginDecoder for Decoder {
    fn decode(&mut self, bytes: &[u8], _received_at: DateTime<Utc>) -> Result<Vec<Value>, String> {
        let mut records = self.inner.decode(bytes).map_err(|e| e.to_string())?;
        if self.shift {
            for r in &mut records {
                let Some(t) = r
                    .get("time")
                    .and_then(Value::as_str)
                    .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                    .map(|t| t.with_timezone(&Utc))
                else {
                    continue;
                };
                let now = Utc::now();
                let offset = match self.offset {
                    Some(o) if (t + o - now).abs() <= REANCHOR => o,
                    _ => *self.offset.insert(now - t),
                };
                r["time"] =
                    json!((t + offset).to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
            }
        }
        Ok(records)
    }

    fn hints(&self) -> Option<StreamHints> {
        use ot_codec_stanag4607::RevisitSource;
        self.inner.revisit().map(|r| StreamHints {
            revisit_secs: r.period_s,
            revisit_source: match r.source {
                RevisitSource::Measured => "measured",
                RevisitSource::Nominal => "nominal",
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sigma_options_have_defaults_and_are_checked() {
        let d = super::super::default_options(Stanag4607::new().manifest());
        assert_eq!(d["range_sigma_m"], 20.0);
        assert_eq!(d["cross_range_sigma_deg"], 0.2);
        let described = super::super::describe(&Stanag4607::new());
        let range = described["options"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["name"] == "range_sigma_m")
            .unwrap();
        assert_eq!(range["type"], "number");
        assert_eq!(range["default"], 20.0);
        assert_eq!(range["unit"], "m");

        assert!(Stanag4607::new().decoder(&d).is_ok());
        assert!(
            Stanag4607::new()
                .decoder(&json!({ "range_sigma_m": null }))
                .is_ok()
        );
        for bad in [json!("20"), json!(0), json!(-1.5), json!(true)] {
            let e = Stanag4607::new()
                .decoder(&json!({ "range_sigma_m": bad }))
                .err()
                .unwrap();
            assert!(e.contains("range_sigma_m must be a positive number"), "{e}");
        }
        let e = Stanag4607::new()
            .decoder(&json!({ "cross_range_sigma_deg": "x" }))
            .err()
            .unwrap();
        assert!(e.contains("cross_range_sigma_deg"), "{e}");
    }
}
