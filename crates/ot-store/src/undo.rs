//! Undoing a track manager's decision. The track graph is temporal: every
//! decision closes the edges it ends (`ended_by`) and opens edges stamped
//! with its id. Undo reverses exactly that, in one transaction, as a new
//! decision (`undo`, linked both ways with `undoes` / `undone_by`):
//!
//! - edges the decision opened and that are still live are closed;
//! - edges it closed open again, as new edges stamped with the undo (so
//!   the history still shows what happened and when);
//! - a source track that has since reported elsewhere (the engine made a
//!   new track for it after a delete) moves back, and a system track left
//!   with no source tracks retires;
//! - a track it retired comes back under its UID; a group it formed
//!   dissolves, one it dissolved re-forms, and a group change takes the
//!   group's spec back.
//!
//! A decision someone made since on the same tracks blocks the undo (undo
//! that first); what the engine did on its own since does not.

use std::collections::BTreeSet;

use ot_core::Uid;
use rusqlite::{OptionalExtension, Transaction, params};
use serde::Serialize;
use serde_json::{Value, json};

use crate::graph::{self, EdgeKind};
use crate::sqlite::{Db, Decision, Result, StoreError, now_ms, record_decision};

/// Track management decisions an undo can reverse.
pub const UNDOABLE: &[&str] = &[
    "pair_tracks",
    "unpair_tracks",
    "delete_track",
    "merge",
    "split",
    "do_not_pair",
    "create_group",
    "update_group",
    "group_members",
    "dissolve_group",
];

/// Actors whose decisions are the engine's own: never undone here, never
/// a reason to refuse an undo.
const AUTOMATIC: &[&str] = &["engine", "system"];

/// What an undo changed, for the engine to bring its picture in line.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Undone {
    /// The undo's own decision.
    pub decision_id: i64,
    pub undoes: i64,
    pub op: String,
    /// Every system track whose source tracks, pairings or groups changed.
    pub tracks: Vec<Uid>,
    /// System tracks that are live again.
    pub revived: Vec<Uid>,
    /// System tracks that retired (left with no source tracks).
    pub retired: Vec<Uid>,
    pub groups_changed: bool,
    pub do_not_pair_changed: bool,
}

struct Row {
    id: i64,
    actor: String,
    op: String,
    before: Option<Value>,
    undone_by: Option<i64>,
}

fn decision_row(tx: &Transaction<'_>, id: i64) -> Result<Option<Row>> {
    Ok(tx
        .query_row(
            "SELECT id, actor, op, before, undone_by FROM decisions WHERE id = ?1",
            [id],
            |r| {
                let before: Option<String> = r.get(3)?;
                Ok(Row {
                    id: r.get(0)?,
                    actor: r.get(1)?,
                    op: r.get(2)?,
                    before: before.and_then(|b| serde_json::from_str(&b).ok()),
                    undone_by: r.get(4)?,
                })
            },
        )
        .optional()?)
}

struct Edge {
    kind: String,
    src: i64,
    dst: i64,
    attrs: String,
    live: bool,
}

fn edges(tx: &Transaction<'_>, sql_where: &str, id: i64) -> Result<Vec<Edge>> {
    let mut st = tx.prepare(&format!(
        "SELECT kind, src, dst, attrs, valid_to_ms IS NULL FROM edges WHERE {sql_where}"
    ))?;
    let rows = st.query_map([id], |r| {
        Ok(Edge {
            kind: r.get(0)?,
            src: r.get(1)?,
            dst: r.get(2)?,
            attrs: r.get(3)?,
            live: r.get(4)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// (kind, key) of a node.
fn node(tx: &Transaction<'_>, id: i64) -> Result<(String, String)> {
    Ok(
        tx.query_row("SELECT kind, key FROM nodes WHERE id = ?1", [id], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?,
    )
}

fn edge_kind(s: &str) -> Option<EdgeKind> {
    Some(match s {
        "REPORTS_FOR" => EdgeKind::ReportsFor,
        "MERGED_INTO" => EdgeKind::MergedInto,
        "DO_NOT_PAIR" => EdgeKind::DoNotPair,
        "MEMBER_OF" => EdgeKind::MemberOf,
        "PAIRED_WITH" => EdgeKind::PairedWith,
        _ => return None,
    })
}

impl Db {
    /// Undo decision `id` (and the decisions made with it, such as a
    /// split's "do not pair"), recording who did and why.
    pub fn undo_decision(&mut self, id: i64, actor: &str, reason: &str) -> Result<Undone> {
        self.write(|tx| {
            let d = decision_row(tx, id)?
                .ok_or_else(|| StoreError::NotFound(format!("decision {id}")))?;
            if !UNDOABLE.contains(&d.op.as_str()) {
                return Err(StoreError::Conflict(format!(
                    "a {} cannot be undone (only track management: {})",
                    d.op,
                    UNDOABLE.join(", ")
                )));
            }
            if AUTOMATIC.contains(&d.actor.as_str()) {
                return Err(StoreError::Conflict(format!(
                    "decision {id} was the engine's own; split or merge the tracks instead"
                )));
            }
            if let Some(u) = d.undone_by {
                return Err(StoreError::Conflict(format!(
                    "decision {id} was undone already (decision {u})"
                )));
            }
            // A split records its "do not pair" as a second decision.
            let mut ids = vec![d.id];
            let mut st = tx.prepare(
                "SELECT id FROM decisions WHERE json_extract(evidence, '$.with_decision') = ?1
                   AND undone_by IS NULL",
            )?;
            ids.extend(
                st.query_map([id], |r| r.get::<_, i64>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?,
            );
            drop(st);

            let (mut opened, mut closed) = (Vec::new(), Vec::new());
            for &i in &ids {
                opened.extend(edges(tx, "decision_id = ?1", i)?);
                closed.extend(edges(tx, "ended_by = ?1", i)?);
            }
            let touched: BTreeSet<i64> = opened
                .iter()
                .chain(&closed)
                .flat_map(|e| [e.src, e.dst])
                .collect();
            blockers(tx, &ids, &touched)?;

            let now = now_ms();
            let undo = record_decision(
                tx,
                &Decision::new(actor, "undo")
                    .reason(reason)
                    .evidence(json!({ "undoes": id, "op": d.op })),
                now,
            )?;
            for &i in &ids {
                tx.execute(
                    "UPDATE decisions SET undone_by = ?2 WHERE id = ?1",
                    params![i, undo],
                )?;
            }
            tx.execute(
                "UPDATE decisions SET undoes = ?2 WHERE id = ?1",
                params![undo, id],
            )?;

            let mut out = Undone {
                decision_id: undo,
                undoes: id,
                op: d.op.clone(),
                ..Default::default()
            };
            let mut systems: BTreeSet<i64> = BTreeSet::new();
            // Close what it opened.
            for &i in &ids {
                tx.execute(
                    "UPDATE edges SET valid_to_ms = max(?2, valid_from_ms), ended_by = ?3
                     WHERE decision_id = ?1 AND valid_to_ms IS NULL",
                    params![i, now, undo],
                )?;
            }
            // Open again what it closed.
            for e in closed.iter().filter(|e| !e.live) {
                let Some(kind) = edge_kind(&e.kind) else {
                    continue;
                };
                if kind == EdgeKind::ReportsFor {
                    // The source track may report elsewhere now.
                    let mut st = tx.prepare(
                        "SELECT dst FROM edges WHERE src = ?1 AND kind = 'REPORTS_FOR' AND valid_to_ms IS NULL",
                    )?;
                    let now_on: Vec<i64> = st
                        .query_map([e.src], |r| r.get(0))?
                        .collect::<rusqlite::Result<_>>()?;
                    drop(st);
                    systems.extend(&now_on);
                    tx.execute(
                        "UPDATE edges SET valid_to_ms = max(?2, valid_from_ms), ended_by = ?3
                         WHERE src = ?1 AND kind = 'REPORTS_FOR' AND valid_to_ms IS NULL",
                        params![e.src, now, undo],
                    )?;
                }
                let exists: bool = tx.query_row(
                    "SELECT EXISTS (SELECT 1 FROM edges WHERE kind = ?1 AND src = ?2 AND dst = ?3
                       AND valid_to_ms IS NULL)",
                    params![e.kind, e.src, e.dst],
                    |r| r.get(0),
                )?;
                if !exists {
                    let attrs: Value = serde_json::from_str(&e.attrs).unwrap_or(json!({}));
                    graph::add_edge(tx, kind, e.src, e.dst, undo, &attrs, now)?;
                }
            }
            for &n in &touched {
                if node(tx, n)?.0 == "system_track" {
                    systems.insert(n);
                }
            }
            // Tracks with source tracks are live; those with none retire.
            for &n in &systems {
                let (_, key) = node(tx, n)?;
                let Ok(uid) = key.parse::<Uid>() else {
                    continue;
                };
                let (sources, retired): (i64, bool) = tx.query_row(
                    "SELECT (SELECT count(*) FROM edges WHERE dst = ?1 AND kind = 'REPORTS_FOR'
                             AND valid_to_ms IS NULL),
                            (SELECT retired_at_ms IS NOT NULL FROM nodes WHERE id = ?1)",
                    [n],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?;
                if sources > 0 && retired {
                    tx.execute("UPDATE nodes SET retired_at_ms = NULL WHERE id = ?1", [n])?;
                    out.revived.push(uid);
                } else if sources == 0 && !retired {
                    tx.execute(
                        "UPDATE edges SET valid_to_ms = max(?2, valid_from_ms), ended_by = ?3
                         WHERE (src = ?1 OR dst = ?1) AND valid_to_ms IS NULL",
                        params![n, now, undo],
                    )?;
                    tx.execute(
                        "UPDATE nodes SET retired_at_ms = ?2 WHERE id = ?1",
                        params![n, now],
                    )?;
                    out.retired.push(uid);
                }
                out.tracks.push(uid);
            }
            // Name the tracks it touched, as other decisions do.
            let names: Vec<String> = out.tracks.iter().map(|u| u.doc_id()).collect();
            tx.execute(
                "UPDATE decisions SET evidence = json_set(evidence, '$.tracks', json(?2)) WHERE id = ?1",
                params![undo, serde_json::to_string(&names)?],
            )?;
            out.do_not_pair_changed = opened
                .iter()
                .chain(&closed)
                .any(|e| e.kind == "DO_NOT_PAIR");
            out.groups_changed = groups(tx, &d, now)?
                || opened
                    .iter()
                    .chain(&closed)
                    .any(|e| e.kind == "MEMBER_OF");
            Ok(out)
        })
    }
}

impl Db {
    /// A system track's live source tracks (`<source>/<key>`) and each
    /// link's attributes (pairing, confidence).
    pub fn reports_for(&self, uid: Uid) -> Result<Vec<(String, Value)>> {
        let mut st = self.connection().prepare(
            "SELECT s.key, e.attrs FROM edges e JOIN nodes s ON s.id = e.src
             JOIN nodes d ON d.id = e.dst
             WHERE e.kind = 'REPORTS_FOR' AND e.valid_to_ms IS NULL
               AND d.kind = 'system_track' AND d.key = ?1",
        )?;
        let rows = st.query_map([uid.to_string()], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        rows.map(|r| {
            let (k, a) = r?;
            Ok((k, serde_json::from_str(&a).unwrap_or(Value::Null)))
        })
        .collect()
    }

    /// The tracks a system track is paired with (GOLD PAIR), live.
    pub fn paired_with(&self, uid: Uid) -> Result<Vec<Uid>> {
        let mut st = self.connection().prepare(
            "SELECT CASE WHEN a.key = ?1 THEN b.key ELSE a.key END FROM edges e
             JOIN nodes a ON a.id = e.src JOIN nodes b ON b.id = e.dst
             WHERE e.kind = 'PAIRED_WITH' AND e.valid_to_ms IS NULL AND (a.key = ?1 OR b.key = ?1)",
        )?;
        let rows = st.query_map([uid.to_string()], |r| r.get::<_, String>(0))?;
        Ok(rows
            .filter_map(|r| r.ok().and_then(|k| k.parse().ok()))
            .collect())
    }
}

/// Refuse when someone made a decision since that touched the same nodes.
fn blockers(tx: &Transaction<'_>, ids: &[i64], touched: &BTreeSet<i64>) -> Result<()> {
    let first = ids.iter().copied().min().unwrap_or_default();
    let nodes = serde_json::to_string(&touched.iter().collect::<Vec<_>>())?;
    let ours = serde_json::to_string(ids)?;
    let later: Option<(i64, String, String)> = tx
        .query_row(
            "SELECT d.id, d.op, d.actor FROM decisions d
             WHERE d.id > ?1 AND d.id NOT IN (SELECT value FROM json_each(?3))
               AND d.undone_by IS NULL AND d.op <> 'undo'
               AND d.actor NOT IN ('engine', 'system')
               AND EXISTS (SELECT 1 FROM edges e
                   WHERE (e.decision_id = d.id OR e.ended_by = d.id)
                     AND (e.src IN (SELECT value FROM json_each(?2))
                          OR e.dst IN (SELECT value FROM json_each(?2))))
             ORDER BY d.id LIMIT 1",
            params![first, nodes, ours],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    if let Some((id, op, actor)) = later {
        return Err(StoreError::Conflict(format!(
            "{actor} changed these tracks since ({op}, decision {id}): undo that first"
        )));
    }
    Ok(())
}

/// A group's own row: formed, dissolved or changed. Whether it changed.
fn groups(tx: &Transaction<'_>, d: &Row, now: i64) -> Result<bool> {
    let group: Option<String> = tx.query_row(
        "SELECT json_extract(evidence, '$.group') FROM decisions WHERE id = ?1",
        [d.id],
        |r| r.get(0),
    )?;
    let Some(uid) = group.and_then(|g| Uid::from_doc_id(&g).ok()) else {
        return Ok(false);
    };
    let key = uid.to_string();
    match d.op.as_str() {
        "create_group" => {
            tx.execute(
                "UPDATE track_groups SET dissolved_at_ms = ?2, updated_at_ms = ?2 WHERE uid = ?1",
                params![key, now],
            )?;
        }
        "dissolve_group" => {
            tx.execute(
                "UPDATE track_groups SET dissolved_at_ms = NULL, updated_at_ms = ?2 WHERE uid = ?1",
                params![key, now],
            )?;
        }
        "update_group" => {
            if let Some(before) = &d.before {
                tx.execute(
                    "UPDATE track_groups SET spec = ?2, updated_at_ms = ?3 WHERE uid = ?1",
                    params![key, before.to_string(), now],
                )?;
            }
        }
        _ => {}
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ot_core::SiteCode;

    fn db() -> (Db, SiteCode) {
        (Db::open_in_memory().unwrap(), SiteCode::new("TST").unwrap())
    }

    fn op(o: &str) -> Decision {
        Decision::new("ann@x.org", o)
    }

    fn reports(db: &Db) -> Vec<(String, Uid)> {
        let mut r = db.live_reports().unwrap();
        r.sort();
        r
    }

    #[test]
    fn a_delete_comes_back_under_its_uid() {
        let (mut db, site) = db();
        let (u, _) = db
            .create_system_track(
                site,
                "ais",
                "1",
                Decision::new("engine", "create_system_track"),
            )
            .unwrap();
        let del = db.retire_system_track(u, op("delete_track")).unwrap();
        // The feed goes on: the engine makes a new track for it.
        let (n, _) = db
            .create_system_track(
                site,
                "ais",
                "1",
                Decision::new("engine", "create_system_track"),
            )
            .unwrap();
        assert_eq!(reports(&db), vec![("ais/1".into(), n)]);

        let undone = db
            .undo_decision(del, "bob@x.org", "deleted by mistake")
            .unwrap();
        assert_eq!(reports(&db), vec![("ais/1".into(), u)]);
        assert_eq!(undone.revived, vec![u]);
        assert_eq!(undone.retired, vec![n]);
        // Once only.
        assert!(matches!(
            db.undo_decision(del, "bob@x.org", "again"),
            Err(StoreError::Conflict(_))
        ));
    }

    #[test]
    fn a_merge_undone_gives_each_track_its_sources_back() {
        let (mut db, site) = db();
        let e = || Decision::new("engine", "create_system_track");
        let (a, _) = db.create_system_track(site, "ais", "1", e()).unwrap();
        let (b, _) = db.create_system_track(site, "radar", "7", e()).unwrap();
        let m = db.merge_system_tracks(a, b, op("merge")).unwrap();
        assert_eq!(reports(&db).len(), 2);
        assert!(reports(&db).iter().all(|(_, u)| *u == b));
        let undone = db.undo_decision(m, "ann@x.org", "not the same").unwrap();
        assert_eq!(
            reports(&db),
            vec![("ais/1".into(), a), ("radar/7".into(), b)]
        );
        assert!(undone.tracks.contains(&a) && undone.tracks.contains(&b));
    }

    #[test]
    fn a_split_undone_rejoins_and_forgets_its_do_not_pair() {
        let (mut db, site) = db();
        let e = || Decision::new("engine", "create_system_track");
        let (a, _) = db.create_system_track(site, "ais", "1", e()).unwrap();
        let (b, _) = db.create_system_track(site, "radar", "7", e()).unwrap();
        db.merge_system_tracks(a, b, Decision::new("engine", "merge"))
            .unwrap();
        let (new, split) = db
            .split_source_track(site, "ais", "1", op("split"))
            .unwrap();
        db.do_not_pair(
            &["ais/1".into()],
            &["radar/7".into()],
            op("do_not_pair").evidence(json!({ "with_decision": split })),
        )
        .unwrap();
        assert_eq!(db.do_not_pairs().unwrap().len(), 1);
        let undone = db
            .undo_decision(split, "ann@x.org", "they are one")
            .unwrap();
        assert!(reports(&db).iter().all(|(_, u)| *u == b));
        assert!(db.do_not_pairs().unwrap().is_empty());
        assert_eq!(undone.retired, vec![new]);
        assert!(undone.do_not_pair_changed);
    }

    #[test]
    fn a_later_decision_on_the_same_tracks_blocks_the_undo() {
        let (mut db, site) = db();
        let e = || Decision::new("engine", "create_system_track");
        let (a, _) = db.create_system_track(site, "ais", "1", e()).unwrap();
        let (b, _) = db.create_system_track(site, "radar", "7", e()).unwrap();
        let pair = db.pair_system_tracks(&[a, b], op("pair_tracks")).unwrap();
        let unpair = db.unpair_system_tracks(a, b, op("unpair_tracks")).unwrap();
        let err = db.undo_decision(pair, "ann@x.org", "x").unwrap_err();
        assert!(
            err.to_string().contains(&format!("decision {unpair}")),
            "{err}"
        );
        // Undo the later one, then the earlier.
        db.undo_decision(unpair, "ann@x.org", "x").unwrap();
        db.undo_decision(pair, "ann@x.org", "x").unwrap();
    }

    #[test]
    fn only_track_management_by_people_is_undone() {
        let (mut db, site) = db();
        let (_, created) = db
            .create_system_track(
                site,
                "ais",
                "1",
                Decision::new("engine", "create_system_track"),
            )
            .unwrap();
        assert!(db.undo_decision(created, "ann@x.org", "x").is_err());
        let s = db.record(&op("app_settings")).unwrap();
        assert!(db.undo_decision(s, "ann@x.org", "x").is_err());
        assert!(matches!(
            db.undo_decision(9999, "ann@x.org", "x"),
            Err(StoreError::NotFound(_))
        ));
    }
}
