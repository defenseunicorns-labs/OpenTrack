use std::collections::{BTreeMap, VecDeque};

use chrono::{DateTime, TimeDelta, Utc};
use serde::Deserialize;
use serde_json::{Value, json};

const MAX_NODES: usize = 128;
const MAX_POSITIONS_PER_NODE: usize = 128;
const MAX_PENDING: usize = 2048;

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct Options {
    pub resolve_bearing_origin: bool,
    pub max_position_gap_secs: u32,
}

impl Options {
    pub fn wait(&self) -> TimeDelta {
        // Leave time for reordered delivery even when exact matching is required.
        TimeDelta::seconds(i64::from(self.max_position_gap_secs.max(2)))
    }
}

#[derive(Clone, Copy)]
struct Position {
    latitude: f64,
    longitude: f64,
}

impl Position {
    fn from_status(record: &Value) -> Option<Self> {
        let location = record.pointer("/status_report/node_location")?;
        match location["datum"].as_str()? {
            "LOCATION_DATUM_WGS84_E" | "LOCATION_DATUM_WGS84_G" => {}
            _ => return None,
        }
        let (mut longitude, mut latitude) = (location["x"].as_f64()?, location["y"].as_f64()?);
        match location["coordinate_system"].as_str()? {
            "LOCATION_COORDINATE_SYSTEM_LAT_LNG_DEG_M" => {}
            "LOCATION_COORDINATE_SYSTEM_LAT_LNG_RAD_M" => {
                longitude = longitude.to_degrees();
                latitude = latitude.to_degrees();
            }
            _ => return None,
        }
        ((-90.0..=90.0).contains(&latitude) && (-180.0..=180.0).contains(&longitude)).then_some(
            Self {
                latitude,
                longitude,
            },
        )
    }

    fn interpolate(self, next: Self, fraction: f64) -> Self {
        let delta = (next.longitude - self.longitude + 180.0).rem_euclid(360.0) - 180.0;
        Self {
            latitude: self.latitude + (next.latitude - self.latitude) * fraction,
            longitude: (self.longitude + delta * fraction + 180.0).rem_euclid(360.0) - 180.0,
        }
    }

    fn attach(self, mut record: Value) -> Value {
        record["sapient_sensor_position"] = json!({
            "latitude": self.latitude,
            "longitude": self.longitude,
        });
        record
    }
}

type Positions = BTreeMap<DateTime<Utc>, Position>;

struct Pending {
    node_id: String,
    observed_at: DateTime<Utc>,
    received_at: DateTime<Utc>,
    record: Value,
}

pub(super) struct Resolver {
    gap: TimeDelta,
    wait: TimeDelta,
    history: BTreeMap<String, Positions>,
    pending: VecDeque<Pending>,
    unresolved: u64,
}

impl Resolver {
    pub fn new(options: &Options) -> Self {
        Self {
            gap: TimeDelta::seconds(i64::from(options.max_position_gap_secs)),
            wait: options.wait(),
            history: BTreeMap::new(),
            pending: VecDeque::new(),
            unresolved: 0,
        }
    }

    pub fn record(&mut self, record: Value, received_at: DateTime<Utc>, output: &mut Vec<Value>) {
        let bearing = record.pointer("/detection_report/range_bearing").is_some();
        let position = Position::from_status(&record);
        if !bearing && position.is_none() {
            output.push(record);
            return;
        }
        let identity = record["node_id"].as_str().filter(|id| !id.is_empty());
        let timestamp = record["timestamp"]
            .as_str()
            .and_then(|time| time.parse::<DateTime<Utc>>().ok());
        let (Some(node_id), Some(observed_at)) = (identity, timestamp) else {
            if bearing {
                self.unresolved += 1;
            } else {
                output.push(record);
            }
            return;
        };

        if let Some(position) = position {
            self.store_position(node_id, observed_at, position, output);
            output.push(record);
        } else if let Some(origin) = self.position(node_id, observed_at) {
            output.push(origin.attach(record));
        } else {
            if self.pending.len() == MAX_PENDING {
                self.pending.pop_front();
                self.unresolved += 1;
            }
            self.pending.push_back(Pending {
                node_id: node_id.to_owned(),
                observed_at,
                received_at,
                record,
            });
        }
    }

    fn store_position(
        &mut self,
        node_id: &str,
        observed_at: DateTime<Utc>,
        position: Position,
        output: &mut Vec<Value>,
    ) {
        if !self.history.contains_key(node_id) && self.history.len() == MAX_NODES {
            let least_recent = self
                .history
                .iter()
                .min_by_key(|(_, positions)| positions.last_key_value().map(|(time, _)| *time))
                .map(|(node, _)| node.clone())
                .expect("history is full");
            self.history.remove(&least_recent);
        }
        let positions = self.history.entry(node_id.to_owned()).or_default();
        positions.insert(observed_at, position);
        if positions.len() > MAX_POSITIONS_PER_NODE {
            positions.pop_first();
        }
        let mut pending = std::mem::take(&mut self.pending);
        while let Some(report) = pending.pop_front() {
            if report.node_id == node_id
                && let Some(origin) = self.position(node_id, report.observed_at)
            {
                output.push(origin.attach(report.record));
            } else {
                self.pending.push_back(report);
            }
        }
    }

    fn position(&self, node_id: &str, time: DateTime<Utc>) -> Option<Position> {
        let positions = self.history.get(node_id)?;
        if let Some(position) = positions.get(&time) {
            return Some(*position);
        }
        if self.gap.is_zero() {
            return None;
        }
        let (before, first) = positions.range(..time).next_back()?;
        let (after, last) = positions.range(time..).next()?;
        let interval = *after - *before;
        if interval > self.gap {
            return None;
        }
        let fraction = (time - *before).as_seconds_f64() / interval.as_seconds_f64();
        Some(first.interpolate(*last, fraction))
    }

    pub fn expire(&mut self, now: DateTime<Utc>, force: bool) {
        let previous = self.pending.len();
        self.pending
            .retain(|report| !force && now - report.received_at < self.wait);
        self.unresolved += (previous - self.pending.len()) as u64;
    }

    pub fn take_unresolved(&mut self) -> u64 {
        std::mem::take(&mut self.unresolved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn time(second: u32) -> DateTime<Utc> {
        format!("2026-01-01T00:00:{second:02}Z").parse().unwrap()
    }

    fn bearing(node: &str, at: u32) -> Value {
        json!({
            "node_id":node, "timestamp":time(at),
            "detection_report":{"range_bearing":{"azimuth":47.5}}
        })
    }

    fn status(node: &str, at: u32, longitude: f64) -> Value {
        json!({
            "node_id":node, "timestamp":time(at),
            "status_report":{"node_location":{
                "x":longitude, "y":63.4, "datum":"LOCATION_DATUM_WGS84_G",
                "coordinate_system":"LOCATION_COORDINATE_SYSTEM_LAT_LNG_DEG_M"
            }}
        })
    }

    #[test]
    fn matches_exact_position_after_detection_and_uses_correct_node() {
        let mut resolver = Resolver::new(&Options::default());
        let mut output = Vec::new();
        resolver.record(bearing("ir", 2), time(2), &mut output);
        assert!(output.is_empty());
        resolver.record(status("eo", 2, 10.0), time(2), &mut output);
        assert_eq!(output.len(), 1);
        resolver.record(status("ir", 2, 10.5), time(3), &mut output);
        assert_eq!(output.len(), 3);
        assert_eq!(output[1]["sapient_sensor_position"]["longitude"], 10.5);
        assert_eq!(output[1]["timestamp"], bearing("ir", 2)["timestamp"]);
        assert_eq!(
            output[1]["detection_report"],
            bearing("ir", 2)["detection_report"]
        );
    }

    #[test]
    fn interpolates_bracketing_positions_only_within_the_configured_gap() {
        let options = Options {
            max_position_gap_secs: 4,
            ..Options::default()
        };
        let mut resolver = Resolver::new(&options);
        let mut output = Vec::new();
        resolver.record(status("eo", 1, 179.0), time(1), &mut output);
        resolver.record(status("eo", 5, -179.0), time(5), &mut output);
        resolver.record(bearing("eo", 3), time(5), &mut output);
        assert_eq!(output[2]["sapient_sensor_position"]["longitude"], -180.0);
        resolver.record(bearing("eo", 6), time(6), &mut output);
        resolver.record(status("eo", 10, -175.0), time(10), &mut output);
        assert_eq!(output.len(), 4);
        resolver.expire(time(10), false);
        assert_eq!(resolver.take_unresolved(), 1);
        assert_eq!(resolver.take_unresolved(), 0);
    }

    #[test]
    fn exact_matching_skips_invalid_positions_and_never_interpolates() {
        let mut resolver = Resolver::new(&Options::default());
        let mut output = Vec::new();
        resolver.record(status("eo", 1, 10.0), time(1), &mut output);
        resolver.record(bearing("eo", 2), time(2), &mut output);
        resolver.record(status("eo", 2, 190.0), time(2), &mut output);
        resolver.record(status("eo", 3, 11.0), time(3), &mut output);
        assert_eq!(output.len(), 3);
        resolver.expire(time(4), false);
        assert_eq!(resolver.take_unresolved(), 1);
    }
}
