//! The notice-and-consent banner (AC-8), enforced on the server: while the
//! warning is on, a person's browser access must accept it before the API
//! answers anything but its own sign-in calls. Accepting is audited
//! (`consent_accepted`).
//!
//! Acceptance is kept per session (a password or SAML sign-in), and per
//! account for client-certificate and OpenStare access, which have no
//! session; the latter lasts [`stig::SESSION_HOURS`]. It is kept in memory,
//! so a restart asks again. API tokens are programs, not people: they are
//! not asked.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::{AuthUser, Via, stig};
use crate::control::AppState;

/// How long the warning's on/off setting is cached.
const SETTING_TTL: Duration = Duration::from_secs(5);

#[derive(Default)]
pub struct Consent {
    /// Acceptance key -> until when (ms).
    accepted: Mutex<HashMap<String, i64>>,
    warning_on: Mutex<Option<(Instant, bool)>>,
}

/// Whose acceptance a caller's is, or `None` when it isn't asked.
fn key(u: &AuthUser) -> Option<(String, i64)> {
    match u.via {
        Via::Session => u.jti.as_ref().map(|(j, exp)| (format!("s:{j}"), *exp)),
        Via::ClientCert | Via::Openstare => Some((
            format!("u:{}:{:?}", u.id, u.via),
            chrono::Utc::now().timestamp_millis() + stig::SESSION_HOURS * 3_600_000,
        )),
        Via::ApiToken | Via::Disabled => None,
    }
}

impl Consent {
    /// Whether the warning banner is on (cached for a few seconds).
    async fn warning_on(&self, s: &AppState) -> bool {
        if let Ok(g) = self.warning_on.lock()
            && let Some((at, on)) = *g
            && at.elapsed() < SETTING_TTL
        {
            return on;
        }
        // Unreadable settings count as on: fail closed.
        let on = crate::settings_api::warning_enabled(s)
            .await
            .unwrap_or(true);
        if let Ok(mut g) = self.warning_on.lock() {
            *g = Some((Instant::now(), on));
        }
        on
    }

    /// Forget the cached setting (after the banners are saved).
    pub fn setting_changed(&self) {
        if let Ok(mut g) = self.warning_on.lock() {
            *g = None;
        }
    }

    /// Whether `u` must still accept the warning.
    pub async fn required(&self, s: &AppState, u: &AuthUser) -> bool {
        let Some((k, _)) = key(u) else {
            return false;
        };
        if !self.warning_on(s).await {
            return false;
        }
        let now = chrono::Utc::now().timestamp_millis();
        let Ok(mut m) = self.accepted.lock() else {
            return true;
        };
        m.retain(|_, until| *until > now);
        !m.contains_key(&k)
    }

    /// Record that `u` accepted the warning.
    pub fn accept(&self, u: &AuthUser) {
        if let Some((k, until)) = key(u)
            && let Ok(mut m) = self.accepted.lock()
        {
            m.insert(k, until);
        }
    }
}
