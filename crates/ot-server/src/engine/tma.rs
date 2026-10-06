//! Locating an emitter from one sensor's bearings over time (see
//! `docs/non-point-contacts.md`, single-sensor location).
//!
//! A single ESM sensor gives no range, but a moving one sees the emitter
//! from a changing place: its own motion is the baseline. The bearings of
//! one sensor track (one emitter, as the sensor's own tracker keeps it) are
//! fitted by weighted least squares, first as a **fixed** emitter (position
//! only), and, when the bearings stop fitting a fixed point, as a **moving**
//! one (position and constant velocity: bearings-only target motion
//! analysis). Either way the covariance comes from the geometry, so a short
//! or straight collection gives a long thin ellipse along the line, and a
//! manoeuvre across it tightens the fix.
//!
//! Everything here is in a local plane: metres north and east of the
//! newest bearing's sensor position, time in seconds before the newest
//! bearing (so the solution is where the emitter is now).

/// One bearing, in the local plane.
#[derive(Debug, Clone, Copy)]
pub struct Line {
    /// Seconds relative to the newest bearing (zero or negative).
    pub t: f64,
    /// Sensor position.
    pub n: f64,
    pub e: f64,
    /// Bearing and its error, radians.
    pub bearing: f64,
    pub sigma: f64,
    /// How far the sensor can detect, metres.
    pub range: f64,
}

/// Which model fitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Model {
    Fixed,
    Moving,
}

/// Where the emitter is now, and how sure.
#[derive(Debug, Clone, PartialEq)]
pub struct Solution {
    pub model: Model,
    pub n: f64,
    pub e: f64,
    /// Velocity north, east (m/s); zero for a fixed emitter.
    pub vn: f64,
    pub ve: f64,
    /// Covariance of (n, e, vn, ve); the velocity rows are zero for a fixed
    /// emitter.
    pub cov: [[f64; 4]; 4],
    /// Sum of squared residuals in sigmas, and its degrees of freedom.
    pub chi2: f64,
    pub dof: usize,
}

impl Solution {
    /// Position error, metres (root of the covariance's trace).
    pub fn sigma_m(&self) -> f64 {
        (self.cov[0][0] + self.cov[1][1]).max(0.0).sqrt()
    }

    /// Speed error, m/s.
    pub fn sigma_mps(&self) -> f64 {
        (self.cov[2][2] + self.cov[3][3]).max(0.0).sqrt()
    }
}

/// The chi-square value a fit's residuals stay under 99% of the time
/// (Wilson-Hilferty).
pub fn chi2_99(dof: usize) -> f64 {
    let k = dof.max(1) as f64;
    let h = 2.0 / (9.0 * k);
    k * (1.0 - h + 2.326 * h.sqrt()).powi(3)
}

fn wrap(a: f64) -> f64 {
    let a = (a + std::f64::consts::PI).rem_euclid(2.0 * std::f64::consts::PI);
    a - std::f64::consts::PI
}

/// Where the emitter was at a line's time, under a state.
fn at(x: &[f64; 4], l: &Line) -> (f64, f64) {
    (x[0] + x[2] * l.t, x[1] + x[3] * l.t)
}

/// Normalised residuals and their Jacobian rows (in sigmas per unit of
/// state). `None` when the emitter would be behind a sensor or on top of it.
fn residuals(x: &[f64; 4], lines: &[Line]) -> Option<Vec<(f64, [f64; 4])>> {
    lines
        .iter()
        .map(|l| {
            let (tn, te) = at(x, l);
            let (dn, de) = (tn - l.n, te - l.e);
            let r2 = dn * dn + de * de;
            // Ahead of the sensor along its bearing, and not on top of it.
            if r2 < 1.0 || dn * l.bearing.cos() + de * l.bearing.sin() <= 0.0 {
                return None;
            }
            let h = de.atan2(dn);
            let r = wrap(l.bearing - h) / l.sigma;
            // d(bearing)/d(target north, east); the residual moves the other way.
            let (gn, ge) = (-de / r2, dn / r2);
            let j = [gn, ge, gn * l.t, ge * l.t].map(|g| g / l.sigma);
            Some((r, j))
        })
        .collect()
}

/// Invert a symmetric positive-definite matrix of size `k` (at most 4).
fn invert(a: &[[f64; 4]; 4], k: usize) -> Option<[[f64; 4]; 4]> {
    let mut m = *a;
    let mut inv = [[0.0; 4]; 4];
    for (i, row) in inv.iter_mut().enumerate().take(k) {
        row[i] = 1.0;
    }
    for c in 0..k {
        let p = (c..k).max_by(|&i, &j| m[i][c].abs().total_cmp(&m[j][c].abs()))?;
        if m[p][c].abs() < 1e-18 {
            return None;
        }
        m.swap(c, p);
        inv.swap(c, p);
        let d = m[c][c];
        for j in 0..k {
            m[c][j] /= d;
            inv[c][j] /= d;
        }
        for i in 0..k {
            if i != c {
                let f = m[i][c];
                for j in 0..k {
                    m[i][j] -= f * m[c][j];
                    inv[i][j] -= f * inv[c][j];
                }
            }
        }
    }
    Some(inv)
}

/// Fit one model from a starting state (Levenberg-Marquardt).
fn fit(lines: &[Line], model: Model, start: [f64; 4]) -> Option<Solution> {
    let k = if model == Model::Fixed { 2 } else { 4 };
    let cost = |x: &[f64; 4]| -> Option<f64> {
        residuals(x, lines).map(|r| r.iter().map(|(v, _)| v * v).sum())
    };
    let mut x = start;
    if k == 2 {
        x[2] = 0.0;
        x[3] = 0.0;
    }
    let mut c = cost(&x)?;
    let mut lambda = 1e-3;
    for _ in 0..60 {
        let res = residuals(&x, lines)?;
        let mut a = [[0.0; 4]; 4];
        let mut g = [0.0; 4];
        for (r, j) in &res {
            for p in 0..k {
                g[p] += j[p] * r;
                for q in 0..k {
                    a[p][q] += j[p] * j[q];
                }
            }
        }
        let mut improved = false;
        for _ in 0..12 {
            let mut damped = a;
            for (p, row) in damped.iter_mut().enumerate().take(k) {
                row[p] += lambda * a[p][p].max(1e-12);
            }
            let Some(inv) = invert(&damped, k) else {
                lambda *= 10.0;
                continue;
            };
            // The residual is measured minus predicted, so step with +J^T r.
            let mut next = x;
            for p in 0..k {
                next[p] += (0..k).map(|q| inv[p][q] * g[q]).sum::<f64>();
            }
            match cost(&next) {
                Some(nc) if nc < c => {
                    let done = (c - nc) < 1e-6 * c.max(1e-9);
                    x = next;
                    c = nc;
                    lambda = (lambda / 10.0).max(1e-9);
                    improved = !done;
                    break;
                }
                _ => lambda *= 10.0,
            }
        }
        if !improved {
            break;
        }
    }
    // Covariance from the information at the solution.
    let res = residuals(&x, lines)?;
    let mut a = [[0.0; 4]; 4];
    for (_, j) in &res {
        for p in 0..k {
            for q in 0..k {
                a[p][q] += j[p] * j[q];
            }
        }
    }
    let cov = invert(&a, k)?;
    if (0..k).any(|p| !cov[p][p].is_finite() || cov[p][p] <= 0.0) {
        return None;
    }
    Some(Solution {
        model,
        n: x[0],
        e: x[1],
        vn: x[2],
        ve: x[3],
        cov,
        chi2: c,
        dof: lines.len().saturating_sub(k),
    })
}

/// The best fit of a model over starting ranges along the newest bearing.
fn best(lines: &[Line], model: Model, seeds: &[[f64; 4]]) -> Option<Solution> {
    seeds
        .iter()
        .filter_map(|s| fit(lines, model, *s))
        .min_by(|a, b| a.chi2.total_cmp(&b.chi2))
}

/// The fewest bearings, and the shortest span, worth fitting.
pub const MIN_LINES: usize = 4;
pub const MIN_SPAN_S: f64 = 30.0;
/// A moving fit needs more bearings than a fixed one.
pub const MIN_LINES_MOVING: usize = 8;

/// Locate the emitter: fixed if the bearings fit a fixed point (chi-square
/// at 99%) and a moving target fits no better, else moving if they fit
/// constant velocity. `moving` says the emitter has been seen moving
/// before: then only the moving model is tried. `max_unseen_mps` is the
/// fastest a fixed-looking emitter may really be moving. `None` until the bearings
/// say something: too few, too short a span, or no fit.
pub fn locate(lines: &[Line], moving: bool, max_unseen_mps: f64) -> Option<Solution> {
    let newest = lines.iter().max_by(|a, b| a.t.total_cmp(&b.t))?;
    let span = lines.iter().map(|l| -l.t).fold(0.0, f64::max);
    if lines.len() < MIN_LINES || span < MIN_SPAN_S {
        return None;
    }
    // Starting points: ranges along the newest bearing, out to the sensor's.
    let seeds: Vec<[f64; 4]> = [0.05, 0.1, 0.2, 0.35, 0.5, 0.75, 0.95]
        .iter()
        .map(|f| {
            let r = f * newest.range;
            [
                newest.n + r * newest.bearing.cos(),
                newest.e + r * newest.bearing.sin(),
                0.0,
                0.0,
            ]
        })
        .collect();
    let fixed = if moving {
        None
    } else {
        best(lines, Model::Fixed, &seeds)
    };
    let fixed_fits = fixed.as_ref().is_some_and(|f| f.chi2 <= chi2_99(f.dof));
    if lines.len() < MIN_LINES_MOVING {
        return fixed.filter(|_| fixed_fits);
    }
    // Moving: start from the fixed fit and from the ranges, at rest.
    let mut seeds = seeds;
    if let Some(f) = &fixed {
        seeds.push([f.n, f.e, 0.0, 0.0]);
    }
    let moved = best(lines, Model::Moving, &seeds).filter(|m| m.chi2 <= chi2_99(m.dof));
    let span = lines.iter().map(|l| -l.t).fold(0.0, f64::max);
    match (fixed.filter(|_| fixed_fits), moved) {
        // Two more parameters must buy a real improvement (likelihood ratio,
        // chi-square with 2 degrees of freedom at 99%) to call it moving.
        (Some(f), Some(m)) if f.chi2 - m.chi2 > chi2_99(2) => Some(m),
        // Fixed, but only as far as the bearings can tell: motion along the
        // line of sight barely turns them. The emitter may have moved as far
        // as the fastest speed they allow (capped) over half the window.
        (Some(mut f), Some(m)) => {
            let v = (m.vn.hypot(m.ve) + 2.0 * m.sigma_mps()).min(max_unseen_mps);
            let d2 = (v * span / 2.0).powi(2) / 2.0;
            f.cov[0][0] += d2;
            f.cov[1][1] += d2;
            Some(f)
        }
        (f, m) => m.or(f),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic normal noise (splitmix64 and Box-Muller).
    fn rng(seed: u64) -> impl FnMut() -> f64 {
        let mut st = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0x632B_E59B_D9B4_E019;
        move || {
            let mut next = || {
                st = st.wrapping_add(0x9E37_79B9_7F4A_7C15);
                let mut z = st;
                z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
                z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
                z ^= z >> 31;
                ((z >> 11) as f64 / (1u64 << 53) as f64).max(1e-12)
            };
            let (a, b) = (next(), next());
            (-2.0 * a.ln()).sqrt() * (2.0 * std::f64::consts::PI * b).cos()
        }
    }

    /// An aircraft at 110 m/s flying `legs` (heading, seconds), bearing an
    /// emitter starting at (n0, e0) and moving at (vn, ve) every 5 s with
    /// `sigma` degrees of noise. Lines are relative to the newest; returns
    /// them and where the emitter truly is now.
    fn collect(
        legs: &[(f64, f64)],
        target: [f64; 4],
        sigma: f64,
        seed: u64,
    ) -> (Vec<Line>, (f64, f64)) {
        let mut noise = rng(seed);
        let (mut an, mut ae, mut t) = (0.0_f64, 0.0_f64, 0.0);
        let mut raw = Vec::new();
        for &(hdg, dur) in legs {
            for _ in 0..(dur / 5.0) as usize {
                an += 550.0 * hdg.to_radians().cos();
                ae += 550.0 * hdg.to_radians().sin();
                t += 5.0;
                let (tn, te) = (target[0] + target[2] * t, target[1] + target[3] * t);
                raw.push((
                    t,
                    an,
                    ae,
                    (te - ae).atan2(tn - an) + sigma.to_radians() * noise(),
                ));
            }
        }
        let (tl, nl, el, _) = *raw.last().unwrap();
        let lines = raw
            .iter()
            .map(|&(t, n, e, b)| Line {
                t: t - tl,
                n: n - nl,
                e: e - el,
                bearing: b,
                sigma: sigma.to_radians(),
                range: 150_000.0,
            })
            .collect();
        let now = (
            target[0] + target[2] * tl - nl,
            target[1] + target[3] * tl - el,
        );
        (lines, now)
    }

    /// The squared Mahalanobis distance of the truth from a solution.
    fn m2(s: &Solution, truth: (f64, f64)) -> f64 {
        let (dn, de) = (s.n - truth.0, s.e - truth.1);
        let (a, b, c) = (s.cov[0][0], s.cov[0][1], s.cov[1][1]);
        (c * dn * dn - 2.0 * b * dn * de + a * de * de) / (a * c - b * b)
    }

    #[test]
    fn a_fixed_emitter_is_located_from_a_straight_leg() {
        // A radar 50 km east; the aircraft flies north for 5 minutes.
        let (lines, truth) = collect(&[(0.0, 300.0)], [0.0, 50_000.0, 0.0, 0.0], 2.0, 1);
        let s = locate(&lines, false, 30.0).unwrap();
        assert_eq!(s.model, Model::Fixed);
        let err = (s.n - truth.0).hypot(s.e - truth.1);
        assert!(
            err < 3.0 * s.sigma_m(),
            "{err:.0} m off, sigma {:.0}",
            s.sigma_m()
        );
        // 33 km of baseline at 50 km: a few kilometres, not tens.
        assert!(s.sigma_m() < 6_000.0, "{}", s.sigma_m());
    }

    #[test]
    fn a_short_collection_says_nothing_yet() {
        let (lines, _) = collect(&[(0.0, 20.0)], [0.0, 50_000.0, 0.0, 0.0], 2.0, 1);
        assert!(locate(&lines, false, 30.0).is_none());
    }

    #[test]
    fn a_fast_boat_needs_the_moving_model_and_a_manoeuvre() {
        // A boat 40 km north-east doing 20 m/s north-west; the aircraft flies
        // east, then turns north, then west: the classic TMA manoeuvre.
        let boat = [30_000.0, 30_000.0, 14.1, -14.1];
        let (lines, truth) = collect(&[(90.0, 150.0), (0.0, 150.0), (270.0, 150.0)], boat, 1.5, 1);
        let s = locate(&lines, false, 30.0).unwrap();
        assert_eq!(s.model, Model::Moving, "a fixed point cannot explain it");
        let err = (s.n - truth.0).hypot(s.e - truth.1);
        assert!(
            err < 3.0 * s.sigma_m(),
            "{err:.0} m off, sigma {:.0}",
            s.sigma_m()
        );
        let verr = (s.vn - boat[2]).hypot(s.ve - boat[3]);
        assert!(
            verr < 3.0 * s.sigma_mps().max(1.0),
            "velocity {verr:.1} m/s off, sigma {:.1}",
            s.sigma_mps()
        );
    }

    /// Over many noise draws, the stated error must be honest: the truth
    /// inside the 2-sigma ellipse (95.4% for two dimensions) nearly always.
    #[test]
    fn the_stated_error_is_honest() {
        type Case<'a> = (&'a [(f64, f64)], [f64; 4], f64);
        let cases: [Case; 3] = [
            (&[(0.0, 300.0)], [0.0, 50_000.0, 0.0, 0.0], 2.0),
            (
                &[(0.0, 150.0), (90.0, 150.0)],
                [0.0, 50_000.0, 0.0, 0.0],
                2.0,
            ),
            (
                &[(90.0, 150.0), (0.0, 150.0), (270.0, 150.0)],
                [30_000.0, 30_000.0, 14.1, -14.1],
                1.5,
            ),
        ];
        for (legs, target, sigma) in cases {
            let runs = 60;
            let within = (0..runs)
                .filter(|&seed| {
                    let (lines, truth) = collect(legs, target, sigma, seed);
                    locate(&lines, false, 30.0).is_some_and(|s| m2(&s, truth) <= 6.18)
                })
                .count();
            assert!(
                within * 100 >= 90 * runs as usize,
                "{legs:?}: {within}/{runs} within 2 sigma"
            );
        }
    }

    #[test]
    fn chi2_bound_is_close_to_the_table() {
        for (k, want) in [(1, 6.63), (2, 9.21), (4, 13.28), (10, 23.21), (50, 76.15)] {
            assert!(
                (chi2_99(k) - want).abs() / want < 0.03,
                "{k}: {}",
                chi2_99(k)
            );
        }
    }
}
