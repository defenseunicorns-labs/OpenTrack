//! Reporting responsibility (R2): of the nodes that see a track, the one
//! that sees it best reports it, and the others hold it quietly. The rules
//! are Link 16's, simplified, and need no coordination:
//!
//! * **Claim**: report a track no one reports, or one whose reporter's
//!   quality this node beats by at least [`MARGIN`] (so two nodes do not
//!   trade it back and forth).
//! * **Yield**: stop when another node reports it with a higher quality, or
//!   an equal one from a lower site code.
//! * **Take over**: a reporter not heard for [`TAKE_OVER_MS`] (two missed
//!   heartbeats) no longer counts.
//!
//! Only reports under the track's own number count: a node reporting the
//! object under another number has not agreed it is this track yet, and
//! keeping both reported is what lets the other nodes merge them.

use ot_core::SiteCode;

/// A reporter re-sends every track at least this often.
pub const HEARTBEAT_MS: i64 = 12_000;
/// A reporter not heard for this long has stopped.
pub const TAKE_OVER_MS: i64 = 2 * HEARTBEAT_MS;
/// Quality another node must beat the reporter's by to claim a track.
pub const MARGIN: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Heard {
    pub site: SiteCode,
    pub quality: u8,
    pub at_ms: i64,
}

/// One track's reporting responsibility, as this node sees it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Responsibility {
    /// This node reports the track.
    pub reporting: bool,
    /// The best other node heard reporting it.
    pub other: Option<Heard>,
    /// A node that gave the track up, and when: its reports that arrive
    /// after (sent before it left, delivered late) do not count for
    /// [`TAKE_OVER_MS`].
    pub released: Option<(SiteCode, i64)>,
}

/// Whether (`site`, `q`) outranks (`me`, `mine`) as a track's reporter.
fn outranks(site: SiteCode, q: u8, me: SiteCode, mine: u8) -> bool {
    q > mine || (q == mine && site < me)
}

impl Responsibility {
    fn live(&self, now_ms: i64) -> Option<Heard> {
        self.other.filter(|h| now_ms - h.at_ms < TAKE_OVER_MS)
    }

    /// Another node reported the track (under its own number).
    pub fn heard(&mut self, me: SiteCode, mine: Option<u8>, h: Heard) {
        if self
            .released
            .is_some_and(|(site, at)| site == h.site && h.at_ms - at < TAKE_OVER_MS)
        {
            return;
        }
        let replace = match self.live(h.at_ms) {
            None => true,
            Some(o) => o.site == h.site || outranks(h.site, h.quality, o.site, o.quality),
        };
        if replace {
            self.other = Some(h);
        }
        if self.reporting && mine.is_none_or(|q| outranks(h.site, h.quality, me, q)) {
            self.reporting = false;
        }
    }

    /// A node gave the track up (it is leaving).
    pub fn released(&mut self, site: SiteCode, at_ms: i64) {
        if self.other.is_some_and(|h| h.site == site) {
            self.other = None;
        }
        self.released = Some((site, at_ms));
    }

    /// Whether this node reports the track now; `mine` is the quality of
    /// this node's own view of it (none: its own sources do not see it).
    pub fn decide(&mut self, me: SiteCode, mine: Option<u8>, now_ms: i64) -> bool {
        self.reporting = match (mine, self.live(now_ms)) {
            (None, _) => false,
            (Some(_), None) => true,
            (Some(q), Some(h)) if self.reporting => !outranks(h.site, h.quality, me, q),
            (Some(q), Some(h)) => q >= h.quality.saturating_add(MARGIN),
        };
        self.reporting
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(c: &str) -> SiteCode {
        c.parse().unwrap()
    }

    fn heard(site: &str, quality: u8, at_ms: i64) -> Heard {
        Heard {
            site: s(site),
            quality,
            at_ms,
        }
    }

    #[test]
    fn a_track_no_one_reports_is_claimed() {
        let mut r = Responsibility::default();
        assert!(r.decide(s("BBB"), Some(3), 0));
        assert!(!Responsibility::default().decide(s("BBB"), None, 0));
    }

    #[test]
    fn a_better_view_claims_only_by_the_margin() {
        let mut r = Responsibility::default();
        r.heard(s("BBB"), Some(10), heard("AAA", 9, 0));
        assert!(
            !r.decide(s("BBB"), Some(10), 1000),
            "one better is not enough"
        );
        assert!(r.decide(s("BBB"), Some(11), 1000));
    }

    #[test]
    fn two_reporters_settle_on_one() {
        // Both claimed a track at once (neither had heard the other).
        let (mut a, mut b) = (Responsibility::default(), Responsibility::default());
        assert!(a.decide(s("AAA"), Some(9), 0) && b.decide(s("BBB"), Some(9), 0));
        a.heard(s("AAA"), Some(9), heard("BBB", 9, 100));
        b.heard(s("BBB"), Some(9), heard("AAA", 9, 100));
        // Equal quality: the lower site code keeps it.
        assert!(a.decide(s("AAA"), Some(9), 200));
        assert!(!b.decide(s("BBB"), Some(9), 200));
        // A reporter holds against one step better; it yields to more.
        b.heard(s("BBB"), Some(10), heard("AAA", 9, 300));
        a.heard(s("AAA"), Some(9), heard("BBB", 10, 300));
        assert!(!a.reporting, "a yields to a better report it hears");
    }

    #[test]
    fn a_silent_reporter_is_taken_over_and_a_leaving_one_at_once() {
        let mut r = Responsibility::default();
        r.heard(s("BBB"), Some(5), heard("AAA", 12, 0));
        assert!(!r.decide(s("BBB"), Some(5), HEARTBEAT_MS));
        assert!(r.decide(s("BBB"), Some(5), TAKE_OVER_MS));
        let mut r = Responsibility::default();
        r.heard(s("BBB"), Some(5), heard("AAA", 12, 0));
        r.released(s("AAA"), 1);
        // A report it sent before leaving, delivered late, does not count.
        r.heard(s("BBB"), Some(5), heard("AAA", 12, 2));
        assert!(r.decide(s("BBB"), Some(5), 3));
    }

    #[test]
    fn a_node_that_stops_seeing_it_stops_reporting() {
        let mut r = Responsibility::default();
        assert!(r.decide(s("BBB"), Some(5), 0));
        assert!(!r.decide(s("BBB"), None, 1));
    }
}
