//! PostgreSQL audit log queries.

use diesel::prelude::*;
use diesel::sql_types::{Array, BigInt, Nullable, Text};

use super::{PgControlStore, PostgresError};
use crate::audit::{Anchor, AuditFilter, AuditRow, AuditVerify, GENESIS, Problem, row_hash};

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

#[derive(QueryableByName)]
struct VerifyRow {
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
    prev_hash: String,
    #[diesel(sql_type = Text)]
    hash: String,
}

#[derive(QueryableByName)]
struct AnchorRow {
    #[diesel(sql_type = BigInt)]
    seq: i64,
    #[diesel(sql_type = Text)]
    hash: String,
}

impl PgControlStore {
    pub fn verify_audit(&mut self) -> Result<AuditVerify, PostgresError> {
        let anchor: Option<Anchor> =
            diesel::sql_query("SELECT seq, hash FROM audit_anchor WHERE id = 1")
                .get_result::<AnchorRow>(&mut self.connection)
                .optional()?
                .map(|row| Anchor {
                    seq: row.seq,
                    hash: row.hash,
                });
        let (mut expected, mut previous) = anchor
            .as_ref()
            .map(|a| (a.seq + 1, a.hash.clone()))
            .unwrap_or((1, GENESIS.to_owned()));
        let mut out = AuditVerify {
            ok: true,
            rows: 0,
            first_seq: None,
            last_seq: None,
            head_hash: previous.clone(),
            anchor: anchor.clone(),
            problems: Vec::new(),
        };
        let mut last_purge_anchor = None;
        let rows: Vec<VerifyRow> = diesel::sql_query(
            "SELECT seq, at_ms, actor, op, outcome, ip, detail, decision_id, prev_hash, hash FROM audit ORDER BY seq"
        ).load(&mut self.connection)?;
        for row in rows {
            out.rows += 1;
            out.first_seq.get_or_insert(row.seq);
            out.last_seq = Some(row.seq);
            let mut problem = |message: &str| {
                out.ok = false;
                if out.problems.len() < 20 {
                    out.problems.push(Problem {
                        seq: row.seq,
                        problem: message.to_owned(),
                    });
                }
            };
            if row.seq != expected {
                problem(&format!("expected row {expected} here: rows are missing"));
            }
            if row.prev_hash != previous {
                problem("does not follow the row before it (prev_hash differs)");
            }
            let want = row_hash(
                &row.prev_hash,
                row.seq,
                row.at_ms,
                &row.actor,
                &row.op,
                &row.outcome,
                row.ip.as_deref(),
                &row.detail,
                row.decision_id,
            );
            if want != row.hash {
                problem("its content does not match its hash (changed)");
            }
            if row.op == "audit_purge" {
                last_purge_anchor = serde_json::from_str::<serde_json::Value>(&row.detail)
                    .ok()
                    .and_then(|v| v["anchor_hash"].as_str().map(str::to_owned));
            }
            expected = row.seq + 1;
            previous = row.hash;
        }
        out.head_hash = previous;
        if let Some(anchor) = &anchor
            && last_purge_anchor.as_deref() != Some(anchor.hash.as_str())
        {
            out.ok = false;
            if out.problems.len() < 20 {
                out.problems.push(Problem {
                    seq: anchor.seq,
                    problem: "the anchor does not match the last purge's record".into(),
                });
            }
        }
        Ok(out)
    }
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
