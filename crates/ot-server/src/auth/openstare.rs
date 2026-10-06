//! Accepting OpenStare's sign-in: OpenStare's `/api/auth/me` says who a
//! session cookie or API token belongs to (and refuses revoked ones); its
//! role maps to one of ours. Answers are kept for [`CACHE`], so an
//! OpenStare sign-out takes at most that long to hold here.

use std::time::{Duration, Instant};

use axum::http::header;
use serde::Deserialize;

use super::{AuthSettings, AuthUser, Via};
use crate::control::AppState;

const CACHE: Duration = Duration::from_secs(30);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Me {
    user_id: String,
    email: String,
    role: String,
    #[serde(default)]
    first_name: Option<String>,
    #[serde(default)]
    last_name: Option<String>,
}

pub(super) async fn identify(
    s: &AppState,
    cookie: Option<&str>,
    bearer: Option<&str>,
) -> Option<AuthUser> {
    let key = format!("{}|{}", cookie.unwrap_or(""), bearer.unwrap_or(""));
    if let Ok(c) = s.auth.openstare_cache.lock()
        && let Some((at, user)) = c.get(&key)
        && at.elapsed() < CACHE
    {
        return user.clone();
    }
    let user = ask(s, cookie, bearer).await;
    if let Ok(mut c) = s.auth.openstare_cache.lock() {
        if c.len() > 10_000 {
            c.retain(|_, (at, _)| at.elapsed() < CACHE);
        }
        c.insert(key, (Instant::now(), user.clone()));
    }
    user
}

async fn ask(s: &AppState, cookie: Option<&str>, bearer: Option<&str>) -> Option<AuthUser> {
    let settings = s.auth.settings().openstare;
    let url = format!("{}/api/auth/me", settings.api_url.trim_end_matches('/'));
    let mut req = s.auth.http.get(&url);
    if let Some(c) = cookie {
        req = req.header(header::COOKIE, format!("{}={c}", super::OPENSTARE_COOKIE));
    }
    if let Some(b) = bearer {
        req = req.header(header::AUTHORIZATION, format!("Bearer {b}"));
    }
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(%url, error = %e, "OpenStare sign-in check failed");
            return None;
        }
    };
    if let Some(peer) = resp.remote_addr()
        && s.auth.openstare_peer.changed(&settings.api_url, peer)
    {
        tracing::info!(component = "openstare", api = %ot_core::secrets::redact_url(&settings.api_url), peer_addr = %peer, "OpenStare reached");
    }
    if !resp.status().is_success() {
        return None;
    }
    let me: Me = resp.json().await.ok()?;
    let Some(role) =
        AuthSettings::map_role(&settings.role_mapping, std::slice::from_ref(&me.role), None)
    else {
        tracing::info!(email = %me.email, role = %me.role, "OpenStare user's role has no OpenTrack role");
        return None;
    };
    let name = [me.first_name, me.last_name]
        .into_iter()
        .flatten()
        .filter(|n| !n.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    Some(AuthUser {
        id: format!("openstare:{}", me.user_id),
        email: me.email,
        name,
        role,
        via: Via::Openstare,
        jti: None,
        must_change: false,
        sso: false,
    })
}
