//! The per-minute metrics, mirrored to OpenTelemetry.
//!
//! Every counter and gauge the roles write to Redis (see
//! [`RedisStore::incr_metrics`](crate::redis_store::RedisStore::incr_metrics))
//! is also recorded on the global meter, which exports them over OTLP when
//! the process set one up and does nothing otherwise. A field is instrument
//! `opentrack.<name>` with attribute `opentrack.component` (a source id, or
//! `_engine`, `_writer`, `_cot`, `_system`); a per-output field
//! `<name>:<id>` is `opentrack.<name>` with `opentrack.output = <id>`.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Gauge, Meter};

fn meter() -> &'static Meter {
    static METER: OnceLock<Meter> = OnceLock::new();
    METER.get_or_init(|| opentelemetry::global::meter("opentrack"))
}

/// Instrument name and attributes for a metrics field.
fn instrument(component: &str, field: &str) -> (String, Vec<KeyValue>) {
    let (name, output) = match field.split_once(':') {
        Some((name, id)) => (name, Some(id)),
        None => (field, None),
    };
    let name: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut attrs = vec![KeyValue::new("opentrack.component", component.to_owned())];
    if let Some(id) = output {
        attrs.push(KeyValue::new("opentrack.output", id.to_owned()));
    }
    (format!("opentrack.{name}"), attrs)
}

/// Add to counters (fields as [`RedisStore::incr_metrics`] takes them).
pub fn count(component: &str, counts: &[(String, u64)]) {
    static COUNTERS: OnceLock<Mutex<HashMap<String, Counter<u64>>>> = OnceLock::new();
    let mut counters = COUNTERS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    for (field, n) in counts {
        let (name, attrs) = instrument(component, field);
        counters
            .entry(name.clone())
            .or_insert_with(|| meter().u64_counter(name).build())
            .add(*n, &attrs);
    }
}

/// Set gauges (fields as [`RedisStore::set_gauges`] takes them).
pub fn gauge(component: &str, gauges: &[(&str, u64)]) {
    static GAUGES: OnceLock<Mutex<HashMap<String, Gauge<u64>>>> = OnceLock::new();
    let mut all = GAUGES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    for (field, v) in gauges {
        let (name, attrs) = instrument(component, field);
        all.entry(name.clone())
            .or_insert_with(|| meter().u64_gauge(name).build())
            .record(*v, &attrs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_per_output_field_is_one_instrument_with_the_output_as_attribute() {
        let (name, attrs) = instrument("_cot", "sent:tak-1");
        assert_eq!(name, "opentrack.sent");
        assert_eq!(attrs[0], KeyValue::new("opentrack.component", "_cot"));
        assert_eq!(attrs[1], KeyValue::new("opentrack.output", "tak-1"));
        let (name, attrs) = instrument("ais", "decode errors");
        assert_eq!(name, "opentrack.decode_errors");
        assert_eq!(attrs.len(), 1);
    }
}
