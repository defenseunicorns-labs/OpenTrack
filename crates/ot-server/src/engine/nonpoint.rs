//! Lines of bearing (see `docs/non-point-contacts.md`).
//!
//! A bearing goes to a track when its emitter identity matches, or when
//! its emitter was cross-fixed onto the track; there it adds evidence and
//! identity but never moves the track. Any other bearing waits, for a
//! minute, to be crossed with bearings from other sensors into a position:
//! a fix, which enters the picture as a report of the built-in source `fix`.
//! Ghosts (lines crossing where nothing is) are kept out: bearings with an
//! emitter identity only fix with the same identity; without one, a fix
//! needs three sensors agreeing, twice, and not all explained by other
//! tracks.
//!
//! Areas of uncertainty correlate as points with a large error; one that
//! carries an emitter identity also pairs with the track that identity was
//! found on (`area_emitter`).

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use ot_core::geometry::{bearing_to, local, offset};
use ot_core::{BearingContact, Covariance, Geometry, Observation, Uid, Uncertainty};
use serde_json::json;

use super::{Engine, EngineCounts, tma};

/// Bearings wait this long to be crossed.
const WINDOW_S: i64 = 60;
/// Associated bearings are shown for this long.
const KEEP_S: i64 = 300;
/// Three sigma, one degree of freedom.
const GATE_D2: f64 = 9.0;
/// How much better the best track must fit than the next.
const AMBIGUITY: f64 = 10.0;
/// Lines must cross at least this steeply for a useful fix.
const MIN_CROSS_DEG: f64 = 20.0;
/// Sensors closer than this are one position (no baseline).
const MIN_BASELINE_M: f64 = 200.0;
/// A sensor's range when it does not say.
const DEFAULT_RANGE_M: f64 = 250_000.0;
/// Source id of cross-fixes.
pub const FIX: &str = "fix";
/// Chi-square at 99% for 1 to 4 degrees of freedom (a set of n lines fixing
/// two coordinates has n - 2).
const CHI2_99: [f64; 4] = [6.63, 9.21, 11.34, 13.28];
/// And at 95%: a set within it fits well; one between the two, marginally.
const CHI2_95: [f64; 4] = [3.84, 5.99, 7.81, 9.49];
/// Single-sensor location: the bearings it uses (seconds, most), and how
/// far the platform must have moved over them.
const TMA_WINDOW_S: i64 = 300;
const TMA_MAX_LINES: usize = 120;
const TMA_MIN_BASELINE_M: f64 = 1000.0;
/// Fastest an emitter moves between an anonymous set's two fixes.
const MAX_SPEED_MPS: f64 = 40.0;

/// A bearing's parts.
#[derive(Debug, Clone)]
struct Lob {
    obs: Observation,
    bearing: f64,
    sigma: f64,
    measured_range: Option<f64>,
    range_sigma: Option<f64>,
    range: f64,
    elevation: Option<f64>,
    /// Where the tracks it passes through are, when no track took it.
    through: Vec<(f64, f64)>,
}

impl Lob {
    fn of(obs: Observation) -> Option<Self> {
        match obs.geometry {
            Some(Geometry::Bearing {
                bearing_deg,
                sigma_deg,
                range_m,
                range_sigma_m,
                max_range_m,
                elevation_deg,
                ..
            }) => Some(Self {
                bearing: bearing_deg,
                sigma: sigma_deg,
                measured_range: range_m,
                range_sigma: range_sigma_m,
                range: max_range_m.unwrap_or(DEFAULT_RANGE_M),
                elevation: elevation_deg,
                through: Vec::new(),
                obs,
            }),
            _ => None,
        }
    }

    fn origin(&self) -> (f64, f64) {
        (self.obs.position.latitude, self.obs.position.longitude)
    }

    /// The emitter identity it carries, e.g. `elnot:A123`.
    fn identity(&self) -> Option<String> {
        identity(&self.obs.identifiers)
    }

    /// Convert a complete polar measurement into a point report, preserving
    /// its radial and cross-range uncertainty in north/east covariance.
    fn ranged_point(&self) -> Option<Observation> {
        let range = self.measured_range?;
        let range_sigma = self.range_sigma?;
        let elevation = self.elevation.unwrap_or(0.0).to_radians();
        let angle_sigma = self.sigma.to_radians();
        let horizontal = range * elevation.cos();
        let (sin_b, cos_b) = self.bearing.to_radians().sin_cos();
        let (lat, lon) = offset(
            self.obs.position.latitude,
            self.obs.position.longitude,
            horizontal * cos_b,
            horizontal * sin_b,
        );

        let radial_sigma =
            (range_sigma * elevation.cos()).hypot(range * elevation.sin() * angle_sigma);
        let cross_sigma = horizontal * angle_sigma;
        let radial_var = radial_sigma.powi(2);
        let cross_var = cross_sigma.powi(2);
        let covariance = [
            cos_b.powi(2) * radial_var + sin_b.powi(2) * cross_var,
            sin_b * cos_b * (radial_var - cross_var),
            sin_b.powi(2) * radial_var + cos_b.powi(2) * cross_var,
        ];

        let mut point = self.obs.clone();
        point.position.latitude = lat;
        point.position.longitude = lon;
        point.position.altitude_hae_m = self
            .obs
            .position
            .altitude_hae_m
            .map(|alt| alt + range * elevation.sin());
        point.uncertainty = Some(Uncertainty {
            vertical_error_m: Some(
                (range_sigma * elevation.sin()).hypot(range * elevation.cos() * angle_sigma),
            ),
            covariance: Some(Covariance {
                position: covariance,
                velocity: None,
                cross: None,
            }),
            ..Uncertainty::default()
        });
        point.geometry = None;
        Some(point)
    }
}

fn identity(ids: &[ot_core::schema::Identifier]) -> Option<String> {
    ids.first().map(|i| format!("{}:{}", i.scheme, i.value))
}

/// A NATS subject token: no dots, spaces or wildcards.
fn token(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c == '.' || c == '*' || c == '>' || c.is_whitespace() {
                '_'
            } else {
                c
            }
        })
        .collect()
}

/// Angle difference in (-180, 180].
fn wrap(d: f64) -> f64 {
    let d = d.rem_euclid(360.0);
    if d > 180.0 { d - 360.0 } else { d }
}

/// State the engine keeps for bearings.
#[derive(Debug, Default)]
pub(super) struct Bearings {
    waiting: Vec<Lob>,
    /// The fix key each sensor track's emitter went into: once that fix
    /// reports for a track, the emitter's bearings go to the track directly.
    members: HashMap<(String, String), String>,
    /// When the newest line not yet swept for fixes came in.
    fresh: Option<DateTime<Utc>>,
    /// Ghost rules off (three-sensor consensus only), to show what they do.
    #[cfg(test)]
    pub(super) naive: bool,
    /// The track each sensor track's lines were found pointing at together.
    bound: HashMap<(String, String), Uid>,
    /// The track each sensor track's lines last went to, and when.
    held: HashMap<(String, String), (Uid, DateTime<Utc>)>,
    /// Sensor tracks whose emitter has been seen moving.
    moving: std::collections::HashSet<(String, String)>,
    /// Each sensor track's emitter range at its last single-sensor location.
    ranges: HashMap<(String, String), f64>,
    /// Each sensor track's recent bearings, for single-sensor location.
    history: HashMap<(String, String), std::collections::VecDeque<Lob>>,
    /// Anonymous sets seen once, waiting to fix again: where, error, when.
    combos: HashMap<String, (f64, f64, f64, DateTime<Utc>)>,
    /// The track each area source track's last report found its emitter
    /// on (`area_emitter`): a second report finding it there pairs them.
    areas: HashMap<String, Uid>,
}

/// How a bearing fits a track.
#[derive(Debug, Clone, Copy)]
struct Fit {
    /// Squared residual in sigmas (the bearing's and the track's).
    d2: f64,
    residual: f64,
    sigma: f64,
    /// Where the track is at the bearing's time.
    lat: f64,
    lon: f64,
    /// The emitter identity matches the track's.
    same: bool,
}

/// A fix: position, covariance `[nn, ne, ee]` and the bearings in it.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Fix {
    pub lat: f64,
    pub lon: f64,
    pub cov: [f64; 3],
}

/// Least-squares crossing of bearings, weighted by each one's angular
/// error at its range; `None` when the lines do not cross ahead of every
/// sensor or cross too flat.
fn cross(lobs: &[&Lob]) -> Option<Fix> {
    cross_at(lobs, MIN_CROSS_DEG)
}

/// `cross`, with lines crossing at least `steepest` degrees.
fn cross_at(lobs: &[&Lob], steepest: f64) -> Option<Fix> {
    let (lat0, lon0) = lobs[0].origin();
    let pts: Vec<(f64, f64, f64, f64)> = lobs
        .iter()
        .map(|l| {
            let (n, e) = local(lat0, lon0, l.origin().0, l.origin().1);
            let t = l.bearing.to_radians();
            (n, e, t, l.sigma.to_radians())
        })
        .collect();
    // Steep enough between at least one pair.
    let steep = lobs.iter().enumerate().any(|(i, a)| {
        lobs[i + 1..].iter().any(|b| {
            let d = wrap(a.bearing - b.bearing).abs();
            (steepest..=180.0 - steepest).contains(&d)
        })
    });
    if !steep {
        return None;
    }
    let mut ranges = vec![10_000.0_f64; pts.len()];
    let mut x = (0.0, 0.0);
    let mut cov = [0.0; 3];
    for _ in 0..4 {
        let (mut a11, mut a12, mut a22, mut b1, mut b2) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for ((n, e, t, s), r) in pts.iter().zip(&ranges) {
            // Normal to the line (north, east components).
            let (mn, me) = (-t.sin(), t.cos());
            let w = 1.0 / (s * r.max(100.0)).powi(2);
            let c = mn * n + me * e;
            a11 += w * mn * mn;
            a12 += w * mn * me;
            a22 += w * me * me;
            b1 += w * mn * c;
            b2 += w * me * c;
        }
        let det = a11 * a22 - a12 * a12;
        if det.abs() < 1e-30 {
            return None;
        }
        x = ((a22 * b1 - a12 * b2) / det, (a11 * b2 - a12 * b1) / det);
        cov = [a22 / det, -a12 / det, a11 / det];
        for ((n, e, _, _), r) in pts.iter().zip(ranges.iter_mut()) {
            *r = ((x.0 - n).powi(2) + (x.1 - e).powi(2)).sqrt();
        }
    }
    let (lat, lon) = offset(lat0, lon0, x.0, x.1);
    // Ahead of every sensor, within its range, and each line within its gate.
    for l in lobs {
        let (b, d) = bearing_to(l.origin().0, l.origin().1, lat, lon);
        let r = wrap(l.bearing - b);
        if r.abs() > 90.0 || d > l.range || (r / l.sigma).powi(2) > GATE_D2 {
            return None;
        }
    }
    Some(Fix { lat, lon, cov })
}

/// Whether two bearings come from different places (a baseline to fix on).
fn apart(a: &Lob, b: &Lob) -> bool {
    let (n, e) = local(a.origin().0, a.origin().1, b.origin().0, b.origin().1);
    n.hypot(e) >= MIN_BASELINE_M
}

/// Sum of the lines' squared residuals, in sigmas, about a point.
fn consistency(lobs: &[&Lob], fix: &Fix) -> f64 {
    lobs.iter()
        .map(|l| {
            let (b, _) = bearing_to(l.origin().0, l.origin().1, fix.lat, fix.lon);
            (wrap(l.bearing - b) / l.sigma).powi(2)
        })
        .sum()
}

/// Whether three or more lines meet at one point as well as chance allows
/// (99%); two lines always meet.
fn consistent(lobs: &[&Lob], fix: &Fix) -> bool {
    lobs.len() < 3 || consistency(lobs, fix) <= CHI2_99[(lobs.len() - 3).min(3)]
}

/// How good a set is, lower better: sets that fit well come first, the
/// most lines first among them; then marginal ones. A large set whose extra
/// line only just fits is often another emitter's line passing close by.
fn rank(lobs: &[&Lob], fix: &Fix) -> f64 {
    let chi2 = consistency(lobs, fix);
    let dof = lobs.len().saturating_sub(2);
    let good = dof == 0 || chi2 <= CHI2_95[(dof - 1).min(3)];
    let per = chi2 / dof.max(1) as f64;
    // Per-line fit is at most a few units within a tier.
    (if good { 0.0 } else { 1000.0 }) - 100.0 * lobs.len() as f64 + per
}

/// Whether every line agrees with the fix of the others (where they fix
/// without it), within its own error and theirs at 99%. A line from another
/// emitter that passes near drags a joint fix towards itself and so can
/// hide in it, but not from the others' fix.
fn no_outlier(lobs: &[&Lob]) -> bool {
    if lobs.len() < 3 {
        return true;
    }
    (0..lobs.len()).all(|i| {
        let rest: Vec<&Lob> = (0..lobs.len())
            .filter(|&j| j != i)
            .map(|j| lobs[j])
            .collect();
        let Some(fix) = cross(&rest) else {
            return true;
        };
        let l = lobs[i];
        let (b, d) = bearing_to(l.origin().0, l.origin().1, fix.lat, fix.lon);
        let [nn, ne, ee] = fix.cov;
        let (s, c) = b.to_radians().sin_cos();
        let across = (nn * s * s - 2.0 * ne * s * c + ee * c * c).max(0.0).sqrt();
        let var = l.sigma.powi(2) + (across / d.max(1.0)).to_degrees().powi(2);
        wrap(l.bearing - b).powi(2) / var <= CHI2_99[0]
    })
}

/// Whether every line of a set passes through a track somewhere other than
/// the fix: each is already accounted for, and their crossing is a ghost of
/// those tracks' lines. (An untracked emitter whose every line also passes
/// through someone else's track is rare.)
fn explained(lobs: &[&Lob], fix: &Fix) -> bool {
    let near = 3.0 * (fix.cov[0] + fix.cov[2]).sqrt().max(300.0);
    lobs.iter().all(|l| {
        l.through.iter().any(|&(lat, lon)| {
            let (n, e) = local(fix.lat, fix.lon, lat, lon);
            n.hypot(e) > near
        })
    })
}

/// A set of bearings by sensor track, in a fixed order.
fn set_of(lobs: &[&Lob]) -> String {
    let mut v: Vec<String> = lobs
        .iter()
        .map(|l| format!("{}/{}", l.obs.source_id, l.obs.source_track_key))
        .collect();
    v.sort_unstable();
    v.join(",")
}

/// Candidates kept per sensor when searching for the best set.
const PER_SENSOR: usize = 4;

impl Bearings {
    /// Fix what the waiting lines allow. Sets are chosen together, once a
    /// moment's lines are all in, best first: the most sensors, then the
    /// tightest. Choosing as each line arrives would let a line join a set
    /// before the emitter's own lines from the other sensors are in.
    ///
    /// Ghosts are kept out three ways:
    /// - lines with an emitter identity only fix with the same identity;
    /// - an anonymous set needs three sensors, and must not split sensor
    ///   tracks whose emitters already fixed apart;
    /// - an anonymous set fixes only the second time the same sensor tracks
    ///   cross where the first crossing could have moved to: three unrelated
    ///   lines meeting once is chance, twice is not.
    ///
    /// Lines in a set are used up either way.
    fn sweep(&mut self) -> Vec<(Vec<Lob>, Fix)> {
        self.fresh = None;
        let mut out = Vec::new();
        loop {
            let best = (0..self.waiting.len())
                .filter_map(|a| self.best_set(a))
                .min_by(|x, y| x.2.total_cmp(&y.2));
            let Some((mut members, fix, _)) = best else {
                break;
            };
            let mut again = true;
            // Every set fixes only the second time, emitter identity or not:
            // an ELNOT is shared by every emitter of its kind, so two lines
            // of it may still be two boats with the same radar.
            if !self.naive() {
                let set: Vec<&Lob> = members.iter().map(|&m| &self.waiting[m]).collect();
                let at = set
                    .iter()
                    .map(|l| l.obs.observed_at)
                    .max()
                    .unwrap_or_else(Utc::now);
                let spread = (fix.cov[0] + fix.cov[2]).sqrt();
                let key = set_of(&set);
                let recent = chrono::Duration::seconds(WINDOW_S * 5);
                self.combos.retain(|_, c| (at - c.3).abs() <= recent);
                again = self.combos.get(&key).is_some_and(|&(lat, lon, s, t)| {
                    let (dn, de) = local(lat, lon, fix.lat, fix.lon);
                    let dt = (at - t).num_milliseconds().abs() as f64 / 1000.0;
                    dt > 0.0 && dn.hypot(de) <= MAX_SPEED_MPS * dt + 3.0 * s.hypot(spread)
                });
                self.combos.insert(key, (fix.lat, fix.lon, spread, at));
            }
            members.sort_unstable_by(|a, b| b.cmp(a));
            let used = members
                .into_iter()
                .map(|m| self.waiting.remove(m))
                .collect();
            if again {
                out.push((used, fix));
            }
        }
        out
    }

    /// The best set waiting line `a` fixes in: members, fix and fit.
    fn best_set(&self, a: usize) -> Option<(Vec<usize>, Fix, f64)> {
        let line = &self.waiting[a];
        let id = line.identity();
        // Lines from elsewhere that cross this one, best first, by sensor.
        let mut groups: Vec<(String, Vec<(f64, usize)>)> = Vec::new();
        for (j, other) in self.waiting.iter().enumerate() {
            let same = match (&id, other.identity()) {
                (Some(a), Some(b)) => *a == b,
                (None, None) => true,
                _ => false,
            };
            if j == a || !same || !apart(line, other) {
                continue;
            }
            // Any crossing: lines from opposite sides of an emitter are
            // nearly parallel, yet fix it with a third.
            let Some(fix) = cross_at(&[line, other], 1.0) else {
                continue;
            };
            let d2 = consistency(&[line, other], &fix);
            match groups.iter_mut().find(|g| g.0 == other.obs.source_id) {
                Some(g) => g.1.push((d2, j)),
                None => groups.push((other.obs.source_id.clone(), vec![(d2, j)])),
            }
        }
        for g in &mut groups {
            g.1.sort_by(|a, b| a.0.total_cmp(&b.0));
            g.1.truncate(PER_SENSOR);
        }
        let need = if id.is_some() { 2 } else { 3 };
        let mut best = None;
        self.search(&groups, 0, &mut vec![a], need, &mut best);
        best
    }

    /// Every choice of at most one line per sensor from `groups[g..]`,
    /// added to `pick`; keeps the best set that fixes.
    fn search(
        &self,
        groups: &[(String, Vec<(f64, usize)>)],
        g: usize,
        pick: &mut Vec<usize>,
        need: usize,
        best: &mut Option<(Vec<usize>, Fix, f64)>,
    ) {
        if g == groups.len() {
            if pick.len() < need {
                return;
            }
            let set: Vec<&Lob> = pick.iter().map(|&m| &self.waiting[m]).collect();
            if need > 2 && !self.naive() && !self.one_emitter(&set) {
                return;
            }
            let Some(fix) = cross(&set) else {
                return;
            };
            if !consistent(&set, &fix)
                || (!self.naive() && (!no_outlier(&set) || (need > 2 && explained(&set, &fix))))
            {
                return;
            }
            let fit = rank(&set, &fix);
            let better = best.as_ref().is_none_or(|(_, _, f)| fit < *f);
            if better {
                *best = Some((pick.clone(), fix, fit));
            }
            return;
        }
        self.search(groups, g + 1, pick, need, best);
        for &(_, j) in &groups[g].1 {
            pick.push(j);
            self.search(groups, g + 1, pick, need, best);
            pick.pop();
        }
    }

    /// Whether the ghost rules are off (only ever in tests).
    fn naive(&self) -> bool {
        #[cfg(test)]
        return self.naive;
        #[cfg(not(test))]
        false
    }

    /// Whether a set keeps together the sensor tracks whose emitters fixed
    /// together before (those not yet in any fix may join), and keeps apart
    /// what one sensor tracks as two: a sensor track may not join a fix
    /// that already holds another of the same sensor's tracks, since the
    /// sensor itself said they are two objects.
    fn one_emitter(&self, set: &[&Lob]) -> bool {
        let mut keys = set.iter().filter_map(|l| {
            self.members
                .get(&(l.obs.source_id.clone(), l.obs.source_track_key.clone()))
        });
        let first = keys.next();
        if !keys.all(|k| Some(k) == first) {
            return false;
        }
        let Some(fix) = first else {
            return true;
        };
        set.iter().all(|l| {
            !self
                .members
                .iter()
                .any(|(m, k)| k == fix && m.0 == l.obs.source_id && m.1 != l.obs.source_track_key)
        })
    }
}

impl Engine {
    /// A line of bearing: to a track, or waiting to be fixed.
    pub(super) async fn bearing(
        &mut self,
        obs: Observation,
        counts: &mut EngineCounts,
    ) -> anyhow::Result<()> {
        let Some(mut lob) = Lob::of(obs) else {
            return Ok(());
        };
        if let Some(point) = lob.ranged_point() {
            let key = format!("{}/{}", point.source_id, point.source_track_key);
            self.ingest(point, counts).await?;
            if let Some(uid) = self.reports.get(&key).copied() {
                self.attach(uid, &lob, 0.0);
                self.save(uid, false).await?;
            }
            return Ok(());
        }
        let member = (lob.obs.source_id.clone(), lob.obs.source_track_key.clone());
        let now = lob.obs.observed_at;
        {
            let h = self.bearings.history.entry(member.clone()).or_default();
            h.retain(|l| {
                let age = (now - l.obs.observed_at).num_milliseconds();
                (0..=TMA_WINDOW_S * 1000).contains(&age) && age > 0
            });
            h.push_back(lob.clone());
            while h.len() > TMA_MAX_LINES {
                h.pop_front();
            }
        }
        let attached = self.lob_track(&mut lob);
        if let Some(uid) = attached {
            counts.paired += 1;
            self.save(uid, false).await?;
            self.bearings.held.insert(member.clone(), (uid, now));
            self.join_fix_track(&member, uid, counts).await?;
        }
        // One sensor's bearings over time: locate the emitter, unless the
        // track the line went to already has it better located.
        if let Some(sol) = self.single_sensor(&member) {
            self.bearings.ranges.insert(member.clone(), sol.2);
            // Seen moving once, it is a moving target from then on.
            if sol.1.model == tma::Model::Moving {
                self.bearings.moving.insert(member.clone());
            }
            let key = self.single_key(&lob);
            // The track this sensor track's lines have lately gone to (this
            // one, or one within the window: a line that misses its gate
            // now and then must not restart a rival location).
            let recent = attached.or_else(|| {
                self.bearings
                    .held
                    .get(&member)
                    .and_then(|&(u, at)| ((now - at).num_seconds() <= TMA_WINDOW_S).then_some(u))
            });
            // Quiet only once our own location has joined that track: while
            // the lines go to another track (ELINT's, say, sharing the
            // ELNOT), keep reporting so the two can pair, and the track keeps
            // its number from first contact.
            let joined = recent
                .is_some_and(|u| self.reports.get(&format!("{FIX}/{key}")).copied() == Some(u));
            let better = joined
                && recent.and_then(|u| self.tracks.get(&u)).is_some_and(|t| {
                    t.state != ot_core::TrackState::Lost
                        && t.contributors.iter().any(|c| c.source_id != FIX)
                        && t.view
                            .uncertainty
                            .as_ref()
                            .and_then(|u| u.position_covariance())
                            .is_some_and(|[nn, _, ee]| (nn + ee).sqrt() <= sol.1.sigma_m())
                });
            if !better {
                let report = self.single_report(&lob, &key, &sol.0, &sol.1);
                self.ingest(report, counts).await?;
                // The line is evidence for the track its own fix made.
                if attached.is_none()
                    && let Some(uid) = self.reports.get(&format!("{FIX}/{key}")).copied()
                {
                    self.attach(uid, &lob, 0.0);
                    self.save(uid, false).await?;
                    counts.paired += 1;
                    return Ok(());
                }
            }
        }
        if attached.is_some() {
            return Ok(());
        }
        // Published live for consumers to draw, until a track or a fix takes it.
        let subject = format!(
            "{}.bearing.{}.{}",
            ot_core::wire::CONTACTS_SUBJECT,
            token(&lob.obs.source_id),
            token(&lob.obs.source_track_key)
        );
        let body = json!({
            "schema": ot_core::wire::CONTACT_SCHEMA, "op": "bearing",
            "source_id": lob.obs.source_id, "source_track_key": lob.obs.source_track_key,
            "observed_at": lob.obs.observed_at,
            "latitude": lob.obs.position.latitude, "longitude": lob.obs.position.longitude,
            "bearing_deg": lob.bearing, "sigma_deg": lob.sigma,
            "range_m": lob.measured_range, "range_sigma_m": lob.range_sigma,
            "max_range_m": (lob.range != DEFAULT_RANGE_M).then_some(lob.range),
            "identifiers": lob.obs.identifiers,
        });
        self.redis.push_contact(&subject, &body).await?;
        let now = lob.obs.observed_at;
        let window = chrono::Duration::seconds(WINDOW_S);
        self.bearings
            .waiting
            .retain(|l| (now - l.obs.observed_at).abs() <= window);
        // A sensor track's newer bearing replaces its older one.
        self.bearings.waiting.retain(|l| {
            l.obs.source_id != lob.obs.source_id
                || l.obs.source_track_key != lob.obs.source_track_key
        });
        self.bearings.waiting.push(lob);
        self.bearings.fresh = Some(now);
        Ok(())
    }

    /// Where a moving sensor's bearings of one emitter put it: the frame's
    /// origin (lat, lon) and the solution in it. `None` while the platform
    /// has not moved enough to give a baseline, or the fit is too loose to
    /// say anything (error over a quarter of the range, or 20 km).
    fn single_sensor(&self, member: &(String, String)) -> Option<((f64, f64), tma::Solution, f64)> {
        let h = self.bearings.history.get(member)?;
        let newest = h.back()?;
        let (lat0, lon0) = newest.origin();
        let moved = h
            .iter()
            .map(|l| {
                let (n, e) = local(lat0, lon0, l.origin().0, l.origin().1);
                n.hypot(e)
            })
            .fold(0.0, f64::max);
        if moved < TMA_MIN_BASELINE_M {
            return None;
        }
        // A target's manoeuvres break the model over long windows, and matter
        // more the closer it is: use a window that shrinks with range (the
        // newest line's last located range, or the sensor's reach).
        let reach = self
            .bearings
            .ranges
            .get(member)
            .copied()
            .unwrap_or(newest.range);
        let window = (reach / 200.0).clamp(60.0, TMA_WINDOW_S as f64);
        let lines: Vec<tma::Line> = h
            .iter()
            .filter(|l| {
                (newest.obs.observed_at - l.obs.observed_at).num_milliseconds() as f64 / 1000.0
                    <= window
            })
            .map(|l| {
                let (n, e) = local(lat0, lon0, l.origin().0, l.origin().1);
                tma::Line {
                    t: (l.obs.observed_at - newest.obs.observed_at).num_milliseconds() as f64
                        / 1000.0,
                    n,
                    e,
                    bearing: l.bearing.to_radians(),
                    sigma: l.sigma.to_radians(),
                    range: l.range,
                }
            })
            .collect();
        let motion = self.motion.get(&member.0).copied().unwrap_or_default();
        let mut sol = tma::locate(
            &lines,
            self.bearings.moving.contains(member),
            motion.max_speed_mps,
        )?;
        let range = sol.n.hypot(sol.e);
        // What the model cannot see sets a floor on the error: a moving
        // target that manoeuvres (a weave, a turn) at up to the source's
        // emitter_motion.manoeuvre_mps2 strays up to half a T-squared from
        // constant velocity over the window; and 2% of the range for
        // everything else.
        let span = lines.iter().map(|l| -l.t).fold(0.0, f64::max);
        let manoeuvre = if sol.model == tma::Model::Moving {
            0.5 * motion.manoeuvre_mps2 * span * span
        } else {
            0.0
        };
        let floor = (manoeuvre.powi(2) + (0.02 * range).powi(2)) / 2.0;
        sol.cov[0][0] += floor;
        sol.cov[1][1] += floor;
        (sol.sigma_m() <= (0.25 * range).min(20_000.0)).then_some(((lat0, lon0), sol, range))
    }

    /// The `fix` source track a sensor track's own location reports under:
    /// the sensor track. Its ELNOT rides along as evidence, never as a key:
    /// two boats with the same radar model share one.
    fn single_key(&self, lob: &Lob) -> String {
        format!("tma:{}/{}", lob.obs.source_id, lob.obs.source_track_key)
    }

    /// A single-sensor location as a report of the `fix` source.
    fn single_report(
        &self,
        lob: &Lob,
        key: &str,
        origin: &(f64, f64),
        sol: &tma::Solution,
    ) -> Observation {
        let (lat, lon) = offset(origin.0, origin.1, sol.n, sol.e);
        let c = &sol.cov;
        let moving = sol.model == tma::Model::Moving;
        let covariance = if moving {
            json!({"position": [c[0][0], c[0][1], c[1][1]],
                   "velocity": [c[2][2], c[2][3], c[3][3]],
                   "cross": [c[0][2], c[0][3], c[1][2], c[1][3]]})
        } else {
            json!({"position": [c[0][0], c[0][1], c[1][1]]})
        };
        let kinematics = if moving {
            json!({"course_deg": sol.ve.atan2(sol.vn).to_degrees().rem_euclid(360.0),
                   "speed_mps": sol.vn.hypot(sol.ve)})
        } else {
            json!({})
        };
        serde_json::from_value(json!({
            "schema_version": lob.obs.schema_version,
            "source_id": FIX,
            "source_track_key": key,
            "observed_at": lob.obs.observed_at,
            "received_at": Utc::now(),
            "position": {"latitude": lat, "longitude": lon},
            "kinematics": kinematics,
            "uncertainty": {"covariance": covariance},
            "identifiers": lob.obs.identifiers,
            "classification": {"domain": lob.obs.classification.domain},
            "provenance": {
                "sensor_code": if moving { "tma-moving" } else { "tma-fixed" },
                "source_code": lob.obs.source_id,
            },
        }))
        .expect("a single-sensor location is an observation")
    }

    /// Cross-fix the waiting lines once time has moved past the newest
    /// (`at`), or at the end of a batch (`None`); the fixes go into the
    /// picture as reports.
    pub(super) async fn sweep_bearings(
        &mut self,
        at: Option<DateTime<Utc>>,
        counts: &mut EngineCounts,
    ) -> anyhow::Result<()> {
        let Some(fresh) = self.bearings.fresh else {
            return Ok(());
        };
        if at.is_some_and(|at| at <= fresh) {
            return Ok(());
        }
        for (used, fix) in self.bearings.sweep() {
            // The emitter is a track we have: its lines go to it from now on.
            if let Some((uid, residuals)) = self.pointed_at(&used, &fix) {
                for (l, r) in used.iter().zip(residuals) {
                    self.attach(uid, l, r);
                    self.bearings.bound.insert(
                        (l.obs.source_id.clone(), l.obs.source_track_key.clone()),
                        uid,
                    );
                    counts.paired += 1;
                }
                self.save(uid, false).await?;
                if let Some(l) = used.first() {
                    let member = (l.obs.source_id.clone(), l.obs.source_track_key.clone());
                    self.join_fix_track(&member, uid, counts).await?;
                }
                continue;
            }
            let report = self.fix_report(&used, &fix);
            self.ingest(report, counts).await?;
        }
        Ok(())
    }

    /// How a bearing fits a track: `None` when the track is out of its
    /// range, behind it, outside its gate, kept up only by fixes, or its
    /// emitter identity conflicts.
    fn fit(&self, lob: &Lob, t: &ot_core::SystemTrack) -> Option<Fit> {
        // A track only fixes keep up is kept up by fixes, not bearings.
        if t.contributors.iter().all(|c| c.source_id == FIX) {
            return None;
        }
        self.fit_any(lob, t)
    }

    /// `fit`, for any live track, fixes' own included.
    fn fit_any(&self, lob: &Lob, t: &ot_core::SystemTrack) -> Option<Fit> {
        if !t.kind.is_track() || t.state == ot_core::TrackState::Lost {
            return None;
        }
        let (olat, olon) = lob.origin();
        let at = lob.obs.observed_at;
        let dt =
            ((at - t.view.observed_at).num_milliseconds() as f64 / 1000.0).clamp(-300.0, 300.0);
        let (mut lat, mut lon) = (t.view.position.latitude, t.view.position.longitude);
        if let (Some(c), Some(v)) = (t.view.kinematics.course_deg, t.view.kinematics.speed_mps) {
            let d = v * dt;
            (lat, lon) = offset(lat, lon, d * c.to_radians().cos(), d * c.to_radians().sin());
        }
        let (b, range) = bearing_to(olat, olon, lat, lon);
        let r = wrap(lob.bearing - b);
        if range > lob.range || r.abs() > 90.0 {
            return None;
        }
        // The track's own error across the line, and its drift since.
        let across = t
            .view
            .uncertainty
            .as_ref()
            .and_then(|u| u.position_covariance())
            .map(|[nn, ne, ee]| {
                let (s, c) = b.to_radians().sin_cos();
                // Unit normal to the line: (-sin, cos) in (north, east).
                (nn * s * s - 2.0 * ne * s * c + ee * c * c).max(0.0).sqrt()
            })
            .unwrap_or(100.0)
            + dt.abs();
        let sigma = (lob.sigma.powi(2) + (across / range.max(1.0)).to_degrees().powi(2)).sqrt();
        let d2 = (r / sigma).powi(2);
        if d2 > GATE_D2 {
            return None;
        }
        // Emitter identity: a different one is a veto, the same one decisive.
        let mut same = false;
        if let Some(id) = lob.identity() {
            let theirs: Vec<String> = t
                .bearings
                .iter()
                .filter_map(|b| identity(&b.identifiers))
                .chain(
                    t.view
                        .identifiers
                        .iter()
                        .map(|i| format!("{}:{}", i.scheme, i.value)),
                )
                .collect();
            // Evidence, not identity: a match counts for the track, a
            // different one counts for nothing (a platform can carry
            // several emitters).
            same = theirs.contains(&id);
        }
        Some(Fit {
            d2,
            residual: r,
            sigma,
            lat,
            lon,
            same,
        })
    }

    /// The one track a bearing points at, if exactly one fits clearly best;
    /// the bearing is recorded on it.
    fn lob_track(&mut self, lob: &mut Lob) -> Option<Uid> {
        let mut fits: Vec<(Uid, f64, f64, bool)> = Vec::new();
        let mut through = Vec::new();
        for t in self.tracks.values() {
            let Some(f) = self.fit(lob, t) else {
                continue;
            };
            let score = (-0.5 * f.d2).exp() / f.sigma
                * if f.same {
                    crate::correlate::EVIDENCE_LN_LR.exp()
                } else {
                    1.0
                };
            fits.push((t.uid, score, f.residual, f.same));
            through.push((f.lat, f.lon));
        }
        // A bearing goes to the track its emitter was found on (its lines
        // from several sensors pointed at it together, or its fixes report
        // for it); else only on a matching emitter identity. One line
        // through a track proves little: an emitter with no track of its
        // own often lies on a line through someone else's, so it waits to be
        // fixed instead.
        //
        // First of all, the track the sensor track's own positioned reports
        // are on: a sensor that measures range now and then (an acoustic
        // array) has already said which object its bearing-only reports
        // are, by reporting them under the same key.
        let member = (lob.obs.source_id.clone(), lob.obs.source_track_key.clone());
        let own = self.own_track(&member);
        let learned = own
            .or_else(|| self.bearings.bound.get(&member).copied())
            .or_else(|| {
                self.bearings
                    .members
                    .get(&member)
                    .and_then(|k| self.reports.get(&format!("{FIX}/{k}")).copied())
            });
        lob.through = through;
        // A line that no longer fits the track it was bound to (a neighbour
        // that passed close by and has moved on) is bound no more.
        if let Some(u) = self.bearings.bound.get(&member).copied()
            && !fits.iter().any(|f| f.0 == u)
        {
            self.bearings.bound.remove(&member);
        }
        fits.sort_by(|a, b| b.1.total_cmp(&a.1));
        let (uid, _, residual, _) = match learned.and_then(|u| fits.iter().find(|f| f.0 == u)) {
            Some(f) => *f,
            None => {
                let (uid, best, residual, same) = *fits.first()?;
                if !same || fits.get(1).is_some_and(|s| best < AMBIGUITY * s.1) {
                    return None;
                }
                (uid, best, residual, same)
            }
        };
        self.attach(uid, lob, residual)
    }

    /// A fix track is its lines' own estimate of where their emitter is.
    /// Once every sensor track whose lines made it (`member`'s fix key)
    /// reports for track `into` instead (their emitter was found there, by
    /// their lines pointing at it together or by its identity), the fix
    /// track is the same emitter: it joins `into` rather than lingering
    /// beside it until it goes stale.
    async fn join_fix_track(
        &mut self,
        member: &(String, String),
        into: Uid,
        counts: &mut EngineCounts,
    ) -> anyhow::Result<()> {
        if self.settings.correlation.approach == crate::correlate::Approach::Identifiers {
            return Ok(());
        }
        let Some(key) = self.bearings.members.get(member).cloned() else {
            return Ok(());
        };
        let Some(from) = self.reports.get(&format!("{FIX}/{key}")).copied() else {
            return Ok(());
        };
        if from == into || self.forbidden(from, into) {
            return Ok(());
        }
        let (Some(f), Some(t)) = (self.tracks.get(&from), self.tracks.get(&into)) else {
            return Ok(());
        };
        let alone = f.state != ot_core::TrackState::Lost
            && f.contributors
                .iter()
                .all(|c| c.source_id == FIX && c.source_track_key == key);
        let mut lines: Vec<&(String, String)> = self
            .bearings
            .members
            .iter()
            .filter(|(_, k)| **k == key)
            .map(|(m, _)| m)
            .collect();
        lines.sort();
        let all_there = lines.iter().all(|m| {
            t.bearings
                .iter()
                .any(|b| b.source_id == m.0 && b.source_track_key == m.1)
        });
        if !alone || !all_there {
            return Ok(());
        }
        let names: Vec<String> = lines.iter().map(|(s, k)| format!("{s}/{k}")).collect();
        let target = into;
        let (from, into) = self.merge_order(from, into);
        let reason = format!(
            "{} and {} are one emitter: every line that fixed {key} ({}) now reports for {}",
            from.doc_id(),
            into.doc_id(),
            names.join(", "),
            target.doc_id()
        );
        let evidence = json!({"rule": "fix-lines", "fix": key, "lines": names});
        if self.settings.correlation.mode == crate::correlate::Mode::Suggest {
            if self
                .suggest("pair", into, Some(from), None, evidence, reason)
                .await?
            {
                counts.suggested += 1;
            }
            return Ok(());
        }
        let decision = super::engine_decision("merge")
            .reason(reason)
            .evidence(evidence);
        let c = self.common.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<i64> {
            Ok(c.open_db()?.merge_system_tracks(from, into, decision)?)
        })
        .await??;
        self.absorb(from, into).await?;
        counts.kinematic_merged += 1;
        self.republish(into).await
    }

    /// The live track a sensor track's own positioned reports (bearings
    /// with a measured range) are on.
    fn own_track(&self, member: &(String, String)) -> Option<Uid> {
        let uid = *self.reports.get(&format!("{}/{}", member.0, member.1))?;
        self.tracks
            .get(&uid)
            .is_some_and(|t| t.state != ot_core::TrackState::Lost)
            .then_some(uid)
    }

    /// Record a bearing on a track.
    fn attach(&mut self, uid: Uid, lob: &Lob, residual: f64) -> Option<Uid> {
        let (olat, olon) = lob.origin();
        let at = lob.obs.observed_at;
        let keep = chrono::Duration::seconds(KEEP_S);
        let t = self.tracks.get_mut(&uid)?;
        t.bearings.retain(|b| {
            !(b.source_id == lob.obs.source_id && b.source_track_key == lob.obs.source_track_key)
                && (at - b.observed_at).abs() <= keep
        });
        t.bearings.push(BearingContact {
            source_id: lob.obs.source_id.clone(),
            source_track_key: lob.obs.source_track_key.clone(),
            observed_at: at,
            latitude: olat,
            longitude: olon,
            bearing_deg: lob.bearing,
            sigma_deg: lob.sigma,
            range_m: lob.measured_range,
            range_sigma_m: lob.range_sigma,
            max_range_m: (lob.range != DEFAULT_RANGE_M).then_some(lob.range),
            residual_deg: (residual * 100.0).round() / 100.0,
            identifiers: lob.obs.identifiers.clone(),
        });
        Some(uid)
    }

    /// The one track every line of a fixed set points at, within their
    /// errors and the track's (chi-square at 99%). Lines from several
    /// sensors all passing through a track's own, precise position say the
    /// emitter is that track far more surely than the fix's position,
    /// kilometres uncertain, ever can.
    fn pointed_at(&self, used: &[Lob], fix: &Fix) -> Option<(Uid, Vec<f64>)> {
        // Two lines meet somewhere: any ship near their crossing fits them.
        let n = used.len();
        if n < 3 {
            return None;
        }
        // The fix tracks these lines made themselves are no rival: they are
        // the lines' own estimate of where the emitter is.
        let own: Vec<String> = used
            .iter()
            .filter_map(|l| {
                let m = (l.obs.source_id.clone(), l.obs.source_track_key.clone());
                self.bearings.members.get(&m).cloned()
            })
            .collect();
        let mine = |t: &ot_core::SystemTrack| {
            t.contributors
                .iter()
                .all(|c| c.source_id == FIX && own.contains(&c.source_track_key))
        };
        // Every track the lines could all be pointing at, of any kind, by
        // how likely the lines are if it is the emitter: the best must be
        // clearly likelier than the next (ships close together along the
        // lines, and it is not for the lines to say which). A wide ELINT
        // area fits loosely, so it is far less likely than a precise ship
        // the lines pass right through.
        let mut found: Vec<(&ot_core::SystemTrack, Vec<Fit>, f64)> = Vec::new();
        // A line whose sensor track's own positioned reports are on a
        // track points at that one, not another.
        let owners: Vec<Uid> = used
            .iter()
            .filter_map(|l| {
                self.own_track(&(l.obs.source_id.clone(), l.obs.source_track_key.clone()))
            })
            .collect();
        for t in self
            .tracks
            .values()
            .filter(|t| !mine(t) && owners.iter().all(|o| *o == t.uid))
        {
            let fits: Option<Vec<Fit>> = used.iter().map(|l| self.fit_any(l, t)).collect();
            let Some(fits) = fits else {
                continue;
            };
            if fits.iter().map(|f| f.d2).sum::<f64>() > CHI2_99[n.min(4) - 1] {
                continue;
            }
            // The fix against the track, with both their errors.
            let [pn, pne, pe] = t
                .view
                .uncertainty
                .as_ref()
                .and_then(|u| u.position_covariance())
                .unwrap_or([1e4, 0.0, 1e4]);
            let (a, b, c) = (fix.cov[0] + pn, fix.cov[1] + pne, fix.cov[2] + pe);
            let det = a * c - b * b;
            if det <= 0.0 {
                continue;
            }
            let (dn, de) = local(fits[0].lat, fits[0].lon, fix.lat, fix.lon);
            let m2 = (c * dn * dn - 2.0 * b * dn * de + a * de * de) / det;
            let ln_l = -0.5 * m2 - 0.5 * det.ln();
            found.push((t, fits, ln_l));
        }
        found.sort_by(|a, b| b.2.total_cmp(&a.2));
        let mut found = found.into_iter();
        let (t, fits, best) = found.next()?;
        if found
            .next()
            .is_some_and(|next| best - next.2 < AMBIGUITY.ln())
        {
            return None;
        }
        // Only a track at least as precise as the lines can say so: a
        // kilometres-wide ELINT area fits any line passing nearby. Nor is a
        // track fixes alone keep up one to bind lines to.
        if t.contributors.iter().all(|c| c.source_id == FIX)
            || fits
                .iter()
                .zip(used)
                .any(|(f, l)| f.sigma > l.sigma * std::f64::consts::SQRT_2)
        {
            return None;
        }
        Some((t.uid, fits.iter().map(|f| f.residual).collect()))
    }

    /// A fix as a report of the `fix` source: the same object's successive
    /// fixes report under one key (its emitter identity, or the nearest
    /// recent fix within its error).
    fn fix_report(&mut self, used: &[Lob], fix: &Fix) -> Observation {
        let at = used
            .iter()
            .map(|l| l.obs.observed_at)
            .max()
            .unwrap_or_else(Utc::now);
        // The key its members' earlier fixes had (most of them), else a new
        // one named after the set. Never the ELNOT: many emitters share one.
        let key = {
            let mut count: HashMap<&String, usize> = HashMap::new();
            for l in used {
                let m = (l.obs.source_id.clone(), l.obs.source_track_key.clone());
                if let Some(k) = self.bearings.members.get(&m) {
                    *count.entry(k).or_default() += 1;
                }
            }
            count
                .into_iter()
                .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(a.0)))
                .map(|(k, _)| k.clone())
                .unwrap_or_else(|| {
                    use std::hash::{Hash, Hasher};
                    let mut h = std::collections::hash_map::DefaultHasher::new();
                    set_of(&used.iter().collect::<Vec<_>>()).hash(&mut h);
                    format!("set-{:08x}", h.finish() as u32)
                })
        };
        if self.recording {
            self.trace.push(json!({
                "t": at, "kind": "fix", "key": key, "lat": fix.lat, "lon": fix.lon,
                "sigma": (fix.cov[0] + fix.cov[2]).sqrt(),
                "members": used.iter()
                    .map(|l| format!("{}/{}", l.obs.source_id, l.obs.source_track_key))
                    .collect::<Vec<_>>(),
            }));
        }
        for l in used {
            self.bearings.members.insert(
                (l.obs.source_id.clone(), l.obs.source_track_key.clone()),
                key.clone(),
            );
        }
        let mut sensors: Vec<&str> = used.iter().map(|l| l.obs.source_id.as_str()).collect();
        sensors.sort_unstable();
        sensors.dedup();
        let ids: Vec<_> = {
            let mut v: Vec<ot_core::schema::Identifier> = used
                .iter()
                .flat_map(|l| l.obs.identifiers.clone())
                .collect();
            v.dedup();
            v
        };
        let domain = used.iter().find_map(|l| l.obs.classification.domain);
        serde_json::from_value(json!({
            "schema_version": used[0].obs.schema_version,
            "source_id": FIX,
            "source_track_key": key,
            "observed_at": at,
            "received_at": Utc::now(),
            "position": {"latitude": fix.lat, "longitude": fix.lon},
            "uncertainty": {"covariance": {"position": fix.cov}},
            "identifiers": ids,
            "classification": {"domain": domain},
            "provenance": {"sensor_code": "crossfix", "source_code": sensors.join(",")},
        }))
        .expect("a fix is an observation")
    }
}

impl Engine {
    /// The track an area report's emitter is on, found by its emitter
    /// identity (an ELNOT): the one track among `compared` (the tracks it
    /// could pair with, each passing its gate or not) that carries the
    /// same identity, on its own identifiers or on bearings reporting for
    /// it, and that fits clearly best: its likelihood at least 10 times
    /// any other's, the shared identity counting 10:1. Position alone can
    /// never say this of a kilometres-wide area at open-water density
    /// (another object is as likely inside it as the track), but the
    /// identity found on a precise track's own lines can. Chance plays its
    /// part once, so it pairs only when two successive reports of the area
    /// find the same track; returns that track, its comparison and the
    /// share of the likelihood it holds.
    pub(super) fn area_emitter(
        &mut self,
        uid: Uid,
        obs: &Observation,
        compared: &[(Uid, crate::correlate::Kinematic)],
    ) -> Option<(Uid, crate::correlate::Kinematic, f64)> {
        use crate::correlate::{EVIDENCE_LN_LR, is_evidence_scheme, kinematic, share_evidence};
        let key = format!("{}/{}", obs.source_id, obs.source_track_key);
        let ids: Vec<String> = obs
            .identifiers
            .iter()
            .filter(|i| is_evidence_scheme(&i.scheme))
            .map(|i| format!("{}:{}", i.scheme.trim(), i.value.trim()).to_lowercase())
            .collect();
        if ids.is_empty() {
            return None;
        }
        let carries = |t: &ot_core::SystemTrack| {
            t.view
                .identifiers
                .iter()
                .chain(t.bearings.iter().flat_map(|b| b.identifiers.iter()))
                .any(|i| {
                    ids.contains(&format!("{}:{}", i.scheme.trim(), i.value.trim()).to_lowercase())
                })
        };
        // Every candidate against the same "another object" (the base
        // density, not a crowd's), so their likelihoods compare.
        let s = self.settings.correlation.kinematic;
        let mut scored: Vec<(Uid, crate::correlate::Kinematic, f64, bool)> = compared
            .iter()
            .filter(|(other, k)| k.pass && *other != uid)
            .filter_map(|(other, _)| {
                let t = self.tracks.get(other)?;
                let k = kinematic(obs, &t.view, &s);
                let same = carries(t);
                let mut ln = k.ln_lr;
                if share_evidence(obs, &t.view) {
                    ln -= EVIDENCE_LN_LR;
                }
                if same {
                    ln += EVIDENCE_LN_LR;
                }
                k.pass.then_some((*other, k, ln, same))
            })
            .collect();
        scored.sort_by(|a, b| b.2.total_cmp(&a.2));
        let found = scored.first().filter(|(_, _, best, same)| {
            *same
                && scored
                    .get(1)
                    .is_none_or(|next| best - next.2 >= AMBIGUITY.ln())
        });
        let Some(&(other, k, best, _)) = found else {
            self.bearings.areas.remove(&key);
            return None;
        };
        if self.bearings.areas.insert(key, other) != Some(other) {
            return None;
        }
        let share = 1.0 / scored.iter().map(|x| (x.2 - best).exp()).sum::<f64>();
        Some((other, k, share))
    }
}

#[cfg(test)]
impl Engine {
    /// The sensor track keys whose lines went into a fix key.
    pub(super) fn fix_members(&self, fix_key: &str) -> Vec<String> {
        self.bearings
            .members
            .iter()
            .filter(|(_, k)| k.as_str() == fix_key)
            .map(|((_, key), _)| key.clone())
            .collect()
    }

    /// The track a sensor track's emitter was found on, if any.
    pub(super) fn bearing_track(&self, source: &str, key: &str) -> Option<Uid> {
        let member = (source.to_owned(), key.to_owned());
        self.bearings.bound.get(&member).copied().or_else(|| {
            self.bearings
                .members
                .get(&member)
                .and_then(|k| self.reports.get(&format!("{FIX}/{k}")).copied())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lob(source: &str, lat: f64, lon: f64, to: (f64, f64), sigma: f64, id: Option<&str>) -> Lob {
        let (b, _) = bearing_to(lat, lon, to.0, to.1);
        let obs: Observation = serde_json::from_value(json!({
            "schema_version": 1, "source_id": source, "source_track_key": "e1",
            "observed_at": "2026-09-27T12:00:00Z", "received_at": "2026-09-27T12:00:00Z",
            "position": {"latitude": lat, "longitude": lon},
            "identifiers": id.map(|v| vec![json!({"scheme": "elnot", "value": v})]).unwrap_or_default(),
            "geometry": {"type": "bearing", "bearing_deg": b, "sigma_deg": sigma}
        }))
        .unwrap();
        Lob::of(obs).unwrap()
    }

    const SHIP: (f64, f64) = (50.70, -1.30);

    #[test]
    fn a_ranged_bearing_becomes_a_point_with_covariance() {
        let obs: Observation = serde_json::from_value(json!({
            "schema_version": 1, "source_id": "acoustic-1", "source_track_key": "drone",
            "observed_at": "2026-09-27T12:00:00Z", "received_at": "2026-09-27T12:00:00Z",
            "position": {"latitude": 50.0, "longitude": -1.0, "altitude_hae_m": 20.0},
            "geometry": {"type": "bearing", "bearing_deg": 90.0, "sigma_deg": 1.0,
                         "range_m": 1000.0, "range_sigma_m": 10.0,
                         "max_range_m": 2000.0, "elevation_deg": 5.0}
        }))
        .unwrap();
        let point = Lob::of(obs).unwrap().ranged_point().unwrap();
        let (bearing, range) = bearing_to(
            50.0,
            -1.0,
            point.position.latitude,
            point.position.longitude,
        );

        assert!((bearing - 90.0).abs() < 0.01);
        assert!((range - 996.2).abs() < 1.0, "{range}");
        assert!((point.position.altitude_hae_m.unwrap() - 107.2).abs() < 1.0);
        let [nn, ne, ee] = point.uncertainty.unwrap().position_covariance().unwrap();
        assert!(nn > ee && ne.abs() < 1e-6, "{nn} {ne} {ee}");
        assert!(point.geometry.is_none());
    }

    #[test]
    fn crossing_bearings_fix_the_emitter_with_an_honest_error() {
        let a = lob("esm-a", 50.60, -1.60, SHIP, 1.0, None);
        let b = lob("esm-b", 50.85, -1.45, SHIP, 1.0, None);
        let fix = cross(&[&a, &b]).unwrap();
        let (n, e) = local(SHIP.0, SHIP.1, fix.lat, fix.lon);
        // On the ship to within the flat-earth approximation (0.1 degree at
        // 20 km), far inside the fix's own error.
        assert!(
            n.hypot(e) < 100.0,
            "exact bearings meet on the ship: {n} {e}"
        );
        // ~20 km ranges at 1 degree: a few hundred metres, not kilometres.
        let s = (fix.cov[0] + fix.cov[2]).sqrt();
        assert!((100.0..2000.0).contains(&s), "{s}");
        // Parallel lines do not fix; nor do lines crossing behind a sensor.
        let c = lob("esm-c", 50.60, -1.59, SHIP, 1.0, None);
        assert!(cross(&[&a, &c]).is_none());
        let mut behind = lob("esm-b", 50.85, -1.45, SHIP, 1.0, None);
        behind.bearing = (behind.bearing + 180.0) % 360.0;
        assert!(cross(&[&a, &behind]).is_none());
    }

    #[test]
    fn a_fix_never_holds_two_of_one_sensors_tracks() {
        let keyed = |source: &str, key: &str| {
            let mut l = lob(source, 50.60, -1.60, SHIP, 1.0, None);
            l.obs.source_track_key = key.into();
            l
        };
        let mut w = Bearings::default();
        let m = |s: &str, k: &str| (s.to_string(), k.to_string());
        w.members.insert(m("esm-a", "x"), "fix-1".into());
        w.members.insert(m("esm-b", "x"), "fix-1".into());
        // esm-a tracks "y" apart from its "x": it is another object, so it
        // may not join the fix "x" went into, whatever its line fits.
        let (y, bx, c) = (
            keyed("esm-a", "y"),
            keyed("esm-b", "x"),
            keyed("esm-c", "z"),
        );
        assert!(!w.one_emitter(&[&y, &bx, &c]));
        // A sensor's own track, and a sensor new to the fix, may.
        let x = keyed("esm-a", "x");
        assert!(w.one_emitter(&[&x, &bx, &c]));
        // As before: sensor tracks that fixed apart stay apart.
        w.members.insert(m("esm-c", "z"), "fix-2".into());
        assert!(!w.one_emitter(&[&x, &bx, &c]));
    }

    #[test]
    fn two_sensors_fix_with_the_same_elnot_only_twice() {
        let mut w = Bearings::default();
        w.waiting.push(lob("esm-a", 50.60, -1.60, SHIP, 1.0, None));
        w.waiting.push(lob("esm-b", 50.85, -1.45, SHIP, 1.0, None));
        assert!(w.sweep().is_empty(), "two anonymous lines may be a ghost");
        w.waiting.push(lob("esm-c", 50.55, -1.10, SHIP, 1.0, None));
        assert!(
            w.sweep().is_empty(),
            "three lines meeting once may be chance"
        );
        assert!(w.waiting.is_empty(), "and are used up");
        for (s, lat, lon) in [
            ("esm-a", 50.60, -1.60),
            ("esm-b", 50.85, -1.45),
            ("esm-c", 50.55, -1.10),
        ] {
            let mut l = lob(s, lat, lon, SHIP, 1.0, None);
            l.obs.observed_at += chrono::Duration::seconds(5);
            w.waiting.push(l);
        }
        let (used, fix) = w.sweep().pop().unwrap();
        assert_eq!(used.len(), 3, "three sensors agreeing twice");
        let (n, e) = local(SHIP.0, SHIP.1, fix.lat, fix.lon);
        assert!(n.hypot(e) < 100.0, "{n} {e}");
        assert!(w.waiting.is_empty());

        let mut w = Bearings::default();
        w.waiting
            .push(lob("esm-a", 50.60, -1.60, SHIP, 1.0, Some("A123")));
        w.waiting
            .push(lob("esm-b", 50.85, -1.45, SHIP, 1.0, Some("B999")));
        assert!(
            w.sweep().is_empty(),
            "different emitters never fix together"
        );
        w.waiting
            .push(lob("esm-c", 50.55, -1.10, SHIP, 1.0, Some("A123")));
        // An ELNOT is shared by every emitter of its kind: two lines of it
        // may be two boats with the same radar, so they too must fix twice.
        assert!(w.sweep().is_empty(), "the same ELNOT once may be chance");
        for (s, lat, lon) in [("esm-a", 50.60, -1.60), ("esm-c", 50.55, -1.10)] {
            let mut l = lob(s, lat, lon, SHIP, 1.0, Some("A123"));
            l.obs.observed_at += chrono::Duration::seconds(5);
            w.waiting.push(l);
        }
        let (used, _) = w.sweep().pop().unwrap();
        assert_eq!(used.len(), 2, "the same ELNOT from two sensors, twice");
        assert!(
            used.iter()
                .all(|l| l.identity().as_deref() == Some("elnot:A123"))
        );
    }

    #[test]
    fn a_line_that_misses_is_not_forced_into_a_fix() {
        let mut w = Bearings::default();
        w.waiting.push(lob("esm-a", 50.60, -1.60, SHIP, 1.0, None));
        w.waiting.push(lob("esm-b", 50.85, -1.45, SHIP, 1.0, None));
        // A third sensor looking at something else, 25 km away.
        w.waiting
            .push(lob("esm-c", 50.55, -1.10, (50.90, -1.05), 1.0, None));
        assert!(w.sweep().is_empty());
        assert_eq!(w.waiting.len(), 3);
    }
}
