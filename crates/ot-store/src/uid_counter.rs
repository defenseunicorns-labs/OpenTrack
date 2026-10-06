//! Keeping the UID counter ahead of every number already issued.
//!
//! The counter (`uid_sequences`) lives in SQLite, so restoring an older
//! backup takes it back to its value then, while Redis and the NATS stream
//! (and consumers) may already have seen higher numbers. On start the engine
//! looks for the highest number of its site in use anywhere it can see and,
//! if the counter is not past it, moves it on ([`Db::advance_uid_counter`]).
//! It never moves back.

use ot_core::{SiteCode, highest_sequence_in};
use rusqlite::OptionalExtension;
use serde::Serialize;
use serde_json::json;

use crate::sqlite::{Db, Decision, Result, now_ms, record_decision};

/// Where a UID was seen: the highest one of a site in one place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UidSighting {
    /// `sqlite`, `redis` or `nats`.
    pub store: String,
    /// What held it: a table, a Redis key, a NATS subject.
    pub location: String,
    pub sequence: u64,
}

impl UidSighting {
    pub fn new(store: &str, location: impl Into<String>, sequence: u64) -> Self {
        Self {
            store: store.to_owned(),
            location: location.into(),
            sequence,
        }
    }
}

/// The highest of `sightings` (first found wins a tie).
pub fn highest(sightings: impl IntoIterator<Item = UidSighting>) -> Option<UidSighting> {
    sightings.into_iter().fold(None, |best, s| match best {
        Some(b) if b.sequence >= s.sequence => Some(b),
        _ => Some(s),
    })
}

/// The counter was moved on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UidCounterAdvanced {
    /// The next sequence before and after.
    pub before: u64,
    pub after: u64,
    pub decision_id: i64,
}

impl Db {
    /// The sequence the next UID of `site` gets.
    pub fn uid_next_sequence(&self, site: SiteCode) -> Result<u64> {
        let next: Option<i64> = self
            .connection()
            .query_row(
                "SELECT next_sequence FROM uid_sequences WHERE site = ?1",
                [site.as_str()],
                |r| r.get(0),
            )
            .optional()?;
        Ok(next.map_or(1, |n| n.max(1) as u64))
    }

    /// The highest UID of `site` this database knows: the track graph's
    /// system tracks and groups, the decision log (which outlives a purge
    /// of the graph) and the multi-node log.
    pub fn uid_sightings(&self, site: SiteCode) -> Result<Vec<UidSighting>> {
        let conn = self.connection();
        let mut out = Vec::new();
        // Keys are the UID itself, zero padded, so the largest sorts last.
        let pattern = format!("{}{}", site.as_str(), "[0-9]".repeat(9));
        let node: Option<String> = conn
            .query_row(
                "SELECT max(key) FROM nodes
                 WHERE kind IN ('system_track', 'group') AND key GLOB ?1",
                [&pattern],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        if let Some(seq) = node.and_then(|k| highest_sequence_in(&k, site)) {
            out.push(UidSighting::new("sqlite", "nodes", seq));
        }
        let texts = [
            (
                "decisions",
                "SELECT evidence || ' ' || coalesce(before, '') || ' ' || coalesce(after, '')
                 FROM decisions WHERE instr(evidence || coalesce(before, '') || coalesce(after, ''), ?1) > 0",
            ),
            (
                "sync_log",
                "SELECT command FROM sync_log WHERE instr(command, ?1) > 0",
            ),
        ];
        for (table, sql) in texts {
            let mut stmt = conn.prepare(sql)?;
            let mut rows = stmt.query([site.as_str()])?;
            let mut best = None;
            while let Some(row) = rows.next()? {
                let text: String = row.get(0)?;
                if let Some(seq) = highest_sequence_in(&text, site) {
                    best = best.max(Some(seq));
                }
            }
            if let Some(seq) = best {
                out.push(UidSighting::new("sqlite", table, seq));
            }
        }
        Ok(out)
    }

    /// Move the counter of `site` past every UID in `sightings` that it is
    /// not already past, and record a `uid_counter_advanced` decision with
    /// where each was found. Never moves it back. `checked` says what was
    /// looked at (and what could not be), for the decision's evidence.
    pub fn advance_uid_counter(
        &mut self,
        site: SiteCode,
        sightings: &[UidSighting],
        checked: serde_json::Value,
    ) -> Result<Option<UidCounterAdvanced>> {
        self.write(|tx| {
            let before: u64 = tx
                .query_row(
                    "SELECT next_sequence FROM uid_sequences WHERE site = ?1",
                    [site.as_str()],
                    |r| r.get::<_, i64>(0),
                )
                .optional()?
                .map_or(1, |n| n.max(1) as u64);
            let found: Vec<&UidSighting> =
                sightings.iter().filter(|s| s.sequence >= before).collect();
            let Some(top) = found.iter().map(|s| s.sequence).max() else {
                return Ok(None);
            };
            let after = top + 1;
            tx.execute(
                "INSERT INTO uid_sequences (site, next_sequence) VALUES (?1, ?2)
                 ON CONFLICT(site) DO UPDATE
                 SET next_sequence = max(next_sequence, excluded.next_sequence)",
                rusqlite::params![site.as_str(), after as i64],
            )?;
            let found: Vec<serde_json::Value> = found
                .iter()
                .map(|s| {
                    json!({
                        "store": s.store,
                        "location": s.location,
                        "uid": format!("{}{:09}", site.as_str(), s.sequence),
                    })
                })
                .collect();
            let decision = Decision::new("system", "uid_counter_advanced")
                .reason(
                    "track numbers at or past the counter were already in use \
                     (a database restored from a backup?)",
                )
                .evidence(json!({
                    "site": site.as_str(),
                    "next_sequence": { "before": before, "after": after },
                    "found": found,
                    "checked": checked,
                }))
                .before(json!({ "next_sequence": before }))
                .after(json!({ "next_sequence": after }));
            let decision_id = record_decision(tx, &decision, now_ms())?;
            Ok(Some(UidCounterAdvanced {
                before,
                after,
                decision_id,
            }))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::allocate_uid;

    fn site() -> SiteCode {
        SiteCode::new("OTK").unwrap()
    }

    #[test]
    fn finds_numbers_in_the_graph_and_the_decision_log() {
        let mut db = Db::open_in_memory().unwrap();
        assert!(db.uid_sightings(site()).unwrap().is_empty());
        for key in ["1", "2", "3"] {
            db.create_system_track(site(), "ais", key, Decision::new("engine", "create"))
                .unwrap();
        }
        // Another site's numbers are not ours.
        db.create_system_track(
            SiteCode::new("AAA").unwrap(),
            "ais",
            "4",
            Decision::new("engine", "create"),
        )
        .unwrap();
        db.write(|tx| {
            record_decision(
                tx,
                &Decision::new("op:x", "note").evidence(json!({"track": "tms-OTK000000077"})),
                now_ms(),
            )
        })
        .unwrap();
        let s = db.uid_sightings(site()).unwrap();
        assert!(s.contains(&UidSighting::new("sqlite", "nodes", 3)), "{s:?}");
        assert!(
            s.contains(&UidSighting::new("sqlite", "decisions", 77)),
            "{s:?}"
        );
        assert_eq!(highest(s).unwrap().sequence, 77);
    }

    #[test]
    fn advances_past_a_number_in_use_and_records_why() {
        let mut db = Db::open_in_memory().unwrap();
        db.write(|tx| allocate_uid(tx, site())).unwrap();
        assert_eq!(db.uid_next_sequence(site()).unwrap(), 2);
        let seen = [UidSighting::new("redis", "tms:sys:OTK000000041", 41)];
        let a = db
            .advance_uid_counter(site(), &seen, json!({"redis": "ok"}))
            .unwrap()
            .unwrap();
        assert_eq!((a.before, a.after), (2, 42));
        let next = db.write(|tx| allocate_uid(tx, site())).unwrap();
        assert_eq!(next.to_string(), "OTK000000042");
        let d = db.decision(a.decision_id).unwrap().unwrap();
        assert_eq!(d["op"], "uid_counter_advanced");
        assert_eq!(
            d["evidence"]["next_sequence"],
            json!({"before": 2, "after": 42})
        );
        assert_eq!(d["evidence"]["found"][0]["uid"], "OTK000000041");
        assert_eq!(
            d["evidence"]["found"][0]["location"],
            "tms:sys:OTK000000041"
        );
    }

    #[test]
    fn never_moves_back() {
        let mut db = Db::open_in_memory().unwrap();
        let far = [UidSighting::new("nats", "tracks.tms-OTK000000500", 500)];
        db.advance_uid_counter(site(), &far, json!({})).unwrap();
        assert_eq!(db.uid_next_sequence(site()).unwrap(), 501);
        // Lower (or the counter's own last) numbers change nothing.
        let low = [
            UidSighting::new("redis", "k", 10),
            UidSighting::new("sqlite", "nodes", 500),
        ];
        assert!(
            db.advance_uid_counter(site(), &low, json!({}))
                .unwrap()
                .is_none()
        );
        assert!(
            db.advance_uid_counter(site(), &[], json!({}))
                .unwrap()
                .is_none()
        );
        assert_eq!(db.uid_next_sequence(site()).unwrap(), 501);
    }

    #[test]
    fn a_number_at_the_limit_exhausts_the_counter() {
        let mut db = Db::open_in_memory().unwrap();
        let top = [UidSighting::new("nats", "s", ot_core::uid::MAX_SEQUENCE)];
        db.advance_uid_counter(site(), &top, json!({})).unwrap();
        assert!(db.write(|tx| allocate_uid(tx, site())).is_err());
    }
}
