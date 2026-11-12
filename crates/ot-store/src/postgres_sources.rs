//! PostgreSQL source configuration and revisions.

use diesel::prelude::*;
use diesel::sql_types::{BigInt, Nullable, Text};
use serde_json::{Value, json};

use super::{PgControlStore, PostgresError, VersionText, record_decision};
use crate::Decision;
use crate::sources::{SourceRevision, SourceRow, SourceWrite};

#[derive(QueryableByName)]
struct PgSourceRow {
    #[diesel(sql_type = Text)]
    id: String,
    #[diesel(sql_type = Text)]
    name: String,
    #[diesel(sql_type = Text)]
    transport: String,
    #[diesel(sql_type = Text)]
    codec: String,
    #[diesel(sql_type = BigInt)]
    enabled: i64,
    #[diesel(sql_type = BigInt)]
    priority: i64,
    #[diesel(sql_type = BigInt)]
    revision: i64,
    #[diesel(sql_type = Text)]
    settings: String,
    #[diesel(sql_type = Nullable<Text>)]
    raw_subject: Option<String>,
    #[diesel(sql_type = BigInt)]
    created_at_ms: i64,
    #[diesel(sql_type = BigInt)]
    updated_at_ms: i64,
    #[diesel(sql_type = Text)]
    problems: String,
}

impl PgSourceRow {
    fn into_source(self) -> Result<SourceRow, PostgresError> {
        let problems: Value = serde_json::from_str(&self.problems)?;
        Ok(SourceRow {
            id: self.id,
            name: self.name,
            transport: self.transport,
            codec: self.codec,
            enabled: self.enabled != 0,
            priority: self.priority,
            revision: self.revision,
            spec: serde_json::from_str(&self.settings)?,
            raw_subject: self.raw_subject,
            created_at_ms: self.created_at_ms,
            updated_at_ms: self.updated_at_ms,
            draft: is_draft(&problems),
            problems,
        })
    }
}

#[derive(QueryableByName)]
struct PgSourceRevision {
    #[diesel(sql_type = BigInt)]
    revision: i64,
    #[diesel(sql_type = BigInt)]
    created_at_ms: i64,
    #[diesel(sql_type = Nullable<BigInt>)]
    decision_id: Option<i64>,
    #[diesel(sql_type = Text)]
    config: String,
}

fn is_draft(problems: &Value) -> bool {
    problems
        .as_array()
        .is_some_and(|problems| !problems.is_empty())
}

fn masked(spec: &Value) -> Value {
    let mut copy = spec.clone();
    ot_core::secrets::redact_spec(&mut copy);
    copy
}

const COLUMNS: &str = "id, name, transport, codec, enabled, priority, revision,
    settings, raw_subject, created_at_ms, updated_at_ms, problems";

impl PgControlStore {
    pub fn list_sources(&mut self) -> Result<Vec<SourceRow>, PostgresError> {
        let rows: Vec<PgSourceRow> =
            diesel::sql_query(format!("SELECT {COLUMNS} FROM sources ORDER BY id"))
                .load(&mut self.connection)?;
        rows.into_iter().map(PgSourceRow::into_source).collect()
    }

    pub fn get_source(&mut self, id: &str) -> Result<Option<SourceRow>, PostgresError> {
        let rows: Vec<PgSourceRow> =
            diesel::sql_query(format!("SELECT {COLUMNS} FROM sources WHERE id = $1"))
                .bind::<Text, _>(id)
                .load(&mut self.connection)?;
        rows.into_iter()
            .next()
            .map(PgSourceRow::into_source)
            .transpose()
    }

    pub fn put_source(
        &mut self,
        write: &SourceWrite<'_>,
        actor: &str,
    ) -> Result<SourceRow, PostgresError> {
        let before = self.get_source(write.id)?;
        if is_draft(write.problems) && before.as_ref().is_some_and(|row| row.enabled) {
            return Err(PostgresError::Conflict(format!(
                "source {} is enabled; it can be saved only complete",
                write.id
            )));
        }
        let revision = before.as_ref().map_or(1, |row| row.revision + 1);
        let op = if before.is_none() {
            "create_source"
        } else {
            "update_source"
        };
        let decision = Decision {
            before: before.as_ref().map(|row| masked(&row.spec)),
            after: Some(masked(write.spec)),
            evidence: json!({ "source": write.id, "revision": revision }),
            ..Decision::new(actor, op)
        };
        let spec = write.spec.to_string();
        let problems = write.problems.to_string();
        let now = crate::sqlite::now_ms();
        self.connection
            .transaction::<(), PostgresError, _>(|connection| {
                let decision_id = record_decision(connection, &decision)?;
                diesel::sql_query(
                    "INSERT INTO sources (id, name, transport, codec, settings, priority,
                 revision, created_at_ms, updated_at_ms, problems)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $8, $9)
                 ON CONFLICT (id) DO UPDATE SET name = excluded.name,
                 transport = excluded.transport, codec = excluded.codec,
                 settings = excluded.settings, priority = excluded.priority,
                 revision = excluded.revision, updated_at_ms = excluded.updated_at_ms,
                 problems = excluded.problems",
                )
                .bind::<Text, _>(write.id)
                .bind::<Text, _>(write.name)
                .bind::<Text, _>(write.transport)
                .bind::<Text, _>(write.codec)
                .bind::<Text, _>(&spec)
                .bind::<BigInt, _>(write.priority)
                .bind::<BigInt, _>(revision)
                .bind::<BigInt, _>(now)
                .bind::<Text, _>(&problems)
                .execute(connection)?;
                diesel::sql_query(
                    "INSERT INTO source_revisions
                 (source_id, revision, config, decision_id, created_at_ms)
                 VALUES ($1, $2, $3, $4, $5)",
                )
                .bind::<Text, _>(write.id)
                .bind::<BigInt, _>(revision)
                .bind::<Text, _>(&spec)
                .bind::<BigInt, _>(decision_id)
                .bind::<BigInt, _>(now)
                .execute(connection)?;
                Ok(())
            })?;
        self.get_source(write.id)?
            .ok_or_else(|| PostgresError::NotFound(format!("source {}", write.id)))
    }

    pub fn set_source_enabled(
        &mut self,
        id: &str,
        enabled: bool,
        problems: Option<&Value>,
        actor: &str,
    ) -> Result<SourceRow, PostgresError> {
        let before = self
            .get_source(id)?
            .ok_or_else(|| PostgresError::NotFound(format!("source {id}")))?;
        if enabled && is_draft(problems.unwrap_or(&before.problems)) {
            return Err(PostgresError::Conflict(format!(
                "source {id} is a draft; complete it before enabling it"
            )));
        }
        let op = if enabled {
            "enable_source"
        } else {
            "disable_source"
        };
        let decision = Decision::new(actor, op).evidence(json!({ "source": id }));
        let now = crate::sqlite::now_ms();
        self.connection
            .transaction::<(), PostgresError, _>(|connection| {
                record_decision(connection, &decision)?;
                diesel::sql_query(
                    "UPDATE sources SET enabled = $2, problems = coalesce($3, problems),
                 updated_at_ms = $4 WHERE id = $1",
                )
                .bind::<Text, _>(id)
                .bind::<BigInt, _>(i64::from(enabled))
                .bind::<Nullable<Text>, _>(problems.map(Value::to_string))
                .bind::<BigInt, _>(now)
                .execute(connection)?;
                Ok(())
            })?;
        self.get_source(id)?
            .ok_or_else(|| PostgresError::NotFound(format!("source {id}")))
    }

    pub fn set_source_problems(&mut self, id: &str, problems: &Value) -> Result<(), PostgresError> {
        let changed = diesel::sql_query("UPDATE sources SET problems = $2 WHERE id = $1")
            .bind::<Text, _>(id)
            .bind::<Text, _>(problems.to_string())
            .execute(&mut self.connection)?;
        if changed == 0 {
            return Err(PostgresError::NotFound(format!("source {id}")));
        }
        Ok(())
    }

    pub fn set_raw_output(
        &mut self,
        id: &str,
        subject: Option<&str>,
        actor: &str,
    ) -> Result<SourceRow, PostgresError> {
        let op = if subject.is_some() {
            "consent_raw_output"
        } else {
            "revoke_raw_output"
        };
        let decision =
            Decision::new(actor, op).evidence(json!({ "source": id, "subject": subject }));
        let now = crate::sqlite::now_ms();
        self.connection
            .transaction::<(), PostgresError, _>(|connection| {
                let decision_id = record_decision(connection, &decision)?;
                let changed = diesel::sql_query(
                    "UPDATE sources SET raw_subject = $2, raw_consent = $3,
                 updated_at_ms = $4 WHERE id = $1",
                )
                .bind::<Text, _>(id)
                .bind::<Nullable<Text>, _>(subject)
                .bind::<Nullable<BigInt>, _>(subject.map(|_| decision_id))
                .bind::<BigInt, _>(now)
                .execute(connection)?;
                if changed == 0 {
                    return Err(PostgresError::NotFound(format!("source {id}")));
                }
                Ok(())
            })?;
        self.get_source(id)?
            .ok_or_else(|| PostgresError::NotFound(format!("source {id}")))
    }

    pub fn delete_source(&mut self, id: &str, actor: &str) -> Result<(), PostgresError> {
        let before = self
            .get_source(id)?
            .ok_or_else(|| PostgresError::NotFound(format!("source {id}")))?;
        let decision = Decision {
            before: Some(masked(&before.spec)),
            evidence: json!({ "source": id }),
            ..Decision::new(actor, "delete_source")
        };
        self.connection
            .transaction::<(), PostgresError, _>(|connection| {
                record_decision(connection, &decision)?;
                diesel::sql_query("DELETE FROM sources WHERE id = $1")
                    .bind::<Text, _>(id)
                    .execute(connection)?;
                Ok(())
            })
    }

    pub fn source_revisions(&mut self, id: &str) -> Result<Vec<SourceRevision>, PostgresError> {
        let rows: Vec<PgSourceRevision> = diesel::sql_query(
            "SELECT revision, created_at_ms, decision_id, config FROM source_revisions
             WHERE source_id = $1 ORDER BY revision DESC",
        )
        .bind::<Text, _>(id)
        .load(&mut self.connection)?;
        rows.into_iter()
            .map(|row| {
                Ok(SourceRevision {
                    revision: row.revision,
                    saved_at_ms: row.created_at_ms,
                    decision_id: row.decision_id,
                    spec: serde_json::from_str(&row.config)?,
                })
            })
            .collect()
    }

    pub fn sources_version(&mut self) -> Result<String, PostgresError> {
        let row: VersionText = diesel::sql_query(
            "SELECT (count(*)::text || ':' || coalesce(max(updated_at_ms), 0)::text
                    || ':' || coalesce(sum(revision), 0)::text) AS version FROM sources",
        )
        .get_result(&mut self.connection)?;
        Ok(row.version)
    }
}
