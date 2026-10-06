//! Track management a track manager does by hand (OTH-GOLD's track
//! management sets): groups of tracks and pairings.
//!
//! A group (a battle group, a flight of bombers, a convoy) is published as a
//! track of its own under a UID from the site's sequence; its members are the
//! track graph's `MEMBER_OF` edges to the group's node. A pairing (GOLD PAIR)
//! says two tracks are the same object without merging them: a
//! `PAIRED_WITH` edge. Every change is a decision.

use std::collections::BTreeSet;

use ot_core::{SiteCode, Uid};
use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::graph::{self, EdgeKind, NodeKind};
use crate::sqlite::{
    Db, Decision, Result, StoreError, allocate_uid, live_system_node, now_ms, record_decision,
};

/// What a group is: its name and symbol, and anything else the UI keeps with
/// it (the symbol's parts, a description...).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupSpec {
    pub name: String,
    /// The group's symbol code (2525C, 2525D or a CoT type).
    pub sidc: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub affiliation: Option<ot_core::Affiliation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<ot_core::Domain>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl GroupSpec {
    pub fn validate(&mut self) -> Result<()> {
        self.name = self.name.trim().to_owned();
        if self.name.is_empty() || self.name.chars().count() > 128 {
            return Err(StoreError::Conflict(
                "a group's name is 1 to 128 characters".into(),
            ));
        }
        self.sidc = self.sidc.trim().to_owned();
        if ot_core::Sidc::parse(&self.sidc).is_none() {
            return Err(StoreError::Conflict(format!(
                "{:?} is not a 2525C, 2525D or CoT symbol code",
                self.sidc
            )));
        }
        Ok(())
    }
}

/// A live group with its members.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Group {
    pub uid: Uid,
    pub track_id: String,
    pub spec: GroupSpec,
    pub members: Vec<Uid>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

fn group_node(tx: &Transaction<'_>, uid: Uid, now: i64) -> Result<i64> {
    graph::upsert_node(tx, NodeKind::Group, &uid.to_string(), now)
}

fn live_group(tx: &Transaction<'_>, uid: Uid) -> Result<GroupSpec> {
    let spec: Option<String> = tx
        .query_row(
            "SELECT spec FROM track_groups WHERE uid = ?1 AND dissolved_at_ms IS NULL",
            [uid.to_string()],
            |r| r.get(0),
        )
        .optional()?;
    let spec = spec.ok_or_else(|| StoreError::NotFound(format!("group {}", uid.doc_id())))?;
    Ok(serde_json::from_str(&spec)?)
}

fn members_of(conn: &rusqlite::Connection, uid: Uid) -> Result<Vec<Uid>> {
    let mut stmt = conn.prepare(
        "SELECT s.key FROM edges e
         JOIN nodes g ON g.id = e.dst AND g.kind = 'group' AND g.key = ?1
         JOIN nodes s ON s.id = e.src
         WHERE e.kind = 'MEMBER_OF' AND e.valid_to_ms IS NULL
         ORDER BY e.valid_from_ms, e.id",
    )?;
    let keys: Vec<String> = stmt
        .query_map([uid.to_string()], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(keys.iter().filter_map(|k| k.parse().ok()).collect())
}

/// Add live members; tracks already in the group are skipped.
fn join(
    tx: &Transaction<'_>,
    group: i64,
    members: &[Uid],
    decision_id: i64,
    now: i64,
) -> Result<()> {
    for m in members {
        let node = live_system_node(tx, *m)?;
        let already: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM edges WHERE kind = 'MEMBER_OF' AND src = ?1 AND dst = ?2
                 AND valid_to_ms IS NULL)",
            params![node, group],
            |r| r.get(0),
        )?;
        if !already {
            graph::add_edge(
                tx,
                EdgeKind::MemberOf,
                node,
                group,
                decision_id,
                &json!({}),
                now,
            )?;
        }
    }
    Ok(())
}

impl Db {
    /// Form a group of live tracks, allocating its UID. Returns the UID and
    /// the decision.
    pub fn create_group(
        &mut self,
        site: SiteCode,
        spec: &GroupSpec,
        members: &[Uid],
        decision: Decision,
    ) -> Result<(Uid, i64)> {
        self.form_group(Err(site), spec, members, decision)
    }

    /// Form a group under the UID another node gave it.
    pub fn create_group_as(
        &mut self,
        uid: Uid,
        spec: &GroupSpec,
        members: &[Uid],
        decision: Decision,
    ) -> Result<(Uid, i64)> {
        self.form_group(Ok(uid), spec, members, decision)
    }

    fn form_group(
        &mut self,
        uid: std::result::Result<Uid, SiteCode>,
        spec: &GroupSpec,
        members: &[Uid],
        decision: Decision,
    ) -> Result<(Uid, i64)> {
        let mut spec = spec.clone();
        spec.validate()?;
        if members.is_empty() {
            return Err(StoreError::Conflict(
                "a group needs at least one member".into(),
            ));
        }
        self.write(|tx| {
            let now = now_ms();
            let uid = match uid {
                Ok(uid) => uid,
                Err(site) => allocate_uid(tx, site)?,
            };
            let decision_id = record_decision(
                tx,
                &decision.evidence(json!({
                    "group": uid.doc_id(), "name": spec.name,
                    "members": members.iter().map(|m| m.doc_id()).collect::<Vec<_>>(),
                })),
                now,
            )?;
            tx.execute(
                "INSERT INTO track_groups (uid, spec, created_at_ms, updated_at_ms) VALUES (?1, ?2, ?3, ?3)",
                params![uid.to_string(), serde_json::to_string(&spec)?, now],
            )?;
            let node = group_node(tx, uid, now)?;
            join(tx, node, members, decision_id, now)?;
            Ok((uid, decision_id))
        })
    }

    /// Change what a group is (its name, symbol...).
    pub fn update_group(&mut self, uid: Uid, spec: &GroupSpec, decision: Decision) -> Result<i64> {
        let mut spec = spec.clone();
        spec.validate()?;
        self.write(|tx| {
            let now = now_ms();
            let before = live_group(tx, uid)?;
            let decision_id = record_decision(
                tx,
                &decision
                    .evidence(json!({ "group": uid.doc_id() }))
                    .before(serde_json::to_value(&before)?)
                    .after(serde_json::to_value(&spec)?),
                now,
            )?;
            tx.execute(
                "UPDATE track_groups SET spec = ?2, updated_at_ms = ?3 WHERE uid = ?1",
                params![uid.to_string(), serde_json::to_string(&spec)?, now],
            )?;
            Ok(decision_id)
        })
    }

    /// Add and remove members.
    pub fn change_group_members(
        &mut self,
        uid: Uid,
        add: &[Uid],
        remove: &[Uid],
        decision: Decision,
    ) -> Result<i64> {
        self.write(|tx| {
            let now = now_ms();
            live_group(tx, uid)?;
            let decision_id = record_decision(
                tx,
                &decision.evidence(json!({
                    "group": uid.doc_id(),
                    "added": add.iter().map(|m| m.doc_id()).collect::<Vec<_>>(),
                    "removed": remove.iter().map(|m| m.doc_id()).collect::<Vec<_>>(),
                })),
                now,
            )?;
            let node = group_node(tx, uid, now)?;
            join(tx, node, add, decision_id, now)?;
            for m in remove {
                tx.execute(
                    "UPDATE edges SET valid_to_ms = max(?3, valid_from_ms), ended_by = ?4
                     WHERE kind = 'MEMBER_OF' AND dst = ?2 AND valid_to_ms IS NULL
                       AND src IN (SELECT id FROM nodes WHERE kind = 'system_track' AND key = ?1)",
                    params![m.to_string(), node, now, decision_id],
                )?;
            }
            tx.execute(
                "UPDATE track_groups SET updated_at_ms = ?2 WHERE uid = ?1",
                params![uid.to_string(), now],
            )?;
            Ok(decision_id)
        })
    }

    /// Dissolve a group: its members leave it, and it is no longer published.
    pub fn dissolve_group(&mut self, uid: Uid, decision: Decision) -> Result<i64> {
        self.write(|tx| {
            let now = now_ms();
            let before = live_group(tx, uid)?;
            let decision_id = record_decision(
                tx,
                &decision
                    .evidence(json!({ "group": uid.doc_id(), "name": before.name }))
                    .before(serde_json::to_value(&before)?),
                now,
            )?;
            let node = group_node(tx, uid, now)?;
            tx.execute(
                "UPDATE edges SET valid_to_ms = max(?2, valid_from_ms), ended_by = ?3
                 WHERE kind = 'MEMBER_OF' AND dst = ?1 AND valid_to_ms IS NULL",
                params![node, now, decision_id],
            )?;
            tx.execute(
                "UPDATE track_groups SET dissolved_at_ms = ?2, updated_at_ms = ?2 WHERE uid = ?1",
                params![uid.to_string(), now],
            )?;
            Ok(decision_id)
        })
    }

    /// Every live group with its members.
    pub fn groups(&self) -> Result<Vec<Group>> {
        let mut stmt = self.connection().prepare(
            "SELECT uid, spec, created_at_ms, updated_at_ms FROM track_groups
             WHERE dissolved_at_ms IS NULL ORDER BY created_at_ms",
        )?;
        let rows: Vec<(String, String, i64, i64)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<_>>()?;
        rows.into_iter()
            .filter_map(|(uid, spec, created_at_ms, updated_at_ms)| {
                let uid: Uid = uid.parse().ok()?;
                Some((uid, spec, created_at_ms, updated_at_ms))
            })
            .map(|(uid, spec, created_at_ms, updated_at_ms)| {
                Ok(Group {
                    uid,
                    track_id: uid.doc_id(),
                    spec: serde_json::from_str(&spec)?,
                    members: members_of(self.connection(), uid)?,
                    created_at_ms,
                    updated_at_ms,
                })
            })
            .collect()
    }

    /// Pair live tracks with each other (GOLD PAIR): each pair not already
    /// paired gets a `PAIRED_WITH` edge.
    pub fn pair_system_tracks(&mut self, uids: &[Uid], decision: Decision) -> Result<i64> {
        let set: BTreeSet<Uid> = uids.iter().copied().collect();
        if set.len() < 2 {
            return Err(StoreError::Conflict(
                "pairing needs at least two tracks".into(),
            ));
        }
        self.write(|tx| {
            let now = now_ms();
            let decision_id = record_decision(
                tx,
                &decision.evidence(json!({
                    "tracks": set.iter().map(|u| u.doc_id()).collect::<Vec<_>>(),
                })),
                now,
            )?;
            let nodes: Vec<i64> = set
                .iter()
                .map(|u| live_system_node(tx, *u))
                .collect::<Result<_>>()?;
            for (i, a) in nodes.iter().enumerate() {
                for b in &nodes[i + 1..] {
                    let exists: bool = tx.query_row(
                        "SELECT EXISTS (SELECT 1 FROM edges WHERE kind = 'PAIRED_WITH' AND valid_to_ms IS NULL
                             AND ((src = ?1 AND dst = ?2) OR (src = ?2 AND dst = ?1)))",
                        params![a, b],
                        |r| r.get(0),
                    )?;
                    if !exists {
                        graph::add_edge(tx, EdgeKind::PairedWith, *a, *b, decision_id, &json!({}), now)?;
                    }
                }
            }
            Ok(decision_id)
        })
    }

    /// End a pairing between two tracks.
    pub fn unpair_system_tracks(&mut self, a: Uid, b: Uid, decision: Decision) -> Result<i64> {
        self.write(|tx| {
            let now = now_ms();
            let decision_id = record_decision(
                tx,
                &decision.evidence(json!({ "tracks": [a.doc_id(), b.doc_id()] })),
                now,
            )?;
            let n = tx.execute(
                "UPDATE edges SET valid_to_ms = max(?3, valid_from_ms), ended_by = ?4
                 WHERE kind = 'PAIRED_WITH' AND valid_to_ms IS NULL AND
                   ((src = (SELECT id FROM nodes WHERE kind = 'system_track' AND key = ?1)
                     AND dst = (SELECT id FROM nodes WHERE kind = 'system_track' AND key = ?2)) OR
                    (src = (SELECT id FROM nodes WHERE kind = 'system_track' AND key = ?2)
                     AND dst = (SELECT id FROM nodes WHERE kind = 'system_track' AND key = ?1)))",
                params![a.to_string(), b.to_string(), now, decision_id],
            )?;
            if n == 0 {
                return Err(StoreError::NotFound(format!(
                    "a pairing of {} and {}",
                    a.doc_id(),
                    b.doc_id()
                )));
            }
            Ok(decision_id)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::Decision;

    fn spec(name: &str) -> GroupSpec {
        serde_json::from_value(
            json!({"name": name, "sidc": "SHSPGG----", "affiliation": "hostile",
                                       "echelon": "G"}),
        )
        .unwrap()
    }

    #[test]
    fn groups_pairs_and_merges() {
        let mut db = Db::open_in_memory().unwrap();
        let site: SiteCode = "TST".parse().unwrap();
        let d = || Decision::new("op:test", "test");
        let t: Vec<Uid> = (0..4)
            .map(|i| {
                db.create_system_track(site, "ais", &i.to_string(), d())
                    .unwrap()
                    .0
            })
            .collect();

        let (g, _) = db
            .create_group(site, &spec("CSG 12"), &t[..2], d())
            .unwrap();
        assert!(t.iter().all(|u| *u != g), "a group has its own UID");
        let groups = db.groups().unwrap();
        assert_eq!(groups[0].members, &t[..2]);
        assert_eq!(groups[0].spec.extra["echelon"], "G");
        assert!(db.create_group(site, &spec(" "), &t[..1], d()).is_err());
        let mut bad = spec("x");
        bad.sidc = "nonsense".into();
        assert!(db.create_group(site, &bad, &t[..1], d()).is_err());

        db.change_group_members(g, &[t[2]], &[t[0]], d()).unwrap();
        assert_eq!(db.groups().unwrap()[0].members, [t[1], t[2]]);

        db.pair_system_tracks(&[t[1], t[3]], d()).unwrap();
        db.pair_system_tracks(&[t[3], t[1]], d()).unwrap();
        let paired = |db: &Db| -> i64 {
            db.connection()
                .query_row(
                    "SELECT count(*) FROM edges WHERE kind = 'PAIRED_WITH' AND valid_to_ms IS NULL",
                    [],
                    |r| r.get(0),
                )
                .unwrap()
        };
        assert_eq!(paired(&db), 1, "pairing twice adds nothing");

        // A merge carries the group and the pairing to the surviving track.
        db.merge_system_tracks(t[1], t[0], d()).unwrap();
        assert_eq!(db.groups().unwrap()[0].members, [t[2], t[0]]);
        db.unpair_system_tracks(t[3], t[0], d()).unwrap();
        assert_eq!(paired(&db), 0);
        assert!(db.unpair_system_tracks(t[3], t[0], d()).is_err());

        db.dissolve_group(g, d()).unwrap();
        assert!(db.groups().unwrap().is_empty());
    }
}
