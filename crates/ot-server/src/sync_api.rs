//! How sharing the picture with other nodes is going (see
//! `docs/multi-node.md`): the settings, the peers the link has heard, the
//! replicated decision log, and how many tracks this node reports. Also
//! the signing keys: this node's public key, and the key an admin pins for
//! each peer (writes are an admin's, see `auth::policy`; each is a decision,
//! so audited).

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::routing::{get, put};
use axum::{Json, Router};
use ot_core::SiteCode;
use ot_sync::sign::PublicKey;
use serde_json::{Value, json};

use crate::control::{ApiError, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/sync/status", get(status))
        .route("/sync/keys", get(keys))
        .route("/sync/keys/{site}", put(pin_key).delete(unpin_key))
}

/// This node's public key and every pinned peer key. Public keys are not
/// secret; the private key never leaves `sync.key`.
async fn keys(State(s): State<AppState>) -> Result<Json<Value>, ApiError> {
    let common = s.common.clone();
    let own = tokio::task::spawn_blocking(move || crate::link::load_node_key(&common))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .map_err(|e| ApiError::internal(format!("{e:#}")))?
        .public();
    let pinned = s.with_db(|db| db.sync_peer_keys()).await?;
    let peers: Vec<Value> = pinned
        .into_iter()
        .map(|k| {
            let fingerprint = k
                .public_key
                .parse::<PublicKey>()
                .ok()
                .map(|p| p.fingerprint_text());
            json!({
                "site": k.site,
                "public_key": k.public_key,
                "fingerprint": fingerprint,
                "pinned_by": k.pinned_by,
                "pinned_at": chrono::DateTime::from_timestamp_millis(k.pinned_at_ms),
            })
        })
        .collect();
    Ok(Json(json!({
        "site": s.common.site.to_string(),
        "public_key": own.to_string(),
        "fingerprint": own.fingerprint_text(),
        "peers": peers,
    })))
}

fn site_of(site: &str, s: &AppState) -> Result<SiteCode, ApiError> {
    let site: SiteCode = site.parse().map_err(|_| {
        ApiError::unprocessable(format!(
            "{site:?} is not a site code (3 characters A-Z, 0-9)"
        ))
    })?;
    if site == s.common.site {
        return Err(ApiError::unprocessable(format!(
            "{site} is this node's own site code"
        )));
    }
    Ok(site)
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PinKey {
    public_key: String,
}

/// Pin or replace a peer's public key.
async fn pin_key(
    State(s): State<AppState>,
    Path(site): Path<String>,
    headers: HeaderMap,
    Json(req): Json<PinKey>,
) -> Result<Json<Value>, ApiError> {
    let site = site_of(&site, &s)?;
    let key: PublicKey = req
        .public_key
        .parse()
        .map_err(|e: ot_sync::sign::SignError| ApiError::unprocessable(e.to_string()))?;
    let (text, who) = (key.to_string(), crate::api::actor(&headers));
    s.with_db(move |db| db.sync_pin_key(site, &text, &who))
        .await?;
    keys(State(s)).await
}

/// Remove a peer's key: its messages are refused from then on.
async fn unpin_key(
    State(s): State<AppState>,
    Path(site): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let site = site_of(&site, &s)?;
    let who = crate::api::actor(&headers);
    s.with_db(move |db| db.sync_unpin_key(site, &who))
        .await?
        .ok_or_else(|| ApiError::not_found(format!("no key is pinned for {site}")))?;
    keys(State(s)).await
}

async fn status(State(s): State<AppState>) -> Result<Json<Value>, ApiError> {
    let (saved, heads, counts) = s
        .with_db(|db| Ok((db.app_settings()?, db.sync_heads()?, db.sync_counts()?)))
        .await?;
    let settings = crate::settings_api::sync_settings(&saved);
    let live = s
        .redis
        .sync_status()
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let heads: Vec<Value> = heads
        .into_iter()
        .map(|(site, seq)| json!({ "site": site.to_string(), "seq": seq }))
        .collect();
    Ok(Json(json!({
        "site": s.common.site.to_string(),
        "settings": settings,
        "log": { "heads": heads, "by_status": counts },
        "link": live.get("link"),
        "engine": live.get("engine"),
    })))
}
