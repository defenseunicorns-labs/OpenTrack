//! An example pairing scorer: OpenTrack's own kinematic comparison, with a
//! veto. Two tracks whose speeds differ by more than `max_speed_diff_mps`
//! are never the same object, however close; otherwise the kinematic
//! evidence stands. A plugin can equally ignore OpenTrack's comparison and
//! score from its own model (features, a classifier, a learned metric).
//!
//! Options: `max_speed_diff_mps` (10).

use opentrack_plugin::{Candidate, Observation, Score, Scorer, Value, json, plugin};

struct SpeedVeto {
    max_diff: f64,
}

impl Scorer for SpeedVeto {
    fn open(options: Value) -> Result<Self, String> {
        Ok(Self {
            max_diff: options
                .get("max_speed_diff_mps")
                .and_then(Value::as_f64)
                .unwrap_or(10.0),
        })
    }

    fn score(&mut self, report: &Observation, candidates: &[Candidate]) -> Result<Vec<Score>, String> {
        Ok(candidates
            .iter()
            .map(|c| {
                let ln_lr = c.kinematic["ln_lr"].as_f64().unwrap_or(-10.0);
                let pass = c.kinematic["pass"].as_bool().unwrap_or(false);
                let diff = match (report.kinematics.speed_mps, c.view.kinematics.speed_mps) {
                    (Some(a), Some(b)) => Some((a - b).abs()),
                    _ => None,
                };
                if diff.is_some_and(|d| d > self.max_diff) {
                    Score {
                        ln_lr: -20.0,
                        pass: false,
                        evidence: json!({"veto": "speed", "speed_diff_mps": diff}),
                    }
                } else {
                    Score { ln_lr, pass, evidence: json!({"speed_diff_mps": diff}) }
                }
            })
            .collect())
    }
}

plugin! {
    manifest: json!({
        "name": "speed-veto",
        "version": "1",
        "description": "Example scorer: OpenTrack's kinematic evidence, vetoed when speeds disagree",
        "options": [
            {"name": "max_speed_diff_mps", "label": "Largest speed difference (m/s)", "type": "number",
             "default": 10.0, "unit": "m/s", "min": 0.0,
             "help": "Tracks further apart in speed than this never pair"}
        ]
    }),
    scorer: SpeedVeto,
}
