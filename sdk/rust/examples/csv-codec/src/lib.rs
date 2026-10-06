//! An example codec plugin: comma-separated reports, one per line,
//! `key,time,lat,lon[,course,speed]` (time as ISO 8601 or seconds since the
//! epoch). Each line becomes a record `{"key", "time", "lat", "lon",
//! "course", "speed"}` for a source's mapping. Lines starting with `#` and
//! a header line (`key,...`) are skipped.
//!
//! Options: `delimiter` (","), `strict` (false: skip bad lines; true: fail
//! the frame).

use opentrack_plugin::{Codec, Value, json, plugin};

struct Csv {
    delimiter: char,
    strict: bool,
    lines: u64,
}

fn field(parts: &[&str], i: usize) -> Option<f64> {
    parts.get(i).and_then(|s| s.trim().parse().ok())
}

impl Codec for Csv {
    fn open(options: Value) -> Result<Self, String> {
        let delimiter = match options.get("delimiter").and_then(Value::as_str) {
            None => ',',
            Some(d) if d.chars().count() == 1 => d.chars().next().unwrap_or(','),
            Some(d) => return Err(format!("delimiter {d:?}: one character")),
        };
        Ok(Self {
            delimiter,
            strict: options.get("strict").and_then(Value::as_bool).unwrap_or(false),
            lines: 0,
        })
    }

    fn decode(&mut self, frame: &[u8], _received_at_ms: i64) -> Result<Vec<Value>, String> {
        let text = std::str::from_utf8(frame).map_err(|e| format!("not UTF-8: {e}"))?;
        let mut out = Vec::new();
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            self.lines += 1;
            let parts: Vec<&str> = line.split(self.delimiter).collect();
            if parts[0].trim().eq_ignore_ascii_case("key") {
                continue;
            }
            let (Some(lat), Some(lon)) = (field(&parts, 2), field(&parts, 3)) else {
                if self.strict {
                    return Err(format!("line {}: no position", self.lines));
                }
                continue;
            };
            let time = parts.get(1).map(|t| t.trim()).unwrap_or_default();
            let time: Value = match time.parse::<f64>() {
                Ok(secs) => json!(secs),
                Err(_) => json!(time),
            };
            out.push(json!({
                "key": parts[0].trim(), "time": time, "lat": lat, "lon": lon,
                "course": field(&parts, 4), "speed": field(&parts, 5),
            }));
        }
        Ok(out)
    }
}

plugin! {
    manifest: json!({
        "name": "csv-reports",
        "version": "1",
        "description": "Example codec: key,time,lat,lon[,course,speed] lines",
        "options": [
            {"name": "delimiter", "label": "Delimiter", "type": "text", "default": ",",
             "help": "The character between fields"},
            {"name": "strict", "label": "Strict", "type": "bool", "default": false,
             "help": "Fail a frame with a bad line instead of skipping the line"}
        ],
        "framing": {"type": "delimiter", "delimiter": "\n"}
    }),
    codec: Csv,
}
