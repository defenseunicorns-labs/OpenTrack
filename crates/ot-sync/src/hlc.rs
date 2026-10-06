//! Hybrid logical clock: wall-clock milliseconds in the high 48 bits and a
//! counter in the low 16, so stamps follow real time closely, never go
//! backwards on a node, and a decision made after hearing another's always
//! stamps later than it, whatever the two clocks say.

use std::fmt;

use serde::{Deserialize, Serialize};

const LOGICAL_BITS: u32 = 16;

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Hlc(pub u64);

impl Hlc {
    pub fn new(ms: u64, logical: u16) -> Self {
        Self((ms << LOGICAL_BITS) | u64::from(logical))
    }

    /// Wall-clock part, Unix milliseconds.
    pub fn ms(self) -> u64 {
        self.0 >> LOGICAL_BITS
    }

    pub fn logical(self) -> u16 {
        (self.0 & 0xffff) as u16
    }

    /// As SQLite stores it (an HLC for the next few thousand years fits).
    pub fn as_i64(self) -> i64 {
        self.0 as i64
    }

    pub fn from_i64(v: i64) -> Self {
        Self(v as u64)
    }
}

impl fmt::Display for Hlc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.ms(), self.logical())
    }
}

/// A node's clock.
#[derive(Debug, Clone, Default)]
pub struct Clock {
    last: Hlc,
}

impl Clock {
    /// Start after every stamp this node has already given or seen.
    pub fn after(last: Hlc) -> Self {
        Self { last }
    }

    pub fn last(&self) -> Hlc {
        self.last
    }

    /// A stamp for something happening now.
    pub fn tick(&mut self, now_ms: u64) -> Hlc {
        self.last = if now_ms > self.last.ms() {
            Hlc::new(now_ms, 0)
        } else {
            Hlc(self.last.0 + 1)
        };
        self.last
    }

    /// Another node's stamp was heard: later stamps here come after it.
    pub fn observe(&mut self, seen: Hlc) {
        self.last = self.last.max(seen);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_rise_even_when_the_wall_clock_does_not() {
        let mut c = Clock::default();
        let a = c.tick(1000);
        let b = c.tick(1000);
        let d = c.tick(999);
        assert!(a < b && b < d);
        assert_eq!(a, Hlc::new(1000, 0));
        assert_eq!(d, Hlc::new(1000, 2));
        assert_eq!(c.tick(2000), Hlc::new(2000, 0));
    }

    #[test]
    fn a_decision_after_hearing_one_stamps_later_than_it() {
        let mut slow = Clock::default();
        let heard = Hlc::new(5000, 3);
        slow.observe(heard);
        assert!(slow.tick(1000) > heard);
    }
}
