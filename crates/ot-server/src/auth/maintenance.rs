//! The account policy's clock, once a minute: save when sessions were
//! last used, end idle and expired sessions, turn off inactive accounts,
//! log the audit record's head, and (with sign-in off) say so loudly. The
//! policy is fixed ([`super::stig`]); the audit record is kept forever.

use std::time::Duration;

use ot_store::AuditEvent;
use ot_store::sqlite::now_ms;
use serde_json::json;

use super::stig;
use crate::control::{ApiError, AppState};

const TICK: Duration = Duration::from_secs(60);

pub async fn run(s: AppState) {
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut n: u64 = 0;
    loop {
        tick.tick().await;
        if s.auth.disabled {
            tracing::warn!(
                "AUTHENTICATION IS DISABLED (OT_AUTH=off): anyone who reaches this server is an admin. Never run it this way outside development."
            );
            continue;
        }
        if let Err(e) = once(&s, n).await {
            tracing::error!(error = %e.message, "account policy upkeep failed");
        }
        n += 1;
    }
}

/// One round; the heavier jobs every 10th and 60th.
async fn once(s: &AppState, n: u64) -> Result<(), ApiError> {
    let settings = s.auth.settings();
    let now = now_ms();

    // Save session use, then end what is idle or expired.
    let seen = s.auth.activity.take_unsaved();
    if !seen.is_empty() {
        s.with_db(move |db| db.touch_sessions(&seen)).await?;
    }
    let idle = stig::idle_ms(crate::auth::Role::Viewer);
    let admin_idle = stig::idle_ms(crate::auth::Role::Admin);
    let ended = s
        .with_db(move |db| {
            let ended = db.end_stale_sessions(now - idle, now - admin_idle)?;
            for e in &ended {
                db.audit(&super::sessions::ended(
                    &e.user_email,
                    &e.id,
                    e.end_reason.as_deref().unwrap_or("idle"),
                    e.ip.as_deref(),
                ))?;
            }
            Ok(ended)
        })
        .await?;
    for e in &ended {
        s.auth.activity.forget(&e.id);
    }
    s.auth.activity.prune(now - 86_400_000);

    disable_expired(s).await?;
    if n.is_multiple_of(10) {
        disable_inactive(s, &settings.inactivity).await?;
    }
    if n.is_multiple_of(60) {
        // The chain's head in the log too: kept elsewhere, it shows if the
        // newest rows were ever cut off.
        let (seq, hash) = s.with_db(|db| db.audit_head()).await?;
        tracing::info!(seq, %hash, "audit record head");
    }
    Ok(())
}

/// Turn off temporary accounts whose time has run out (AC-2(2)).
pub async fn disable_expired(s: &AppState) -> Result<usize, ApiError> {
    let now = now_ms();
    let off = s
        .with_db(move |db| {
            let off = db.disable_expired(now)?;
            for u in &off {
                db.audit(
                    &AuditEvent::new("system", "account_disabled").detail(json!({
                        "user": u.id,
                        "email": u.email,
                        "reason": "expired",
                        "expires_at_ms": u.expires_at_ms,
                    })),
                )?;
            }
            Ok(off)
        })
        .await?;
    for u in &off {
        tracing::warn!(email = %u.email, "turned off a temporary account whose time ran out");
    }
    Ok(off.len())
}

/// Turn off accounts nobody has signed in to for
/// [`stig::DISABLE_INACTIVE_AFTER_DAYS`], except the break-glass ones.
pub async fn disable_inactive(
    s: &AppState,
    p: &super::settings::InactivityPolicy,
) -> Result<usize, ApiError> {
    let cutoff = now_ms() - stig::INACTIVE_MS;
    let (exempt, days) = (p.exempt.clone(), stig::DISABLE_INACTIVE_AFTER_DAYS);
    let off = s
        .with_db(move |db| {
            let off = db.disable_inactive(cutoff, &exempt)?;
            for u in &off {
                db.audit(
                    &AuditEvent::new("system", "account_disabled").detail(json!({
                        "user": u.id,
                        "email": u.email,
                        "reason": "inactivity",
                        "days": days,
                        "last_activity_ms": u.last_activity_ms(),
                    })),
                )?;
            }
            Ok(off)
        })
        .await?;
    for u in &off {
        tracing::warn!(email = %u.email, days, "turned off an inactive account");
    }
    Ok(off.len())
}
