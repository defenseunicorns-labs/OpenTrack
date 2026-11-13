//! PostgreSQL group reads.

use diesel::prelude::*;
use diesel::sql_types::{BigInt, Bool, Text};
use ot_core::{SiteCode, Uid};
use serde_json::json;
use std::collections::BTreeSet;

use super::graph::{add_edge, live_system_node, upsert_node};
use super::{PgControlStore, PostgresError, record_decision};
use crate::Decision;
use crate::graph::{EdgeKind, NodeKind};
use crate::groups::{Group, GroupSpec};

#[derive(QueryableByName)]
struct GroupRow {
    #[diesel(sql_type = Text)]
    uid: String,
    #[diesel(sql_type = Text)]
    spec: String,
    #[diesel(sql_type = BigInt)]
    created_at_ms: i64,
    #[diesel(sql_type = BigInt)]
    updated_at_ms: i64,
}

#[derive(QueryableByName)]
struct MemberRow {
    #[diesel(sql_type = Text)]
    key: String,
}

#[derive(QueryableByName)]
struct SequenceRow {
    #[diesel(sql_type = BigInt)]
    sequence: i64,
}

#[derive(QueryableByName)]
struct ExistsRow {
    #[diesel(sql_type = Bool)]
    exists: bool,
}

#[derive(QueryableByName)]
struct SpecRow {
    #[diesel(sql_type = Text)]
    spec: String,
}

fn group_node(
    connection: &mut diesel::pg::PgConnection,
    uid: Uid,
    now: i64,
) -> Result<i64, PostgresError> {
    upsert_node(connection, NodeKind::Group, &uid.to_string(), now)
}

fn live_group(
    connection: &mut diesel::pg::PgConnection,
    uid: Uid,
) -> Result<GroupSpec, PostgresError> {
    let row: Option<SpecRow> = diesel::sql_query(
        "SELECT spec FROM track_groups WHERE uid = $1 AND dissolved_at_ms IS NULL",
    )
    .bind::<Text, _>(uid.to_string())
    .get_result(connection)
    .optional()?;
    let row = row.ok_or_else(|| PostgresError::NotFound(format!("group {}", uid.doc_id())))?;
    Ok(serde_json::from_str(&row.spec)?)
}

fn join(
    connection: &mut diesel::pg::PgConnection,
    group: i64,
    members: &[Uid],
    decision_id: i64,
    now: i64,
) -> Result<(), PostgresError> {
    for uid in members {
        let node = live_system_node(connection, *uid)?;
        let row: ExistsRow = diesel::sql_query(
            "SELECT EXISTS (SELECT 1 FROM edges WHERE kind = 'MEMBER_OF' AND src = $1 AND dst = $2 AND valid_to_ms IS NULL) AS exists"
        ).bind::<BigInt, _>(node).bind::<BigInt, _>(group).get_result(connection)?;
        if !row.exists {
            add_edge(
                connection,
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

impl PgControlStore {
    pub fn form_group(
        &mut self,
        uid: Result<Uid, SiteCode>,
        spec: &GroupSpec,
        members: &[Uid],
        decision: &Decision,
    ) -> Result<(Uid, i64), PostgresError> {
        self.connection.transaction::<(Uid, i64), PostgresError, _>(|connection| {
            let now = crate::sqlite::now_ms();
            let uid = match uid {
                Ok(uid) => uid,
                Err(site) => {
                    let row: SequenceRow = diesel::sql_query(
                        "INSERT INTO uid_sequences (site, next_sequence) VALUES ($1, 2)
                         ON CONFLICT (site) DO UPDATE SET next_sequence = uid_sequences.next_sequence + 1
                         RETURNING next_sequence - 1 AS sequence"
                    ).bind::<Text, _>(site.as_str()).get_result(connection)?;
                    Uid::new(site, row.sequence as u64)?
                }
            };
            let decision_id = record_decision(connection, &decision.clone().evidence(json!({
                "group": uid.doc_id(), "name": spec.name,
                "members": members.iter().map(|m| m.doc_id()).collect::<Vec<_>>(),
            })))?;
            diesel::sql_query("INSERT INTO track_groups (uid, spec, created_at_ms, updated_at_ms) VALUES ($1,$2,$3,$3)")
                .bind::<Text, _>(uid.to_string()).bind::<Text, _>(serde_json::to_string(spec)?)
                .bind::<BigInt, _>(now).execute(connection)?;
            let node = group_node(connection, uid, now)?;
            join(connection, node, members, decision_id, now)?;
            Ok((uid, decision_id))
        })
    }

    pub fn update_group(
        &mut self,
        uid: Uid,
        spec: &GroupSpec,
        decision: &Decision,
    ) -> Result<i64, PostgresError> {
        self.connection
            .transaction::<i64, PostgresError, _>(|connection| {
                let now = crate::sqlite::now_ms();
                let before = live_group(connection, uid)?;
                let id = record_decision(
                    connection,
                    &decision
                        .clone()
                        .evidence(json!({"group":uid.doc_id()}))
                        .before(serde_json::to_value(before)?)
                        .after(serde_json::to_value(spec)?),
                )?;
                diesel::sql_query(
                    "UPDATE track_groups SET spec = $2, updated_at_ms = $3 WHERE uid = $1",
                )
                .bind::<Text, _>(uid.to_string())
                .bind::<Text, _>(serde_json::to_string(spec)?)
                .bind::<BigInt, _>(now)
                .execute(connection)?;
                Ok(id)
            })
    }

    pub fn change_group_members(
        &mut self,
        uid: Uid,
        add: &[Uid],
        remove: &[Uid],
        decision: &Decision,
    ) -> Result<i64, PostgresError> {
        self.connection
            .transaction::<i64, PostgresError, _>(|connection| {
                let now = crate::sqlite::now_ms();
                live_group(connection, uid)?;
                let id = record_decision(connection, &decision.clone().evidence(json!({
                "group": uid.doc_id(), "added": add.iter().map(|m| m.doc_id()).collect::<Vec<_>>(),
                "removed": remove.iter().map(|m| m.doc_id()).collect::<Vec<_>>(),
            })))?;
                let group = group_node(connection, uid, now)?;
                join(connection, group, add, id, now)?;
                for member in remove {
                    diesel::sql_query(
                        "UPDATE edges SET valid_to_ms = greatest($3, valid_from_ms), ended_by = $4
                     WHERE kind = 'MEMBER_OF' AND dst = $2 AND valid_to_ms IS NULL
                     AND src IN (SELECT id FROM nodes WHERE kind = 'system_track' AND key = $1)",
                    )
                    .bind::<Text, _>(member.to_string())
                    .bind::<BigInt, _>(group)
                    .bind::<BigInt, _>(now)
                    .bind::<BigInt, _>(id)
                    .execute(connection)?;
                }
                diesel::sql_query("UPDATE track_groups SET updated_at_ms = $2 WHERE uid = $1")
                    .bind::<Text, _>(uid.to_string())
                    .bind::<BigInt, _>(now)
                    .execute(connection)?;
                Ok(id)
            })
    }

    pub fn dissolve_group(&mut self, uid: Uid, decision: &Decision) -> Result<i64, PostgresError> {
        self.connection.transaction::<i64, PostgresError, _>(|connection| {
            let now = crate::sqlite::now_ms();
            let before = live_group(connection, uid)?;
            let id = record_decision(connection, &decision.clone()
                .evidence(json!({"group": uid.doc_id(), "name": before.name}))
                .before(serde_json::to_value(before)?))?;
            let group = group_node(connection, uid, now)?;
            diesel::sql_query("UPDATE edges SET valid_to_ms = greatest($2, valid_from_ms), ended_by = $3 WHERE kind = 'MEMBER_OF' AND dst = $1 AND valid_to_ms IS NULL")
                .bind::<BigInt, _>(group).bind::<BigInt, _>(now).bind::<BigInt, _>(id).execute(connection)?;
            diesel::sql_query("UPDATE track_groups SET dissolved_at_ms = $2, updated_at_ms = $2 WHERE uid = $1")
                .bind::<Text, _>(uid.to_string()).bind::<BigInt, _>(now).execute(connection)?;
            Ok(id)
        })
    }

    pub fn pair_system_tracks(
        &mut self,
        uids: &BTreeSet<Uid>,
        decision: &Decision,
    ) -> Result<i64, PostgresError> {
        self.connection.transaction::<i64, PostgresError, _>(|connection| {
            let now = crate::sqlite::now_ms();
            let id = record_decision(connection, &decision.clone().evidence(json!({
                "tracks": uids.iter().map(|u| u.doc_id()).collect::<Vec<_>>()
            })))?;
            let nodes: Vec<i64> = uids.iter().map(|u| live_system_node(connection, *u)).collect::<Result<_, _>>()?;
            for (index, a) in nodes.iter().enumerate() {
                for b in &nodes[index+1..] {
                    let row: ExistsRow = diesel::sql_query(
                        "SELECT EXISTS (SELECT 1 FROM edges WHERE kind = 'PAIRED_WITH' AND valid_to_ms IS NULL AND ((src = $1 AND dst = $2) OR (src = $2 AND dst = $1))) AS exists"
                    ).bind::<BigInt, _>(*a).bind::<BigInt, _>(*b).get_result(connection)?;
                    if !row.exists { add_edge(connection, EdgeKind::PairedWith, *a, *b, id, &json!({}), now)?; }
                }
            }
            Ok(id)
        })
    }

    pub fn unpair_system_tracks(
        &mut self,
        a: Uid,
        b: Uid,
        decision: &Decision,
    ) -> Result<i64, PostgresError> {
        self.connection
            .transaction::<i64, PostgresError, _>(|connection| {
                let now = crate::sqlite::now_ms();
                let id = record_decision(
                    connection,
                    &decision
                        .clone()
                        .evidence(json!({"tracks":[a.doc_id(), b.doc_id()]})),
                )?;
                let changed = diesel::sql_query(
                    "UPDATE edges SET valid_to_ms = greatest($3, valid_from_ms), ended_by = $4
                 WHERE kind = 'PAIRED_WITH' AND valid_to_ms IS NULL AND
                 ((src = (SELECT id FROM nodes WHERE kind = 'system_track' AND key = $1)
                   AND dst = (SELECT id FROM nodes WHERE kind = 'system_track' AND key = $2)) OR
                  (src = (SELECT id FROM nodes WHERE kind = 'system_track' AND key = $2)
                   AND dst = (SELECT id FROM nodes WHERE kind = 'system_track' AND key = $1)))",
                )
                .bind::<Text, _>(a.to_string())
                .bind::<Text, _>(b.to_string())
                .bind::<BigInt, _>(now)
                .bind::<BigInt, _>(id)
                .execute(connection)?;
                if changed == 0 {
                    return Err(PostgresError::NotFound(format!(
                        "a pairing of {} and {}",
                        a.doc_id(),
                        b.doc_id()
                    )));
                }
                Ok(id)
            })
    }
    pub fn groups(&mut self) -> Result<Vec<Group>, PostgresError> {
        let rows: Vec<GroupRow> = diesel::sql_query(
            "SELECT uid, spec, created_at_ms, updated_at_ms FROM track_groups
             WHERE dissolved_at_ms IS NULL ORDER BY created_at_ms",
        )
        .load(&mut self.connection)?;
        let mut groups = Vec::with_capacity(rows.len());
        for row in rows {
            let Ok(uid) = row.uid.parse::<Uid>() else {
                continue;
            };
            let members: Vec<MemberRow> = diesel::sql_query(
                "SELECT n.key FROM edges e
                 JOIN nodes n ON n.id = e.src JOIN nodes g ON g.id = e.dst
                 WHERE e.kind = 'MEMBER_OF' AND e.valid_to_ms IS NULL
                   AND g.kind = 'group' AND g.key = $1 ORDER BY n.key",
            )
            .bind::<Text, _>(uid.to_string())
            .load(&mut self.connection)?;
            groups.push(Group {
                uid,
                track_id: uid.doc_id(),
                spec: serde_json::from_str(&row.spec)?,
                members: members
                    .into_iter()
                    .filter_map(|row| row.key.parse().ok())
                    .collect(),
                created_at_ms: row.created_at_ms,
                updated_at_ms: row.updated_at_ms,
            });
        }
        Ok(groups)
    }
}
