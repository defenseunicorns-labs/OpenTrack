//! The audit record over the API (admins): rows by time, actor, operation
//! and outcome, as JSON or CSV (marked on its first line, see
//! `crate::marking`), and a check of its hash chain.

use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use ot_store::{AuditEvent, AuditFilter};
use serde::Deserialize;
use serde_json::json;

use crate::api::actor;
use crate::control::{ApiError, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/audit", get(list))
        .route("/audit/verify", get(verify))
}

#[derive(Deserialize, Default)]
struct ListQuery {
    /// From and to (Unix ms, inclusive).
    from_ms: Option<i64>,
    to_ms: Option<i64>,
    actor: Option<String>,
    /// Comma-separated operations (`login,logout`).
    op: Option<String>,
    /// `success` or `failure`.
    outcome: Option<String>,
    /// Rows before this sequence number (the next page back).
    before_seq: Option<i64>,
    /// Default 200 (JSON) or 100 000 (CSV).
    limit: Option<usize>,
    /// `csv` for a file.
    format: Option<String>,
}

async fn list(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> Result<Response, ApiError> {
    let csv = q.format.as_deref() == Some("csv");
    let f = AuditFilter {
        from_ms: q.from_ms,
        to_ms: q.to_ms,
        actor: q.actor,
        ops: q
            .op
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|o| !o.is_empty())
            .map(str::to_owned)
            .collect(),
        outcome: q.outcome,
        before_seq: q.before_seq,
        limit: q
            .limit
            .unwrap_or(if csv { 100_000 } else { 200 })
            .min(if csv { 100_000 } else { 1_000 }),
    };
    if !csv {
        let limit = f.limit;
        let rows = s.with_db(move |db| db.audit_rows(&f)).await?;
        let next = (rows.len() == limit)
            .then(|| rows.last().map(|r| r.seq))
            .flatten();
        return Ok(Json(json!({ "rows": rows, "next_before_seq": next })).into_response());
    }
    // An export is itself recorded.
    let who = actor(&headers);
    let rows = s
        .with_db(move |db| {
            let rows = db.audit_rows(&f)?;
            db.audit(&AuditEvent::new(&who, "audit_export").detail(json!({
                "rows": rows.len(),
                "from_ms": f.from_ms,
                "to_ms": f.to_ms,
                "actor": f.actor,
                "ops": f.ops,
                "outcome": f.outcome,
            })))?;
            Ok(rows)
        })
        .await?;
    // Marked with the banner's text: audit rows carry no labels.
    let marking = crate::marking::Marker::load(&s).await?.file([]);
    let mut w = csv::WriterBuilder::new()
        .flexible(true)
        .from_writer(Vec::new());
    let err = |e: csv::Error| ApiError::internal(e.to_string());
    w.write_record([&marking]).map_err(err)?;
    w.write_record([
        "seq",
        "time",
        "actor",
        "op",
        "outcome",
        "ip",
        "decision_id",
        "detail",
        "hash",
    ])
    .map_err(err)?;
    for r in rows {
        let time = ot_store::sqlite::from_ms(r.at_ms).to_rfc3339();
        w.write_record([
            r.seq.to_string(),
            time,
            r.actor,
            r.op,
            r.outcome,
            r.ip.unwrap_or_default(),
            r.decision_id.map(|d| d.to_string()).unwrap_or_default(),
            r.detail.to_string(),
            r.hash,
        ])
        .map_err(err)?;
    }
    let body = w
        .into_inner()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok((
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/csv; charset=utf-8"),
            ),
            (
                header::CONTENT_DISPOSITION,
                HeaderValue::from_static("attachment; filename=\"opentrack-audit.csv\""),
            ),
        ],
        body,
    )
        .into_response())
}

/// Check the whole chain (and record that it was checked, with the result).
async fn verify(
    State(s): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ot_store::AuditVerify>, ApiError> {
    let who = actor(&headers);
    let v = s
        .with_db(move |db| {
            let v = db.verify_audit()?;
            let mut e = AuditEvent::new(&who, "audit_verify").detail(json!({
                "ok": v.ok,
                "rows": v.rows,
                "last_seq": v.last_seq,
                "head_hash": v.head_hash,
                "problems": v.problems.len(),
            }));
            if !v.ok {
                e = e.failure();
            }
            db.audit(&e)?;
            Ok(v)
        })
        .await?;
    if !v.ok {
        tracing::error!(problems = ?v.problems, "the audit record's hash chain is broken");
    }
    Ok(Json(v))
}
