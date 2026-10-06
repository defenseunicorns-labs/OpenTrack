//! Track management routes: what a track manager does to tracks by hand,
//! beyond correlation's merge and split (OTH-GOLD's track management sets):
//! pair tracks, delete them, and form groups (a battle group, a flight, a
//! convoy) published as tracks of their own. A track manager's merge is
//! `POST /tracks/merge` with `hold`.
//!
//! Changes are commands to the engine, which owns the live state.

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::control::{ApiError, AppState};
use crate::correlation_api::command;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/tracks/pair", post(pair))
        .route("/tracks/unpair", post(unpair))
        .route("/tracks/delete", post(delete))
        .route("/groups", get(list_groups).post(create_group))
        .route("/groups/{id}", put(update_group).delete(dissolve_group))
        .route("/groups/{id}/members", post(group_members))
}

#[derive(Deserialize)]
struct Tracks {
    tracks: Vec<String>,
    reason: Option<String>,
}

async fn pair(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(b): Json<Tracks>,
) -> Result<Json<Value>, ApiError> {
    command(
        &s,
        &headers,
        json!({ "op": "pair", "tracks": b.tracks, "reason": b.reason }),
    )
    .await
}

#[derive(Deserialize)]
struct Unpair {
    a: String,
    b: String,
    reason: Option<String>,
}

async fn unpair(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(b): Json<Unpair>,
) -> Result<Json<Value>, ApiError> {
    command(
        &s,
        &headers,
        json!({ "op": "unpair", "a": b.a, "b": b.b, "reason": b.reason }),
    )
    .await
}

async fn delete(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(b): Json<Tracks>,
) -> Result<Json<Value>, ApiError> {
    command(
        &s,
        &headers,
        json!({ "op": "delete", "tracks": b.tracks, "reason": b.reason }),
    )
    .await
}

/// Every live group: what it is and its members.
async fn list_groups(State(s): State<AppState>) -> Result<Json<Value>, ApiError> {
    let groups = s.with_db(|db| db.groups()).await?;
    let out: Vec<Value> = groups
        .into_iter()
        .map(|g| {
            json!({
                "track_id": g.track_id, "uid": g.uid, "spec": g.spec,
                "members": g.members.iter().map(|m| m.doc_id()).collect::<Vec<_>>(),
                "created_at_ms": g.created_at_ms, "updated_at_ms": g.updated_at_ms,
            })
        })
        .collect();
    Ok(Json(json!({ "groups": out })))
}

#[derive(Deserialize)]
struct CreateGroup {
    spec: Value,
    members: Vec<String>,
    reason: Option<String>,
}

async fn create_group(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(b): Json<CreateGroup>,
) -> Result<Json<Value>, ApiError> {
    command(
        &s,
        &headers,
        json!({ "op": "group_create", "spec": b.spec, "members": b.members, "reason": b.reason }),
    )
    .await
}

#[derive(Deserialize)]
struct UpdateGroup {
    spec: Value,
    reason: Option<String>,
}

async fn update_group(
    State(s): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(b): Json<UpdateGroup>,
) -> Result<Json<Value>, ApiError> {
    command(
        &s,
        &headers,
        json!({ "op": "group_update", "group": id, "spec": b.spec, "reason": b.reason }),
    )
    .await
}

#[derive(Deserialize)]
struct Members {
    #[serde(default)]
    add: Vec<String>,
    #[serde(default)]
    remove: Vec<String>,
    reason: Option<String>,
}

async fn group_members(
    State(s): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(b): Json<Members>,
) -> Result<Json<Value>, ApiError> {
    command(
        &s,
        &headers,
        json!({ "op": "group_members", "group": id, "add": b.add, "remove": b.remove, "reason": b.reason }),
    )
    .await
}

async fn dissolve_group(
    State(s): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    command(&s, &headers, json!({ "op": "group_dissolve", "group": id })).await
}
