//! Captured probe frames, kept per source for mapping preview and plugin
//! conformance tests. Capped per source (oldest dropped).

use rusqlite::params;

use crate::sqlite::{Db, Result, now_ms};

/// Default number of frames kept per source.
pub const DEFAULT_SAMPLE_CAP: usize = 200;

/// One stored sample: when it was captured, its bytes and any transport
/// metadata (a JSON object).
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub captured_at_ms: i64,
    pub bytes: Vec<u8>,
    pub meta: Option<serde_json::Value>,
}

impl Db {
    /// Replace a source's stored samples with `frames` (bytes and optional
    /// metadata), keeping at most `cap`.
    pub fn store_probe_samples(
        &mut self,
        source_id: &str,
        frames: &[(Vec<u8>, Option<serde_json::Value>)],
        cap: usize,
    ) -> Result<usize> {
        self.write(|tx| {
            tx.execute("DELETE FROM probe_samples WHERE source_id = ?1", [source_id])?;
            let now = now_ms();
            let mut n = 0;
            for (bytes, meta) in frames.iter().take(cap) {
                tx.execute(
                    "INSERT INTO probe_samples (source_id, captured_at_ms, frame, meta) VALUES (?1, ?2, ?3, ?4)",
                    params![source_id, now, bytes, meta.as_ref().map(|m| m.to_string())],
                )?;
                n += 1;
            }
            Ok(n)
        })
    }

    /// Stored samples of a source, oldest first.
    pub fn probe_samples(&self, source_id: &str, limit: usize) -> Result<Vec<Sample>> {
        let mut stmt = self.connection().prepare(
            "SELECT captured_at_ms, frame, meta FROM probe_samples WHERE source_id = ?1 ORDER BY id LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![source_id, limit as i64], |r| {
            let meta: Option<String> = r.get(2)?;
            Ok(Sample {
                captured_at_ms: r.get(0)?,
                bytes: r.get(1)?,
                meta: meta.and_then(|m| serde_json::from_str(&m).ok()),
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_are_replaced_and_capped() {
        let mut db = Db::open_in_memory().unwrap();
        let frames: Vec<(Vec<u8>, Option<serde_json::Value>)> = (0..5)
            .map(|i| (format!("f{i}").into_bytes(), None))
            .collect();
        assert_eq!(db.store_probe_samples("s", &frames, 3).unwrap(), 3);
        assert_eq!(db.probe_samples("s", 10).unwrap().len(), 3);
        let with_topic = [(
            b"f4".to_vec(),
            Some(serde_json::json!({"topic": "ais/1/pos"})),
        )];
        db.store_probe_samples("s", &with_topic, 3).unwrap();
        let got = db.probe_samples("s", 10).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].bytes, b"f4");
        assert_eq!(got[0].meta.as_ref().unwrap()["topic"], "ais/1/pos");
        assert!(db.probe_samples("other", 10).unwrap().is_empty());
    }
}
