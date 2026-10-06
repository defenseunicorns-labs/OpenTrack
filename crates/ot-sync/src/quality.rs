//! Track quality (TQ), 0–15, as Link 16 grades it: how well a node knows
//! where a track is. It decides which node reports a track. 15 is a
//! position known to 10 m or better; each step down doubles the error, so 0
//! is 160 km or worse (or unknown).

/// TQ for a position error (the error ellipse's semi-major axis, metres).
pub fn quality(error_m: f64) -> u8 {
    if !error_m.is_finite() || error_m <= 0.0 {
        return if error_m.is_finite() { 15 } else { 0 };
    }
    let steps = (error_m / 10.0).log2().ceil().max(0.0);
    (15.0 - steps).clamp(0.0, 15.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_step_doubles_the_error() {
        assert_eq!(quality(3.0), 15);
        assert_eq!(quality(10.0), 15);
        assert_eq!(quality(11.0), 14);
        assert_eq!(quality(20.0), 14);
        assert_eq!(quality(80.0), 12);
        assert_eq!(quality(1000.0), 8);
        assert_eq!(quality(500_000.0), 0);
        assert_eq!(quality(f64::INFINITY), 0);
        assert_eq!(quality(f64::NAN), 0);
    }
}
