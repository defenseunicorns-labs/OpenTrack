//! Server-side sessions: each browser sign-in is a row (named by its
//! token's id) that ends when its user signs out, sits idle too long, is
//! pushed out by a newer one past the per-account limit, or is ended by
//! its user or an admin. API tokens are not sessions: they only expire.
//!
//! Use is noted in memory and saved at most once a minute (by
//! [`super::maintenance`]), so a request never waits on a write for it. The
//! browser says how long its user has been idle (`x-ot-idle-ms`), so the
//! UI's own polling does not keep an unattended session alive.
//!
//! Sessions belong to the node that made them: with several nodes (see
//! docs/multi-node.md) each signs its users in on its own.

use std::collections::HashMap;
use std::sync::Mutex;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::get;
use axum::{Extension, Json, Router};
use ot_store::AuditEvent;
use ot_store::sqlite::now_ms;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{AuthUser, Role};
use crate::control::{ApiError, AppState};

/// The header the UI sends: milliseconds since its user last did anything.
pub const IDLE_HEADER: &str = "x-ot-idle-ms";

/// When each session was last used, and whether that is saved yet.
#[derive(Default)]
pub struct Activity {
    seen: Mutex<HashMap<String, (i64, bool)>>,
}

impl Activity {
    /// Note that session `id` was used at `at_ms`.
    pub fn note(&self, id: &str, at_ms: i64) {
        if let Ok(mut m) = self.seen.lock() {
            let e = m.entry(id.to_owned()).or_insert((0, false));
            if at_ms > e.0 {
                *e = (at_ms, true);
            }
        }
    }

    pub fn last(&self, id: &str) -> Option<i64> {
        self.seen.lock().ok()?.get(id).map(|e| e.0)
    }

    /// Uses not saved yet; they count as saved from now.
    pub fn take_unsaved(&self) -> Vec<(String, i64)> {
        let Ok(mut m) = self.seen.lock() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (id, (at, dirty)) in m.iter_mut() {
            if *dirty {
                out.push((id.clone(), *at));
                *dirty = false;
            }
        }
        out
    }

    pub fn forget(&self, id: &str) {
        if let Ok(mut m) = self.seen.lock() {
            m.remove(id);
        }
    }

    /// Forget saved uses older than `before_ms`.
    pub fn prune(&self, before_ms: i64) {
        if let Ok(mut m) = self.seen.lock() {
            m.retain(|_, (at, dirty)| *dirty || *at >= before_ms);
        }
    }
}

/// How long the browser says its user has been idle (0 when it does not
/// say: any other client's request is use).
pub fn client_idle_ms(headers: &HeaderMap) -> i64 {
    headers
        .get(IDLE_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<i64>().ok())
        .unwrap_or(0)
        .clamp(0, 86_400_000)
}

/// Whether a session stands: it exists here, has not ended, and has not
/// been idle longer than its account's role allows (then it ends now).
pub(super) async fn alive(
    s: &AppState,
    user: &ot_store::User,
    role: Role,
    row: Option<&ot_store::Session>,
    client_idle_ms: i64,
) -> bool {
    let Some(row) = row else {
        return false;
    };
    if row.ended_at_ms.is_some()
        || row.user_id != user.id
        || row.created_at_ms < user.tokens_valid_from_ms
    {
        return false;
    }
    let now = now_ms();
    let last = row
        .last_seen_ms
        .max(s.auth.activity.last(&row.id).unwrap_or(0));
    if now - last > super::stig::idle_ms(role) {
        let (id, email) = (row.id.clone(), user.email.clone());
        let ip = row.ip.clone();
        let _ = s
            .with_db(move |db| {
                if db.end_session(&id, "idle")? {
                    db.audit(&ended(&email, &id, "idle", ip.as_deref()))?;
                }
                Ok(())
            })
            .await;
        s.auth.activity.forget(&row.id);
        return false;
    }
    s.auth.activity.note(&row.id, now - client_idle_ms);
    true
}

/// The audit event for a session that ended.
pub fn ended(email: &str, session: &str, reason: &str, ip: Option<&str>) -> AuditEvent {
    let op = if reason == "idle" {
        "session_timeout"
    } else {
        "session_end"
    };
    let mut e = AuditEvent::new(email, op).detail(json!({ "session": session, "reason": reason }));
    if let Some(ip) = ip {
        e = e.ip(ip);
    }
    e
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/auth/sessions", get(list))
        .route("/auth/sessions/{id}", axum::routing::delete(end))
}

#[derive(Deserialize, Default)]
struct ListQuery {
    /// An admin: every account's.
    #[serde(default)]
    all: bool,
    /// An admin: this account's.
    user: Option<String>,
    /// Ended ones too.
    #[serde(default)]
    ended: bool,
}

/// Your sessions (an admin: anyone's, or everyone's).
async fn list(
    State(s): State<AppState>,
    Extension(me): Extension<AuthUser>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Value>, ApiError> {
    let admin = me.role == Role::Admin;
    let who = match (&q.user, q.all) {
        (_, true) if admin => None,
        (Some(u), _) if admin => Some(u.clone()),
        (Some(u), _) if *u != me.id => {
            return Err(ApiError::forbidden("only an admin sees others' sessions"));
        }
        (None, true) => return Err(ApiError::forbidden("only an admin sees every session")),
        _ => Some(me.id.clone()),
    };
    let ended = q.ended;
    let mut rows = s
        .with_db(move |db| db.sessions(who.as_deref(), ended, 500))
        .await?;
    for r in &mut rows {
        if let Some(at) = s.auth.activity.last(&r.id) {
            r.last_seen_ms = r.last_seen_ms.max(at);
        }
    }
    let current = me.jti.as_ref().map(|(j, _)| j.clone());
    Ok(Json(json!({ "sessions": rows, "current": current })))
}

/// End a session: your own, or (an admin) anyone's.
async fn end(
    State(s): State<AppState>,
    Extension(me): Extension<AuthUser>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let lookup = id.clone();
    let row = s
        .with_db(move |db| db.session(&lookup))
        .await?
        .ok_or_else(|| ApiError::not_found(format!("session {id}")))?;
    if row.user_id != me.id && me.role != Role::Admin {
        return Err(ApiError::not_found(format!("session {id}")));
    }
    let by = if row.user_id == me.id {
        "ended by its user"
    } else {
        "ended by an admin"
    };
    let actor = crate::api::actor(&headers);
    let (sid, exp) = (row.id.clone(), row.expires_at_ms);
    s.with_db(move |db| {
        if db.end_session(&sid, "ended")? {
            db.revoke_token(&sid, exp)?;
            db.audit(
                &ended(&row.user_email, &sid, "ended", None)
                    .detail(json!({ "by": actor, "why": by })),
            )?;
        }
        Ok(())
    })
    .await?;
    s.auth.activity.forget(&id);
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_is_noted_and_saved_in_batches() {
        let a = Activity::default();
        a.note("s1", 100);
        a.note("s1", 50);
        a.note("s2", 70);
        assert_eq!(a.last("s1"), Some(100));
        let mut saved = a.take_unsaved();
        saved.sort();
        assert_eq!(saved, [("s1".into(), 100), ("s2".into(), 70)]);
        assert!(a.take_unsaved().is_empty(), "saved already");
        a.prune(80);
        assert_eq!((a.last("s1"), a.last("s2")), (Some(100), None));
        a.forget("s1");
        assert_eq!(a.last("s1"), None);
    }

    #[test]
    fn the_browser_says_how_long_it_has_been_idle() {
        let mut h = HeaderMap::new();
        assert_eq!(client_idle_ms(&h), 0);
        h.insert(IDLE_HEADER, "90000".parse().unwrap());
        assert_eq!(client_idle_ms(&h), 90_000);
        h.insert(IDLE_HEADER, "-5".parse().unwrap());
        assert_eq!(client_idle_ms(&h), 0);
    }
}
