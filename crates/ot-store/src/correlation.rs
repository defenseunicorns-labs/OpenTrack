//! Correlation state people act on: runtime settings, the engine's pair and
//! split suggestions, operators' "do not pair" decisions (DO_NOT_PAIR edges
//! between source tracks, which the engine never overrides) and splitting a
//! source track off its system track.

use ot_core::{SiteCode, Uid};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use serde_json::Value;

use crate::graph::{self, EdgeKind, NodeKind};
use crate::sqlite::{Db, Decision, Result, StoreError, allocate_uid, now_ms, record_decision};

/// A suggestion as stored.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Suggestion {
    pub id: i64,
    /// `pair` or `split`.
    pub kind: String,
    pub track_a: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_b: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_track: Option<String>,
    pub evidence: Value,
    pub status: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision_id: Option<i64>,
}

/// A decision from the log, for review.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DecisionRow {
    pub id: i64,
    pub at_ms: i64,
    pub actor: String,
    pub op: String,
    pub reason: Option<String>,
    pub evidence: Value,
    /// The decision that undid this one, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub undone_by: Option<i64>,
    /// The decision this one undid (an `undo`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub undoes: Option<i64>,
}

fn suggestion(r: &rusqlite::Row<'_>) -> rusqlite::Result<Suggestion> {
    let evidence: String = r.get(5)?;
    Ok(Suggestion {
        id: r.get(0)?,
        kind: r.get(1)?,
        track_a: r.get(2)?,
        track_b: r.get(3)?,
        source_track: r.get(4)?,
        evidence: serde_json::from_str(&evidence).unwrap_or(Value::Null),
        status: r.get(6)?,
        created_at_ms: r.get(7)?,
        updated_at_ms: r.get(8)?,
        decision_id: r.get(9)?,
    })
}

const SUGGESTION_COLUMNS: &str = "id, kind, track_a, track_b, source_track, evidence, status,
     created_at_ms, updated_at_ms, decision_id";

/// The operations that make up correlation, for the decision review.
pub const CORRELATION_OPS: &[&str] = &[
    "pair",
    "merge",
    "split",
    "do_not_pair",
    "reject_split",
    "end_source_track",
    "retire_system_track",
    "correlation_settings",
];

impl Db {
    /// The saved settings, if an operator has saved any.
    pub fn correlation_settings(&self) -> Result<Option<Value>> {
        let raw: Option<String> = self
            .connection()
            .query_row(
                "SELECT settings FROM correlation_settings WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .optional()?;
        Ok(raw.map(|s| serde_json::from_str(&s)).transpose()?)
    }

    pub fn save_correlation_settings(
        &mut self,
        settings: &Value,
        decision: Decision,
    ) -> Result<i64> {
        self.write(|tx| {
            let now = now_ms();
            let d = record_decision(tx, &decision, now)?;
            tx.execute(
                "INSERT INTO correlation_settings (id, settings, updated_at_ms, decision_id)
                 VALUES (1, ?1, ?2, ?3)
                 ON CONFLICT(id) DO UPDATE SET settings = excluded.settings,
                     updated_at_ms = excluded.updated_at_ms, decision_id = excluded.decision_id",
                params![settings.to_string(), now, d],
            )?;
            Ok(d)
        })
    }

    /// Changes when settings or "do not pair" decisions change, so the engine
    /// reloads only then.
    pub fn correlation_version(&self) -> Result<String> {
        Ok(self.connection().query_row(
            "SELECT (SELECT coalesce(max(updated_at_ms), 0) FROM correlation_settings) || ':' ||
                    (SELECT count(*) FROM edges WHERE kind = 'DO_NOT_PAIR' AND valid_to_ms IS NULL) || ':' ||
                    (SELECT coalesce(max(id), 0) FROM edges WHERE kind = 'DO_NOT_PAIR')",
            [],
            |r| r.get(0),
        )?)
    }

    /// Record or refresh an open suggestion (one per subject); returns its id.
    pub fn upsert_suggestion(
        &mut self,
        kind: &str,
        track_a: &str,
        track_b: Option<&str>,
        source_track: Option<&str>,
        evidence: &Value,
    ) -> Result<i64> {
        self.write(|tx| {
            let now = now_ms();
            let existing: Option<i64> = tx
                .query_row(
                    "SELECT id FROM correlation_suggestions
                     WHERE status = 'open' AND kind = ?1 AND track_a = ?2
                       AND coalesce(track_b, '') = coalesce(?3, '')
                       AND coalesce(source_track, '') = coalesce(?4, '')",
                    params![kind, track_a, track_b, source_track],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(id) = existing {
                tx.execute(
                    "UPDATE correlation_suggestions SET evidence = ?2, updated_at_ms = ?3 WHERE id = ?1",
                    params![id, evidence.to_string(), now],
                )?;
                return Ok(id);
            }
            tx.execute(
                "INSERT INTO correlation_suggestions
                     (kind, track_a, track_b, source_track, evidence, created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
                params![kind, track_a, track_b, source_track, evidence.to_string(), now],
            )?;
            Ok(tx.last_insert_rowid())
        })
    }

    pub fn suggestion(&self, id: i64) -> Result<Option<Suggestion>> {
        Ok(self
            .connection()
            .query_row(
                &format!("SELECT {SUGGESTION_COLUMNS} FROM correlation_suggestions WHERE id = ?1"),
                [id],
                suggestion,
            )
            .optional()?)
    }

    /// Suggestions with this status (all when None), newest first.
    /// Suggestions by status, e.g. `{"open": 3}` (statuses with none left out).
    pub fn suggestion_counts(&self) -> Result<std::collections::BTreeMap<String, u64>> {
        let mut stmt = self
            .connection()
            .prepare("SELECT status, count(*) FROM correlation_suggestions GROUP BY status")?;
        let rows = stmt
            .query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64))
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn suggestions(&self, status: Option<&str>, limit: usize) -> Result<Vec<Suggestion>> {
        let mut stmt = self.connection().prepare(&format!(
            "SELECT {SUGGESTION_COLUMNS} FROM correlation_suggestions
             WHERE ?1 IS NULL OR status = ?1 ORDER BY updated_at_ms DESC LIMIT ?2"
        ))?;
        let rows = stmt
            .query_map(params![status, limit as i64], suggestion)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Close an open suggestion (accepted, rejected or expired).
    pub fn close_suggestion(
        &mut self,
        id: i64,
        status: &str,
        decision: Option<Decision>,
    ) -> Result<()> {
        self.write(|tx| {
            let now = now_ms();
            let d = decision.map(|d| record_decision(tx, &d, now)).transpose()?;
            let n = tx.execute(
                "UPDATE correlation_suggestions SET status = ?2, updated_at_ms = ?3,
                     decision_id = coalesce(?4, decision_id)
                 WHERE id = ?1 AND status = 'open'",
                params![id, status, now, d],
            )?;
            if n == 0 {
                return Err(StoreError::NotFound(format!("open suggestion {id}")));
            }
            Ok(())
        })
    }

    /// Expire open suggestions about a track that no longer exists.
    pub fn expire_suggestions(&mut self, uid: Uid) -> Result<usize> {
        let key = uid.to_string();
        self.write(|tx| {
            Ok(tx.execute(
                "UPDATE correlation_suggestions SET status = 'expired', updated_at_ms = ?2
                 WHERE status = 'open' AND (track_a = ?1 OR track_b = ?1)",
                params![key, now_ms()],
            )?)
        })
    }

    /// An operator's decision that two groups of source tracks are different
    /// objects: a DO_NOT_PAIR edge between every pair across them.
    pub fn do_not_pair(
        &mut self,
        left: &[String],
        right: &[String],
        decision: Decision,
    ) -> Result<i64> {
        self.write(|tx| {
            let now = now_ms();
            let d = record_decision(tx, &decision, now)?;
            for a in left {
                for b in right {
                    if a == b {
                        continue;
                    }
                    let (a, b) = if a < b { (a, b) } else { (b, a) };
                    let na = graph::upsert_node(tx, NodeKind::SourceTrack, a, now)?;
                    let nb = graph::upsert_node(tx, NodeKind::SourceTrack, b, now)?;
                    let exists: i64 = tx.query_row(
                        "SELECT count(*) FROM edges WHERE kind = 'DO_NOT_PAIR' AND valid_to_ms IS NULL
                           AND src = ?1 AND dst = ?2",
                        params![na, nb],
                        |r| r.get(0),
                    )?;
                    if exists == 0 {
                        graph::add_edge(tx, EdgeKind::DoNotPair, na, nb, d, &serde_json::json!({}), now)?;
                    }
                }
            }
            Ok(d)
        })
    }

    /// Every live "do not pair" decision, as pairs of source track keys.
    pub fn do_not_pairs(&self) -> Result<Vec<(String, String)>> {
        let mut stmt = self.connection().prepare(
            "SELECT a.key, b.key FROM edges e JOIN nodes a ON a.id = e.src JOIN nodes b ON b.id = e.dst
             WHERE e.kind = 'DO_NOT_PAIR' AND e.valid_to_ms IS NULL",
        )?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Split a source track off its system track onto a new one; returns the
    /// new UID and the decision id.
    pub fn split_source_track(
        &mut self,
        site: SiteCode,
        source_id: &str,
        source_track_key: &str,
        decision: Decision,
    ) -> Result<(Uid, i64)> {
        self.write(|tx| {
            let now = now_ms();
            let key = graph::source_track_key(source_id, source_track_key);
            let src: i64 = tx
                .query_row(
                    "SELECT id FROM nodes WHERE kind = 'source_track' AND key = ?1",
                    [&key],
                    |r| r.get(0),
                )
                .optional()?
                .ok_or_else(|| StoreError::NotFound(format!("source track {key}")))?;
            let d = record_decision(tx, &decision, now)?;
            tx.execute(
                "UPDATE edges SET valid_to_ms = max(?2, valid_from_ms), ended_by = ?3
                 WHERE src = ?1 AND kind = 'REPORTS_FOR' AND valid_to_ms IS NULL",
                params![src, now, d],
            )?;
            let uid = allocate_uid(tx, site)?;
            let sys = graph::upsert_node(tx, NodeKind::SystemTrack, &uid.to_string(), now)?;
            graph::add_edge(
                tx,
                EdgeKind::ReportsFor,
                src,
                sys,
                d,
                &serde_json::json!({ "pairing": "split", "confidence": 1.0 }),
                now,
            )?;
            Ok((uid, d))
        })
    }

    /// The latest decisions of these operations, newest first.
    pub fn decisions_by_op(&self, ops: &[&str], limit: usize) -> Result<Vec<DecisionRow>> {
        let list = serde_json::to_string(ops).expect("strings");
        let mut stmt = self.connection().prepare(
            "SELECT id, at_ms, actor, op, reason, evidence, undone_by, undoes FROM decisions
             WHERE op IN (SELECT value FROM json_each(?1)) ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![list, limit as i64], |r| {
                let evidence: Option<String> = r.get(5)?;
                Ok(DecisionRow {
                    id: r.get(0)?,
                    at_ms: r.get(1)?,
                    actor: r.get(2)?,
                    op: r.get(3)?,
                    reason: r.get(4)?,
                    evidence: evidence
                        .and_then(|e| serde_json::from_str(&e).ok())
                        .unwrap_or(Value::Null),
                    undone_by: r.get(6)?,
                    undoes: r.get(7)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn db() -> (Db, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        (Db::open(dir.path().join("t.db")).unwrap(), dir)
    }

    #[test]
    fn settings_round_trip_and_change_the_version() {
        let (mut db, _d) = db();
        assert_eq!(db.correlation_settings().unwrap(), None);
        let v0 = db.correlation_version().unwrap();
        db.save_correlation_settings(
            &json!({"mode": "suggest"}),
            Decision::new("op", "correlation_settings"),
        )
        .unwrap();
        assert_eq!(
            db.correlation_settings().unwrap(),
            Some(json!({"mode": "suggest"}))
        );
        assert_ne!(db.correlation_version().unwrap(), v0);
    }

    #[test]
    fn suggestions_are_one_per_subject_until_closed() {
        let (mut db, _d) = db();
        let a = db
            .upsert_suggestion("pair", "OTK1", Some("OTK2"), None, &json!({"hits": 4}))
            .unwrap();
        let b = db
            .upsert_suggestion("pair", "OTK1", Some("OTK2"), None, &json!({"hits": 5}))
            .unwrap();
        assert_eq!(a, b);
        assert_eq!(db.suggestion(a).unwrap().unwrap().evidence["hits"], 5);
        db.close_suggestion(a, "rejected", Some(Decision::new("op", "do_not_pair")))
            .unwrap();
        assert!(
            db.close_suggestion(a, "accepted", None).is_err(),
            "already closed"
        );
        let c = db
            .upsert_suggestion("pair", "OTK1", Some("OTK2"), None, &json!({}))
            .unwrap();
        assert_ne!(a, c);
        assert_eq!(db.suggestions(Some("open"), 10).unwrap().len(), 1);
        assert_eq!(db.suggestions(None, 10).unwrap().len(), 2);
    }

    #[test]
    fn do_not_pair_and_split_are_recorded_in_the_graph() {
        let (mut db, _d) = db();
        let site = SiteCode::new("TST").unwrap();
        let (uid, _) = db
            .create_system_track(
                site,
                "ais",
                "366",
                Decision::new("engine", "create_system_track"),
            )
            .unwrap();
        db.pair_source_track(
            "radar",
            "r1",
            uid,
            &json!({}),
            Decision::new("engine", "pair"),
        )
        .unwrap();
        let v0 = db.correlation_version().unwrap();
        db.do_not_pair(
            &["radar/r1".into()],
            &["ais/366".into()],
            Decision::new("op", "do_not_pair"),
        )
        .unwrap();
        // Recording it twice changes nothing.
        db.do_not_pair(
            &["ais/366".into()],
            &["radar/r1".into()],
            Decision::new("op", "do_not_pair"),
        )
        .unwrap();
        assert_eq!(
            db.do_not_pairs().unwrap(),
            [("ais/366".into(), "radar/r1".into())]
        );
        assert_ne!(db.correlation_version().unwrap(), v0);

        let (split, _) = db
            .split_source_track(site, "radar", "r1", Decision::new("op", "split"))
            .unwrap();
        assert_ne!(split, uid);
        let edges = db.explain(split).unwrap();
        assert!(
            edges
                .iter()
                .any(|e| e.src_key == "radar/r1" && e.valid_to_ms.is_none())
        );
        let old = db.explain(uid).unwrap();
        assert!(
            old.iter()
                .any(|e| e.src_key == "radar/r1" && e.valid_to_ms.is_some())
        );
        assert_eq!(
            db.decisions_by_op(&["split", "do_not_pair"], 10)
                .unwrap()
                .first()
                .unwrap()
                .op,
            "split"
        );
    }
}
