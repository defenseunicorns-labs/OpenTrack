//! Correlation routes: settings, the engine's suggestions and operators'
//! decisions (accept, reject, split, merge, do not pair).
//!
//! Decisions that change tracks are commands to the engine, which owns the
//! live state: the route queues one and waits (up to ten seconds) for the
//! engine's answer, so this works whether the engine runs in this process or
//! another.

use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::routing::{get, post};
use axum::{Json, Router};
use ot_core::Uid;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::api::actor;
use crate::control::{ApiError, AppState};
use crate::correlate::CorrelationSettings;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/correlation/settings", get(get_settings).put(put_settings))
        .route("/correlation/suggestions", get(list_suggestions))
        .route("/correlation/suggestions/count", get(count_suggestions))
        .route(
            "/correlation/suggestions/{id}/{decision}",
            post(decide_suggestion),
        )
        .route("/correlation/decisions", get(list_decisions))
        .route("/tracks/{uid}/split", post(split_track))
        .route("/tracks/merge", post(merge_tracks))
        .route("/tracks/do-not-pair", post(do_not_pair))
}

/// The settings in force (saved, else defaults), and whether any are saved.
async fn get_settings(State(s): State<AppState>) -> Result<Json<Value>, ApiError> {
    let saved = s.with_db(|db| db.correlation_settings()).await?;
    let settings: CorrelationSettings = saved
        .clone()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    Ok(Json(json!({
        "settings": settings,
        "saved": saved.is_some(),
        "defaults": CorrelationSettings::default(),
        "version": crate::correlate::VERSION,
    })))
}

async fn put_settings(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let settings: CorrelationSettings =
        serde_json::from_value(body).map_err(|e| ApiError::unprocessable(e.to_string()))?;
    settings.validate().map_err(ApiError::unprocessable)?;
    let value = serde_json::to_value(&settings).expect("settings serialize");
    let decision = ot_store::Decision::new(actor(&headers), "correlation_settings")
        .reason("correlation settings saved")
        .evidence(value.clone());
    let (stored, site, who) = (value.clone(), s.common.site, actor(&headers));
    s.with_db(move |db| {
        db.save_correlation_settings(&stored, decision)?;
        crate::settings_api::share_profile(
            db,
            site,
            &who,
            &json!({ "op": "profile_correlation", "settings": stored }),
        )
    })
    .await?;
    Ok(Json(json!({ "settings": value, "saved": true })))
}

#[derive(Deserialize)]
struct SuggestionQuery {
    status: Option<String>,
    limit: Option<usize>,
}

/// A live track as the suggestion list shows it.
async fn track_summary(s: &AppState, uid: &str) -> Value {
    let Ok(uid) = uid.parse::<Uid>() else {
        return json!({ "uid": uid, "live": false });
    };
    match s.redis.get_system_track(uid).await {
        Ok(Some(t)) => json!({
            "uid": uid.to_string(),
            "track_id": uid.doc_id(),
            "live": true,
            "name": t.view.name,
            "state": t.state,
            "published": t.is_published(),
            "latitude": t.view.position.latitude,
            "longitude": t.view.position.longitude,
            "sources": t.contributors.iter().map(|c| format!("{}/{}", c.source_id, c.source_track_key)).collect::<Vec<_>>(),
        }),
        _ => json!({ "uid": uid.to_string(), "track_id": uid.doc_id(), "live": false }),
    }
}

/// How many suggestions wait for a track manager (`open`), cheap enough for
/// the tab bar to poll.
async fn count_suggestions(State(s): State<AppState>) -> Result<Json<Value>, ApiError> {
    let counts = s.with_db(|db| db.suggestion_counts()).await?;
    Ok(Json(
        json!({ "open": counts.get("open").copied().unwrap_or(0), "by_status": counts }),
    ))
}

async fn list_suggestions(
    State(s): State<AppState>,
    Query(q): Query<SuggestionQuery>,
) -> Result<Json<Value>, ApiError> {
    let status = q.status.filter(|x| x != "all");
    let limit = q.limit.unwrap_or(200).min(1000);
    let rows = s
        .with_db(move |db| db.suggestions(status.as_deref(), limit))
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let a = track_summary(&s, &r.track_a).await;
        let b = match &r.track_b {
            Some(b) => track_summary(&s, b).await,
            None => Value::Null,
        };
        let mut v = serde_json::to_value(&r).expect("suggestion serializes");
        v["a"] = a;
        v["b"] = b;
        out.push(v);
    }
    Ok(Json(json!({ "suggestions": out })))
}

/// Queue a command for the engine and wait for its answer.
pub(crate) async fn command(
    s: &AppState,
    headers: &HeaderMap,
    cmd: Value,
) -> Result<Json<Value>, ApiError> {
    command_waiting(s, headers, cmd, Duration::from_secs(10)).await
}

/// Queue a command for the engine and wait up to `wait` for its answer.
pub(crate) async fn command_waiting(
    s: &AppState,
    headers: &HeaderMap,
    mut cmd: Value,
    wait: Duration,
) -> Result<Json<Value>, ApiError> {
    let id = format!(
        "{}-{}",
        chrono::Utc::now().timestamp_micros(),
        std::process::id()
    );
    cmd["id"] = json!(id);
    cmd["actor"] = json!(actor(headers));
    cmd["ip"] = json!(crate::auth::access::current_ip());
    s.redis
        .push_command(&cmd)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    for _ in 0..(wait.as_millis() / 100).max(1) {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if let Some(answer) = s
            .redis
            .command_result(&id)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?
        {
            return if answer["ok"] == true {
                Ok(Json(answer["result"].clone()))
            } else {
                Err(ApiError::conflict(
                    answer["error"].as_str().unwrap_or("the engine refused"),
                ))
            };
        }
    }
    Err(ApiError::internal(format!(
        "the engine did not answer within {} s (is it running?)",
        wait.as_secs()
    )))
}

async fn decide_suggestion(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((id, decision)): Path<(i64, String)>,
) -> Result<Json<Value>, ApiError> {
    let op = match decision.as_str() {
        "accept" | "reject" => decision,
        other => {
            return Err(ApiError::not_found(format!(
                "no decision {other:?}: accept or reject"
            )));
        }
    };
    command(&s, &headers, json!({ "op": op, "suggestion": id })).await
}

#[derive(Deserialize)]
struct DecisionQuery {
    limit: Option<usize>,
}

async fn list_decisions(
    State(s): State<AppState>,
    Query(q): Query<DecisionQuery>,
) -> Result<Json<Value>, ApiError> {
    let limit = q.limit.unwrap_or(100).min(1000);
    let rows = s
        .with_db(move |db| db.decisions_by_op(ot_store::correlation::CORRELATION_OPS, limit))
        .await?;
    Ok(Json(json!({ "decisions": rows })))
}

#[derive(Deserialize)]
struct SplitBody {
    /// `<source>/<key>` of the source track that leaves.
    source_track: String,
    reason: Option<String>,
}

async fn split_track(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(uid): Path<String>,
    Json(b): Json<SplitBody>,
) -> Result<Json<Value>, ApiError> {
    command(
        &s,
        &headers,
        json!({ "op": "split", "track": uid, "source_track": b.source_track, "reason": b.reason }),
    )
    .await
}

#[derive(Deserialize)]
struct MergeBody {
    from: String,
    into: String,
    reason: Option<String>,
    /// A track manager's merge (GOLD MRG): correlation never splits it.
    #[serde(default)]
    hold: bool,
}

async fn merge_tracks(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(b): Json<MergeBody>,
) -> Result<Json<Value>, ApiError> {
    command(
        &s,
        &headers,
        json!({ "op": "merge", "from": b.from, "into": b.into, "reason": b.reason, "hold": b.hold }),
    )
    .await
}

#[derive(Deserialize)]
struct PairBody {
    a: String,
    b: String,
    reason: Option<String>,
}

async fn do_not_pair(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(b): Json<PairBody>,
) -> Result<Json<Value>, ApiError> {
    command(
        &s,
        &headers,
        json!({ "op": "do_not_pair", "a": b.a, "b": b.b, "reason": b.reason }),
    )
    .await
}
