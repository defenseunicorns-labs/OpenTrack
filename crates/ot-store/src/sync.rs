//! The replicated decision log (see `ot_sync`): every node's
//! track-management decisions, and which local decisions each one made.

use ot_core::SiteCode;
use ot_sync::{Entry, GlobalId, Hlc};
use rusqlite::{OptionalExtension, params};
use serde_json::Value;

use crate::sqlite::{Db, Result, now_ms};

/// Where an entry stands on this node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncStatus {
    Applied,
    /// Names a track this node does not have yet.
    Pending,
    /// A later decision on the same tracks was applied first.
    Superseded,
    Failed,
}

impl SyncStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            SyncStatus::Applied => "applied",
            SyncStatus::Pending => "pending",
            SyncStatus::Superseded => "superseded",
            SyncStatus::Failed => "failed",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "applied" => SyncStatus::Applied,
            "pending" => SyncStatus::Pending,
            "superseded" => SyncStatus::Superseded,
            _ => SyncStatus::Failed,
        }
    }
}

const COLUMNS: &str = "site, seq, hlc, actor, role, command";

fn entry(r: &rusqlite::Row<'_>) -> rusqlite::Result<Entry> {
    let site: String = r.get(0)?;
    let seq: i64 = r.get(1)?;
    let command: String = r.get(5)?;
    let conv = |e: String| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, e.into())
    };
    Ok(Entry {
        id: GlobalId {
            site: site
                .parse()
                .map_err(|e: ot_core::UidError| conv(e.to_string()))?,
            seq: seq as u64,
        },
        hlc: Hlc::from_i64(r.get(2)?),
        actor: r.get(3)?,
        role: r.get(4)?,
        command: serde_json::from_str(&command).map_err(|e| conv(e.to_string()))?,
    })
}

impl Db {
    /// The sequence this node's next entry gets.
    pub fn sync_next_seq(&self, site: SiteCode) -> Result<u64> {
        let max: Option<i64> = self.connection().query_row(
            "SELECT max(seq) FROM sync_log WHERE site = ?1",
            [site.as_str()],
            |r| r.get(0),
        )?;
        Ok(max.unwrap_or(0) as u64 + 1)
    }

    /// The latest stamp this node has given or seen, to start its clock after.
    pub fn sync_last_hlc(&self) -> Result<Hlc> {
        let max: Option<i64> =
            self.connection()
                .query_row("SELECT max(hlc) FROM sync_log", [], |r| r.get(0))?;
        Ok(Hlc::from_i64(max.unwrap_or(0)))
    }

    /// Log a decision of this node's: the next sequence, and a stamp later
    /// than every stamp given or heard here (the log is the clock), in one
    /// transaction, so the API and the engine can both log.
    pub fn sync_append_own(
        &mut self,
        site: SiteCode,
        actor: &str,
        role: &str,
        command: &Value,
        now_ms: u64,
    ) -> Result<Entry> {
        self.write(|tx| {
            let (seq, last): (i64, i64) = tx.query_row(
                "SELECT (SELECT coalesce(max(seq), 0) + 1 FROM sync_log WHERE site = ?1),
                        (SELECT coalesce(max(hlc), 0) FROM sync_log)",
                [site.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let hlc = ot_sync::Clock::after(Hlc::from_i64(last)).tick(now_ms);
            let e = Entry {
                id: GlobalId {
                    site,
                    seq: seq as u64,
                },
                hlc,
                actor: actor.to_owned(),
                role: role.to_owned(),
                command: command.clone(),
            };
            tx.execute(
                "INSERT INTO sync_log
                   (site, seq, hlc, actor, role, command, received_at_ms, status)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'applied')",
                params![
                    site.as_str(),
                    seq,
                    hlc.as_i64(),
                    actor,
                    role,
                    command.to_string(),
                    now_ms as i64
                ],
            )?;
            Ok(e)
        })
    }

    /// Entries by where they stand here, e.g. `{"applied": 12, "pending": 1}`.
    pub fn sync_counts(&self) -> Result<std::collections::BTreeMap<String, u64>> {
        let mut st = self
            .connection()
            .prepare("SELECT status, count(*) FROM sync_log GROUP BY status")?;
        let rows = st
            .query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64))
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// The highest sequence held from each site.
    pub fn sync_heads(&self) -> Result<Vec<(SiteCode, u64)>> {
        let mut st = self
            .connection()
            .prepare("SELECT site, max(seq) FROM sync_log GROUP BY site ORDER BY site")?;
        let rows: Vec<(String, i64)> = st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows
            .into_iter()
            .filter_map(|(s, n)| Some((s.parse().ok()?, n as u64)))
            .collect())
    }

    /// Which of a site's sequences up to `upto` are missing here, as
    /// inclusive ranges (at most `most`).
    pub fn sync_missing(&self, site: SiteCode, upto: u64, most: usize) -> Result<Vec<(u64, u64)>> {
        let mut st = self
            .connection()
            .prepare("SELECT seq FROM sync_log WHERE site = ?1 AND seq <= ?2 ORDER BY seq")?;
        let have: Vec<i64> = st
            .query_map(params![site.as_str(), upto as i64], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let mut out = Vec::new();
        let mut next = 1u64;
        for s in have.into_iter().map(|s| s as u64).chain([upto + 1]) {
            if s > next {
                out.push((next, s - 1));
                if out.len() == most {
                    break;
                }
            }
            next = s + 1;
        }
        Ok(out)
    }

    /// A site's entries in `from..=to`, in order.
    pub fn sync_range(&self, site: SiteCode, from: u64, to: u64) -> Result<Vec<Entry>> {
        let mut st = self.connection().prepare(&format!(
            "SELECT {COLUMNS} FROM sync_log WHERE site = ?1 AND seq BETWEEN ?2 AND ?3 ORDER BY seq"
        ))?;
        Ok(st
            .query_map(params![site.as_str(), from as i64, to as i64], entry)?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Keep an entry. False if it was here already (entries arrive twice).
    pub fn sync_record(
        &mut self,
        e: &Entry,
        status: SyncStatus,
        detail: Option<&str>,
    ) -> Result<bool> {
        let n = self.connection().execute(
            "INSERT OR IGNORE INTO sync_log
               (site, seq, hlc, actor, role, command, received_at_ms, status, detail)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                e.id.site.as_str(),
                e.id.seq as i64,
                e.hlc.as_i64(),
                e.actor,
                e.role,
                e.command.to_string(),
                now_ms(),
                status.as_str(),
                detail,
            ],
        )?;
        Ok(n == 1)
    }

    pub fn sync_set_status(
        &mut self,
        id: GlobalId,
        status: SyncStatus,
        detail: Option<&str>,
    ) -> Result<()> {
        self.connection().execute(
            "UPDATE sync_log SET status = ?3, detail = ?4 WHERE site = ?1 AND seq = ?2",
            params![id.site.as_str(), id.seq as i64, status.as_str(), detail],
        )?;
        Ok(())
    }

    pub fn sync_entry(&self, id: GlobalId) -> Result<Option<(Entry, SyncStatus)>> {
        Ok(self
            .connection()
            .query_row(
                &format!("SELECT {COLUMNS}, status FROM sync_log WHERE site = ?1 AND seq = ?2"),
                params![id.site.as_str(), id.seq as i64],
                |r| Ok((entry(r)?, SyncStatus::parse(&r.get::<_, String>(6)?))),
            )
            .optional()?)
    }

    /// Applied entries stamped later than `hlc`: what an entry that arrives
    /// now is judged against.
    pub fn sync_applied_after(&self, hlc: Hlc) -> Result<Vec<Entry>> {
        let mut st = self.connection().prepare(&format!(
            "SELECT {COLUMNS} FROM sync_log WHERE status = 'applied' AND hlc > ?1 ORDER BY hlc"
        ))?;
        Ok(st
            .query_map([hlc.as_i64()], entry)?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// A site's entries after sequence `after`, in order (what a peer lacks).
    pub fn sync_since(&self, site: SiteCode, after: u64, limit: usize) -> Result<Vec<Entry>> {
        let mut st = self.connection().prepare(&format!(
            "SELECT {COLUMNS} FROM sync_log WHERE site = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3"
        ))?;
        Ok(st
            .query_map(params![site.as_str(), after as i64, limit as i64], entry)?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Entries waiting for their tracks, oldest first.
    pub fn sync_pending(&self) -> Result<Vec<Entry>> {
        let mut st = self.connection().prepare(&format!(
            "SELECT {COLUMNS} FROM sync_log WHERE status = 'pending' ORDER BY hlc"
        ))?;
        Ok(st.query_map([], entry)?.collect::<rusqlite::Result<_>>()?)
    }

    /// Give up on pending entries received before `before_ms`.
    pub fn sync_expire_pending(&mut self, before_ms: i64) -> Result<usize> {
        Ok(self.connection().execute(
            "UPDATE sync_log SET status = 'failed', detail = 'its tracks never arrived'
              WHERE status = 'pending' AND received_at_ms < ?1",
            [before_ms],
        )?)
    }

    /// The undo naming entry `id`, if one has arrived.
    pub fn sync_undone_by(&self, id: GlobalId) -> Result<Option<GlobalId>> {
        let row: Option<(String, i64)> = self
            .connection()
            .query_row(
                "SELECT site, seq FROM sync_log
                  WHERE json_extract(command, '$.op') = 'undo'
                    AND json_extract(command, '$.decision') = ?1
                  ORDER BY hlc LIMIT 1",
                [id.to_string()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(row.and_then(|(site, seq)| {
            Some(GlobalId {
                site: site.parse().ok()?,
                seq: seq as u64,
            })
        }))
    }

    pub fn max_decision_id(&self) -> Result<i64> {
        let max: Option<i64> =
            self.connection()
                .query_row("SELECT max(id) FROM decisions", [], |r| r.get(0))?;
        Ok(max.unwrap_or(0))
    }

    /// Mark the decisions `actor` made after decision `after` as entry
    /// `id`'s, the n-th with `parts[n]` when there is one decision per part.
    /// Returns their ids.
    pub fn tag_decisions(
        &mut self,
        after: i64,
        actor: &str,
        id: GlobalId,
        parts: &[String],
    ) -> Result<Vec<i64>> {
        self.write(|tx| {
            let ids: Vec<i64> = {
                let mut st = tx
                    .prepare("SELECT id FROM decisions WHERE id > ?1 AND actor = ?2 ORDER BY id")?;
                st.query_map(params![after, actor], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?
            };
            let parts = (parts.len() == ids.len() && !parts.is_empty()).then_some(parts);
            for (n, d) in ids.iter().enumerate() {
                let mut tag = serde_json::json!({ "sync": id.to_string() });
                if let Some(p) = parts {
                    tag["sync_part"] = Value::String(p[n].clone());
                }
                tx.execute(
                    "UPDATE decisions SET evidence = json_patch(evidence, ?2) WHERE id = ?1",
                    params![d, tag.to_string()],
                )?;
            }
            Ok(ids)
        })
    }

    /// The entry a local decision was made for, and its part.
    pub fn sync_of_decision(&self, decision: i64) -> Result<Option<(GlobalId, Option<String>)>> {
        let row: Option<(Option<String>, Option<String>)> = self
            .connection()
            .query_row(
                "SELECT sync_id, json_extract(evidence, '$.sync_part') FROM decisions WHERE id = ?1",
                [decision],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(row.and_then(|(id, part)| Some((id?.parse().ok()?, part))))
    }

    /// The local decision an entry made (its `part`'s, when it made several).
    pub fn decision_of_sync(&self, id: GlobalId, part: Option<&str>) -> Result<Option<i64>> {
        Ok(self
            .connection()
            .query_row(
                "SELECT id FROM decisions WHERE sync_id = ?1
                   AND (?2 IS NULL OR json_extract(evidence, '$.sync_part') = ?2)
                 ORDER BY id LIMIT 1",
                params![id.to_string(), part],
                |r| r.get(0),
            )
            .optional()?)
    }
}

/// A peer's pinned public key (see `sync_peer_keys`).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct PeerKey {
    pub site: String,
    /// `ed25519:<base64>`, checked by the caller.
    pub public_key: String,
    pub pinned_by: String,
    pub pinned_at_ms: i64,
}

impl Db {
    /// Every pinned peer key, by site code.
    pub fn sync_peer_keys(&self) -> Result<Vec<PeerKey>> {
        let mut stmt = self.connection().prepare(
            "SELECT site, public_key, pinned_by, pinned_at_ms FROM sync_peer_keys ORDER BY site",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(PeerKey {
                site: r.get(0)?,
                public_key: r.get(1)?,
                pinned_by: r.get(2)?,
                pinned_at_ms: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Pin (or replace) a peer's key, recorded as a decision with the key
    /// it replaced.
    pub fn sync_pin_key(&mut self, site: SiteCode, public_key: &str, actor: &str) -> Result<i64> {
        self.write(|tx| {
            let now = now_ms();
            let before: Option<String> = tx
                .query_row(
                    "SELECT public_key FROM sync_peer_keys WHERE site = ?1",
                    [site.as_str()],
                    |r| r.get(0),
                )
                .optional()?;
            let d = crate::sqlite::Decision::new(actor, "sync_key_pin")
                .evidence(serde_json::json!({ "site": site.as_str() }))
                .before(serde_json::json!({ "public_key": before }))
                .after(serde_json::json!({ "public_key": public_key }));
            let id = crate::sqlite::record_decision(tx, &d, now)?;
            tx.execute(
                "INSERT INTO sync_peer_keys (site, public_key, pinned_by, pinned_at_ms) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(site) DO UPDATE SET public_key = excluded.public_key,
                     pinned_by = excluded.pinned_by, pinned_at_ms = excluded.pinned_at_ms",
                params![site.as_str(), public_key, actor, now],
            )?;
            Ok(id)
        })
    }

    /// Remove a peer's key: its messages are refused from then on. `None`
    /// when it had none.
    pub fn sync_unpin_key(&mut self, site: SiteCode, actor: &str) -> Result<Option<i64>> {
        self.write(|tx| {
            let before: Option<String> = tx
                .query_row(
                    "SELECT public_key FROM sync_peer_keys WHERE site = ?1",
                    [site.as_str()],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(before) = before else {
                return Ok(None);
            };
            let d = crate::sqlite::Decision::new(actor, "sync_key_remove")
                .evidence(serde_json::json!({ "site": site.as_str() }))
                .before(serde_json::json!({ "public_key": before }));
            let id = crate::sqlite::record_decision(tx, &d, now_ms())?;
            tx.execute(
                "DELETE FROM sync_peer_keys WHERE site = ?1",
                [site.as_str()],
            )?;
            Ok(Some(id))
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::Decision;

    fn e(id: &str, hlc: u64, command: Value) -> Entry {
        Entry {
            id: id.parse().unwrap(),
            hlc: Hlc::new(hlc, 0),
            actor: "tm".into(),
            role: "track_manager".into(),
            command,
        }
    }

    #[test]
    fn entries_are_kept_once_and_judged_against_later_ones() {
        let mut db = Db::open_in_memory().unwrap();
        let site: SiteCode = "AAA".parse().unwrap();
        assert_eq!(db.sync_next_seq(site).unwrap(), 1);
        let a = e(
            "AAA:1",
            100,
            json!({"op": "pair", "tracks": ["AAA000000001", "BBB000000001"]}),
        );
        let b = e(
            "BBB:4",
            200,
            json!({"op": "delete", "tracks": ["AAA000000001"]}),
        );
        assert!(db.sync_record(&a, SyncStatus::Applied, None).unwrap());
        assert!(!db.sync_record(&a, SyncStatus::Applied, None).unwrap());
        db.sync_record(&b, SyncStatus::Pending, None).unwrap();
        assert_eq!(db.sync_next_seq(site).unwrap(), 2);
        assert_eq!(db.sync_last_hlc().unwrap(), Hlc::new(200, 0));
        assert_eq!(
            db.sync_applied_after(Hlc::new(50, 0)).unwrap(),
            vec![a.clone()]
        );
        assert!(db.sync_applied_after(Hlc::new(100, 0)).unwrap().is_empty());
        assert_eq!(db.sync_pending().unwrap(), vec![b.clone()]);
        db.sync_set_status(b.id, SyncStatus::Applied, None).unwrap();
        assert_eq!(db.sync_entry(b.id).unwrap().unwrap().1, SyncStatus::Applied);
    }

    #[test]
    fn own_entries_follow_every_stamp_and_gaps_are_found() {
        let mut db = Db::open_in_memory().unwrap();
        let (a, b): (SiteCode, SiteCode) = ("AAA".parse().unwrap(), "BBB".parse().unwrap());
        // Heard from B, stamped ahead of our wall clock.
        for seq in [1, 2, 5, 8] {
            let mut x = e(&format!("BBB:{seq}"), 9_000, json!({"op": "unpair"}));
            x.hlc = Hlc::new(9_000, seq as u16);
            db.sync_record(&x, SyncStatus::Applied, None).unwrap();
        }
        let mine = db
            .sync_append_own(a, "tm", "admin", &json!({"op": "pair"}), 1_000)
            .unwrap();
        assert_eq!(mine.id.to_string(), "AAA:1");
        assert!(mine.hlc > Hlc::new(9_000, 8));
        let next = db
            .sync_append_own(a, "tm", "admin", &json!({"op": "pair"}), 20_000)
            .unwrap();
        assert_eq!((next.id.seq, next.hlc), (2, Hlc::new(20_000, 0)));
        assert_eq!(db.sync_heads().unwrap(), vec![(a, 2), (b, 8)]);
        assert_eq!(db.sync_missing(b, 8, 10).unwrap(), vec![(3, 4), (6, 7)]);
        assert_eq!(
            db.sync_missing(b, 10, 10).unwrap(),
            vec![(3, 4), (6, 7), (9, 10)]
        );
        assert_eq!(db.sync_missing(b, 10, 1).unwrap(), vec![(3, 4)]);
        assert_eq!(db.sync_missing(a, 2, 10).unwrap(), vec![]);
        let seqs: Vec<u64> = db
            .sync_range(b, 2, 6)
            .unwrap()
            .iter()
            .map(|e| e.id.seq)
            .collect();
        assert_eq!(seqs, [2, 5]);
    }

    #[test]
    fn decisions_are_tagged_with_their_entry_and_found_by_it() {
        let mut db = Db::open_in_memory().unwrap();
        let before = db.max_decision_id().unwrap();
        db.record(&Decision::new("tm", "delete_track")).unwrap();
        db.record(&Decision::new("tm", "delete_track")).unwrap();
        db.record(&Decision::new("someone else", "update_settings"))
            .unwrap();
        let id: GlobalId = "AAA:3".parse().unwrap();
        let parts = ["AAA000000001".to_owned(), "AAA000000002".to_owned()];
        let tagged = db.tag_decisions(before, "tm", id, &parts).unwrap();
        assert_eq!(tagged.len(), 2);
        assert_eq!(
            db.decision_of_sync(id, Some("AAA000000002")).unwrap(),
            Some(tagged[1])
        );
        assert_eq!(db.decision_of_sync(id, None).unwrap(), Some(tagged[0]));
        assert_eq!(
            db.sync_of_decision(tagged[1]).unwrap(),
            Some((id, Some("AAA000000002".to_owned())))
        );
        assert_eq!(db.sync_of_decision(tagged[1] + 1).unwrap(), None);
    }

    #[test]
    fn peer_keys_are_pinned_replaced_and_removed_as_decisions() {
        let mut db = Db::open_in_memory().unwrap();
        let bbb: SiteCode = "BBB".parse().unwrap();
        assert!(db.sync_peer_keys().unwrap().is_empty());
        db.sync_pin_key(bbb, "ed25519:one", "admin").unwrap();
        db.sync_pin_key(bbb, "ed25519:two", "admin2").unwrap();
        let keys = db.sync_peer_keys().unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].public_key, "ed25519:two");
        assert_eq!(keys[0].pinned_by, "admin2");
        assert!(db.sync_unpin_key(bbb, "admin").unwrap().is_some());
        assert!(db.sync_unpin_key(bbb, "admin").unwrap().is_none());
        assert!(db.sync_peer_keys().unwrap().is_empty());
        let ops: Vec<String> = db
            .connection()
            .prepare("SELECT op FROM decisions WHERE op LIKE 'sync_key%' ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        assert_eq!(ops, ["sync_key_pin", "sync_key_pin", "sync_key_remove"]);
    }
}
