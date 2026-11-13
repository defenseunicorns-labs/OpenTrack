//! PostgreSQL UID allocation guard.

use diesel::prelude::*;
use diesel::sql_types::{BigInt, Text};
use ot_core::{SiteCode, highest_sequence_in};
use serde_json::{Value, json};

use super::{PgControlStore, PostgresError, SchemaVersion, record_decision};
use crate::Decision;
use crate::uid_counter::{UidCounterAdvanced, UidSighting};

#[derive(QueryableByName)]
struct TextRow {
    #[diesel(sql_type = Text)]
    value: String,
}

impl PgControlStore {
    pub fn uid_next_sequence(&mut self, site: SiteCode) -> Result<u64, PostgresError> {
        let rows: Vec<SchemaVersion> =
            diesel::sql_query("SELECT next_sequence AS version FROM uid_sequences WHERE site = $1")
                .bind::<Text, _>(site.as_str())
                .load(&mut self.connection)?;
        Ok(rows
            .into_iter()
            .next()
            .map_or(1, |row| row.version.max(1) as u64))
    }

    pub fn uid_sightings(&mut self, site: SiteCode) -> Result<Vec<UidSighting>, PostgresError> {
        let mut sightings = Vec::new();
        let node: Vec<TextRow> = diesel::sql_query(
            "SELECT key AS value FROM nodes WHERE kind IN ('system_track', 'group')
             AND key LIKE $1 ORDER BY key DESC LIMIT 1",
        )
        .bind::<Text, _>(format!("{}%", site.as_str()))
        .load(&mut self.connection)?;
        if let Some(sequence) = node
            .first()
            .and_then(|row| highest_sequence_in(&row.value, site))
        {
            sightings.push(UidSighting::new("postgres", "nodes", sequence));
        }
        let decisions: Vec<TextRow> = diesel::sql_query(
            "SELECT evidence || ' ' || coalesce(before, '') || ' ' || coalesce(after, '') AS value
             FROM decisions WHERE evidence LIKE $1 OR before LIKE $1 OR after LIKE $1",
        )
        .bind::<Text, _>(format!("%{}%", site.as_str()))
        .load(&mut self.connection)?;
        if let Some(sequence) = decisions
            .iter()
            .filter_map(|row| highest_sequence_in(&row.value, site))
            .max()
        {
            sightings.push(UidSighting::new("postgres", "decisions", sequence));
        }
        let sync: Vec<TextRow> =
            diesel::sql_query("SELECT command AS value FROM sync_log WHERE command LIKE $1")
                .bind::<Text, _>(format!("%{}%", site.as_str()))
                .load(&mut self.connection)?;
        if let Some(sequence) = sync
            .iter()
            .filter_map(|row| highest_sequence_in(&row.value, site))
            .max()
        {
            sightings.push(UidSighting::new("postgres", "sync_log", sequence));
        }
        Ok(sightings)
    }

    pub fn advance_uid_counter(
        &mut self,
        site: SiteCode,
        sightings: &[UidSighting],
        checked: Value,
    ) -> Result<Option<UidCounterAdvanced>, PostgresError> {
        self.connection.transaction::<Option<UidCounterAdvanced>, PostgresError, _>(|connection| {
            let _: SchemaVersion = diesel::sql_query(
                "SELECT 1::bigint AS version FROM pg_advisory_xact_lock(6869400123457)",
            )
            .get_result(connection)?;
            let rows: Vec<SchemaVersion> = diesel::sql_query(
                "SELECT next_sequence AS version FROM uid_sequences WHERE site = $1",
            )
            .bind::<Text, _>(site.as_str())
            .load(connection)?;
            let before = rows.into_iter().next().map_or(1, |row| row.version.max(1) as u64);
            let found: Vec<&UidSighting> = sightings.iter()
                .filter(|sighting| sighting.sequence >= before).collect();
            let Some(top) = found.iter().map(|sighting| sighting.sequence).max() else {
                return Ok(None);
            };
            let after = top + 1;
            diesel::sql_query(
                "INSERT INTO uid_sequences (site, next_sequence) VALUES ($1, $2)
                 ON CONFLICT (site) DO UPDATE SET
                 next_sequence = greatest(uid_sequences.next_sequence, excluded.next_sequence)",
            )
            .bind::<Text, _>(site.as_str())
            .bind::<BigInt, _>(after as i64)
            .execute(connection)?;
            let found: Vec<Value> = found.iter()
                .map(|sighting| json!({
                    "store": sighting.store,
                    "location": sighting.location,
                    "uid": format!("{}{:09}", site.as_str(), sighting.sequence),
                }))
                .collect();
            let decision = Decision::new("system", "uid_counter_advanced")
                .reason("track numbers at or past the counter were already in use (a database restored from a backup?)")
                .evidence(json!({
                    "site": site.as_str(),
                    "next_sequence": { "before": before, "after": after },
                    "found": found,
                    "checked": checked,
                }))
                .before(json!({ "next_sequence": before }))
                .after(json!({ "next_sequence": after }));
            let decision_id = record_decision(connection, &decision)?;
            Ok(Some(UidCounterAdvanced { before, after, decision_id }))
        })
    }
}
