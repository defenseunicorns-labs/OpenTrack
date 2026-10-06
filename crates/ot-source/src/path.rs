//! Field paths into decoded records.
//!
//! Syntax: dot-separated keys with optional array indexes, e.g.
//! `Message.PositionReport.Latitude`, `ac[0].hex`, `event.point.@lat`.
//! A key containing dots or brackets is quoted: `attrs["a.b"]`.
//! An empty path or `$` addresses the whole record.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Key(String),
    Index(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Path {
    raw: String,
    segments: Vec<Segment>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid path {path:?}: {reason}")]
pub struct PathError {
    pub path: String,
    pub reason: &'static str,
}

impl Path {
    pub fn root() -> Self {
        Self {
            raw: "$".into(),
            segments: Vec::new(),
        }
    }

    pub fn get<'v>(&self, value: &'v Value) -> Option<&'v Value> {
        let mut cur = value;
        for seg in &self.segments {
            cur = match (seg, cur) {
                (Segment::Key(k), Value::Object(map)) => map.get(k)?,
                (Segment::Index(i), Value::Array(items)) => items.get(*i)?,
                _ => return None,
            };
        }
        Some(cur)
    }

    /// Set a value, creating intermediate objects. Array indexes must exist.
    pub fn set(&self, target: &mut Value, value: Value) -> bool {
        let mut cur = target;
        for (i, seg) in self.segments.iter().enumerate() {
            let last = i + 1 == self.segments.len();
            match seg {
                Segment::Key(k) => {
                    if !cur.is_object() {
                        *cur = Value::Object(Default::default());
                    }
                    let map = cur.as_object_mut().expect("object");
                    if last {
                        map.insert(k.clone(), value);
                        return true;
                    }
                    cur = map.entry(k.clone()).or_insert(Value::Null);
                }
                Segment::Index(idx) => {
                    let Some(slot) = cur.as_array_mut().and_then(|a| a.get_mut(*idx)) else {
                        return false;
                    };
                    if last {
                        *slot = value;
                        return true;
                    }
                    cur = slot;
                }
            }
        }
        *cur = value;
        true
    }

    pub fn as_str(&self) -> &str {
        &self.raw
    }
}

impl FromStr for Path {
    type Err = PathError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let err = |reason| PathError {
            path: raw.to_owned(),
            reason,
        };
        let s = raw.trim();
        let s = s.strip_prefix('$').unwrap_or(s);
        let s = s.strip_prefix('.').unwrap_or(s);
        let bytes = s.as_bytes();
        let mut segments = Vec::new();
        let mut i = 0;
        let mut key = String::new();
        let flush = |key: &mut String, segments: &mut Vec<Segment>| {
            if !key.is_empty() {
                segments.push(Segment::Key(std::mem::take(key)));
            }
        };
        while i < bytes.len() {
            match bytes[i] {
                b'.' => {
                    // A dot must follow a key or a closing bracket.
                    if key.is_empty() && !(i > 0 && bytes[i - 1] == b']') {
                        return Err(err("empty segment"));
                    }
                    flush(&mut key, &mut segments);
                    i += 1;
                    if i == bytes.len() {
                        return Err(err("trailing dot"));
                    }
                }
                b'[' => {
                    flush(&mut key, &mut segments);
                    let close = s[i..].find(']').ok_or_else(|| err("unclosed ["))? + i;
                    let inner = s[i + 1..close].trim();
                    if let Some(q) = inner
                        .strip_prefix('"')
                        .and_then(|x| x.strip_suffix('"'))
                        .or_else(|| inner.strip_prefix('\'').and_then(|x| x.strip_suffix('\'')))
                    {
                        segments.push(Segment::Key(q.to_owned()));
                    } else {
                        let idx = inner
                            .parse()
                            .map_err(|_| err("index must be a number or quoted key"))?;
                        segments.push(Segment::Index(idx));
                    }
                    i = close + 1;
                }
                _ => {
                    let ch = s[i..].chars().next().expect("char");
                    key.push(ch);
                    i += ch.len_utf8();
                }
            }
        }
        flush(&mut key, &mut segments);
        Ok(Self {
            raw: raw.trim().to_owned(),
            segments,
        })
    }
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.raw)
    }
}

impl Serialize for Path {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.raw)
    }
}

impl<'de> Deserialize<'de> for Path {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn p(s: &str) -> Path {
        s.parse().unwrap()
    }

    #[test]
    fn reads_nested_keys_indexes_and_quoted_keys() {
        let v = json!({"a": {"b": [ {"c": 1}, {"c": 2} ]}, "x.y": 3, "event": {"@uid": "u"}});
        assert_eq!(p("a.b[1].c").get(&v), Some(&json!(2)));
        assert_eq!(p("$.a.b[0].c").get(&v), Some(&json!(1)));
        assert_eq!(p("[\"x.y\"]").get(&v), Some(&json!(3)));
        assert_eq!(p("event.@uid").get(&v), Some(&json!("u")));
        assert_eq!(p("$").get(&v), Some(&v));
        assert_eq!(p("a.b[9].c").get(&v), None);
        assert_eq!(p("a.z").get(&v), None);
    }

    #[test]
    fn rejects_malformed_paths() {
        for bad in ["a..b", "a.", "a[1", "a[x]", ".."] {
            assert!(bad.parse::<Path>().is_err(), "{bad}");
        }
    }

    #[test]
    fn sets_creating_objects() {
        let mut v = json!({});
        assert!(p("kinematics.speed_mps").set(&mut v, json!(5.0)));
        assert_eq!(v, json!({"kinematics": {"speed_mps": 5.0}}));
    }
}
