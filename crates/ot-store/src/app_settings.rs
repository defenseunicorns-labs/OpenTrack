//! Instance settings (site name, classification banner) and purging the
//! track history.

use rusqlite::{OptionalExtension, params};
use serde_json::Value;

use crate::sqlite::{Db, Decision, Result, now_ms, record_decision};

impl Db {
    /// The saved instance settings (an empty object before any save).
    pub fn app_settings(&self) -> Result<Value> {
        let text: Option<String> = self
            .connection()
            .query_row("SELECT settings FROM app_settings WHERE id = 1", [], |r| {
                r.get(0)
            })
            .optional()?;
        Ok(match text {
            Some(t) => serde_json::from_str(&t)?,
            None => Value::Object(Default::default()),
        })
    }

    /// Save the instance settings (checked by the caller), recording who did.
    /// The decision log, which every viewer reads, gets them without the
    /// basemap tile URL (it may carry a key): only whether one is set.
    pub fn put_app_settings(&mut self, settings: &Value, actor: &str) -> Result<i64> {
        let mut evidence = settings.clone();
        if let Some(u) = evidence.get_mut("basemap_tiles_url")
            && u.as_str().is_some_and(|s| !s.is_empty())
        {
            *u = Value::String("(set)".into());
        }
        self.write(|tx| {
            let now = now_ms();
            let d = Decision::new(actor, "app_settings").evidence(evidence);
            let id = record_decision(tx, &d, now)?;
            tx.execute(
                "INSERT INTO app_settings (id, settings, updated_at_ms, decision_id) VALUES (1, ?1, ?2, ?3)
                 ON CONFLICT(id) DO UPDATE SET settings = excluded.settings,
                     updated_at_ms = excluded.updated_at_ms, decision_id = excluded.decision_id",
                params![settings.to_string(), now, id],
            )?;
            Ok(id)
        })
    }

    /// Delete the track graph's history: every source and system track node
    /// and the edges touching them, and the correlation suggestions. The
    /// decision log, configuration, registry stay; UIDs are never
    /// reused. Returns (nodes, edges) deleted.
    pub fn purge_track_history(&mut self, decision: Decision) -> Result<(usize, usize)> {
        self.write(|tx| {
            let tracks = "SELECT id FROM nodes WHERE kind IN ('source_track', 'system_track')";
            let edges = tx.execute(
                &format!("DELETE FROM edges WHERE src IN ({tracks}) OR dst IN ({tracks})"),
                [],
            )?;
            let nodes = tx.execute(
                "DELETE FROM nodes WHERE kind IN ('source_track', 'system_track')",
                [],
            )?;
            tx.execute("DELETE FROM correlation_suggestions", [])?;
            let d = decision.evidence(serde_json::json!({ "nodes": nodes, "edges": edges }));
            record_decision(tx, &d, now_ms())?;
            Ok((nodes, edges))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ot_core::SiteCode;

    #[test]
    fn settings_round_trip_and_history_purge() {
        let mut db = Db::open_in_memory().unwrap();
        assert_eq!(db.app_settings().unwrap(), serde_json::json!({}));
        let s = serde_json::json!({"site_name": "Garden Island"});
        db.put_app_settings(&s, "op:test").unwrap();
        assert_eq!(db.app_settings().unwrap(), s);

        let site = SiteCode::new("OTK").unwrap();
        let (uid, _) = db
            .create_system_track(
                site,
                "ais",
                "366",
                Decision::new("engine", "create_system_track"),
            )
            .unwrap();
        assert_eq!(db.explain(uid).unwrap().len(), 1);
        let (nodes, edges) = db
            .purge_track_history(Decision::new("op:test", "purge_track_history"))
            .unwrap();
        assert_eq!((nodes, edges), (2, 1));
        assert!(db.explain(uid).unwrap().is_empty());
        // UIDs are not reused.
        let (next, _) = db
            .create_system_track(
                site,
                "ais",
                "367",
                Decision::new("engine", "create_system_track"),
            )
            .unwrap();
        assert_ne!(next, uid);
        assert_eq!(db.app_settings().unwrap(), s, "settings stay");
    }

    #[test]
    fn the_decision_log_never_holds_the_tile_url() {
        let mut db = Db::open_in_memory().unwrap();
        let s = serde_json::json!({"basemap_tiles_url": "https://t.example/{z}/{x}/{y}.png?key=k"});
        db.put_app_settings(&s, "op:test").unwrap();
        assert_eq!(db.app_settings().unwrap(), s, "saved as given");
        let rows = db.decisions_by_op(&["app_settings"], 10).unwrap();
        assert_eq!(rows[0].evidence["basemap_tiles_url"], "(set)");
    }
}
