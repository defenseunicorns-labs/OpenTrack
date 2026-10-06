//! The decision log: every change to the picture and the configuration,
//! who made it and why.

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::{AuthUser, Role};
use crate::control::{ApiError, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/decisions", get(list))
        .route("/decisions/{id}/undo", post(undo))
}

#[derive(Deserialize)]
struct ListQuery {
    /// Comma-separated operations (`pair,merge`).
    op: String,
    limit: Option<usize>,
}

/// Decisions any role may list: the picture and its curation, and the
/// configuration's revisions (sources, schema, correlation, settings; their
/// secrets are masked for non-admins). Accounts, API tokens, sign-in and
/// configuration import/export are an admin's: they name accounts and
/// sessions (ASD V-222500). An operation not listed here is an admin's.
const TRACK_OPS: &[&str] = &[
    "create_source",
    "update_source",
    "delete_source",
    "app_settings",
    "update_settings",
    "correlation_settings",
    "publish_schema",
    "edit_schema_draft",
    "discard_schema_draft",
    "registry_import",
    "create_entity",
    "import_tracker_profile",
    "delete_tracker_profile",
    "configure_plugin",
    "update_plugin",
    "delete_plugin",
    "purge_track_history",
    "note",
    "pair_tracks",
    "unpair_tracks",
    "delete_track",
    "merge",
    "split",
    "reject_split",
    "do_not_pair",
    "create_group",
    "update_group",
    "group_members",
    "dissolve_group",
    "undo",
    "delete_history_point",
    "create_system_track",
    "retire_system_track",
    "stale_tracks_deleted",
    "update_entity",
    "delete_entity",
];

async fn list(
    State(s): State<AppState>,
    user: Option<Extension<AuthUser>>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Value>, ApiError> {
    let limit = q.limit.unwrap_or(100).min(1000);
    let ops: Vec<String> =
        q.op.split(',')
            .map(str::trim)
            .filter(|o| !o.is_empty())
            .map(str::to_owned)
            .collect();
    let admin = user.is_none_or(|u| u.role == Role::Admin);
    if !admin && let Some(op) = ops.iter().find(|o| !TRACK_OPS.contains(&o.as_str())) {
        return Err(ApiError::forbidden(format!(
            "only an admin may list `{op}` decisions"
        )));
    }
    let rows = s
        .with_db(move |db| {
            let ops: Vec<&str> = ops.iter().map(String::as_str).collect();
            db.decisions_by_op(&ops, limit)
        })
        .await?;
    Ok(Json(json!({ "decisions": rows })))
}

#[derive(Deserialize, Default)]
struct UndoBody {
    reason: Option<String>,
}

/// Undo a track management decision (the engine does it, as a decision of
/// its own): a merge or split, a pairing, a delete, a group change.
async fn undo(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    body: Option<Json<UndoBody>>,
) -> Result<Json<Value>, ApiError> {
    let reason = body.and_then(|b| b.0.reason);
    crate::correlation_api::command(
        &s,
        &headers,
        json!({ "op": "undo", "decision": id, "reason": reason }),
    )
    .await
}
