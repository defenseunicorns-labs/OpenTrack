//! PostgreSQL correlation suggestions and decision review.

use std::collections::BTreeMap;

use diesel::prelude::*;
use diesel::sql_types::{Array, BigInt, Nullable, Text};
use ot_core::{SiteCode, Uid};
use serde_json::{Value, json};

use super::graph::{add_edge, upsert_node};
use super::{PgControlStore, PostgresError, record_decision};
use crate::Decision;
use crate::correlation::{DecisionRow, Suggestion};
use crate::graph::{self, EdgeKind, NodeKind};

#[derive(QueryableByName)]
struct IdRow {
    #[diesel(sql_type = BigInt)]
    id: i64,
}

#[derive(QueryableByName)]
struct NumberRow {
    #[diesel(sql_type = BigInt)]
    sequence: i64,
}

#[derive(QueryableByName)]
struct ExistsRow {
    #[diesel(sql_type = diesel::sql_types::Bool)]
    exists: bool,
}

#[derive(QueryableByName)]
struct SuggestionSql {
    #[diesel(sql_type = BigInt)]
    id: i64,
    #[diesel(sql_type = Text)]
    kind: String,
    #[diesel(sql_type = Text)]
    track_a: String,
    #[diesel(sql_type = Nullable<Text>)]
    track_b: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    source_track: Option<String>,
    #[diesel(sql_type = Text)]
    evidence: String,
    #[diesel(sql_type = Text)]
    status: String,
    #[diesel(sql_type = BigInt)]
    created_at_ms: i64,
    #[diesel(sql_type = BigInt)]
    updated_at_ms: i64,
    #[diesel(sql_type = Nullable<BigInt>)]
    decision_id: Option<i64>,
}

impl SuggestionSql {
    fn into_suggestion(self) -> Result<Suggestion, PostgresError> {
        Ok(Suggestion {
            id: self.id,
            kind: self.kind,
            track_a: self.track_a,
            track_b: self.track_b,
            source_track: self.source_track,
            evidence: serde_json::from_str(&self.evidence)?,
            status: self.status,
            created_at_ms: self.created_at_ms,
            updated_at_ms: self.updated_at_ms,
            decision_id: self.decision_id,
        })
    }
}

#[derive(QueryableByName)]
struct CountRow {
    #[diesel(sql_type = Text)]
    status: String,
    #[diesel(sql_type = BigInt)]
    n: i64,
}

#[derive(QueryableByName)]
struct DecisionSql {
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
    #[diesel(sql_type = Nullable<BigInt>)]
    undone_by: Option<i64>,
    #[diesel(sql_type = Nullable<BigInt>)]
    undoes: Option<i64>,
}

const SUGGESTION_COLUMNS: &str = "id, kind, track_a, track_b, source_track, evidence, status, created_at_ms, updated_at_ms, decision_id";

impl PgControlStore {
    pub fn do_not_pair(
        &mut self,
        left: &[String],
        right: &[String],
        decision: &Decision,
    ) -> Result<i64, PostgresError> {
        self.connection
            .transaction::<i64, PostgresError, _>(|connection| {
                let now = crate::sqlite::now_ms();
                let decision_id = record_decision(connection, decision)?;
                for a in left {
                    for b in right {
                        if a == b {
                            continue;
                        }
                        let (a, b) = if a < b { (a, b) } else { (b, a) };
                        let src = upsert_node(connection, NodeKind::SourceTrack, a, now)?;
                        let dst = upsert_node(connection, NodeKind::SourceTrack, b, now)?;
                        let exists: ExistsRow = diesel::sql_query(
                            "SELECT exists(SELECT 1 FROM edges
                         WHERE kind = 'DO_NOT_PAIR' AND valid_to_ms IS NULL
                           AND src = $1 AND dst = $2) AS exists",
                        )
                        .bind::<BigInt, _>(src)
                        .bind::<BigInt, _>(dst)
                        .get_result(connection)?;
                        if !exists.exists {
                            add_edge(
                                connection,
                                EdgeKind::DoNotPair,
                                src,
                                dst,
                                decision_id,
                                &json!({}),
                                now,
                            )?;
                        }
                    }
                }
                Ok(decision_id)
            })
    }

    pub fn split_source_track(
        &mut self,
        site: SiteCode,
        source_id: &str,
        source_track_key: &str,
        decision: &Decision,
    ) -> Result<(Uid, i64), PostgresError> {
        self.connection
            .transaction::<(Uid, i64), PostgresError, _>(|connection| {
                let now = crate::sqlite::now_ms();
                let key = graph::source_track_key(source_id, source_track_key);
                let sources: Vec<IdRow> = diesel::sql_query(
                    "SELECT id FROM nodes WHERE kind = 'source_track' AND key = $1",
                )
                .bind::<Text, _>(&key)
                .load(connection)?;
                let src = sources
                    .first()
                    .ok_or_else(|| PostgresError::NotFound(format!("source track {key}")))?
                    .id;
                let decision_id = record_decision(connection, decision)?;
                diesel::sql_query(
                    "UPDATE edges SET valid_to_ms = greatest($2, valid_from_ms), ended_by = $3
                 WHERE src = $1 AND kind = 'REPORTS_FOR' AND valid_to_ms IS NULL",
                )
                .bind::<BigInt, _>(src)
                .bind::<BigInt, _>(now)
                .bind::<BigInt, _>(decision_id)
                .execute(connection)?;
                let row: NumberRow = diesel::sql_query(
                    "INSERT INTO uid_sequences (site, next_sequence) VALUES ($1, 2)
                 ON CONFLICT (site) DO UPDATE SET next_sequence = uid_sequences.next_sequence + 1
                 RETURNING next_sequence - 1 AS sequence",
                )
                .bind::<Text, _>(site.as_str())
                .get_result(connection)?;
                let uid = Uid::new(site, row.sequence as u64)?;
                let dst = upsert_node(connection, NodeKind::SystemTrack, &uid.to_string(), now)?;
                add_edge(
                    connection,
                    EdgeKind::ReportsFor,
                    src,
                    dst,
                    decision_id,
                    &json!({"pairing": "split", "confidence": 1.0}),
                    now,
                )?;
                Ok((uid, decision_id))
            })
    }

    pub fn upsert_suggestion(
        &mut self,
        kind: &str,
        track_a: &str,
        track_b: Option<&str>,
        source_track: Option<&str>,
        evidence: &Value,
    ) -> Result<i64, PostgresError> {
        let now = crate::sqlite::now_ms();
        self.connection.transaction::<i64, PostgresError, _>(|connection| {
            let found: Vec<IdRow> = diesel::sql_query(
                "SELECT id FROM correlation_suggestions
                 WHERE status = 'open' AND kind = $1 AND track_a = $2
                   AND coalesce(track_b, '') = coalesce($3, '')
                   AND coalesce(source_track, '') = coalesce($4, '')",
            )
            .bind::<Text, _>(kind)
            .bind::<Text, _>(track_a)
            .bind::<Nullable<Text>, _>(track_b)
            .bind::<Nullable<Text>, _>(source_track)
            .load(connection)?;
            if let Some(row) = found.first() {
                diesel::sql_query(
                    "UPDATE correlation_suggestions SET evidence = $2, updated_at_ms = $3 WHERE id = $1",
                )
                .bind::<BigInt, _>(row.id)
                .bind::<Text, _>(evidence.to_string())
                .bind::<BigInt, _>(now)
                .execute(connection)?;
                return Ok(row.id);
            }
            let row: IdRow = diesel::sql_query(
                "INSERT INTO correlation_suggestions
                 (kind, track_a, track_b, source_track, evidence, created_at_ms, updated_at_ms)
                 VALUES ($1, $2, $3, $4, $5, $6, $6) RETURNING id",
            )
            .bind::<Text, _>(kind)
            .bind::<Text, _>(track_a)
            .bind::<Nullable<Text>, _>(track_b)
            .bind::<Nullable<Text>, _>(source_track)
            .bind::<Text, _>(evidence.to_string())
            .bind::<BigInt, _>(now)
            .get_result(connection)?;
            Ok(row.id)
        })
    }

    pub fn suggestion(&mut self, id: i64) -> Result<Option<Suggestion>, PostgresError> {
        let rows: Vec<SuggestionSql> = diesel::sql_query(format!(
            "SELECT {SUGGESTION_COLUMNS} FROM correlation_suggestions WHERE id = $1",
        ))
        .bind::<BigInt, _>(id)
        .load(&mut self.connection)?;
        rows.into_iter()
            .next()
            .map(SuggestionSql::into_suggestion)
            .transpose()
    }

    pub fn suggestion_counts(&mut self) -> Result<BTreeMap<String, u64>, PostgresError> {
        let rows: Vec<CountRow> = diesel::sql_query(
            "SELECT status, count(*)::bigint AS n FROM correlation_suggestions GROUP BY status",
        )
        .load(&mut self.connection)?;
        Ok(rows
            .into_iter()
            .map(|row| (row.status, row.n as u64))
            .collect())
    }

    pub fn suggestions(
        &mut self,
        status: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Suggestion>, PostgresError> {
        let rows: Vec<SuggestionSql> = diesel::sql_query(format!(
            "SELECT {SUGGESTION_COLUMNS} FROM correlation_suggestions
             WHERE $1::text IS NULL OR status = $1 ORDER BY updated_at_ms DESC LIMIT $2",
        ))
        .bind::<Nullable<Text>, _>(status)
        .bind::<BigInt, _>(limit.min(i64::MAX as usize) as i64)
        .load(&mut self.connection)?;
        rows.into_iter()
            .map(SuggestionSql::into_suggestion)
            .collect()
    }

    pub fn close_suggestion(
        &mut self,
        id: i64,
        status: &str,
        decision: Option<&Decision>,
    ) -> Result<(), PostgresError> {
        self.connection
            .transaction::<(), PostgresError, _>(|connection| {
                let decision_id = decision
                    .map(|decision| record_decision(connection, decision))
                    .transpose()?;
                let changed = diesel::sql_query(
                    "UPDATE correlation_suggestions SET status = $2, updated_at_ms = $3,
                 decision_id = coalesce($4, decision_id) WHERE id = $1 AND status = 'open'",
                )
                .bind::<BigInt, _>(id)
                .bind::<Text, _>(status)
                .bind::<BigInt, _>(crate::sqlite::now_ms())
                .bind::<Nullable<BigInt>, _>(decision_id)
                .execute(connection)?;
                if changed == 0 {
                    return Err(PostgresError::NotFound(format!("open suggestion {id}")));
                }
                Ok(())
            })
    }

    pub fn expire_suggestions(&mut self, uid: Uid) -> Result<usize, PostgresError> {
        Ok(diesel::sql_query(
            "UPDATE correlation_suggestions SET status = 'expired', updated_at_ms = $2
             WHERE status = 'open' AND (track_a = $1 OR track_b = $1)",
        )
        .bind::<Text, _>(uid.to_string())
        .bind::<BigInt, _>(crate::sqlite::now_ms())
        .execute(&mut self.connection)?)
    }

    pub fn decisions_by_op(
        &mut self,
        ops: &[&str],
        limit: usize,
    ) -> Result<Vec<DecisionRow>, PostgresError> {
        let ops: Vec<String> = ops.iter().map(|op| (*op).to_owned()).collect();
        let rows: Vec<DecisionSql> = diesel::sql_query(
            "SELECT id, at_ms, actor, op, reason, evidence, undone_by, undoes
             FROM decisions WHERE op = ANY($1) ORDER BY id DESC LIMIT $2",
        )
        .bind::<Array<Text>, _>(ops)
        .bind::<BigInt, _>(limit.min(i64::MAX as usize) as i64)
        .load(&mut self.connection)?;
        rows.into_iter()
            .map(|row| {
                Ok(DecisionRow {
                    id: row.id,
                    at_ms: row.at_ms,
                    actor: row.actor,
                    op: row.op,
                    reason: row.reason,
                    evidence: serde_json::from_str(&row.evidence)?,
                    undone_by: row.undone_by,
                    undoes: row.undoes,
                })
            })
            .collect()
    }
}
