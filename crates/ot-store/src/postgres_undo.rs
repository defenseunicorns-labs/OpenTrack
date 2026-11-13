//! PostgreSQL implementation of temporal track management undo.

use std::collections::BTreeSet;

use diesel::prelude::*;
use diesel::sql_types::{BigInt, Bool, Nullable, Text};
use ot_core::Uid;
use serde_json::{Value, json};

use super::graph::add_edge;
use super::{PgControlStore, PostgresError, record_decision};
use crate::Decision;
use crate::graph::EdgeKind;
use crate::undo::{UNDOABLE, Undone};

#[derive(QueryableByName)]
struct DecisionRow {
    #[diesel(sql_type = Text)]
    actor: String,
    #[diesel(sql_type = Text)]
    op: String,
    #[diesel(sql_type = Nullable<Text>)]
    before: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    evidence: Option<String>,
    #[diesel(sql_type = Nullable<BigInt>)]
    undone_by: Option<i64>,
}
#[derive(QueryableByName)]
struct IdRow {
    #[diesel(sql_type = BigInt)]
    id: i64,
}
#[derive(QueryableByName)]
struct EdgeRow {
    #[diesel(sql_type = Text)]
    kind: String,
    #[diesel(sql_type = BigInt)]
    src: i64,
    #[diesel(sql_type = BigInt)]
    dst: i64,
    #[diesel(sql_type = Text)]
    attrs: String,
    #[diesel(sql_type = Bool)]
    live: bool,
}
#[derive(QueryableByName)]
struct NodeRow {
    #[diesel(sql_type = Text)]
    kind: String,
    #[diesel(sql_type = Text)]
    key: String,
    #[diesel(sql_type = Nullable<BigInt>)]
    retired_at_ms: Option<i64>,
}
#[derive(QueryableByName)]
struct ConflictRow {
    #[diesel(sql_type = BigInt)]
    id: i64,
    #[diesel(sql_type = Text)]
    op: String,
    #[diesel(sql_type = Text)]
    actor: String,
}
#[derive(QueryableByName)]
struct ExistsRow {
    #[diesel(sql_type = Bool)]
    exists: bool,
}

fn edge_kind(kind: &str) -> Option<EdgeKind> {
    Some(match kind {
        "REPORTS_FOR" => EdgeKind::ReportsFor,
        "MERGED_INTO" => EdgeKind::MergedInto,
        "DO_NOT_PAIR" => EdgeKind::DoNotPair,
        "MEMBER_OF" => EdgeKind::MemberOf,
        "PAIRED_WITH" => EdgeKind::PairedWith,
        _ => return None,
    })
}

impl PgControlStore {
    pub fn undo_decision(
        &mut self,
        id: i64,
        actor: &str,
        reason: &str,
    ) -> Result<Undone, PostgresError> {
        self.connection.transaction::<Undone, PostgresError, _>(|connection| {
            let d: DecisionRow = diesel::sql_query("SELECT actor, op, before, evidence, undone_by FROM decisions WHERE id = $1 FOR UPDATE")
                .bind::<BigInt, _>(id).get_result(connection).optional()?
                .ok_or_else(|| PostgresError::NotFound(format!("decision {id}")))?;
            if !UNDOABLE.contains(&d.op.as_str()) {
                return Err(PostgresError::Conflict(format!("a {} cannot be undone (only track management: {})", d.op, UNDOABLE.join(", "))));
            }
            if ["engine", "system"].contains(&d.actor.as_str()) {
                return Err(PostgresError::Conflict(format!("decision {id} was the engine's own; split or merge the tracks instead")));
            }
            if let Some(undo) = d.undone_by {
                return Err(PostgresError::Conflict(format!("decision {id} was undone already (decision {undo})")));
            }
            let mut ids = vec![id];
            let companions: Vec<IdRow> = diesel::sql_query("SELECT id FROM decisions WHERE evidence::jsonb->>'with_decision' = $1 AND undone_by IS NULL")
                .bind::<Text, _>(id.to_string()).load(connection)?;
            ids.extend(companions.into_iter().map(|r| r.id));
            let mut opened = Vec::new();
            let mut closed = Vec::new();
            for decision in &ids {
                opened.extend(diesel::sql_query("SELECT kind, src, dst, attrs, (valid_to_ms IS NULL) AS live FROM edges WHERE decision_id = $1")
                    .bind::<BigInt, _>(decision).load::<EdgeRow>(connection)?);
                closed.extend(diesel::sql_query("SELECT kind, src, dst, attrs, (valid_to_ms IS NULL) AS live FROM edges WHERE ended_by = $1")
                    .bind::<BigInt, _>(decision).load::<EdgeRow>(connection)?);
            }
            let touched: BTreeSet<i64> = opened.iter().chain(&closed).flat_map(|e| [e.src, e.dst]).collect();
            let first = *ids.iter().min().unwrap_or(&id);
            let touched_json = serde_json::to_string(&touched)?;
            let ids_json = serde_json::to_string(&ids)?;
            let conflicts: Vec<ConflictRow> = diesel::sql_query(
                "SELECT d.id, d.op, d.actor FROM decisions d WHERE d.id > $1 AND NOT (d.id IN (SELECT value::bigint FROM jsonb_array_elements_text($3::jsonb))) AND d.undone_by IS NULL AND d.op <> 'undo' AND d.actor NOT IN ('engine','system') AND EXISTS (SELECT 1 FROM edges e WHERE (e.decision_id = d.id OR e.ended_by = d.id) AND (e.src IN (SELECT value::bigint FROM jsonb_array_elements_text($2::jsonb)) OR e.dst IN (SELECT value::bigint FROM jsonb_array_elements_text($2::jsonb)))) ORDER BY d.id LIMIT 1")
                .bind::<BigInt, _>(first).bind::<Text, _>(touched_json).bind::<Text, _>(ids_json).load(connection)?;
            if let Some(c) = conflicts.first() {
                return Err(PostgresError::Conflict(format!("{} changed these tracks since ({}, decision {}): undo that first", c.actor, c.op, c.id)));
            }
            let now = crate::sqlite::now_ms();
            let undo = record_decision(connection, &Decision::new(actor, "undo").reason(reason).evidence(json!({"undoes":id,"op":d.op})))?;
            for decision in &ids {
                diesel::sql_query("UPDATE decisions SET undone_by = $2 WHERE id = $1")
                    .bind::<BigInt, _>(decision).bind::<BigInt, _>(undo).execute(connection)?;
            }
            diesel::sql_query("UPDATE decisions SET undoes = $2 WHERE id = $1")
                .bind::<BigInt, _>(undo).bind::<BigInt, _>(id).execute(connection)?;
            let mut out = Undone { decision_id: undo, undoes: id, op: d.op.clone(), ..Default::default() };
            let mut systems = BTreeSet::new();
            for decision in &ids {
                diesel::sql_query("UPDATE edges SET valid_to_ms = greatest($2, valid_from_ms), ended_by = $3 WHERE decision_id = $1 AND valid_to_ms IS NULL")
                    .bind::<BigInt, _>(decision).bind::<BigInt, _>(now).bind::<BigInt, _>(undo).execute(connection)?;
            }
            for edge in closed.iter().filter(|e| !e.live) {
                let Some(kind) = edge_kind(&edge.kind) else { continue };
                if kind == EdgeKind::ReportsFor {
                    let current: Vec<IdRow> = diesel::sql_query("SELECT dst AS id FROM edges WHERE src = $1 AND kind = 'REPORTS_FOR' AND valid_to_ms IS NULL")
                        .bind::<BigInt, _>(edge.src).load(connection)?;
                    systems.extend(current.into_iter().map(|r| r.id));
                    diesel::sql_query("UPDATE edges SET valid_to_ms = greatest($2, valid_from_ms), ended_by = $3 WHERE src = $1 AND kind = 'REPORTS_FOR' AND valid_to_ms IS NULL")
                        .bind::<BigInt, _>(edge.src).bind::<BigInt, _>(now).bind::<BigInt, _>(undo).execute(connection)?;
                }
                let exists: ExistsRow = diesel::sql_query("SELECT EXISTS (SELECT 1 FROM edges WHERE kind = $1 AND src = $2 AND dst = $3 AND valid_to_ms IS NULL) AS exists")
                    .bind::<Text, _>(&edge.kind).bind::<BigInt, _>(edge.src).bind::<BigInt, _>(edge.dst).get_result(connection)?;
                if !exists.exists {
                    let attrs: Value = serde_json::from_str(&edge.attrs).unwrap_or(json!({}));
                    add_edge(connection, kind, edge.src, edge.dst, undo, &attrs, now)?;
                }
            }
            for node in &touched {
                let row: NodeRow = diesel::sql_query("SELECT kind, key, retired_at_ms FROM nodes WHERE id = $1")
                    .bind::<BigInt, _>(node).get_result(connection)?;
                if row.kind == "system_track" { systems.insert(*node); }
            }
            for node in &systems {
                let row: NodeRow = diesel::sql_query("SELECT kind, key, retired_at_ms FROM nodes WHERE id = $1")
                    .bind::<BigInt, _>(node).get_result(connection)?;
                let Ok(uid) = row.key.parse::<Uid>() else { continue };
                let sources: IdRow = diesel::sql_query("SELECT count(*)::bigint AS id FROM edges WHERE dst = $1 AND kind = 'REPORTS_FOR' AND valid_to_ms IS NULL")
                    .bind::<BigInt, _>(node).get_result(connection)?;
                if sources.id > 0 && row.retired_at_ms.is_some() {
                    diesel::sql_query("UPDATE nodes SET retired_at_ms = NULL WHERE id = $1").bind::<BigInt, _>(node).execute(connection)?;
                    out.revived.push(uid);
                } else if sources.id == 0 && row.retired_at_ms.is_none() {
                    diesel::sql_query("UPDATE edges SET valid_to_ms = greatest($2, valid_from_ms), ended_by = $3 WHERE (src = $1 OR dst = $1) AND valid_to_ms IS NULL")
                        .bind::<BigInt, _>(node).bind::<BigInt, _>(now).bind::<BigInt, _>(undo).execute(connection)?;
                    diesel::sql_query("UPDATE nodes SET retired_at_ms = $2 WHERE id = $1")
                        .bind::<BigInt, _>(node).bind::<BigInt, _>(now).execute(connection)?;
                    out.retired.push(uid);
                }
                out.tracks.push(uid);
            }
            let names: Vec<String> = out.tracks.iter().map(|u| u.doc_id()).collect();
            diesel::sql_query("UPDATE decisions SET evidence = jsonb_set(evidence::jsonb, '{tracks}', $2::jsonb)::text WHERE id = $1")
                .bind::<BigInt, _>(undo).bind::<Text, _>(serde_json::to_string(&names)?).execute(connection)?;
            out.do_not_pair_changed = opened.iter().chain(&closed).any(|e| e.kind == "DO_NOT_PAIR");
            let group = d.evidence.as_deref().and_then(|v| serde_json::from_str::<Value>(v).ok())
                .and_then(|v| v.get("group").and_then(Value::as_str).map(str::to_owned))
                .and_then(|g| Uid::from_doc_id(&g).ok());
            if let Some(group) = group {
                match d.op.as_str() {
                    "create_group" => { diesel::sql_query("UPDATE track_groups SET dissolved_at_ms = $2, updated_at_ms = $2 WHERE uid = $1")
                        .bind::<Text, _>(group.to_string()).bind::<BigInt, _>(now).execute(connection)?; }
                    "dissolve_group" => { diesel::sql_query("UPDATE track_groups SET dissolved_at_ms = NULL, updated_at_ms = $2 WHERE uid = $1")
                        .bind::<Text, _>(group.to_string()).bind::<BigInt, _>(now).execute(connection)?; }
                    "update_group" => if let Some(before) = d.before.as_deref() {
                        diesel::sql_query("UPDATE track_groups SET spec = $2, updated_at_ms = $3 WHERE uid = $1")
                            .bind::<Text, _>(group.to_string()).bind::<Text, _>(before).bind::<BigInt, _>(now).execute(connection)?;
                    },
                    _ => {}
                }
                out.groups_changed = true;
            }
            out.groups_changed |= opened.iter().chain(&closed).any(|e| e.kind == "MEMBER_OF");
            Ok(out)
        })
    }
}
