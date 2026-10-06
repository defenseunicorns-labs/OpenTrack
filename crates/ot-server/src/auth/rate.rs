//! The API's request rate limit (SC-5): a token bucket per client, so one
//! client (a runaway script, a flood) can't take the server from everyone
//! else. A client is its account once signed in (all its sessions and
//! browsers together), each API token on its own, and its address before
//! then. Sign-in has its own, stricter limit (`ATTEMPT_BURST`).
//!
//! The values are fixed, not settings. A busy page (the overview polling,
//! the track table, a map loading its tiles) makes a few requests a second
//! and bursts of a few dozen, well inside them.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Requests a second a client may keep up.
pub const PER_SEC: f64 = 20.0;
/// Requests a client may make at once, after a quiet spell.
pub const BURST: f64 = 100.0;
/// A limited client is logged and audited at most this often.
const NOTE_EVERY: Duration = Duration::from_secs(60);

struct Bucket {
    tokens: f64,
    at: Instant,
    noted: Option<Instant>,
}

/// Every client's bucket.
#[derive(Default)]
pub struct Limiter {
    buckets: Mutex<HashMap<String, Bucket>>,
}

/// Whether a request may go ahead.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    /// Not now: try again after this many seconds. `note`: the first
    /// refusal in [`NOTE_EVERY`], to log and audit.
    Refuse {
        retry_after: u64,
        note: bool,
    },
}

impl Limiter {
    /// Take one request from `client`'s bucket.
    pub fn check(&self, client: &str) -> Verdict {
        self.check_at(client, Instant::now())
    }

    fn check_at(&self, client: &str, now: Instant) -> Verdict {
        let Ok(mut m) = self.buckets.lock() else {
            return Verdict::Allow;
        };
        if m.len() > 10_000 {
            // A bucket left alone this long is full again: the same as none.
            let full = Duration::from_secs_f64(BURST / PER_SEC);
            m.retain(|_, b| now.duration_since(b.at) < full);
        }
        let b = m.entry(client.to_owned()).or_insert(Bucket {
            tokens: BURST,
            at: now,
            noted: None,
        });
        b.tokens = (b.tokens + now.duration_since(b.at).as_secs_f64() * PER_SEC).min(BURST);
        b.at = now;
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            return Verdict::Allow;
        }
        let retry_after = ((1.0 - b.tokens) / PER_SEC).ceil().max(1.0) as u64;
        let note = b.noted.is_none_or(|t| now.duration_since(t) >= NOTE_EVERY);
        if note {
            b.noted = Some(now);
        }
        Verdict::Refuse { retry_after, note }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_client_gets_its_burst_then_its_rate() {
        let l = Limiter::default();
        let t0 = Instant::now();
        assert!((0..100).all(|_| l.check_at("a", t0) == Verdict::Allow));
        assert_eq!(
            l.check_at("a", t0),
            Verdict::Refuse {
                retry_after: 1,
                note: true
            }
        );
        // Noted once a minute, not per request.
        assert_eq!(
            l.check_at("a", t0),
            Verdict::Refuse {
                retry_after: 1,
                note: false
            }
        );
        // Another client is not held up.
        assert_eq!(l.check_at("b", t0), Verdict::Allow);
        // A second later, 20 more.
        let t1 = t0 + Duration::from_secs(1);
        assert!((0..20).all(|_| l.check_at("a", t1) == Verdict::Allow));
        assert!(matches!(
            l.check_at("a", t1),
            Verdict::Refuse { note: false, .. }
        ));
        // Refused again after a minute: noted again.
        let t2 = t0 + Duration::from_secs(61);
        assert!((0..100).all(|_| l.check_at("a", t2) == Verdict::Allow));
        assert!(matches!(
            l.check_at("a", t2),
            Verdict::Refuse { note: true, .. }
        ));
    }

    #[test]
    fn a_steady_twenty_a_second_is_never_refused() {
        let l = Limiter::default();
        let t0 = Instant::now();
        for i in 0..2_000u64 {
            let t = t0 + Duration::from_millis(i * 50);
            assert_eq!(l.check_at("ui", t), Verdict::Allow, "request {i}");
        }
    }
}
