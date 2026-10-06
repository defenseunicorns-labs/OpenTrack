//! What the API gate records beyond sign-in (AU-2, AU-3, AU-12):
//! - every request, as an `access` log line (actor, method, path, status,
//!   address, user agent, referrer, forwarded-for, duration), which the
//!   collector carries to the SIEM;
//! - in the audit record: a credential that doesn't identify anyone, a role
//!   refusal, a refused change, an admin reading a security object, and the
//!   first use in an hour of an API token, client certificate or OpenStare
//!   identity (they have no sign-in of their own);
//! - the client address, for the decisions a request records.

use std::collections::HashMap;
use std::sync::Mutex;

use axum::http::{HeaderMap, Method, StatusCode, header};
use ot_store::audit::AuditEvent;
use serde_json::{Value, json};

use super::{AuthUser, Via};
use crate::control::AppState;

tokio::task_local! {
    /// The client address of the request (or engine command) this task
    /// serves.
    pub static CLIENT_IP: String;
}

/// The client address of the current request, if any.
pub fn current_ip() -> Option<String> {
    CLIENT_IP.try_with(Clone::clone).ok()
}

/// How often a token, certificate or OpenStare identity's use is audited.
const USE_EVERY_MS: i64 = 3_600_000;

/// When each non-session identity was last audited as used.
#[derive(Default)]
pub struct Uses {
    last: Mutex<HashMap<String, i64>>,
}

impl Uses {
    /// Whether `u`'s use now is the first in [`USE_EVERY_MS`] (sessions are
    /// audited at sign-in, so never).
    fn first_in_a_while(&self, u: &AuthUser, now_ms: i64) -> bool {
        if !matches!(u.via, Via::ApiToken | Via::ClientCert | Via::Openstare) {
            return false;
        }
        let key = format!(
            "{:?}:{}",
            u.via,
            u.jti.as_ref().map_or(u.id.as_str(), |j| j.0.as_str())
        );
        let Ok(mut m) = self.last.lock() else {
            return false;
        };
        m.retain(|_, at| now_ms - *at < USE_EVERY_MS);
        if m.contains_key(&key) {
            return false;
        }
        m.insert(key, now_ms);
        true
    }
}

/// Whether the request carried any credential (a failed one is audited; no
/// credential at all is just "not signed in").
pub fn credential_presented(headers: &HeaderMap, cert: bool) -> bool {
    cert || headers.contains_key(header::AUTHORIZATION)
        || super::cookie(headers, super::COOKIE).is_some()
        || super::cookie(headers, super::OPENSTARE_COOKIE).is_some()
}

/// Admin reads of security objects that go to the audit record.
const SECURITY_READS: &[&str] = &[
    "/auth/users",
    "/auth/api-tokens",
    "/auth/settings",
    "/auth/sessions/all",
    "/export/config",
    "/audit",
];

/// Paths whose refusals the handler audits itself.
const SELF_AUDITED: &[&str] = &["/auth/password", "/auth/login", "/auth/consent"];

fn is_security_read(path: &str) -> bool {
    SECURITY_READS
        .iter()
        .any(|p| path == *p || path.starts_with(&format!("{p}/")))
}

/// Append an audit event; failing to is logged, not fatal (the request was
/// already refused or answered).
pub async fn audit(s: &AppState, e: AuditEvent) {
    if let Err(err) = s.with_db(move |db| db.audit(&e)).await {
        tracing::warn!(error = %err.message, "audit event not recorded");
    }
}

fn event(actor: &str, op: &str, ip: &str, detail: Value) -> AuditEvent {
    AuditEvent::new(actor, op).ip(ip).detail(detail)
}

/// Record what a request that passed the gate did.
pub async fn after(
    s: &AppState,
    user: &AuthUser,
    method: &Method,
    path: &str,
    status: StatusCode,
    ip: &str,
) {
    let now = chrono::Utc::now().timestamp_millis();
    if s.auth.uses.first_in_a_while(user, now) {
        audit(
            s,
            event(
                &user.email,
                "login",
                ip,
                json!({ "via": user.via, "first_use_in": "1 h" }),
            ),
        )
        .await;
    }
    let read = matches!(*method, Method::GET | Method::HEAD);
    if !read && status.is_client_error() && !SELF_AUDITED.contains(&path) {
        audit(
            s,
            event(
                &user.email,
                "change_refused",
                ip,
                json!({ "method": method.as_str(), "path": path, "status": status.as_u16() }),
            )
            .failure(),
        )
        .await;
    } else if read && status.is_success() && is_security_read(path) {
        audit(
            s,
            event(
                &user.email,
                "read_security_object",
                ip,
                json!({ "path": path }),
            ),
        )
        .await;
    }
}

/// Record a request the gate refused: a credential that identifies no one,
/// or a role that doesn't reach.
pub async fn refused(
    s: &AppState,
    actor: &str,
    op: &str,
    method: &Method,
    path: &str,
    ip: &str,
    detail: Value,
) {
    let mut d = json!({ "method": method.as_str(), "path": path });
    if let (Value::Object(d), Value::Object(extra)) = (&mut d, detail) {
        d.extend(extra);
    }
    audit(s, event(actor, op, ip, d).failure()).await;
}

/// The `access` log line for one request.
#[allow(clippy::too_many_arguments)]
pub fn log(
    actor: Option<&str>,
    method: &Method,
    path: &str,
    status: StatusCode,
    ip: &str,
    headers: &HeaderMap,
    ms: u128,
) {
    let h = |n: header::HeaderName| {
        headers
            .get(n)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.chars().take(256).collect::<String>())
    };
    tracing::info!(
        target: "access",
        actor = actor.unwrap_or("-"),
        method = method.as_str(),
        path,
        status = status.as_u16(),
        ip,
        user_agent = h(header::USER_AGENT).as_deref(),
        referer = h(header::REFERER).as_deref(),
        forwarded_for = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()),
        ms = ms as u64,
        "request"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(via: Via, jti: Option<&str>) -> AuthUser {
        AuthUser {
            id: "u1".into(),
            email: "a@x.org".into(),
            name: String::new(),
            role: super::super::Role::Viewer,
            via,
            jti: jti.map(|j| (j.into(), 0)),
            must_change: false,
            sso: false,
        }
    }

    #[test]
    fn a_tokens_use_is_audited_once_an_hour_and_a_sessions_never() {
        let u = Uses::default();
        let tok = user(Via::ApiToken, Some("t1"));
        assert!(u.first_in_a_while(&tok, 0));
        assert!(!u.first_in_a_while(&tok, 1_000));
        assert!(
            u.first_in_a_while(&user(Via::ApiToken, Some("t2")), 1_000),
            "another token"
        );
        assert!(u.first_in_a_while(&tok, USE_EVERY_MS + 1));
        assert!(!u.first_in_a_while(&user(Via::Session, Some("s1")), 0));
    }

    #[test]
    fn security_reads_are_the_admins_objects() {
        assert!(is_security_read("/auth/users"));
        assert!(is_security_read("/auth/api-tokens/abc"));
        assert!(is_security_read("/audit"));
        assert!(!is_security_read("/auditx"));
        assert!(!is_security_read("/tracks"));
    }
}
