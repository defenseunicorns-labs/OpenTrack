//! Source configuration in SQLite. The full source spec is stored as JSON;
//! the server validates it (against `ot-source`) before it gets here. Every
//! change bumps the revision, keeps the old version and records a decision.

use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use serde_json::{Value, json};

use crate::sqlite::{Db, Decision, Result, StoreError, now_ms, record_decision};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SourceRow {
    pub id: String,
    pub name: String,
    pub transport: String,
    pub codec: String,
    pub enabled: bool,
    pub priority: i64,
    pub revision: i64,
    pub spec: Value,
    pub raw_subject: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    /// Why the source may not run yet, as the server found when it was
    /// saved: a JSON array, empty when it is complete.
    pub problems: Value,
    /// Whether it has problems: a draft cannot be enabled.
    pub draft: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SourceRevision {
    pub revision: i64,
    pub saved_at_ms: i64,
    pub decision_id: Option<i64>,
    pub spec: Value,
}

/// What to save for a source.
#[derive(Debug, Clone)]
pub struct SourceWrite<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub transport: &'a str,
    pub codec: &'a str,
    pub priority: i64,
    pub spec: &'a Value,
    /// Its problems (a JSON array; empty: complete).
    pub problems: &'a Value,
}

const COLUMNS: &str = "id, name, transport, codec, enabled, priority, revision, settings, raw_subject, created_at_ms, updated_at_ms, problems";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<(SourceRow, String, String)> {
    let spec: String = r.get(7)?;
    Ok((
        SourceRow {
            id: r.get(0)?,
            name: r.get(1)?,
            transport: r.get(2)?,
            codec: r.get(3)?,
            enabled: r.get(4)?,
            priority: r.get(5)?,
            revision: r.get(6)?,
            spec: Value::Null,
            raw_subject: r.get(8)?,
            created_at_ms: r.get(9)?,
            updated_at_ms: r.get(10)?,
            problems: Value::Null,
            draft: false,
        },
        spec,
        r.get(11)?,
    ))
}

fn finish((mut row, spec, problems): (SourceRow, String, String)) -> Result<SourceRow> {
    row.spec = serde_json::from_str(&spec)?;
    row.problems = serde_json::from_str(&problems)?;
    row.draft = is_draft(&row.problems);
    Ok(row)
}

/// Whether a source with these problems is a draft: any problem listed.
fn is_draft(problems: &Value) -> bool {
    problems.as_array().is_some_and(|a| !a.is_empty())
}

/// A source's stored problems, if it exists.
fn stored_problems(tx: &rusqlite::Transaction<'_>, id: &str) -> Result<Option<(bool, Value)>> {
    tx.query_row(
        "SELECT enabled, problems FROM sources WHERE id = ?1",
        [id],
        |r| Ok((r.get::<_, bool>(0)?, r.get::<_, String>(1)?)),
    )
    .optional()?
    .map(|(enabled, p)| Ok((enabled, serde_json::from_str(&p)?)))
    .transpose()
}

impl Db {
    pub fn list_sources(&self) -> Result<Vec<SourceRow>> {
        let mut stmt = self
            .connection()
            .prepare(&format!("SELECT {COLUMNS} FROM sources ORDER BY id"))?;
        let rows = stmt.query_map([], row)?;
        rows.map(|r| finish(r?)).collect()
    }

    pub fn get_source(&self, id: &str) -> Result<Option<SourceRow>> {
        self.connection()
            .query_row(
                &format!("SELECT {COLUMNS} FROM sources WHERE id = ?1"),
                [id],
                row,
            )
            .optional()?
            .map(finish)
            .transpose()
    }

    /// Create or update a source. Returns the saved row. An enabled source
    /// is saved only complete (an edit never stops a running source).
    pub fn put_source(&mut self, w: &SourceWrite<'_>, actor: &str) -> Result<SourceRow> {
        let before = self.get_source(w.id)?;
        self.write(|tx| {
            if is_draft(w.problems)
                && stored_problems(tx, w.id)?.is_some_and(|(enabled, _)| enabled)
            {
                return Err(StoreError::Conflict(format!(
                    "source {} is enabled; it can be saved only complete",
                    w.id
                )));
            }
            let now = now_ms();
            let (op, revision) = match &before {
                None => ("create_source", 1),
                Some(b) => ("update_source", b.revision + 1),
            };
            // The decision (and its audit copy) records the change with
            // the secrets hidden; the revision keeps the whole spec (ASD
            // V-222444).
            let decision = Decision {
                before: before.as_ref().map(|b| masked(&b.spec)),
                after: Some(masked(w.spec)),
                evidence: json!({ "source": w.id, "revision": revision }),
                ..Decision::new(actor, op)
            };
            let decision_id = record_decision(tx, &decision, now)?;
            let spec = w.spec.to_string();
            let problems = w.problems.to_string();
            if before.is_none() {
                tx.execute(
                    "INSERT INTO sources (id, name, transport, codec, settings, priority, revision,
                                          created_at_ms, updated_at_ms, problems)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, ?7, ?8)",
                    params![w.id, w.name, w.transport, w.codec, spec, w.priority, now, problems],
                )?;
            } else {
                tx.execute(
                    "UPDATE sources SET name = ?2, transport = ?3, codec = ?4, settings = ?5,
                            priority = ?6, revision = ?7, updated_at_ms = ?8, problems = ?9
                     WHERE id = ?1",
                    params![
                        w.id, w.name, w.transport, w.codec, spec, w.priority, revision, now,
                        problems
                    ],
                )?;
            }
            tx.execute(
                "INSERT INTO source_revisions (source_id, revision, config, decision_id, created_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![w.id, revision, spec, decision_id, now],
            )?;
            Ok(())
        })?;
        self.get_source(w.id)?
            .ok_or_else(|| StoreError::NotFound(format!("source {}", w.id)))
    }

    /// Enable or disable a source. `problems`, when given, is what checking
    /// it again found now (a schema published since it was saved, say),
    /// stored first: a draft cannot be enabled.
    pub fn set_source_enabled(
        &mut self,
        id: &str,
        enabled: bool,
        problems: Option<&Value>,
        actor: &str,
    ) -> Result<SourceRow> {
        self.write(|tx| {
            let now = now_ms();
            let Some((_, stored)) = stored_problems(tx, id)? else {
                return Err(StoreError::NotFound(format!("source {id}")));
            };
            if let Some(p) = problems {
                tx.execute(
                    "UPDATE sources SET problems = ?2 WHERE id = ?1",
                    params![id, p.to_string()],
                )?;
            }
            if enabled && is_draft(problems.unwrap_or(&stored)) {
                return Err(StoreError::Conflict(format!(
                    "source {id} is a draft; complete it before enabling it"
                )));
            }
            let op = if enabled {
                "enable_source"
            } else {
                "disable_source"
            };
            let d = Decision::new(actor, op).evidence(json!({ "source": id }));
            record_decision(tx, &d, now)?;
            tx.execute(
                "UPDATE sources SET enabled = ?2, updated_at_ms = ?3 WHERE id = ?1",
                params![id, enabled, now],
            )?;
            Ok(())
        })?;
        self.get_source(id)?
            .ok_or_else(|| StoreError::NotFound(format!("source {id}")))
    }

    /// Store what checking a source again found, without a new revision
    /// (nothing about the source itself changed).
    pub fn set_source_problems(&mut self, id: &str, problems: &Value) -> Result<()> {
        self.write(|tx| {
            let n = tx.execute(
                "UPDATE sources SET problems = ?2 WHERE id = ?1",
                params![id, problems.to_string()],
            )?;
            if n == 0 {
                return Err(StoreError::NotFound(format!("source {id}")));
            }
            Ok(())
        })
    }

    /// Set or clear the raw-output subject. Setting one records the admin's
    /// consent to publish this source's raw tracks.
    pub fn set_raw_output(
        &mut self,
        id: &str,
        subject: Option<&str>,
        actor: &str,
    ) -> Result<SourceRow> {
        self.write(|tx| {
            let now = now_ms();
            let op = if subject.is_some() {
                "consent_raw_output"
            } else {
                "revoke_raw_output"
            };
            let d = Decision::new(actor, op).evidence(json!({ "source": id, "subject": subject }));
            let decision_id = record_decision(tx, &d, now)?;
            let n = tx.execute(
                "UPDATE sources SET raw_subject = ?2, raw_consent = ?3, updated_at_ms = ?4
                 WHERE id = ?1",
                params![id, subject, subject.map(|_| decision_id), now],
            )?;
            if n == 0 {
                return Err(StoreError::NotFound(format!("source {id}")));
            }
            Ok(())
        })?;
        self.get_source(id)?
            .ok_or_else(|| StoreError::NotFound(format!("source {id}")))
    }

    pub fn delete_source(&mut self, id: &str, actor: &str) -> Result<()> {
        let before = self
            .get_source(id)?
            .ok_or_else(|| StoreError::NotFound(format!("source {id}")))?;
        self.write(|tx| {
            let d = Decision {
                before: Some(masked(&before.spec)),
                evidence: json!({ "source": id }),
                ..Decision::new(actor, "delete_source")
            };
            record_decision(tx, &d, now_ms())?;
            tx.execute("DELETE FROM sources WHERE id = ?1", [id])?;
            Ok(())
        })
    }

    /// Past revisions of a source, newest first.
    pub fn source_revisions(&self, id: &str) -> Result<Vec<SourceRevision>> {
        let mut stmt = self.connection().prepare(
            "SELECT revision, created_at_ms, decision_id, config FROM source_revisions
             WHERE source_id = ?1 ORDER BY revision DESC",
        )?;
        let rows = stmt.query_map([id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get::<_, String>(3)?))
        })?;
        rows.map(|r| {
            let (revision, saved_at_ms, decision_id, cfg) = r?;
            Ok(SourceRevision {
                revision,
                saved_at_ms,
                decision_id,
                spec: serde_json::from_str(&cfg)?,
            })
        })
        .collect()
    }

    /// Changes whenever any source is added, edited, toggled or removed;
    /// workers poll it to know when to reload.
    pub fn sources_version(&self) -> Result<String> {
        Ok(self.connection().query_row(
            "SELECT count(*) || ':' || coalesce(max(updated_at_ms), 0) || ':' || coalesce(sum(revision), 0)
             FROM sources",
            [],
            |r| r.get(0),
        )?)
    }
}

/// A source specification with its secrets hidden, for the decision log.
fn masked(spec: &Value) -> Value {
    let mut v = spec.clone();
    ot_core::secrets::redact_spec(&mut v);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMPLETE: &Value = &Value::Array(Vec::new());

    fn write<'a>(spec: &'a Value) -> SourceWrite<'a> {
        SourceWrite {
            id: "ais",
            name: "AIS",
            transport: "websocket",
            codec: "json",
            priority: 100,
            spec,
            problems: COMPLETE,
        }
    }

    #[test]
    fn a_draft_keeps_its_problems_and_cannot_be_enabled() {
        let mut db = Db::open_in_memory().unwrap();
        let spec = json!({"v": 1});
        let problems = json!([{"kind": "incomplete", "stage": "source", "field": "url",
                               "message": "the URL is not set"}]);
        let row = db
            .put_source(
                &SourceWrite {
                    problems: &problems,
                    ..write(&spec)
                },
                "op:test",
            )
            .unwrap();
        assert!(row.draft);
        assert_eq!(row.problems, problems);
        assert!(matches!(
            db.set_source_enabled("ais", true, None, "op:test"),
            Err(StoreError::Conflict(_))
        ));
        assert!(!db.get_source("ais").unwrap().unwrap().enabled);
        // Turning a draft off is fine; completing it lets it run.
        db.set_source_enabled("ais", false, None, "op:test")
            .unwrap();
        let row = db.put_source(&write(&spec), "op:test").unwrap();
        assert!(!row.draft);
        assert_eq!(row.problems, json!([]));
        assert!(
            db.set_source_enabled("ais", true, None, "op:test")
                .unwrap()
                .enabled
        );
    }

    #[test]
    fn decisions_hide_a_sources_secrets_and_the_revision_keeps_them() {
        let mut db = Db::open_in_memory().unwrap();
        let spec = json!({"transport": {"kind": "mqtt", "url": "mqtts://broker:8883", "password": "hunter2hunter2", "username": "ot"}});
        db.put_source(&write(&spec), "op:test").unwrap();
        let spec2 = json!({"transport": {"kind": "mqtt", "url": "mqtts://broker:8883", "password": "n3w-s3cret-pw", "username": "ot"}});
        db.put_source(&write(&spec2), "op:test").unwrap();
        // What runs the source (its revision) is whole.
        let revs = db.source_revisions("ais").unwrap();
        assert!(
            serde_json::to_string(&revs)
                .unwrap()
                .contains("n3w-s3cret-pw")
        );
        db.delete_source("ais", "op:test").unwrap();
        let rows = db
            .audit_rows(&crate::audit::AuditFilter {
                ops: vec![
                    "create_source".into(),
                    "update_source".into(),
                    "delete_source".into(),
                ],
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(rows.len(), 3);
        let all = serde_json::to_string(&rows).unwrap();
        assert!(
            !all.contains("hunter2") && !all.contains("n3w-s3cret"),
            "{all}"
        );
        assert!(
            all.contains("username") && all.contains("mqtts://broker:8883"),
            "the rest is kept: {all}"
        );
    }

    #[test]
    fn lifecycle_with_revisions_and_decisions() {
        let mut db = Db::open_in_memory().unwrap();
        let v0 = db.sources_version().unwrap();
        let s1 = json!({"v": 1});
        let row = db.put_source(&write(&s1), "op:test").unwrap();
        assert_eq!((row.revision, row.enabled), (1, false));
        let s2 = json!({"v": 2});
        let row = db.put_source(&write(&s2), "op:test").unwrap();
        assert_eq!(row.revision, 2);
        assert_eq!(row.spec, s2);
        let v1 = db.sources_version().unwrap();
        assert_ne!(v0, v1);

        let row = db.set_source_enabled("ais", true, None, "op:test").unwrap();
        assert!(row.enabled);
        let revs = db.source_revisions("ais").unwrap();
        assert_eq!(revs.len(), 2);
        assert_eq!(revs[1].spec, s1);

        let row = db
            .set_raw_output("ais", Some("opentrack.raw.ais"), "op:test")
            .unwrap();
        assert_eq!(row.raw_subject.as_deref(), Some("opentrack.raw.ais"));

        db.delete_source("ais", "op:test").unwrap();
        assert!(db.get_source("ais").unwrap().is_none());
        assert!(db.list_sources().unwrap().is_empty());
        let ops: Vec<String> = db
            .connection()
            .prepare("SELECT op FROM decisions ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        assert_eq!(
            ops,
            [
                "create_source",
                "update_source",
                "enable_source",
                "consent_raw_output",
                "delete_source"
            ]
        );
        assert!(matches!(
            db.set_source_enabled("nope", true, None, "x"),
            Err(StoreError::NotFound(_))
        ));
    }
}
