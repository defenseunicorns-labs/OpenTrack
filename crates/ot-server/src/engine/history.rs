//! Deleting a point from a track's position history (GOLD's delete
//! history point): a track manager removes a bad position. The deletion is
//! a decision, goes out on NATS, and when the point was the track's latest
//! the track steps back to the point before it and republishes (the report
//! behind the bad point is dropped from what the engine holds).

use chrono::DateTime;
use ot_core::Uid;
use serde_json::{Value, json};

use super::Engine;

impl Engine {
    pub(super) async fn delete_history_point(
        &mut self,
        uid: Uid,
        t_ms: i64,
        actor: &str,
        reason: &str,
    ) -> anyhow::Result<Value> {
        let point = self
            .redis
            .track_history(uid, Some(t_ms), Some(t_ms), 1)
            .await?
            .pop()
            .ok_or_else(|| anyhow::anyhow!("{} has no history point at {t_ms}", uid.doc_id()))?;
        let d = ot_store::Decision {
            before: Some(json!(point)),
            ..ot_store::Decision::new(actor, "delete_history_point")
                .reason(reason)
                .evidence(json!({ "track": uid.doc_id(), "t": t_ms }))
        };
        let decision = self.db(move |db| db.record(&d)).await?;
        self.redis
            .delete_history_point(uid, t_ms, reason, decision)
            .await?;

        // The track's current position was that point: step back.
        let latest = self
            .tracks
            .get(&uid)
            .is_some_and(|t| t.view.observed_at.timestamp_millis() == t_ms);
        let mut stepped_back = false;
        if latest {
            let previous = self
                .redis
                .track_history(uid, None, Some(t_ms - 1), 1)
                .await?
                .pop();
            let bad: Vec<String> = self
                .latest
                .iter()
                .filter(|(_, o)| o.observed_at.timestamp_millis() == t_ms)
                .map(|(k, _)| k.clone())
                .collect();
            for k in bad {
                self.latest.remove(&k);
            }
            if let (Some(p), Some(t)) = (previous, self.tracks.get_mut(&uid)) {
                let v = &mut t.view;
                v.position.latitude = p.lat;
                v.position.longitude = p.lon;
                v.position.altitude_hae_m = p.alt;
                v.kinematics.course_deg = p.course;
                v.kinematics.speed_mps = p.speed;
                if let Some(at) = DateTime::from_timestamp_millis(p.t) {
                    v.observed_at = at;
                }
                self.grid.put(uid, p.lat, p.lon);
                stepped_back = true;
                self.save(uid, true).await?;
            }
        }
        tracing::info!(track = %uid.doc_id(), t = t_ms, %actor, stepped_back, "history point deleted");
        Ok(json!({ "decision": decision, "deleted": point, "stepped_back": stepped_back }))
    }
}
