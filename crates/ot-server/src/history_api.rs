//! A system track's position history (kept for Settings' `history_hours`),
//! and deleting a bad point of it.

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::control::{ApiError, AppState, parse_uid};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/tracks/{uid}/history", get(history))
        .route("/history/{uid}/delete", post(delete_point))
}

#[derive(Deserialize)]
struct Range {
    /// Unix ms.
    since: Option<i64>,
    until: Option<i64>,
    limit: Option<usize>,
}

async fn history(
    State(s): State<AppState>,
    Path(uid): Path<String>,
    Query(q): Query<Range>,
) -> Result<Json<Value>, ApiError> {
    let uid = parse_uid(&uid)?;
    let points = s
        .redis
        .track_history(uid, q.since, q.until, q.limit.unwrap_or(5000).min(50_000))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(json!({ "track_id": uid.doc_id(), "points": points })))
}

#[derive(Deserialize)]
struct DeletePoint {
    /// The point's time (Unix ms), as the history lists it.
    t: i64,
    reason: Option<String>,
}

async fn delete_point(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(uid): Path<String>,
    Json(b): Json<DeletePoint>,
) -> Result<Json<Value>, ApiError> {
    let uid = parse_uid(&uid)?;
    crate::correlation_api::command(
        &s,
        &headers,
        json!({ "op": "delete_history_point", "track": uid.doc_id(), "t": b.t, "reason": b.reason }),
    )
    .await
}
