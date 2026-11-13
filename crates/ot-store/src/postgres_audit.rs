//! PostgreSQL audit log queries.

use diesel::prelude::*;
use diesel::sql_types::{Array, BigInt, Nullable, Text};

use super::{PgControlStore, PostgresError};
use crate::audit::{AuditFilter, AuditRow};

#[derive(QueryableByName)]
struct Row {
    #[diesel(sql_type = BigInt)]
    seq: i64,
    #[diesel(sql_type = BigInt)]
    at_ms: i64,
    #[diesel(sql_type = Text)]
    actor: String,
    #[diesel(sql_type = Text)]
    op: String,
    #[diesel(sql_type = Text)]
    outcome: String,
    #[diesel(sql_type = Nullable<Text>)]
    ip: Option<String>,
    #[diesel(sql_type = Text)]
    detail: String,
    #[diesel(sql_type = Nullable<BigInt>)]
    decision_id: Option<i64>,
    #[diesel(sql_type = Text)]
    hash: String,
}

impl PgControlStore {
    pub fn audit_rows(&mut self, f: &AuditFilter) -> Result<Vec<AuditRow>, PostgresError> {
        let ops: Vec<String> = f
            .ops
            .iter()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect();
        let actor = f.actor.as_deref().map(str::trim).filter(|s| !s.is_empty());
        let outcome = f
            .outcome
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let rows: Vec<Row> = diesel::sql_query(
            "SELECT seq, at_ms, actor, op, outcome, ip, detail, decision_id, hash FROM audit
             WHERE ($1::bigint IS NULL OR at_ms >= $1)
               AND ($2::bigint IS NULL OR at_ms <= $2)
               AND ($3::text IS NULL OR lower(actor) = lower($3))
               AND ($4::text IS NULL OR outcome = $4)
               AND ($5::bigint IS NULL OR seq < $5)
               AND ($6::text[] IS NULL OR op = ANY($6))
             ORDER BY seq DESC LIMIT $7",
        )
        .bind::<Nullable<BigInt>, _>(f.from_ms)
        .bind::<Nullable<BigInt>, _>(f.to_ms)
        .bind::<Nullable<Text>, _>(actor)
        .bind::<Nullable<Text>, _>(outcome)
        .bind::<Nullable<BigInt>, _>(f.before_seq)
        .bind::<Nullable<Array<Text>>, _>((!ops.is_empty()).then_some(ops))
        .bind::<BigInt, _>(f.limit.clamp(1, 100_000) as i64)
        .load(&mut self.connection)?;
        rows.into_iter()
            .map(|row| {
                Ok(AuditRow {
                    seq: row.seq,
                    at_ms: row.at_ms,
                    actor: row.actor,
                    op: row.op,
                    outcome: row.outcome,
                    ip: row.ip,
                    detail: serde_json::from_str(&row.detail)?,
                    decision_id: row.decision_id,
                    hash: row.hash,
                })
            })
            .collect()
    }
}
