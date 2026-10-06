//! A target's measurement error on the ground: the radar's polar (slant
//! range, cross range) one-sigma errors propagated to first order into the
//! local horizontal plane at the target, as an error ellipse and a
//! north/east covariance (see the README, "Ground error").
//!
//! Geometry: a flat-earth tangent plane at the sensor. The target's offset
//! from the sensor is converted to east/north metres with the WGS 84
//! meridional and prime-vertical radii of curvature at the sensor latitude.
//! Earth curvature is ignored, which at GMTI ranges (tens to a few hundred
//! kilometres) changes the grazing angle by well under a degree and the
//! bearing not at all to first order.

use serde::Serialize;

/// WGS 84 semi-major axis (m).
const WGS84_A: f64 = 6_378_137.0;
/// WGS 84 first eccentricity squared.
const WGS84_E2: f64 = 6.694_379_990_141_32e-3;
/// Below this ground range (m) the bearing is undefined (target at nadir).
const MIN_GROUND_RANGE_M: f64 = 1.0;

/// Where a sigma came from, most specific first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum SigmaSource {
    TargetReport,
    JobNominal,
    Options,
}

impl SigmaSource {
    fn name(self) -> &'static str {
        match self {
            SigmaSource::TargetReport => "target_report",
            SigmaSource::JobNominal => "job_nominal",
            SigmaSource::Options => "options",
        }
    }
}

/// The cross-range sigma: an angle (job nominal, options) or a distance
/// (the target report's own).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum CrossRange {
    Deg(f64),
    Metres(f64),
}

/// One-sigma polar measurement errors of a detection.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Sigmas {
    pub slant_range_m: f64,
    pub cross_range: CrossRange,
    /// The least specific source of the two sigmas.
    pub source: SigmaSource,
    pub radial_velocity_mps: Option<f64>,
}

/// Geodetic position: latitude, longitude (deg), height above WGS 84 (m).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Geo {
    pub lat: f64,
    pub lon: f64,
    pub h: f64,
}

/// The `ground_error` object of a target record (one sigma).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct GroundError {
    pub semi_major_m: f64,
    pub semi_minor_m: f64,
    /// Major axis, degrees clockwise from true north, in [0, 180).
    pub orientation_deg: f64,
    pub nn_m2: f64,
    pub ne_m2: f64,
    pub ee_m2: f64,
    /// Range error on the ground (slant range sigma / cos grazing angle).
    pub range_sigma_m: f64,
    pub cross_range_sigma_m: f64,
    pub slant_range_m: f64,
    pub ground_range_m: f64,
    /// Sensor to target, degrees clockwise from north; absent at nadir.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bearing_deg: Option<f64>,
    pub source: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub radial_velocity_sigma_mps: Option<f64>,
}

/// A usable sigma: finite and not negative.
pub(crate) fn sigma(v: Option<f64>) -> Option<f64> {
    v.filter(|v| v.is_finite() && *v >= 0.0)
}

/// East and north offset (m) of `to` from `from` on the tangent plane at `from`.
fn east_north(from: Geo, to: Geo) -> (f64, f64) {
    let phi = from.lat.to_radians();
    let w = (1.0 - WGS84_E2 * phi.sin().powi(2)).sqrt();
    let meridional = WGS84_A * (1.0 - WGS84_E2) / (w * w * w);
    let prime_vertical = WGS84_A / w;
    let dlat = to.lat - from.lat;
    let dlon = (to.lon - from.lon + 540.0).rem_euclid(360.0) - 180.0;
    (
        dlon.to_radians() * prime_vertical * phi.cos(),
        dlat.to_radians() * meridional,
    )
}

/// Propagate the polar sigmas at `target`, seen from `sensor`, to the ground.
/// None when an input is not finite.
pub(crate) fn ground_error(sensor: Geo, target: Geo, s: &Sigmas) -> Option<GroundError> {
    let finite = [
        sensor.lat,
        sensor.lon,
        sensor.h,
        target.lat,
        target.lon,
        target.h,
        s.slant_range_m,
    ];
    if finite.iter().any(|v| !v.is_finite()) {
        return None;
    }
    let (east, north) = east_north(sensor, target);
    let rho = east.hypot(north);
    let dh = sensor.h - target.h;
    let slant = rho.hypot(dh);
    let sigma_x = match s.cross_range {
        CrossRange::Deg(d) => slant * d.to_radians(),
        CrossRange::Metres(m) => m,
    };
    if !sigma_x.is_finite() || sigma_x < 0.0 || s.slant_range_m < 0.0 {
        return None;
    }

    let (sigma_g, bearing, nn, ne, ee) = if rho < MIN_GROUND_RANGE_M {
        // At nadir there is no range direction: a circle as large as the larger sigma.
        let r = s.slant_range_m.max(sigma_x);
        (s.slant_range_m, None, r * r, 0.0, r * r)
    } else {
        // cos(grazing angle) = rho / slant, with slant >= rho > 0.
        let sigma_g = s.slant_range_m * slant / rho;
        let beta = east.atan2(north);
        let (sb, cb) = beta.sin_cos();
        let (g2, x2) = (sigma_g * sigma_g, sigma_x * sigma_x);
        // Range axis (east, north) = (sin b, cos b); cross-range axis (cos b, -sin b).
        let nn = g2 * cb * cb + x2 * sb * sb;
        let ee = g2 * sb * sb + x2 * cb * cb;
        let ne = (g2 - x2) * sb * cb;
        (
            sigma_g,
            Some(beta.to_degrees().rem_euclid(360.0)),
            nn,
            ne,
            ee,
        )
    };

    let mean = (nn + ee) / 2.0;
    let half = ((nn - ee) / 2.0).hypot(ne);
    let major = (mean + half).max(0.0).sqrt();
    let minor = (mean - half).max(0.0).sqrt().min(major);
    // Direction (east, north) = (sin t, cos t) of the larger eigenvector.
    let orientation = if half > 0.0 {
        (0.5 * (2.0 * ne).atan2(nn - ee))
            .to_degrees()
            .rem_euclid(180.0)
    } else {
        0.0
    };
    // rem_euclid can round up to exactly 180.
    let orientation = if orientation >= 180.0 {
        0.0
    } else {
        orientation
    };

    let out = GroundError {
        semi_major_m: major,
        semi_minor_m: minor,
        orientation_deg: orientation,
        nn_m2: nn,
        ne_m2: ne,
        ee_m2: ee,
        range_sigma_m: sigma_g,
        cross_range_sigma_m: sigma_x,
        slant_range_m: slant,
        ground_range_m: rho,
        bearing_deg: bearing,
        source: s.source.name(),
        radial_velocity_sigma_mps: sigma(s.radial_velocity_mps),
    };
    let all = [
        out.semi_major_m,
        out.semi_minor_m,
        out.orientation_deg,
        out.nn_m2,
        out.ne_m2,
        out.ee_m2,
        out.range_sigma_m,
        out.cross_range_sigma_m,
        out.slant_range_m,
        out.ground_range_m,
    ];
    all.iter().all(|v| v.is_finite()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    /// A sensor 10 km up and a target at sea level `ground_m` from it along `bearing_deg`.
    fn at(bearing_deg: f64, ground_m: f64) -> (Geo, Geo) {
        let sensor = Geo {
            lat: -34.0,
            lon: 138.0,
            h: 10_000.0,
        };
        // Invert the tangent-plane conversion at the sensor.
        let phi = sensor.lat.to_radians();
        let w = (1.0 - WGS84_E2 * phi.sin().powi(2)).sqrt();
        let m = WGS84_A * (1.0 - WGS84_E2) / (w * w * w);
        let n = WGS84_A / w;
        let b = bearing_deg.to_radians();
        let target = Geo {
            lat: sensor.lat + (ground_m * b.cos() / m).to_degrees(),
            lon: sensor.lon + (ground_m * b.sin() / (n * phi.cos())).to_degrees(),
            h: 0.0,
        };
        (sensor, target)
    }

    fn sig(range: f64, cross: CrossRange) -> Sigmas {
        Sigmas {
            slant_range_m: range,
            cross_range: cross,
            source: SigmaSource::Options,
            radial_velocity_mps: None,
        }
    }

    /// The covariance's eigen-decomposition is the ellipse.
    fn ellipse_matches_covariance(e: &GroundError) {
        let t = e.orientation_deg.to_radians();
        let (st, ct) = t.sin_cos();
        let (a2, b2) = (e.semi_major_m.powi(2), e.semi_minor_m.powi(2));
        let tol = 1e-6 * a2.max(1.0);
        assert!(close(e.nn_m2, a2 * ct * ct + b2 * st * st, tol), "{e:?}");
        assert!(close(e.ee_m2, a2 * st * st + b2 * ct * ct, tol), "{e:?}");
        assert!(close(e.ne_m2, (a2 - b2) * st * ct, tol), "{e:?}");
        assert!((0.0..180.0).contains(&e.orientation_deg));
        assert!(e.semi_minor_m <= e.semi_major_m);
    }

    #[test]
    fn sensor_due_south_cross_range_dominates() {
        // 50 km ground range, 10 km up: slant ~ 50.99 km.
        let (s, t) = at(0.0, 50_000.0);
        let e = ground_error(s, t, &sig(20.0, CrossRange::Deg(0.2))).unwrap();
        let slant = 50_000f64.hypot(10_000.0);
        assert!(close(e.ground_range_m, 50_000.0, 1e-3), "{e:?}");
        assert!(close(e.slant_range_m, slant, 1e-3));
        assert!(
            close(e.bearing_deg.unwrap(), 0.0, 1e-6) || close(e.bearing_deg.unwrap(), 360.0, 1e-6)
        );
        // sigma_g = sigma_R / cos(psi), cos(psi) = rho / R.
        let cos_psi = 50_000.0 / slant;
        assert!(close(e.range_sigma_m, 20.0 / cos_psi, 1e-6));
        // sigma_x = R sigma_theta.
        let sx = slant * 0.2f64.to_radians();
        assert!(close(e.cross_range_sigma_m, sx, 1e-6));
        // Cross range (east-west here) dominates: major axis east.
        assert!(sx > e.range_sigma_m);
        assert!(close(e.orientation_deg, 90.0, 1e-6), "{e:?}");
        assert!(close(e.semi_major_m, sx, 1e-6));
        assert!(close(e.semi_minor_m, e.range_sigma_m, 1e-6));
        assert!(close(e.ee_m2, sx * sx, 1e-3));
        assert!(close(e.nn_m2, e.range_sigma_m.powi(2), 1e-3));
        assert!(e.ne_m2.abs() < 1e-6);
        assert_eq!(e.source, "options");
        ellipse_matches_covariance(&e);
    }

    #[test]
    fn sensor_due_south_range_dominates() {
        let (s, t) = at(0.0, 20_000.0);
        let e = ground_error(s, t, &sig(50.0, CrossRange::Deg(0.01))).unwrap();
        assert!(e.range_sigma_m > e.cross_range_sigma_m);
        assert!(close(e.orientation_deg, 0.0, 1e-6), "{e:?}");
        assert!(close(e.semi_major_m, e.range_sigma_m, 1e-6));
        let cos_psi = 20_000.0 / 20_000f64.hypot(10_000.0);
        assert!(close(e.range_sigma_m, 50.0 / cos_psi, 1e-6));
        ellipse_matches_covariance(&e);
    }

    #[test]
    fn oblique_bearing_rotates_the_ellipse() {
        // Target north-east of the sensor, range dominant: major axis along 45 deg.
        let (s, t) = at(45.0, 30_000.0);
        let e = ground_error(s, t, &sig(40.0, CrossRange::Metres(10.0))).unwrap();
        assert!(close(e.bearing_deg.unwrap(), 45.0, 1e-6), "{e:?}");
        assert!(close(e.orientation_deg, 45.0, 1e-6), "{e:?}");
        assert!(e.ne_m2 > 0.0);
        assert_eq!(e.cross_range_sigma_m, 10.0, "metres used directly");
        ellipse_matches_covariance(&e);
        // Cross range dominant, target to the south-west: major axis at 135 deg.
        let (s, t) = at(225.0, 30_000.0);
        let e = ground_error(s, t, &sig(1.0, CrossRange::Metres(100.0))).unwrap();
        assert!(close(e.bearing_deg.unwrap(), 225.0, 1e-6), "{e:?}");
        assert!(close(e.orientation_deg, 135.0, 1e-6), "{e:?}");
        ellipse_matches_covariance(&e);
    }

    #[test]
    fn nadir_is_finite() {
        let (s, t) = at(0.0, 0.0);
        let e = ground_error(s, t, &sig(20.0, CrossRange::Deg(0.2))).unwrap();
        assert_eq!(e.range_sigma_m, 20.0);
        assert_eq!(e.bearing_deg, None);
        assert_eq!(e.orientation_deg, 0.0);
        assert!(close(
            e.cross_range_sigma_m,
            10_000.0 * 0.2f64.to_radians(),
            1e-9
        ));
        assert_eq!(e.semi_major_m, e.semi_minor_m);
        ellipse_matches_covariance(&e);
        // Sensor on the target: every value finite.
        let e = ground_error(t, t, &sig(20.0, CrossRange::Deg(0.2))).unwrap();
        assert_eq!(e.slant_range_m, 0.0);
        assert_eq!((e.semi_major_m, e.semi_minor_m), (20.0, 20.0));
    }

    #[test]
    fn longitude_wraps_at_the_antimeridian() {
        let s = Geo {
            lat: 0.0,
            lon: 179.9,
            h: 0.0,
        };
        let t = Geo {
            lat: 0.0,
            lon: -179.9,
            h: 0.0,
        };
        let e = ground_error(s, t, &sig(1.0, CrossRange::Metres(1.0))).unwrap();
        assert!(close(e.bearing_deg.unwrap(), 90.0, 1e-6));
        assert!(close(e.ground_range_m, 0.2f64.to_radians() * WGS84_A, 1.0));
    }

    #[test]
    fn non_finite_input_gives_none() {
        let (s, mut t) = at(0.0, 1000.0);
        t.lat = f64::NAN;
        assert!(ground_error(s, t, &sig(1.0, CrossRange::Deg(0.1))).is_none());
    }
}
