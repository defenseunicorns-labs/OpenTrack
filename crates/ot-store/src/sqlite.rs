//! The SQLite store: one file in WAL mode, migrated on open.

use std::path::Path;

use chrono::{DateTime, TimeZone, Utc};
use ot_core::{SiteCode, Uid};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde_json::Value;

use crate::graph::{self, EdgeKind, NodeKind};

/// Ordered migrations. Append only; never edit a released one.
const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/0001_init.sql"),
    include_str!("../migrations/0002_registry.sql"),
    include_str!("../migrations/0003_schema_seed.sql"),
    include_str!("../migrations/0004_raw_subject.sql"),
    include_str!("../migrations/0005_sample_meta.sql"),
    include_str!("../migrations/0006_cards.sql"),
    include_str!("../migrations/0007_correlation.sql"),
    include_str!("../migrations/0008_app_settings.sql"),
    include_str!("../migrations/0009_entities.sql"),
    include_str!("../migrations/0010_track_groups.sql"),
    include_str!("../migrations/0011_entity_publish.sql"),
    include_str!("../migrations/0012_plugins.sql"),
    include_str!("../migrations/0013_auth.sql"),
    include_str!("../migrations/0014_sync.sql"),
    include_str!("../migrations/0015_account_policy.sql"),
    include_str!("../migrations/0016_audit.sql"),
    include_str!("../migrations/0017_account_expiry.sql"),
    include_str!("../migrations/0018_sync_keys.sql"),
    include_str!("../migrations/0019_plugin_secret.sql"),
];

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Redis(redis::RedisError),
    #[error(transparent)]
    Uid(#[from] ot_core::UidError),
    #[error("database schema version {found} is newer than this build supports ({supported})")]
    TooNew { found: i64, supported: i64 },
    #[error("UID sequence for site {0} is exhausted")]
    UidExhausted(String),
    #[error("{0}")]
    Conflict(String),
    #[error("not found: {0}")]
    NotFound(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// A decision about to be recorded in the audit log.
#[derive(Debug, Clone)]
pub struct Decision {
    pub actor: String,
    pub op: String,
    pub reason: Option<String>,
    pub evidence: Value,
    pub before: Option<Value>,
    pub after: Option<Value>,
}

impl Decision {
    pub fn new(actor: impl Into<String>, op: impl Into<String>) -> Self {
        Self {
            actor: actor.into(),
            op: op.into(),
            reason: None,
            evidence: Value::Object(Default::default()),
            before: None,
            after: None,
        }
    }

    pub fn reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    /// Evidence for the decision; object keys add to what is already there.
    pub fn evidence(mut self, evidence: Value) -> Self {
        match (&mut self.evidence, evidence) {
            (Value::Object(have), Value::Object(add)) => have.extend(add),
            (slot, other) => *slot = other,
        }
        self
    }

    /// State before the change, for review and undo.
    pub fn before(mut self, before: Value) -> Self {
        self.before = Some(before);
        self
    }

    /// State after the change.
    pub fn after(mut self, after: Value) -> Self {
        self.after = Some(after);
        self
    }
}

pub struct Db {
    conn: Connection,
}

impl Db {
    /// Open (creating if needed) and migrate the database at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Self::init(conn)
    }

    /// An in-memory database, for tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let mut db = Self { conn };
        db.migrate()?;
        crate::audit::start_chain(&mut db)?;
        Ok(db)
    }

    fn migrate(&mut self) -> Result<()> {
        let current: i64 = self
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))?;
        let supported = MIGRATIONS.len() as i64;
        if current > supported {
            return Err(StoreError::TooNew {
                found: current,
                supported,
            });
        }
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
            let tx = self.conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", (i + 1) as i64)?;
            tx.commit()?;
            tracing::info!(version = i + 1, "applied SQLite migration");
        }
        Ok(())
    }

    pub fn schema_version(&self) -> Result<i64> {
        Ok(self
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))?)
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Record a decision about something kept outside the database (a
    /// tracker profile file); returns its id.
    pub fn record(&mut self, d: &Decision) -> Result<i64> {
        self.write(|tx| record_decision(tx, d, now_ms()))
    }

    /// Run `f` in an immediate (write-locked) transaction.
    pub fn write<T>(&mut self, f: impl FnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
        // Audit rows appended in it reach the server log only if it commits.
        let done = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(Into::into)
            .and_then(|tx| {
                let out = f(&tx)?;
                tx.commit()?;
                Ok(out)
            });
        crate::audit::log_pending(done.is_ok());
        done
    }

    /// Create a system track for a source track: allocate a UID, record the
    /// decision and link the source track to it, in one transaction.
    pub fn create_system_track(
        &mut self,
        site: SiteCode,
        source_id: &str,
        source_track_key: &str,
        decision: Decision,
    ) -> Result<(Uid, i64)> {
        self.write(|tx| {
            let uid = allocate_uid(tx, site)?;
            let decision_id = open_system_track(tx, uid, source_id, source_track_key, &decision)?;
            Ok((uid, decision_id))
        })
    }

    /// Create a system track under a UID another node minted, for that
    /// node's report of it (a track it retired here comes back under the
    /// same UID). Returns the decision id.
    pub fn adopt_system_track(
        &mut self,
        uid: Uid,
        source_id: &str,
        source_track_key: &str,
        decision: Decision,
    ) -> Result<i64> {
        self.write(|tx| open_system_track(tx, uid, source_id, source_track_key, &decision))
    }

    /// Retire a system track: record the decision, close every live edge
    /// touching it and mark the node retired. Returns the decision id.
    pub fn retire_system_track(&mut self, uid: Uid, decision: Decision) -> Result<i64> {
        self.write(|tx| {
            let now = now_ms();
            let node: i64 = tx
                .query_row(
                    "SELECT id FROM nodes WHERE kind = 'system_track' AND key = ?1 AND retired_at_ms IS NULL",
                    [uid.to_string()],
                    |r| r.get(0),
                )
                .optional()?
                .ok_or_else(|| StoreError::NotFound(format!("live system track {uid}")))?;
            let decision_id = record_decision(tx, &decision, now)?;
            tx.execute(
                "UPDATE edges SET valid_to_ms = max(?2, valid_from_ms), ended_by = ?3
                 WHERE (src = ?1 OR dst = ?1) AND valid_to_ms IS NULL",
                params![node, now, decision_id],
            )?;
            tx.execute(
                "UPDATE nodes SET retired_at_ms = ?2 WHERE id = ?1",
                params![node, now],
            )?;
            Ok(decision_id)
        })
    }

    /// Link a source track to an existing live system track (ending any link
    /// it had), recording the decision; `attrs` carries the pairing type,
    /// confidence and evidence. Returns the decision id.
    pub fn pair_source_track(
        &mut self,
        source_id: &str,
        source_track_key: &str,
        uid: Uid,
        attrs: &Value,
        decision: Decision,
    ) -> Result<i64> {
        self.write(|tx| {
            let now = now_ms();
            let sys = live_system_node(tx, uid)?;
            let decision_id = record_decision(tx, &decision, now)?;
            let src = graph::upsert_node(
                tx,
                NodeKind::SourceTrack,
                &graph::source_track_key(source_id, source_track_key),
                now,
            )?;
            tx.execute(
                "UPDATE edges SET valid_to_ms = max(?2, valid_from_ms), ended_by = ?3
                 WHERE src = ?1 AND kind = 'REPORTS_FOR' AND valid_to_ms IS NULL",
                params![src, now, decision_id],
            )?;
            graph::add_edge(tx, EdgeKind::ReportsFor, src, sys, decision_id, attrs, now)?;
            Ok(decision_id)
        })
    }

    /// Record facts about a source track on its graph node (merged into what
    /// is there), e.g. `{"tracker": "gnn-1"}` for one a tracker formed.
    pub fn note_source_track(
        &mut self,
        source_id: &str,
        source_track_key: &str,
        attrs: &Value,
    ) -> Result<()> {
        self.write(|tx| {
            let node = graph::upsert_node(
                tx,
                NodeKind::SourceTrack,
                &graph::source_track_key(source_id, source_track_key),
                now_ms(),
            )?;
            graph::merge_node_attrs(tx, node, attrs)
        })
    }

    /// End a source track's link to its system track (its source said it
    /// ended), recording the decision. Returns the decision id, or None when
    /// it had no live link.
    pub fn end_source_track(
        &mut self,
        source_id: &str,
        source_track_key: &str,
        decision: Decision,
    ) -> Result<Option<i64>> {
        self.write(|tx| {
            let now = now_ms();
            let key = graph::source_track_key(source_id, source_track_key);
            let src: Option<i64> = tx
                .query_row(
                    "SELECT id FROM nodes WHERE kind = 'source_track' AND key = ?1",
                    [&key],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(src) = src else { return Ok(None) };
            let live: i64 = tx.query_row(
                "SELECT count(*) FROM edges WHERE src = ?1 AND kind = 'REPORTS_FOR' AND valid_to_ms IS NULL",
                [src],
                |r| r.get(0),
            )?;
            if live == 0 {
                return Ok(None);
            }
            let decision_id = record_decision(tx, &decision, now)?;
            tx.execute(
                "UPDATE edges SET valid_to_ms = max(?2, valid_from_ms), ended_by = ?3
                 WHERE src = ?1 AND kind = 'REPORTS_FOR' AND valid_to_ms IS NULL",
                params![src, now, decision_id],
            )?;
            Ok(Some(decision_id))
        })
    }

    /// Merge system track `from` into `into`: every source track reporting
    /// for `from` now reports for `into`, `from` is retired and a live
    /// `MERGED_INTO` edge keeps it resolvable as an alias. One transaction;
    /// returns the decision id.
    pub fn merge_system_tracks(&mut self, from: Uid, into: Uid, decision: Decision) -> Result<i64> {
        if from == into {
            return Err(StoreError::Conflict(format!(
                "cannot merge {from} into itself"
            )));
        }
        self.write(|tx| {
            let now = now_ms();
            let from_node = live_system_node(tx, from)?;
            let into_node = live_system_node(tx, into)?;
            let decision_id = record_decision(tx, &decision, now)?;
            // Groups and pairings follow the merge to the surviving track.
            let carried: Vec<(String, i64)> = {
                let mut stmt = tx.prepare(
                    "SELECT kind, CASE WHEN src = ?1 THEN dst ELSE src END FROM edges
                     WHERE (src = ?1 OR dst = ?1) AND kind IN ('MEMBER_OF', 'PAIRED_WITH')
                       AND valid_to_ms IS NULL",
                )?;
                stmt.query_map([from_node], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<rusqlite::Result<_>>()?
            };
            let moved: Vec<(i64, String)> = {
                let mut stmt = tx.prepare(
                    "SELECT src, attrs FROM edges
                     WHERE dst = ?1 AND kind = 'REPORTS_FOR' AND valid_to_ms IS NULL",
                )?;
                stmt.query_map([from_node], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<rusqlite::Result<_>>()?
            };
            tx.execute(
                "UPDATE edges SET valid_to_ms = max(?2, valid_from_ms), ended_by = ?3
                 WHERE (src = ?1 OR dst = ?1) AND valid_to_ms IS NULL",
                params![from_node, now, decision_id],
            )?;
            for (src, attrs) in moved {
                let attrs: Value = serde_json::from_str(&attrs).unwrap_or(Value::Null);
                graph::add_edge(
                    tx,
                    EdgeKind::ReportsFor,
                    src,
                    into_node,
                    decision_id,
                    &attrs,
                    now,
                )?;
            }
            graph::add_edge(
                tx,
                EdgeKind::MergedInto,
                from_node,
                into_node,
                decision_id,
                &serde_json::json!({}),
                now,
            )?;
            for (kind, other) in carried {
                let pairing = kind == "PAIRED_WITH";
                if pairing && other == into_node {
                    continue;
                }
                let exists: bool = tx.query_row(
                    "SELECT EXISTS (SELECT 1 FROM edges WHERE kind = ?1 AND valid_to_ms IS NULL
                         AND ((src = ?2 AND dst = ?3) OR (src = ?3 AND dst = ?2)))",
                    params![kind, into_node, other],
                    |r| r.get(0),
                )?;
                if !exists {
                    let kind = if pairing {
                        EdgeKind::PairedWith
                    } else {
                        EdgeKind::MemberOf
                    };
                    graph::add_edge(
                        tx,
                        kind,
                        into_node,
                        other,
                        decision_id,
                        &serde_json::json!({}),
                        now,
                    )?;
                }
            }
            tx.execute(
                "UPDATE nodes SET retired_at_ms = ?2 WHERE id = ?1",
                params![from_node, now],
            )?;
            Ok(decision_id)
        })
    }

    /// Every live source-track → system-track link, as
    /// (`<source>/<key>`, UID). The engine warms its map from this.
    pub fn live_reports(&self) -> Result<Vec<(String, Uid)>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.key, d.key FROM edges e
             JOIN nodes s ON s.id = e.src JOIN nodes d ON d.id = e.dst
             WHERE e.kind = 'REPORTS_FOR' AND e.valid_to_ms IS NULL",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        rows.map(|r| {
            let (src, uid) = r?;
            Ok((src, uid.parse()?))
        })
        .collect()
    }

    /// Every edge that ever touched a system track, with the decisions that
    /// opened and closed it: the answer to "why is this one track?".
    pub fn explain(&self, uid: Uid) -> Result<Vec<graph::EdgeRecord>> {
        graph::explain_system_track(&self.conn, &uid.to_string())
    }

    pub fn decision(&self, id: i64) -> Result<Option<Value>> {
        self.conn
            .query_row(
                "SELECT json_object('id', id, 'at_ms', at_ms, 'actor', actor, 'op', op,
                        'reason', reason, 'evidence', json(evidence))
                 FROM decisions WHERE id = ?1",
                [id],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .map(|s| serde_json::from_str(&s).map_err(StoreError::from))
            .transpose()
    }
}

/// Allocate the next UID for `site`.
/// The node id of a live system track.
pub(crate) fn live_system_node(tx: &Transaction<'_>, uid: Uid) -> Result<i64> {
    tx.query_row(
        "SELECT id FROM nodes WHERE kind = 'system_track' AND key = ?1 AND retired_at_ms IS NULL",
        [uid.to_string()],
        |r| r.get(0),
    )
    .optional()?
    .ok_or_else(|| StoreError::NotFound(format!("live system track {uid}")))
}

pub fn allocate_uid(tx: &Transaction<'_>, site: SiteCode) -> Result<Uid> {
    let seq: i64 = tx.query_row(
        "INSERT INTO uid_sequences (site, next_sequence) VALUES (?1, 2)
         ON CONFLICT(site) DO UPDATE SET next_sequence = next_sequence + 1
         RETURNING next_sequence - 1",
        [site.as_str()],
        |r| r.get(0),
    )?;
    Uid::new(site, seq as u64).map_err(|_| StoreError::UidExhausted(site.to_string()))
}

/// A system track node `uid` (live again if it was retired), with the source
/// track reporting for it.
fn open_system_track(
    tx: &Transaction<'_>,
    uid: Uid,
    source_id: &str,
    source_track_key: &str,
    decision: &Decision,
) -> Result<i64> {
    let now = now_ms();
    let decision_id = record_decision(tx, decision, now)?;
    let sys = graph::upsert_node(tx, NodeKind::SystemTrack, &uid.to_string(), now)?;
    tx.execute("UPDATE nodes SET retired_at_ms = NULL WHERE id = ?1", [sys])?;
    let src = graph::upsert_node(
        tx,
        NodeKind::SourceTrack,
        &graph::source_track_key(source_id, source_track_key),
        now,
    )?;
    graph::add_edge(
        tx,
        EdgeKind::ReportsFor,
        src,
        sys,
        decision_id,
        &serde_json::json!({ "pairing": "auto", "confidence": 1.0 }),
        now,
    )?;
    Ok(decision_id)
}

pub fn record_decision(tx: &Transaction<'_>, d: &Decision, at_ms: i64) -> Result<i64> {
    tx.execute(
        "INSERT INTO decisions (at_ms, actor, op, reason, evidence, before, after)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            at_ms,
            d.actor,
            d.op,
            d.reason,
            d.evidence.to_string(),
            d.before.as_ref().map(Value::to_string),
            d.after.as_ref().map(Value::to_string),
        ],
    )?;
    let id = tx.last_insert_rowid();
    // Its copy in the audit record, as recorded now (undo later marks the
    // decision in place; the copy never changes).
    let detail = serde_json::json!({
        "reason": d.reason,
        "evidence": d.evidence,
        "before": d.before,
        "after": d.after,
    });
    let ip = crate::audit::client();
    crate::audit::append(
        tx,
        at_ms,
        &d.actor,
        &d.op,
        true,
        ip.as_deref(),
        &detail.to_string(),
        Some(id),
    )?;
    Ok(id)
}

pub fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

pub fn from_ms(ms: i64) -> DateTime<Utc> {
    Utc.timestamp_millis_opt(ms).single().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_once_and_reopens() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ot.db");
        assert_eq!(Db::open(&path).unwrap().schema_version().unwrap(), 19);
        // Re-opening applies nothing and keeps the version.
        assert_eq!(Db::open(&path).unwrap().schema_version().unwrap(), 19);
    }

    #[test]
    fn refuses_a_newer_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ot.db");
        drop(Db::open(&path).unwrap());
        Connection::open(&path)
            .unwrap()
            .pragma_update(None, "user_version", 99)
            .unwrap();
        assert!(matches!(
            Db::open(&path),
            Err(StoreError::TooNew { found: 99, .. })
        ));
    }

    #[test]
    fn uids_are_sequential_per_site() {
        let mut db = Db::open_in_memory().unwrap();
        let a = SiteCode::new("OTK").unwrap();
        let b = SiteCode::new("SD1").unwrap();
        let got: Vec<String> = [a, a, b, a]
            .into_iter()
            .map(|s| db.write(|tx| allocate_uid(tx, s)).unwrap().to_string())
            .collect();
        assert_eq!(
            got,
            [
                "OTK000000001",
                "OTK000000002",
                "SD1000000001",
                "OTK000000003"
            ]
        );
    }

    #[test]
    fn create_system_track_links_source_and_records_decision() {
        let mut db = Db::open_in_memory().unwrap();
        let site = SiteCode::new("OTK").unwrap();
        let (uid, decision_id) = db
            .create_system_track(
                site,
                "synthetic",
                "T1",
                Decision::new("system", "create_system_track").reason("test"),
            )
            .unwrap();
        assert_eq!(uid.to_string(), "OTK000000001");

        let d = db.decision(decision_id).unwrap().unwrap();
        assert_eq!(d["op"], "create_system_track");
        assert_eq!(d["reason"], "test");

        let why = db.explain(uid).unwrap();
        assert_eq!(why.len(), 1);
        assert_eq!(why[0].kind, EdgeKind::ReportsFor);
        assert_eq!(why[0].src_key, "synthetic/T1");
        assert_eq!(why[0].decision_id, decision_id);
        assert!(why[0].valid_to_ms.is_none());
    }

    #[test]
    fn retiring_closes_live_edges_once() {
        let mut db = Db::open_in_memory().unwrap();
        let site = SiteCode::new("OTK").unwrap();
        let (uid, _) = db
            .create_system_track(
                site,
                "s",
                "k",
                Decision::new("system", "create_system_track"),
            )
            .unwrap();
        let d = db
            .retire_system_track(uid, Decision::new("op:test", "retire"))
            .unwrap();
        let why = db.explain(uid).unwrap();
        assert_eq!(why[0].ended_by, Some(d));
        assert!(why[0].valid_to_ms.is_some());
        assert!(matches!(
            db.retire_system_track(uid, Decision::new("op:test", "retire")),
            Err(StoreError::NotFound(_))
        ));
        // The source track is free to report for a new system track.
        db.create_system_track(
            site,
            "s",
            "k",
            Decision::new("system", "create_system_track"),
        )
        .unwrap();
    }

    #[test]
    fn a_source_track_reports_for_one_system_track_at_a_time() {
        let mut db = Db::open_in_memory().unwrap();
        let site = SiteCode::new("OTK").unwrap();
        db.create_system_track(
            site,
            "s",
            "k",
            Decision::new("system", "create_system_track"),
        )
        .unwrap();
        let err = db
            .create_system_track(
                site,
                "s",
                "k",
                Decision::new("system", "create_system_track"),
            )
            .unwrap_err();
        assert!(matches!(err, StoreError::Sqlite(_)), "{err}");
        // The failed transaction rolled back its UID allocation too.
        let next = db.write(|tx| allocate_uid(tx, site)).unwrap().to_string();
        assert_eq!(next, "OTK000000002");
    }

    #[test]
    fn raw_output_requires_recorded_consent() {
        let db = Db::open_in_memory().unwrap();
        let err = db.connection().execute(
            "INSERT INTO sources (id, name, transport, codec, raw_subject, created_at_ms, updated_at_ms)
             VALUES ('ais', 'AIS', 'websocket', 'json', 'opentrack.raw.ais', 0, 0)",
            [],
        );
        assert!(err.is_err());
    }

    #[test]
    fn a_tracker_formed_source_track_names_its_tracker() {
        let mut db = Db::open_in_memory().unwrap();
        let site = SiteCode::new("OTK").unwrap();
        let (uid, _) = db
            .create_system_track(
                site,
                "gmti",
                "G1-7",
                Decision::new("engine", "create_system_track"),
            )
            .unwrap();
        db.note_source_track("gmti", "G1-7", &serde_json::json!({ "tracker": "gnn-1" }))
            .unwrap();
        db.note_source_track("gmti", "G1-7", &serde_json::json!({ "other": 1 }))
            .unwrap();
        let edges = db.explain(uid).unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].src_key, "gmti/G1-7");
        assert_eq!(
            edges[0].src_attrs,
            serde_json::json!({ "tracker": "gnn-1", "other": 1 })
        );
    }

    #[test]
    fn pairing_and_merging_keep_the_graph_explainable() {
        let mut db = Db::open_in_memory().unwrap();
        let site = SiteCode::new("OTK").unwrap();
        let (a, _) = db
            .create_system_track(
                site,
                "ais",
                "366",
                Decision::new("engine", "create_system_track"),
            )
            .unwrap();
        let (b, _) = db
            .create_system_track(
                site,
                "tak",
                "uid-1",
                Decision::new("engine", "create_system_track"),
            )
            .unwrap();
        // A third source pairs straight onto A.
        db.pair_source_track(
            "radar",
            "t7",
            a,
            &serde_json::json!({"pairing": "auto", "confidence": 1.0}),
            Decision::new("engine", "pair").evidence(serde_json::json!({"rule": "identifier"})),
        )
        .unwrap();
        // B turns out to be the same ship: merge it into A.
        db.merge_system_tracks(b, a, Decision::new("engine", "merge"))
            .unwrap();

        let mut live = db.live_reports().unwrap();
        live.sort();
        assert_eq!(
            live,
            vec![
                ("ais/366".to_string(), a),
                ("radar/t7".to_string(), a),
                ("tak/uid-1".to_string(), a)
            ]
        );
        // A's history includes B's, through the merge.
        let kinds: Vec<(String, String, bool)> = db
            .explain(a)
            .unwrap()
            .into_iter()
            .map(|e| {
                (
                    e.kind.as_str().to_owned(),
                    e.src_key,
                    e.valid_to_ms.is_none(),
                )
            })
            .collect();
        assert!(kinds.contains(&("MERGED_INTO".into(), b.to_string(), true)));
        assert!(kinds.contains(&("REPORTS_FOR".into(), "tak/uid-1".into(), false)));
        // B is retired: nothing more can pair to it or merge it.
        assert!(
            db.merge_system_tracks(b, a, Decision::new("engine", "merge"))
                .is_err()
        );
        assert!(
            db.pair_source_track(
                "x",
                "y",
                b,
                &serde_json::json!({}),
                Decision::new("engine", "pair")
            )
            .is_err()
        );
    }
}
