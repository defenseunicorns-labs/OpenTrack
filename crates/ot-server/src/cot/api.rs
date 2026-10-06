//! `GET /tak/status`: each configured TAK output with how it is going, as
//! the `cot` role last reported (connected, clients, events sent, errors).

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{Value, json};

use crate::control::{ApiError, AppState};

pub fn routes() -> Router<AppState> {
    Router::new().route("/tak/status", get(status))
}

async fn status(State(s): State<AppState>) -> Result<Json<Value>, ApiError> {
    let saved = s.with_db(|db| db.app_settings()).await?;
    let settings = super::settings::tak_settings(&saved);
    let live = s
        .redis
        .cot_status()
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let running = live.is_some();
    let live = live.unwrap_or_default();
    let outputs: Vec<Value> = settings
        .outputs
        .iter()
        .map(|o| {
            let l = &live["outputs"][&o.id];
            json!({
                "id": o.id,
                "kind": o.delivery.kind(),
                "enabled": o.enabled,
                "encrypted": o.delivery.encrypted(),
                "state": if !o.enabled { json!("off") } else { l.get("state").cloned().unwrap_or(json!("not started")) },
                "clients": l.get("clients").cloned().unwrap_or(json!(0)),
                "sent": l.get("sent").cloned().unwrap_or(json!(0)),
                "errors": l.get("errors").cloned().unwrap_or(json!(0)),
                "dropped": l.get("dropped").cloned().unwrap_or(json!(0)),
                "last_error": l.get("last_error").cloned().unwrap_or(Value::Null),
            })
        })
        .collect();
    Ok(Json(json!({
        "running": running,
        "at": live.get("at"),
        "tracks": live.get("tracks"),
        "outputs": outputs,
    })))
}
