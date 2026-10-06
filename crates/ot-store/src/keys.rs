//! Redis key layout. Every key starts with a namespace (default `tms`) so
//! OpenTrack can share a Redis with other services, and tests can isolate
//! themselves under a unique namespace.
//!
//! | Key                               | Holds                                          |
//! |-----------------------------------|------------------------------------------------|
//! | `<ns>:obs:<source>`               | stream of mapped observations (time-trimmed)   |
//! | `<ns>:src:<source>:<key>`         | current state of a source track                |
//! | `<ns>:sys:<uid>`                  | current state of a system track                |
//! | `<ns>:hist:<source>:<key>`        | capped report history of a source track        |
//! | `<ns>:thist:<uid>`                | a system track's positions (by time, retained) |
//! | `<ns>:out`                        | stream of publish work for the writer (capped) |
//! | `<ns>:out:ctl`                    | deletes for the writer (never trimmed)         |
//! | `<ns>:cot:status`                 | the TAK outputs' status (expires)              |
//! | `<ns>:telemetry`                  | each process's OTLP export status (hash)       |
//! | `<ns>:metrics:<source>:<minute>`  | per-source, per-minute counters                |
//! | `<ns>:cache:*`, `<ns>:throttle:*` | static-join cache and write throttles (TTL)    |

pub const DEFAULT_NAMESPACE: &str = "tms";

#[derive(Debug, Clone)]
pub struct Keys {
    ns: String,
}

impl Default for Keys {
    fn default() -> Self {
        Self::new(DEFAULT_NAMESPACE)
    }
}

impl Keys {
    pub fn new(ns: impl Into<String>) -> Self {
        Self { ns: ns.into() }
    }

    pub fn namespace(&self) -> &str {
        &self.ns
    }

    pub fn obs_stream(&self, source: &str) -> String {
        format!("{}:obs:{source}", self.ns)
    }

    pub fn source_track(&self, source: &str, key: &str) -> String {
        format!("{}:src:{source}:{key}", self.ns)
    }

    pub fn system_track(&self, uid: &str) -> String {
        format!("{}:sys:{uid}", self.ns)
    }

    pub fn history(&self, source: &str, key: &str) -> String {
        format!("{}:hist:{source}:{key}", self.ns)
    }

    /// Operator commands for the engine (a list; the engine pops them).
    pub fn commands(&self) -> String {
        format!("{}:cmd", self.ns)
    }

    /// The engine's answer to one command.
    pub fn command_result(&self, id: &str) -> String {
        format!("{}:cmdres:{id}", self.ns)
    }

    pub fn track_history(&self, uid: &str) -> String {
        format!("{}:thist:{uid}", self.ns)
    }

    pub fn outbox(&self) -> String {
        format!("{}:out", self.ns)
    }

    /// Deletes and history-point deletions for the writer: rare, and never
    /// trimmed (the publication outbox is capped, since a later update
    /// queues a track again, but a lost delete would leave a ghost).
    pub fn outbox_control(&self) -> String {
        format!("{}:out:ctl", self.ns)
    }

    /// Sync messages the engine has for other nodes, oldest first, for the
    /// link role to send (see `ot_sync::wire`).
    pub fn sync_out(&self) -> String {
        format!("{}:sync:out", self.ns)
    }

    /// Non-point contacts no track took (lines of bearing), for the writer
    /// to publish live on `contacts.>`.
    pub fn contacts_out(&self) -> String {
        format!("{}:contacts:out", self.ns)
    }

    /// How sharing with other nodes is going, as the link and the engine
    /// last wrote it (a hash of JSON by part).
    pub fn sync_status(&self) -> String {
        format!("{}:sync:status", self.ns)
    }

    /// How the TAK outputs are going, as the `cot` role last wrote it
    /// (JSON, expiring when the role stops writing it).
    pub fn cot_status(&self) -> String {
        format!("{}:cot:status", self.ns)
    }

    /// How each process's OpenTelemetry export is going: a hash, one field
    /// per process (JSON, stamped with when it was written).
    pub fn telemetry_status(&self) -> String {
        format!("{}:telemetry", self.ns)
    }

    /// Cached static (identity) fields of a source track, for the static join.
    pub fn static_cache(&self, source: &str, key: &str) -> String {
        format!("{}:cache:{source}:{key}", self.ns)
    }

    /// Pattern for all of a source's static cache entries.
    pub fn static_cache_all(&self, source: &str) -> String {
        format!("{}:cache:{source}:*", self.ns)
    }

    /// Live status of a source worker (link state, last error), with a TTL.
    pub fn source_status(&self, source: &str) -> String {
        format!("{}:status:{source}", self.ns)
    }

    /// `minute` is Unix minutes (epoch seconds / 60).
    pub fn metrics(&self, source: &str, minute: i64) -> String {
        format!("{}:metrics:{source}:{minute}", self.ns)
    }

    /// Pattern matching every key in the namespace, for SCAN.
    pub fn all(&self) -> String {
        format!("{}:*", self.ns)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_matches_the_design() {
        let k = Keys::default();
        assert_eq!(k.obs_stream("ais"), "tms:obs:ais");
        assert_eq!(k.source_track("ais", "338924210"), "tms:src:ais:338924210");
        assert_eq!(k.system_track("OTK000000001"), "tms:sys:OTK000000001");
        assert_eq!(k.history("ais", "1"), "tms:hist:ais:1");
        assert_eq!(k.outbox(), "tms:out");
        assert_eq!(k.metrics("ais", 29_837_520), "tms:metrics:ais:29837520");
        assert_eq!(Keys::new("t1").all(), "t1:*");
    }
}
