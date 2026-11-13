//! PostgreSQL track history maintenance and graph inspection.

use diesel::prelude::*;
use diesel::sql_types::Text;
use ot_core::Uid;
use serde_json::Value;

use super::{PgControlStore, PostgresError, record_decision};
use crate::Decision;

#[derive(QueryableByName)]
struct ReportRow {
    #[diesel(sql_type = Text)]
    key: String,
    #[diesel(sql_type = Text)]
    attrs: String,
}

#[derive(QueryableByName)]
struct KeyRow {
    #[diesel(sql_type = Text)]
    key: String,
}

impl PgControlStore {
    pub fn purge_track_history(
        &mut self,
        decision: &Decision,
    ) -> Result<(usize, usize), PostgresError> {
        self.connection.transaction::<(usize, usize), PostgresError, _>(|connection| {
            let edges = diesel::sql_query("DELETE FROM edges WHERE src IN (SELECT id FROM nodes WHERE kind IN ('source_track', 'system_track')) OR dst IN (SELECT id FROM nodes WHERE kind IN ('source_track', 'system_track'))").execute(connection)?;
            let nodes = diesel::sql_query("DELETE FROM nodes WHERE kind IN ('source_track', 'system_track')").execute(connection)?;
            diesel::sql_query("DELETE FROM correlation_suggestions").execute(connection)?;
            record_decision(connection, &decision.clone().evidence(serde_json::json!({"nodes":nodes,"edges":edges})))?;
            Ok((nodes, edges))
        })
    }

    pub fn reports_for(&mut self, uid: Uid) -> Result<Vec<(String, Value)>, PostgresError> {
        let rows: Vec<ReportRow> = diesel::sql_query("SELECT s.key, e.attrs FROM edges e JOIN nodes s ON s.id = e.src JOIN nodes d ON d.id = e.dst WHERE e.kind = 'REPORTS_FOR' AND e.valid_to_ms IS NULL AND d.kind = 'system_track' AND d.key = $1")
            .bind::<Text, _>(uid.to_string()).load(&mut self.connection)?;
        rows.into_iter()
            .map(|r| Ok((r.key, serde_json::from_str(&r.attrs)?)))
            .collect()
    }

    pub fn paired_with(&mut self, uid: Uid) -> Result<Vec<Uid>, PostgresError> {
        let rows: Vec<KeyRow> = diesel::sql_query("SELECT CASE WHEN a.key = $1 THEN b.key ELSE a.key END AS key FROM edges e JOIN nodes a ON a.id = e.src JOIN nodes b ON b.id = e.dst WHERE e.kind = 'PAIRED_WITH' AND e.valid_to_ms IS NULL AND (a.key = $1 OR b.key = $1)")
            .bind::<Text, _>(uid.to_string()).load(&mut self.connection)?;
        rows.into_iter().map(|r| Ok(r.key.parse()?)).collect()
    }
}
