//! Value expressions and conditions used by mappings and filters.
//!
//! Both are plain JSON so the UI can build and display them. A value spec is
//! either a path string (`"Message.PositionReport.Sog"`) or an object:
//!
//! ```json
//! { "path": "Sog", "null_if": [102.3], "transforms": ["knots_to_mps", "round:2"] }
//! { "first": ["flight", "r", "hex"], "transforms": ["trim", "nonempty"] }
//! { "template": "a-{aff}-A", "default": "a-u-A" }
//! { "path": "t", "table": { "B738": "civil" }, "table_default": "unknown" }
//! { "const": "surface" }
//! { "cases": [ { "when": { "path": "nac_p", "gte": 10 }, "then": { "const": 0.95 } } ],
//!   "default": 0.5 }
//! { "arith": "sub", "args": [ { "path": "_frame.now", "transforms": ["scale:0.001"] }, "seen_pos" ] }
//! { "path": "hex", "transforms": ["hex"], "ranges": [ [10485760, 11534335, "US"] ] }
//! ```
//!
//! Evaluation order: source (`path` | `const` | `first` | `template` |
//! `cases` | `arith`), then `null_if`, `transforms` in order, `table`,
//! `ranges`, `range`, and finally `default` when the result is null.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::path::Path;

pub const KNOTS_TO_MPS: f64 = 0.514_444;
pub const FEET_TO_M: f64 = 0.3048;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ValueSpec {
    Path(Path),
    Full(Box<FullSpec>),
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FullSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<Path>,
    /// Index the source value (an object) by this key, e.g. `{"path":
    /// "Message", "key": "MessageType"}` reads `Message[<MessageType>]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<Box<ValueSpec>>,
    #[serde(default, rename = "const", skip_serializing_if = "Option::is_none")]
    pub constant: Option<Value>,
    /// The first of these that is not null.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first: Option<Vec<ValueSpec>>,
    /// String with `{path}` placeholders; null if any placeholder is missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    /// The `then` of the first case whose `when` holds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cases: Option<Vec<Case>>,
    /// Arithmetic over `args` (all must be numbers, else null).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arith: Option<ArithOp>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<ValueSpec>,
    /// Values treated as "not available" (feed sentinels such as heading 511).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub null_if: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transforms: Vec<Transform>,
    /// Lookup table keyed by the value's string form.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table: Option<BTreeMap<String, Value>>,
    /// Result for values missing from `table` (otherwise null).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table_default: Option<Value>,
    /// Numeric range table: `[low, high, result]` rows, inclusive; the
    /// narrowest matching row wins. Non-matching values become null.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ranges: Vec<(f64, f64, Value)>,
    /// Numeric bounds; values outside become null.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<[f64; 2]>,
    /// Result when everything above yields null.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Case {
    pub when: Condition,
    pub then: ValueSpec,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArithOp {
    Add,
    Sub,
    Mul,
    Div,
    Min,
    Max,
}

/// A value transform, written as a string: `knots_to_mps`, `scale:0.5`,
/// `round:2`, `split: :0`, `regex:^(\d+)`...
#[derive(Debug, Clone, PartialEq)]
pub enum Transform {
    KnotsToMps,
    FeetToM,
    /// Feet per minute to metres per second.
    FpmToMps,
    KmhToMps,
    NmToM,
    Scale(f64),
    Offset(f64),
    Round(u32),
    /// Normalise an angle into [0, 360).
    Wrap360,
    Trim,
    Upper,
    Lower,
    /// Empty (after trimming) strings become null.
    Nonempty,
    ToString,
    ToNumber,
    ToBool,
    /// Parse a time: RFC 3339, common "YYYY-MM-DD hh:mm:ss[.f] [+zzzz] [UTC]"
    /// forms, or a Unix epoch number (seconds, or milliseconds if large).
    Time,
    TimeUnixS,
    TimeUnixMs,
    /// Split a string and take one part (negative counts from the end).
    Split(String, i64),
    /// First capture group that participated in the match (or the whole
    /// match if the pattern has no groups).
    Regex(RegexCell),
    /// Parse a hexadecimal string (optional `0x`) as an integer.
    Hex,
    /// Replace every occurrence of a substring: `replace:@: ` (from `@` to a space).
    Replace(String, String),
    /// Integer bit test: true if `value & mask != 0`.
    BitsAny(u64),
}

/// A compiled regex that compares and serialises by its pattern.
#[derive(Debug, Clone)]
pub struct RegexCell(pub Regex);

impl PartialEq for RegexCell {
    fn eq(&self, other: &Self) -> bool {
        self.0.as_str() == other.0.as_str()
    }
}

impl Transform {
    pub fn parse(s: &str) -> Result<Self, String> {
        let (name, arg) = match s.split_once(':') {
            Some((n, a)) => (n.trim(), Some(a)),
            None => (s.trim(), None),
        };
        let num = |a: Option<&str>| -> Result<f64, String> {
            a.ok_or_else(|| format!("{name} needs an argument"))?
                .trim()
                .parse()
                .map_err(|_| format!("{name}: argument must be a number"))
        };
        Ok(match name {
            "knots_to_mps" => Transform::KnotsToMps,
            "feet_to_m" => Transform::FeetToM,
            "fpm_to_mps" => Transform::FpmToMps,
            "kmh_to_mps" => Transform::KmhToMps,
            "nm_to_m" => Transform::NmToM,
            "scale" => Transform::Scale(num(arg)?),
            "offset" => Transform::Offset(num(arg)?),
            "round" => Transform::Round(num(arg)? as u32),
            "wrap360" => Transform::Wrap360,
            "trim" => Transform::Trim,
            "upper" => Transform::Upper,
            "lower" => Transform::Lower,
            "nonempty" => Transform::Nonempty,
            "string" => Transform::ToString,
            "number" => Transform::ToNumber,
            "bool" => Transform::ToBool,
            "time" => Transform::Time,
            "time_unix_s" => Transform::TimeUnixS,
            "time_unix_ms" => Transform::TimeUnixMs,
            "bits_any" => Transform::BitsAny(num(arg)? as u64),
            "hex" => Transform::Hex,
            "replace" => {
                let a = arg.ok_or("replace needs from:to")?;
                let (from, to) = a.split_once(':').ok_or("replace needs from:to")?;
                if from.is_empty() {
                    return Err("replace: 'from' must not be empty".into());
                }
                Transform::Replace(from.to_owned(), to.to_owned())
            }
            "split" => {
                let a = arg.ok_or("split needs sep:index")?;
                let (sep, idx) = a.rsplit_once(':').ok_or("split needs sep:index")?;
                let idx = idx
                    .trim()
                    .parse()
                    .map_err(|_| "split index must be an integer")?;
                Transform::Split(sep.to_owned(), idx)
            }
            "regex" => {
                let pat = arg.ok_or("regex needs a pattern")?;
                Transform::Regex(RegexCell(
                    Regex::new(pat).map_err(|e| format!("regex: {e}"))?,
                ))
            }
            other => return Err(format!("unknown transform {other:?}")),
        })
    }

    fn render(&self) -> String {
        match self {
            Transform::KnotsToMps => "knots_to_mps".into(),
            Transform::FeetToM => "feet_to_m".into(),
            Transform::FpmToMps => "fpm_to_mps".into(),
            Transform::KmhToMps => "kmh_to_mps".into(),
            Transform::NmToM => "nm_to_m".into(),
            Transform::Scale(f) => format!("scale:{f}"),
            Transform::Offset(f) => format!("offset:{f}"),
            Transform::Round(n) => format!("round:{n}"),
            Transform::Wrap360 => "wrap360".into(),
            Transform::Trim => "trim".into(),
            Transform::Upper => "upper".into(),
            Transform::Lower => "lower".into(),
            Transform::Nonempty => "nonempty".into(),
            Transform::ToString => "string".into(),
            Transform::ToNumber => "number".into(),
            Transform::ToBool => "bool".into(),
            Transform::Time => "time".into(),
            Transform::TimeUnixS => "time_unix_s".into(),
            Transform::TimeUnixMs => "time_unix_ms".into(),
            Transform::Split(sep, i) => format!("split:{sep}:{i}"),
            Transform::Regex(r) => format!("regex:{}", r.0.as_str()),
            Transform::BitsAny(m) => format!("bits_any:{m}"),
            Transform::Hex => "hex".into(),
            Transform::Replace(from, to) => format!("replace:{from}:{to}"),
        }
    }

    fn apply(&self, v: Value) -> Value {
        if v.is_null() {
            return v;
        }
        let f = |x: f64| number(x);
        match self {
            Transform::KnotsToMps => as_f64(&v).map_or(Value::Null, |x| f(x * KNOTS_TO_MPS)),
            Transform::FeetToM => as_f64(&v).map_or(Value::Null, |x| f(x * FEET_TO_M)),
            Transform::FpmToMps => as_f64(&v).map_or(Value::Null, |x| f(x * FEET_TO_M / 60.0)),
            Transform::KmhToMps => as_f64(&v).map_or(Value::Null, |x| f(x / 3.6)),
            Transform::NmToM => as_f64(&v).map_or(Value::Null, |x| f(x * 1852.0)),
            Transform::Scale(k) => as_f64(&v).map_or(Value::Null, |x| f(x * k)),
            Transform::Offset(k) => as_f64(&v).map_or(Value::Null, |x| f(x + k)),
            Transform::Round(n) => as_f64(&v).map_or(Value::Null, |x| {
                let p = 10f64.powi(*n as i32);
                f((x * p).round() / p)
            }),
            Transform::Wrap360 => as_f64(&v).map_or(Value::Null, |x| f(x.rem_euclid(360.0))),
            Transform::Trim => map_str(v, |s| s.trim().to_owned()),
            Transform::Upper => map_str(v, |s| s.to_uppercase()),
            Transform::Lower => map_str(v, |s| s.to_lowercase()),
            Transform::Nonempty => match &v {
                Value::String(s) if s.trim().is_empty() => Value::Null,
                _ => v,
            },
            Transform::ToString => Value::String(as_string(&v)),
            Transform::ToNumber => as_f64(&v).map_or(Value::Null, f),
            Transform::ToBool => match &v {
                Value::Bool(_) => v,
                Value::Number(n) => Value::Bool(n.as_f64().is_some_and(|x| x != 0.0)),
                Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
                    "true" | "1" | "yes" | "y" | "t" => Value::Bool(true),
                    "false" | "0" | "no" | "n" | "f" | "" => Value::Bool(false),
                    _ => Value::Null,
                },
                _ => Value::Null,
            },
            Transform::Time => parse_time(&v).map_or(Value::Null, time_value),
            Transform::TimeUnixS => as_f64(&v)
                .and_then(|s| Utc.timestamp_millis_opt((s * 1000.0) as i64).single())
                .map_or(Value::Null, time_value),
            Transform::TimeUnixMs => as_f64(&v)
                .and_then(|ms| Utc.timestamp_millis_opt(ms as i64).single())
                .map_or(Value::Null, time_value),
            Transform::Split(sep, idx) => {
                let s = as_string(&v);
                let parts: Vec<&str> = s.split(sep.as_str()).collect();
                let i = if *idx < 0 {
                    parts.len() as i64 + idx
                } else {
                    *idx
                };
                usize::try_from(i)
                    .ok()
                    .and_then(|i| parts.get(i))
                    .map_or(Value::Null, |p| Value::String((*p).to_owned()))
            }
            Transform::Regex(re) => {
                let s = as_string(&v);
                re.0.captures(&s).map_or(Value::Null, |c| {
                    let m = c
                        .iter()
                        .skip(1)
                        .flatten()
                        .next()
                        .or_else(|| c.get(0))
                        .expect("match");
                    Value::String(m.as_str().to_owned())
                })
            }
            Transform::BitsAny(mask) => {
                as_f64(&v).map_or(Value::Null, |x| Value::Bool((x as u64) & mask != 0))
            }
            Transform::Replace(from, to) => map_str(v, |s| s.replace(from.as_str(), to)),
            Transform::Hex => {
                let s = as_string(&v);
                let s = s.trim();
                let s = s
                    .strip_prefix("0x")
                    .or_else(|| s.strip_prefix("0X"))
                    .unwrap_or(s);
                u64::from_str_radix(s, 16).map_or(Value::Null, |n| json!(n))
            }
        }
    }
}

impl Serialize for Transform {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.render())
    }
}

impl<'de> Deserialize<'de> for Transform {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Transform::parse(&String::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

impl ValueSpec {
    pub fn path(p: &str) -> Self {
        ValueSpec::Path(p.parse().expect("valid path"))
    }

    pub fn eval(&self, record: &Value) -> Value {
        match self {
            ValueSpec::Path(p) => p.get(record).cloned().unwrap_or(Value::Null),
            ValueSpec::Full(spec) => spec.eval(record),
        }
    }

    /// Paths this spec reads, for probe coverage and mapping suggestions.
    pub fn paths(&self, out: &mut Vec<String>) {
        match self {
            ValueSpec::Path(p) => out.push(p.to_string()),
            ValueSpec::Full(s) => {
                if let Some(p) = &s.path {
                    out.push(p.to_string());
                }
                for f in s.first.iter().flatten() {
                    f.paths(out);
                }
                if let Some(t) = &s.template {
                    out.extend(placeholders(t).map(str::to_owned));
                }
                for c in s.cases.iter().flatten() {
                    c.then.paths(out);
                }
                for a in &s.args {
                    a.paths(out);
                }
            }
        }
    }
}

impl FullSpec {
    fn eval(&self, record: &Value) -> Value {
        let mut v = if let Some(p) = &self.path {
            p.get(record).cloned().unwrap_or(Value::Null)
        } else if let Some(c) = &self.constant {
            c.clone()
        } else if let Some(options) = &self.first {
            options
                .iter()
                .map(|o| o.eval(record))
                .find(|v| !is_blank(v))
                .unwrap_or(Value::Null)
        } else if let Some(t) = &self.template {
            render_template(t, record)
        } else if let Some(cases) = &self.cases {
            cases
                .iter()
                .find(|c| c.when.eval(record))
                .map_or(Value::Null, |c| c.then.eval(record))
        } else if let Some(op) = self.arith {
            arith(op, &self.args, record)
        } else {
            Value::Null
        };
        if let Some(key) = &self.key {
            let k = as_string(&key.eval(record));
            v = v.get(k.as_str()).cloned().unwrap_or(Value::Null);
        }
        if self.null_if.iter().any(|n| loose_eq(n, &v)) {
            v = Value::Null;
        }
        for t in &self.transforms {
            v = t.apply(v);
        }
        if let Some(table) = &self.table
            && !v.is_null()
        {
            v = table
                .get(&as_string(&v))
                .cloned()
                .or_else(|| self.table_default.clone())
                .unwrap_or(Value::Null);
        }
        if !self.ranges.is_empty() && !v.is_null() {
            v = as_f64(&v)
                .and_then(|x| {
                    self.ranges
                        .iter()
                        .filter(|(lo, hi, _)| x >= *lo && x <= *hi)
                        .min_by(|a, b| (a.1 - a.0).total_cmp(&(b.1 - b.0)))
                        .map(|(_, _, r)| r.clone())
                })
                .unwrap_or(Value::Null);
        }
        if let Some([lo, hi]) = self.range
            && as_f64(&v).is_some_and(|x| x < lo || x > hi)
        {
            v = Value::Null;
        }
        if v.is_null()
            && let Some(d) = &self.default
        {
            v = d.clone();
        }
        v
    }
}

fn arith(op: ArithOp, args: &[ValueSpec], record: &Value) -> Value {
    let Some(nums) = args
        .iter()
        .map(|a| as_f64(&a.eval(record)))
        .collect::<Option<Vec<f64>>>()
    else {
        return Value::Null;
    };
    let Some((&first, rest)) = nums.split_first() else {
        return Value::Null;
    };
    let out = rest.iter().try_fold(first, |acc, &x| match op {
        ArithOp::Add => Some(acc + x),
        ArithOp::Sub => Some(acc - x),
        ArithOp::Mul => Some(acc * x),
        ArithOp::Div => (x != 0.0).then(|| acc / x),
        ArithOp::Min => Some(acc.min(x)),
        ArithOp::Max => Some(acc.max(x)),
    });
    out.map_or(Value::Null, number)
}

fn placeholders(t: &str) -> impl Iterator<Item = &str> {
    t.split('{')
        .skip(1)
        .filter_map(|p| p.split_once('}').map(|(k, _)| k.trim()))
}

fn render_template(t: &str, record: &Value) -> Value {
    let mut out = String::with_capacity(t.len());
    let mut rest = t;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let Some(close) = rest[open..].find('}') else {
            out.push_str(&rest[open..]);
            return Value::String(out);
        };
        let key = rest[open + 1..open + close].trim();
        let Ok(path) = key.parse::<Path>() else {
            return Value::Null;
        };
        match path.get(record) {
            Some(v) if !is_blank(v) => out.push_str(&as_string(v)),
            _ => return Value::Null,
        }
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    Value::String(out)
}

fn is_blank(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::String(s) => s.trim().is_empty(),
        _ => false,
    }
}

fn map_str(v: Value, f: impl Fn(&str) -> String) -> Value {
    match v {
        Value::String(s) => Value::String(f(&s)),
        other => other,
    }
}

fn number(x: f64) -> Value {
    serde_json::Number::from_f64(x).map_or(Value::Null, Value::Number)
}

fn time_value(t: DateTime<Utc>) -> Value {
    json!(t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}

/// A number, or a string holding one.
pub fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        _ => None,
    }
    .filter(|x| x.is_finite())
}

/// String form used for tables, templates and comparisons.
pub fn as_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Number(n) => match (n.as_i64(), n.as_f64()) {
            (Some(i), _) => i.to_string(),
            (None, Some(f)) if f.fract() == 0.0 && f.abs() < 1e15 => format!("{}", f as i64),
            _ => n.to_string(),
        },
        other => other.to_string(),
    }
}

/// Equality that treats `1`, `1.0` and `"1"` as equal.
pub fn loose_eq(a: &Value, b: &Value) -> bool {
    if a == b {
        return true;
    }
    match (as_f64(a), as_f64(b)) {
        (Some(x), Some(y)) if !(a.is_string() && b.is_string()) => (x - y).abs() < 1e-9,
        _ => !a.is_null() && !b.is_null() && as_string(a) == as_string(b),
    }
}

/// Parse any supported time form into the canonical RFC 3339 string, or null.
pub fn parse_time_value(v: &Value) -> Value {
    parse_time(v).map_or(Value::Null, time_value)
}

fn parse_time(v: &Value) -> Option<DateTime<Utc>> {
    if let Value::Number(_) = v {
        let x = as_f64(v)?;
        // Heuristic: epoch milliseconds are > 1e11 for any date after 1973.
        let ms = if x.abs() > 1e11 { x } else { x * 1000.0 };
        return Utc.timestamp_millis_opt(ms as i64).single();
    }
    let s = v.as_str()?.trim();
    if let Ok(t) = DateTime::parse_from_rfc3339(s) {
        return Some(t.with_timezone(&Utc));
    }
    // e.g. aisstream "2026-09-24 12:00:00.123456789 +0000 UTC".
    static TRAILING: OnceLock<Regex> = OnceLock::new();
    let trailing = TRAILING.get_or_init(|| Regex::new(r"\s+(UTC|GMT|Z)$").expect("regex"));
    let s = trailing.replace(s, "");
    for fmt in [
        "%Y-%m-%d %H:%M:%S%.f %z",
        "%Y-%m-%dT%H:%M:%S%.f%z",
        "%Y-%m-%d %H:%M:%S%.f%:z",
    ] {
        if let Ok(t) = DateTime::parse_from_str(&s, fmt) {
            return Some(t.with_timezone(&Utc));
        }
    }
    for fmt in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S%.f"] {
        if let Ok(t) = NaiveDateTime::parse_from_str(&s, fmt) {
            return Some(t.and_utc());
        }
    }
    if let Ok(x) = s.parse::<f64>() {
        return parse_time(&json!(x));
    }
    None
}

/// A boolean test over a record or observation. Several operators in one test
/// must all hold. Combine tests with `all`, `any` and `not`.
///
/// ```json
/// { "all": [ { "path": "dbFlags", "bits_any": 1 },
///            { "not": { "path": "flight", "matches": "^TEST" } } ] }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Condition {
    All { all: Vec<Condition> },
    Any { any: Vec<Condition> },
    Not { not: Box<Condition> },
    Test(Box<Test>),
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Test {
    /// What to test: a path, or a full value spec (to transform first).
    #[serde(flatten)]
    pub value: TestValue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eq: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ne: Option<Value>,
    #[serde(default, rename = "in", skip_serializing_if = "Option::is_none")]
    pub in_: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_in: Option<Vec<Value>>,
    /// true: must be present and non-null; false: must be absent or null.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exists: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gt: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gte: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lt: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lte: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matches: Option<RegexString>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starts_with: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contains: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bits_any: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TestValue {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<Path>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<ValueSpec>,
}

/// A regex kept as its source string and compiled once.
#[derive(Debug, Clone)]
pub struct RegexString(pub RegexCell);

impl PartialEq for RegexString {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Serialize for RegexString {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.0.0.as_str())
    }
}

impl<'de> Deserialize<'de> for RegexString {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Regex::new(&s)
            .map(|r| RegexString(RegexCell(r)))
            .map_err(serde::de::Error::custom)
    }
}

impl Condition {
    pub fn eval(&self, record: &Value) -> bool {
        match self {
            Condition::All { all } => all.iter().all(|c| c.eval(record)),
            Condition::Any { any } => any.iter().any(|c| c.eval(record)),
            Condition::Not { not } => !not.eval(record),
            Condition::Test(t) => t.eval(record),
        }
    }
}

impl Test {
    fn eval(&self, record: &Value) -> bool {
        let v = match (&self.value.path, &self.value.value) {
            (Some(p), _) => p.get(record).cloned().unwrap_or(Value::Null),
            (None, Some(spec)) => spec.eval(record),
            (None, None) => Value::Null,
        };
        let num = as_f64(&v);
        let s = if v.is_null() {
            None
        } else {
            Some(as_string(&v))
        };
        self.exists.is_none_or(|want| want == !v.is_null())
            && self.eq.as_ref().is_none_or(|x| loose_eq(x, &v))
            && self.ne.as_ref().is_none_or(|x| !loose_eq(x, &v))
            && self
                .in_
                .as_ref()
                .is_none_or(|xs| xs.iter().any(|x| loose_eq(x, &v)))
            && self
                .not_in
                .as_ref()
                .is_none_or(|xs| !xs.iter().any(|x| loose_eq(x, &v)))
            && self.gt.is_none_or(|x| num.is_some_and(|n| n > x))
            && self.gte.is_none_or(|x| num.is_some_and(|n| n >= x))
            && self.lt.is_none_or(|x| num.is_some_and(|n| n < x))
            && self.lte.is_none_or(|x| num.is_some_and(|n| n <= x))
            && self
                .matches
                .as_ref()
                .is_none_or(|r| s.as_deref().is_some_and(|s| r.0.0.is_match(s)))
            && self
                .starts_with
                .as_ref()
                .is_none_or(|p| s.as_deref().is_some_and(|s| s.starts_with(p.as_str())))
            && self
                .contains
                .as_ref()
                .is_none_or(|p| s.as_deref().is_some_and(|s| s.contains(p.as_str())))
            && self
                .bits_any
                .is_none_or(|m| num.is_some_and(|n| (n as u64) & m != 0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(v: Value) -> ValueSpec {
        serde_json::from_value(v).unwrap()
    }

    fn cond(v: Value) -> Condition {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn path_shorthand_and_transforms() {
        let rec = json!({"Sog": 10.0, "TrueHeading": 511, "name": "  USS X  ", "alt": 1000});
        assert_eq!(spec(json!("Sog")).eval(&rec), json!(10.0));
        let v = spec(json!({"path": "Sog", "transforms": ["knots_to_mps", "round:3"]})).eval(&rec);
        assert_eq!(v, json!(5.144));
        assert_eq!(
            spec(json!({"path": "TrueHeading", "null_if": [511]})).eval(&rec),
            Value::Null
        );
        assert_eq!(
            spec(json!({"path": "name", "transforms": ["trim"]})).eval(&rec),
            json!("USS X")
        );
        assert_eq!(
            spec(json!({"path": "alt", "transforms": ["feet_to_m"]})).eval(&rec),
            json!(304.8)
        );
    }

    #[test]
    fn first_template_table_range_default() {
        let rec = json!({"flight": "  ", "r": "N123", "aff": "f", "t": "B738", "lat": 91});
        assert_eq!(
            spec(json!({"first": ["flight", "r"], "transforms": ["trim"]})).eval(&rec),
            json!("N123")
        );
        assert_eq!(
            spec(json!({"template": "a-{aff}-A"})).eval(&rec),
            json!("a-f-A")
        );
        assert_eq!(
            spec(json!({"template": "a-{missing}-A", "default": "a-u-A"})).eval(&rec),
            json!("a-u-A")
        );
        let t = json!({"path": "t", "table": {"B738": "airliner"}, "table_default": "other"});
        assert_eq!(spec(t).eval(&rec), json!("airliner"));
        assert_eq!(
            spec(json!({"path": "lat", "range": [-90, 90]})).eval(&rec),
            Value::Null
        );
        assert_eq!(spec(json!({"const": 7})).eval(&rec), json!(7));
    }

    #[test]
    fn cases_arith_ranges_hex_and_regex_groups() {
        let rec = json!({"nac_p": 9, "type": "adsb_icao", "now": 1_000_000, "seen_pos": 2.5,
                         "hex": "ae1234", "mmsi": "003669999"});
        let conf = json!({"cases": [
            {"when": {"path": "type", "in": ["mlat", "mode_s"]}, "then": {"const": 0.6}},
            {"when": {"path": "nac_p", "gte": 10}, "then": {"const": 0.95}},
            {"when": {"path": "nac_p", "gte": 8}, "then": {"const": 0.9}}
        ], "default": 0.5});
        assert_eq!(spec(conf).eval(&rec), json!(0.9));
        let t = json!({"arith": "sub", "args": [{"path": "now", "transforms": ["scale:0.001"]}, "seen_pos"]});
        assert_eq!(spec(t).eval(&rec), json!(997.5));
        let country = json!({"path": "hex", "transforms": ["hex"],
            "ranges": [[10485760, 11534335, "US"], [11403264, 11403519, "XX"]]});
        assert_eq!(spec(country).eval(&rec), json!("US"));
        // MID: the first group that matched.
        let mid = json!({"path": "mmsi", "transforms": [r"regex:^(?:00(\d{3})|0(\d{3})|(\d{3}))"]});
        assert_eq!(spec(mid).eval(&rec), json!("366"));
    }

    #[test]
    fn dynamic_keys_and_replace() {
        let rec = json!({"MessageType": "PositionReport",
                         "Message": {"PositionReport": {"Latitude": 1.5, "Name": "USS@X@@@"}}});
        let body = json!({"path": "Message", "key": "MessageType"});
        assert_eq!(spec(body).eval(&rec)["Latitude"], json!(1.5));
        let name =
            json!({"path": "Message.PositionReport.Name", "transforms": ["replace:@: ", "trim"]});
        assert_eq!(spec(name).eval(&rec), json!("USS X"));
        assert!(Transform::parse("replace::x").is_err());
    }

    #[test]
    fn times_in_common_feed_formats() {
        let t =
            |v: Value| spec(json!({"path": "t", "transforms": ["time"]})).eval(&json!({"t": v}));
        let want = json!("2026-09-24T12:00:00.123Z");
        assert_eq!(t(json!("2026-09-24T12:00:00.123Z")), want);
        assert_eq!(t(json!("2026-09-24 12:00:00.123456789 +0000 UTC")), want);
        assert_eq!(t(json!(1_790_251_200.123)), want);
        assert_eq!(t(json!(1_790_251_200_123_i64)), want);
        assert_eq!(t(json!("not a time")), Value::Null);
    }

    #[test]
    fn transform_strings_round_trip_and_reject_unknowns() {
        for s in [
            "knots_to_mps",
            "scale:0.5",
            "round:2",
            "split::-1",
            "regex:^(\\d+)",
            "bits_any:1",
        ] {
            let t = Transform::parse(s).unwrap();
            assert_eq!(Transform::parse(&t.render()).unwrap(), t);
        }
        assert!(Transform::parse("warp_speed").is_err());
        assert!(
            serde_json::from_value::<ValueSpec>(json!({"path": "a", "tranforms": []})).is_err()
        );
    }

    #[test]
    fn conditions() {
        let rec = json!({"dbFlags": 9, "flight": "TEST12", "type": "PositionReport", "speed": 3});
        assert!(cond(json!({"path": "dbFlags", "bits_any": 1})).eval(&rec));
        assert!(!cond(json!({"path": "dbFlags", "bits_any": 2})).eval(&rec));
        assert!(
            cond(json!({"path": "type", "in": ["PositionReport", "StandardClassBPositionReport"]}))
                .eval(&rec)
        );
        assert!(cond(json!({"not": {"path": "flight", "matches": "^CIV"}})).eval(&rec));
        assert!(cond(json!({"all": [{"path": "speed", "gt": 1, "lte": 3}, {"path": "nope", "exists": false}]})).eval(&rec));
        assert!(
            cond(json!({"any": [{"path": "speed", "eq": "3"}, {"path": "x", "exists": true}]}))
                .eval(&rec)
        );
        assert!(!cond(json!({"path": "missing", "gt": 0})).eval(&rec));
        assert!(
            cond(
                json!({"value": {"path": "flight", "transforms": ["lower"]}, "starts_with": "test"})
            )
            .eval(&rec)
        );
    }
}
