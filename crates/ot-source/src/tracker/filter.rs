//! A constant-velocity Kalman filter in each track's own local frame:
//! position as latitude/longitude, velocity and covariance in metres
//! (north, east), so it works anywhere without a global projection.

// Small fixed-size matrix algebra reads best as index loops.
#![allow(clippy::needless_range_loop)]

use chrono::{DateTime, Utc};

const EARTH_RADIUS_M: f64 = 6_371_008.8;

type M4 = [[f64; 4]; 4];
type M2 = [[f64; 2]; 2];

/// North/east metres from `(lat1, lon1)` to `(lat2, lon2)` (local tangent plane).
pub fn offset_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> (f64, f64) {
    let mut dlon = lon2 - lon1;
    if dlon > 180.0 {
        dlon -= 360.0;
    } else if dlon < -180.0 {
        dlon += 360.0;
    }
    let lat = ((lat1 + lat2) / 2.0).to_radians();
    (
        (lat2 - lat1).to_radians() * EARTH_RADIUS_M,
        dlon.to_radians() * EARTH_RADIUS_M * lat.cos().max(1e-6),
    )
}

/// `(lat, lon)` moved by `dn` metres north and `de` metres east.
pub fn moved(lat: f64, lon: f64, dn: f64, de: f64) -> (f64, f64) {
    let lat2 = (lat + (dn / EARTH_RADIUS_M).to_degrees()).clamp(-90.0, 90.0);
    let mut lon2 = lon + (de / (EARTH_RADIUS_M * lat.to_radians().cos().max(1e-6))).to_degrees();
    if lon2 > 180.0 {
        lon2 -= 360.0;
    } else if lon2 < -180.0 {
        lon2 += 360.0;
    }
    (lat2, lon2)
}

/// A track's state: where it is, how it moves, and how sure we are.
#[derive(Debug, Clone, PartialEq)]
pub struct Kf {
    pub lat: f64,
    pub lon: f64,
    pub vn: f64,
    pub ve: f64,
    /// Covariance of (north, east, v north, v east), metres and m/s.
    pub p: M4,
    pub t: DateTime<Utc>,
}

/// One plot compared with a predicted track.
#[derive(Debug, Clone, Copy)]
pub struct Innovation {
    v: [f64; 2],
    si: M2,
    /// Squared Mahalanobis distance (chi-square, 2 degrees of freedom).
    pub d2: f64,
    /// ln |S|, for likelihoods and assignment costs.
    pub ln_det: f64,
}

impl Innovation {
    /// ln of the Gaussian likelihood of the plot given the track.
    pub fn ln_likelihood(&self) -> f64 {
        -0.5 * self.d2 - (2.0 * std::f64::consts::PI).ln() - 0.5 * self.ln_det
    }
}

impl Kf {
    /// A new track at a plot with position covariance `r` (`[nn, ne, ee]`,
    /// m²), not moving as far as anyone knows yet.
    pub fn birth(lat: f64, lon: f64, r: [f64; 3], v0_mps: f64, t: DateTime<Utc>) -> Self {
        let mut p = [[0.0; 4]; 4];
        p[0][0] = r[0];
        p[0][1] = r[1];
        p[1][0] = r[1];
        p[1][1] = r[2];
        p[2][2] = v0_mps * v0_mps;
        p[3][3] = v0_mps * v0_mps;
        Self {
            lat,
            lon,
            vn: 0.0,
            ve: 0.0,
            p,
            t,
        }
    }

    /// The state at `t`, moving at constant velocity with white-acceleration
    /// process noise `q` (m/s²). Earlier times return the state unchanged.
    pub fn predict(&self, t: DateTime<Utc>, q: f64) -> Self {
        let dt = ((t - self.t).num_microseconds().unwrap_or(0) as f64 / 1e6).max(0.0);
        if dt == 0.0 {
            return Self {
                t: self.t.max(t),
                ..self.clone()
            };
        }
        let (lat, lon) = moved(self.lat, self.lon, self.vn * dt, self.ve * dt);
        let mut f = identity();
        f[0][2] = dt;
        f[1][3] = dt;
        let mut p = mul(&mul(&f, &self.p), &transpose(&f));
        let q2 = q * q;
        let (a, b, c) = (dt.powi(4) / 4.0, dt.powi(3) / 2.0, dt * dt);
        for (i, j) in [(0, 2), (1, 3)] {
            p[i][i] += a * q2;
            p[i][j] += b * q2;
            p[j][i] += b * q2;
            p[j][j] += c * q2;
        }
        Self {
            lat,
            lon,
            vn: self.vn,
            ve: self.ve,
            p,
            t,
        }
    }

    /// Compare a plot (position covariance `r`, `[nn, ne, ee]` m²) with this state.
    pub fn innovation(&self, lat: f64, lon: f64, r: [f64; 3]) -> Innovation {
        let (dn, de) = offset_m(self.lat, self.lon, lat, lon);
        let s = [
            [self.p[0][0] + r[0], self.p[0][1] + r[1]],
            [self.p[1][0] + r[1], self.p[1][1] + r[2]],
        ];
        let det = (s[0][0] * s[1][1] - s[0][1] * s[1][0]).max(1e-9);
        let si = [
            [s[1][1] / det, -s[0][1] / det],
            [-s[1][0] / det, s[0][0] / det],
        ];
        let v = [dn, de];
        let d2 =
            v[0] * (si[0][0] * v[0] + si[0][1] * v[1]) + v[1] * (si[1][0] * v[0] + si[1][1] * v[1]);
        Innovation {
            v,
            si,
            d2,
            ln_det: det.ln(),
        }
    }

    /// The state corrected by a plot.
    pub fn update(&self, inn: &Innovation) -> Self {
        // K = P Hᵀ S⁻¹ (H picks the position rows).
        let mut k = [[0.0; 2]; 4];
        for (i, row) in k.iter_mut().enumerate() {
            for (j, cell) in row.iter_mut().enumerate() {
                *cell = self.p[i][0] * inn.si[0][j] + self.p[i][1] * inn.si[1][j];
            }
        }
        let dx: Vec<f64> = (0..4)
            .map(|i| k[i][0] * inn.v[0] + k[i][1] * inn.v[1])
            .collect();
        let (lat, lon) = moved(self.lat, self.lon, dx[0], dx[1]);
        // P = (I - K H) P
        let mut p = self.p;
        for i in 0..4 {
            for j in 0..4 {
                p[i][j] = self.p[i][j] - (k[i][0] * self.p[0][j] + k[i][1] * self.p[1][j]);
            }
        }
        for i in 0..4 {
            for j in 0..i {
                let m = (p[i][j] + p[j][i]) / 2.0;
                p[i][j] = m;
                p[j][i] = m;
            }
        }
        Self {
            lat,
            lon,
            vn: self.vn + dx[2],
            ve: self.ve + dx[3],
            p,
            t: self.t,
        }
    }

    /// Course (degrees true) and speed (m/s).
    pub fn course_speed(&self) -> (f64, f64) {
        (
            self.ve.atan2(self.vn).to_degrees().rem_euclid(360.0),
            self.vn.hypot(self.ve),
        )
    }

    /// Circular error (CEP) of the position, metres.
    pub fn cep_m(&self) -> f64 {
        1.1774 * ((self.p[0][0] + self.p[1][1]) / 2.0).max(0.0).sqrt()
    }

    /// Position covariance `[nn, ne, ee]` (m²).
    pub fn position_cov(&self) -> [f64; 3] {
        [self.p[0][0], self.p[0][1], self.p[1][1]]
    }

    /// The whole state covariance, for consumers that propagate it.
    pub fn covariance(&self) -> ot_core::Covariance {
        let p = &self.p;
        ot_core::Covariance {
            position: [p[0][0], p[0][1], p[1][1]],
            velocity: Some([p[2][2], p[2][3], p[3][3]]),
            cross: Some([p[0][2], p[0][3], p[1][2], p[1][3]]),
        }
    }
}

fn identity() -> M4 {
    let mut m = [[0.0; 4]; 4];
    for (i, row) in m.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    m
}

fn mul(a: &M4, b: &M4) -> M4 {
    let mut m = [[0.0; 4]; 4];
    for i in 0..4 {
        for j in 0..4 {
            m[i][j] = (0..4).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    m
}

fn transpose(a: &M4) -> M4 {
    let mut m = [[0.0; 4]; 4];
    for i in 0..4 {
        for j in 0..4 {
            m[i][j] = a[j][i];
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_790_000_000 + secs, 0).unwrap()
    }

    #[test]
    fn offsets_round_trip_and_wrap_the_antimeridian() {
        let (lat, lon) = moved(63.44, 10.40, 100.0, -50.0);
        let (n, e) = offset_m(63.44, 10.40, lat, lon);
        assert!(
            (n - 100.0).abs() < 0.01 && (e + 50.0).abs() < 0.01,
            "{n} {e}"
        );
        let (_, e) = offset_m(0.0, 179.999, 0.0, -179.999);
        assert!((e - 222.4).abs() < 1.0, "{e}");
    }

    #[test]
    fn a_track_learns_its_velocity() {
        // A target moving 5 m/s east, seen every second with 3 m noise-free plots.
        let mut kf = Kf::birth(63.44, 10.40, [9.0, 0.0, 9.0], 5.0, at(0));
        for s in 1..=20 {
            let (lat, lon) = moved(63.44, 10.40, 0.0, 5.0 * s as f64);
            let pred = kf.predict(at(s), 0.5);
            let inn = pred.innovation(lat, lon, [9.0, 0.0, 9.0]);
            assert!(inn.d2 < 13.8, "second {s}: d2 {}", inn.d2);
            kf = pred.update(&inn);
        }
        let (course, speed) = kf.course_speed();
        assert!(
            (course - 90.0).abs() < 2.0 && (speed - 5.0).abs() < 0.3,
            "{course} {speed}"
        );
        assert!(kf.cep_m() < 3.0);
    }
}
