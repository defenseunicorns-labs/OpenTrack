//! Non-point contacts (see `docs/non-point-contacts.md`): a line of bearing
//! (a direction from the sensor, optionally with measured range) or an area of
//! uncertainty (a polygon the object is somewhere inside). An observation that
//! carries one keeps a `position` all the same: a bearing's is the sensor's
//! position, where the line starts; an area's is its centre.

use serde::{Deserialize, Serialize};

const R: f64 = 6_371_008.8;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Geometry {
    /// From the observation's position, the object lies this way.
    Bearing {
        /// Degrees true.
        bearing_deg: f64,
        /// One standard deviation, degrees.
        sigma_deg: f64,
        /// Measured slant range to the object, metres, if known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        range_m: Option<f64>,
        /// One standard deviation of the measured range, metres.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        range_sigma_m: Option<f64>,
        /// How far the sensor could have detected it, metres, if known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_range_m: Option<f64>,
        /// Degrees above the horizon, for air, if known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        elevation_deg: Option<f64>,
    },
    /// The object is somewhere inside this polygon (`[lat, lon]` vertices,
    /// not closed: the last joins the first).
    Area { polygon: Vec<[f64; 2]> },
}

impl Geometry {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Geometry::Bearing {
                bearing_deg,
                sigma_deg,
                range_m,
                range_sigma_m,
                max_range_m,
                elevation_deg,
            } => {
                if !(bearing_deg.is_finite() && (0.0..=360.0).contains(bearing_deg)) {
                    return Err(format!("bearing_deg {bearing_deg} is outside [0, 360]"));
                }
                if !(sigma_deg.is_finite() && *sigma_deg > 0.0 && *sigma_deg <= 90.0) {
                    return Err(format!("sigma_deg {sigma_deg} is outside (0, 90]"));
                }
                if max_range_m.is_some_and(|r| !(r.is_finite() && r > 0.0)) {
                    return Err("max_range_m must be a positive number".into());
                }
                if range_m.is_some_and(|r| !(r.is_finite() && r > 0.0)) {
                    return Err("range_m must be a positive number".into());
                }
                if range_sigma_m.is_some_and(|r| !(r.is_finite() && r > 0.0)) {
                    return Err("range_sigma_m must be a positive number".into());
                }
                if range_sigma_m.is_some() && range_m.is_none() {
                    return Err("range_sigma_m requires range_m".into());
                }
                if let (Some(range), Some(max)) = (range_m, max_range_m)
                    && range > max
                {
                    return Err("range_m cannot exceed max_range_m".into());
                }
                if elevation_deg.is_some_and(|e| !(e.is_finite() && (-90.0..=90.0).contains(&e))) {
                    return Err("elevation_deg is outside [-90, 90]".into());
                }
            }
            Geometry::Area { polygon } => {
                if polygon.len() < 3 || polygon.len() > 1000 {
                    return Err(format!(
                        "an area needs 3 to 1000 vertices, not {}",
                        polygon.len()
                    ));
                }
                if polygon.iter().any(|[la, lo]| {
                    !(la.is_finite()
                        && lo.is_finite()
                        && (-90.0..=90.0).contains(la)
                        && (-180.0..=180.0).contains(lo))
                }) {
                    return Err("an area's vertices must be [latitude, longitude]".into());
                }
            }
        }
        Ok(())
    }
}

/// North/east metres of `(lat, lon)` from `(lat0, lon0)` (local, flat).
pub fn local(lat0: f64, lon0: f64, lat: f64, lon: f64) -> (f64, f64) {
    let n = (lat - lat0).to_radians() * R;
    let e = (lon - lon0).to_radians() * R * lat0.to_radians().cos();
    (n, e)
}

/// The point `(n, e)` metres from `(lat0, lon0)`.
pub fn offset(lat0: f64, lon0: f64, n: f64, e: f64) -> (f64, f64) {
    (
        lat0 + (n / R).to_degrees(),
        lon0 + (e / (R * lat0.to_radians().cos().max(1e-9))).to_degrees(),
    )
}

/// Bearing (degrees true) and distance (metres) from one point to another.
pub fn bearing_to(lat0: f64, lon0: f64, lat: f64, lon: f64) -> (f64, f64) {
    let (n, e) = local(lat0, lon0, lat, lon);
    (e.atan2(n).to_degrees().rem_euclid(360.0), n.hypot(e))
}

/// A polygon's centre and a 1-sigma covariance `[nn, ne, ee]` (m²) that
/// treats the polygon as the object's 95% region.
pub fn area_estimate(polygon: &[[f64; 2]]) -> (f64, f64, [f64; 3]) {
    let n = polygon.len().max(1) as f64;
    let lat = polygon.iter().map(|p| p[0]).sum::<f64>() / n;
    let lon = polygon.iter().map(|p| p[1]).sum::<f64>() / n;
    let pts: Vec<(f64, f64)> = polygon
        .iter()
        .map(|p| local(lat, lon, p[0], p[1]))
        .collect();
    let (mut nn, mut ne, mut ee) = (0.0, 0.0, 0.0);
    for (a, b) in &pts {
        nn += a * a;
        ne += a * b;
        ee += b * b;
    }
    // Vertices lie on the 95% boundary: about 2.45 sigma out (2 dof).
    let k = 1.0 / (n * 2.45f64.powi(2) / 2.0);
    (lat, lon, [nn * k, ne * k, ee * k])
}

/// Whether the polygon contains the point (ray casting in lat/lon).
pub fn contains(polygon: &[[f64; 2]], lat: f64, lon: f64) -> bool {
    let mut inside = false;
    let mut j = polygon.len().wrapping_sub(1);
    for i in 0..polygon.len() {
        let ([yi, xi], [yj, xj]) = (polygon[i], polygon[j]);
        if (yi > lat) != (yj > lat) && lon < (xj - xi) * (lat - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;

    const SQUARE: [[f64; 2]; 4] = [[50.0, -1.0], [50.0, -0.9], [50.1, -0.9], [50.1, -1.0]];

    #[test]
    fn an_area_has_a_centre_a_spread_and_an_inside() {
        let (lat, lon, [nn, ne, ee]) = area_estimate(&SQUARE);
        assert!((lat - 50.05).abs() < 1e-9 && (lon + 0.95).abs() < 1e-9);
        // ~11 km by ~7 km: sigmas of a few km, no tilt.
        assert!(nn.sqrt() > 1500.0 && nn.sqrt() < 4000.0, "{}", nn.sqrt());
        assert!(ee < nn && ne.abs() < 1.0);
        assert!(contains(&SQUARE, 50.05, -0.95));
        assert!(!contains(&SQUARE, 50.2, -0.95));
        assert!(!contains(&SQUARE, 50.05, -0.8));
    }

    #[test]
    fn bearings_and_offsets_agree() {
        let (lat, lon) = offset(50.0, -1.0, 1000.0, 1000.0);
        let (b, d) = bearing_to(50.0, -1.0, lat, lon);
        assert!(
            (b - 45.0).abs() < 0.01 && (d - 1414.2).abs() < 1.0,
            "{b} {d}"
        );
    }

    #[test]
    fn nonsense_is_refused() {
        let bad = |g: Geometry| g.validate().is_err();
        assert!(bad(Geometry::Bearing {
            bearing_deg: 400.0,
            sigma_deg: 1.0,
            range_m: None,
            range_sigma_m: None,
            max_range_m: None,
            elevation_deg: None
        }));
        assert!(bad(Geometry::Bearing {
            bearing_deg: 10.0,
            sigma_deg: 0.0,
            range_m: None,
            range_sigma_m: None,
            max_range_m: None,
            elevation_deg: None
        }));
        assert!(bad(Geometry::Bearing {
            bearing_deg: 10.0,
            sigma_deg: 1.0,
            range_m: Some(100.0),
            range_sigma_m: None,
            max_range_m: Some(50.0),
            elevation_deg: None
        }));
        assert!(bad(Geometry::Bearing {
            bearing_deg: 10.0,
            sigma_deg: 1.0,
            range_m: None,
            range_sigma_m: Some(5.0),
            max_range_m: None,
            elevation_deg: None
        }));
        assert!(bad(Geometry::Area {
            polygon: vec![[50.0, -1.0], [50.1, -1.0]]
        }));
        assert!(
            Geometry::Area {
                polygon: SQUARE.to_vec()
            }
            .validate()
            .is_ok()
        );
    }
}
