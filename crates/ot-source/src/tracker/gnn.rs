//! Global nearest neighbour: each scan, predict every track, gate every
//! plot, and take the assignment with the least total cost (normalised
//! distance plus the log of the innovation's size, so a vague track does not
//! win a plot just by being vague). Unassigned plots start tracks.

use chrono::{DateTime, Utc};

use super::assign::assign;
use super::filter::Kf;
use super::{Associate, Life, Plot, Report, TrackerSpec};

struct Track {
    id: u64,
    kf: Kf,
    life: Life,
}

pub(super) struct Gnn {
    spec: TrackerSpec,
    tracks: Vec<Track>,
    next: u64,
}

impl Gnn {
    pub(super) fn new(spec: TrackerSpec) -> Self {
        Self {
            spec,
            tracks: Vec::new(),
            next: 1,
        }
    }
}

const OUT_OF_GATE: f64 = 1e12;

impl Associate for Gnn {
    fn spec_mut(&mut self) -> &mut TrackerSpec {
        &mut self.spec
    }

    fn scan(&mut self, t: DateTime<Utc>, plots: &[Plot]) -> Vec<Report> {
        let q = self.spec.process_noise_mps2;
        let preds: Vec<Kf> = self.tracks.iter().map(|tr| tr.kf.predict(t, q)).collect();
        let mut inns = Vec::with_capacity(preds.len());
        let cost: Vec<Vec<f64>> = preds
            .iter()
            .map(|p| {
                let row: Vec<_> = plots
                    .iter()
                    .map(|z| p.innovation(z.lat, z.lon, z.r))
                    .collect();
                let c = row
                    .iter()
                    .map(|i| {
                        if i.d2 <= self.spec.gate {
                            i.d2 + i.ln_det
                        } else {
                            OUT_OF_GATE
                        }
                    })
                    .collect();
                inns.push(row);
                c
            })
            .collect();
        let mut taken = vec![false; plots.len()];
        let mut reports = Vec::new();
        for (i, pick) in assign(&cost).into_iter().enumerate() {
            let tr = &mut self.tracks[i];
            match pick.filter(|&j| cost[i][j] < OUT_OF_GATE) {
                Some(j) => {
                    taken[j] = true;
                    tr.kf = preds[i].update(&inns[i][j]);
                    tr.life.hit(t, &plots[j], &inns[i][j], &self.spec);
                    if tr.life.confirmed {
                        reports.push(Report {
                            id: tr.id,
                            kf: tr.kf.clone(),
                            domain: tr.life.domain(&self.spec),
                            plot: j,
                            dropped: false,
                            existence: tr.life.existence,
                        });
                    }
                }
                None => {
                    tr.kf = preds[i].clone();
                    tr.life.miss(t, &self.spec);
                }
            }
        }
        let spec = &self.spec;
        self.tracks.retain(|tr| {
            let alive = tr.life.alive(t, spec);
            if !alive && tr.life.confirmed && !plots.is_empty() {
                reports.push(Report {
                    id: tr.id,
                    kf: tr.kf.clone(),
                    domain: tr.life.domain(spec),
                    plot: 0,
                    dropped: true,
                    existence: tr.life.existence,
                });
            }
            alive
        });
        for (j, z) in plots.iter().enumerate() {
            if taken[j] {
                continue;
            }
            let life = Life::new(t, z, spec);
            let id = self.next;
            self.next += 1;
            let kf = Kf::birth(z.lat, z.lon, z.r, spec.initial_speed_sigma_mps, t);
            if life.confirmed {
                reports.push(Report {
                    id,
                    kf: kf.clone(),
                    domain: life.domain(spec),
                    plot: j,
                    dropped: false,
                    existence: life.existence,
                });
            }
            self.tracks.push(Track { id, kf, life });
        }
        reports
    }
}
