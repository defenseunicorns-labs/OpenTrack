//! Revisit timing: how often a job's dwells come back to the same ground,
//! measured from the dwells' revisit indexes (D2) and times (D6).

use std::collections::{HashMap, VecDeque};

use serde::Serialize;

use crate::segments::{DwellSegment, JobDefinitionSegment};

/// Periods kept per job; their median is the job's period.
const KEEP: usize = 9;
/// A job counts as active while its last dwell is this recent, or within
/// three of its revisits, whichever is longer (ms of the stream's dwell clock).
const ACTIVE_MS: u32 = 60_000;

/// Where a revisit period came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RevisitSource {
    /// Timed from the job's dwells.
    Measured,
    /// The job definition's nominal revisit interval (J15).
    Nominal,
}

/// The revisit period governing a stream now.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Revisit {
    pub job_id: u32,
    pub period_s: f64,
    pub source: RevisitSource,
    /// Complete revisits the measured period is the median of (0 for nominal).
    pub samples: usize,
}

#[derive(Debug, Default)]
struct Clock {
    /// The revisit under way: its index and first dwell's time.
    current: Option<(u16, u32)>,
    periods: VecDeque<u32>,
    last_dwell_ms: u32,
}

#[derive(Debug, Default)]
pub(crate) struct Revisits {
    jobs: HashMap<u32, Clock>,
    /// The time of the latest dwell, of any job.
    now_ms: u32,
}

impl Revisits {
    pub(crate) fn dwell(&mut self, job_id: u32, d: &DwellSegment) {
        let c = self.jobs.entry(job_id).or_default();
        let (idx, t) = (d.revisit_index, d.dwell_time_ms);
        match c.current {
            Some((cur, _)) if cur == idx => {}
            // The next revisit: the previous one took from its start to now.
            Some((cur, start)) if idx == cur.wrapping_add(1) && t > start => {
                c.periods.push_back(t - start);
                if c.periods.len() > KEEP {
                    c.periods.pop_front();
                }
                c.current = Some((idx, t));
            }
            // First dwell, or a jump (lost packets, a restarted recording).
            _ => c.current = Some((idx, t)),
        }
        c.last_dwell_ms = t;
        self.now_ms = t;
    }

    pub(crate) fn current(&self, jobs: &HashMap<u32, JobDefinitionSegment>) -> Option<Revisit> {
        self.jobs
            .iter()
            .filter_map(|(&job_id, c)| {
                let r = if c.periods.is_empty() {
                    let s = jobs.get(&job_id)?.nominal_revisit_interval_s;
                    (s > 0.0).then_some(Revisit {
                        job_id,
                        period_s: s,
                        source: RevisitSource::Nominal,
                        samples: 0,
                    })?
                } else {
                    let mut p: Vec<u32> = c.periods.iter().copied().collect();
                    p.sort_unstable();
                    Revisit {
                        job_id,
                        period_s: f64::from(p[p.len() / 2]) / 1000.0,
                        source: RevisitSource::Measured,
                        samples: p.len(),
                    }
                };
                let window = ACTIVE_MS.max((r.period_s * 3000.0) as u32);
                (self.now_ms.abs_diff(c.last_dwell_ms) <= window).then_some(r)
            })
            .max_by(|a, b| {
                a.period_s
                    .total_cmp(&b.period_s)
                    .then(b.job_id.cmp(&a.job_id))
            })
    }
}
