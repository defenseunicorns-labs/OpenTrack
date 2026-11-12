//! PostgreSQL track graph reads used by the engine and API.

use diesel::prelude::*;
use diesel::sql_types::{BigInt, Nullable, Text};
use ot_core::Uid;
use serde_json::{Value, json};

use super::{PgControlStore, PostgresError};
use crate::graph::{EdgeKind, EdgeRecord};

#[derive(QueryableByName)]
struct ReportRow {
    #[diesel(sql_type = Text)]
    source: String,
    #[diesel(sql_type = Text)]
    uid: String,
}

#[derive(QueryableByName)]
struct EdgeRow {
    #[diesel(sql_type = BigInt)]
    id: i64,
    #[diesel(sql_type = Text)]
    kind: String,
    #[diesel(sql_type = Text)]
    src_key: String,
    #[diesel(sql_type = Text)]
    dst_key: String,
    #[diesel(sql_type = Text)]
    attrs: String,
    #[diesel(sql_type = Text)]
    src_attrs: String,
    #[diesel(sql_type = BigInt)]
    valid_from_ms: i64,
    #[diesel(sql_type = Nullable<BigInt>)]
    valid_to_ms: Option<i64>,
    #[diesel(sql_type = BigInt)]
    decision_id: i64,
    #[diesel(sql_type = Text)]
    decision_op: String,
    #[diesel(sql_type = Text)]
    decision_actor: String,
    #[diesel(sql_type = Nullable<Text>)]
    decision_reason: Option<String>,
    #[diesel(sql_type = Nullable<BigInt>)]
    ended_by: Option<i64>,
}

#[derive(QueryableByName)]
struct DecisionRow {
    #[diesel(sql_type = BigInt)]
    id: i64,
    #[diesel(sql_type = BigInt)]
    at_ms: i64,
    #[diesel(sql_type = Text)]
    actor: String,
    #[diesel(sql_type = Text)]
    op: String,
    #[diesel(sql_type = Nullable<Text>)]
    reason: Option<String>,
    #[diesel(sql_type = Text)]
    evidence: String,
}

impl PgControlStore {
    pub fn live_reports(&mut self) -> Result<Vec<(String, Uid)>, PostgresError> {
        let rows: Vec<ReportRow> = diesel::sql_query(
            "SELECT s.key AS source, d.key AS uid FROM edges e
             JOIN nodes s ON s.id = e.src JOIN nodes d ON d.id = e.dst
             WHERE e.kind = 'REPORTS_FOR' AND e.valid_to_ms IS NULL",
        )
        .load(&mut self.connection)?;
        rows.into_iter()
            .map(|row| Ok((row.source, row.uid.parse()?)))
            .collect()
    }

    pub fn explain(&mut self, uid: Uid) -> Result<Vec<EdgeRecord>, PostgresError> {
        let rows: Vec<EdgeRow> = diesel::sql_query(
            "WITH RECURSIVE family(id) AS (
                 SELECT id FROM nodes WHERE kind = 'system_track' AND key = $1
                 UNION SELECT e.src FROM edges e JOIN family f ON e.dst = f.id
                 WHERE e.kind = 'MERGED_INTO'
             )
             SELECT e.id, e.kind, s.key AS src_key, d.key AS dst_key, e.attrs,
                    s.attrs AS src_attrs, e.valid_from_ms, e.valid_to_ms,
                    e.decision_id, dec.op AS decision_op, dec.actor AS decision_actor,
                    dec.reason AS decision_reason, e.ended_by
             FROM edges e JOIN nodes s ON s.id = e.src
             JOIN nodes d ON d.id = e.dst JOIN decisions dec ON dec.id = e.decision_id
             WHERE e.src IN (SELECT id FROM family) OR e.dst IN (SELECT id FROM family)
             ORDER BY e.valid_from_ms, e.id",
        )
        .bind::<Text, _>(uid.to_string())
        .load(&mut self.connection)?;
        rows.into_iter()
            .map(|row| {
                Ok(EdgeRecord {
                    id: row.id,
                    kind: EdgeKind::parse(&row.kind).ok_or_else(|| {
                        PostgresError::Conflict(format!("unknown edge kind {}", row.kind))
                    })?,
                    src_key: row.src_key,
                    dst_key: row.dst_key,
                    attrs: serde_json::from_str(&row.attrs)?,
                    src_attrs: serde_json::from_str(&row.src_attrs)?,
                    valid_from_ms: row.valid_from_ms,
                    valid_to_ms: row.valid_to_ms,
                    decision_id: row.decision_id,
                    decision_op: row.decision_op,
                    decision_actor: row.decision_actor,
                    decision_reason: row.decision_reason,
                    ended_by: row.ended_by,
                })
            })
            .collect()
    }

    pub fn decision(&mut self, id: i64) -> Result<Option<Value>, PostgresError> {
        let rows: Vec<DecisionRow> = diesel::sql_query(
            "SELECT id, at_ms, actor, op, reason, evidence FROM decisions WHERE id = $1",
        )
        .bind::<BigInt, _>(id)
        .load(&mut self.connection)?;
        rows.into_iter()
            .next()
            .map(|row| {
                Ok(json!({
                    "id": row.id,
                    "at_ms": row.at_ms,
                    "actor": row.actor,
                    "op": row.op,
                    "reason": row.reason,
                    "evidence": serde_json::from_str::<Value>(&row.evidence)?,
                }))
            })
            .transpose()
    }
}
