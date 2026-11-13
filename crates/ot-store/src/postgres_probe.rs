//! PostgreSQL probe frame preview storage.

use diesel::prelude::*;
use diesel::sql_types::{BigInt, Binary, Nullable, Text};

use super::{PgControlStore, PostgresError};
use crate::probe::Sample;

#[derive(QueryableByName)]
struct SampleRow {
    #[diesel(sql_type = BigInt)]
    captured_at_ms: i64,
    #[diesel(sql_type = Binary)]
    frame: Vec<u8>,
    #[diesel(sql_type = Nullable<Text>)]
    meta: Option<String>,
}

impl PgControlStore {
    pub fn store_probe_samples(
        &mut self,
        source_id: &str,
        frames: &[(Vec<u8>, Option<serde_json::Value>)],
        cap: usize,
    ) -> Result<usize, PostgresError> {
        self.connection.transaction::<usize, PostgresError, _>(|connection| {
            diesel::sql_query("DELETE FROM probe_samples WHERE source_id = $1")
                .bind::<Text, _>(source_id).execute(connection)?;
            let now = crate::sqlite::now_ms();
            let mut count = 0;
            for (bytes, meta) in frames.iter().take(cap) {
                diesel::sql_query("INSERT INTO probe_samples (source_id, captured_at_ms, frame, meta) VALUES ($1,$2,$3,$4)")
                    .bind::<Text, _>(source_id).bind::<BigInt, _>(now)
                    .bind::<Binary, _>(bytes).bind::<Nullable<Text>, _>(meta.as_ref().map(|m| m.to_string()))
                    .execute(connection)?;
                count += 1;
            }
            Ok(count)
        })
    }

    pub fn probe_samples(
        &mut self,
        source_id: &str,
        limit: usize,
    ) -> Result<Vec<Sample>, PostgresError> {
        let rows: Vec<SampleRow> = diesel::sql_query(
            "SELECT captured_at_ms, frame, meta FROM probe_samples WHERE source_id = $1 ORDER BY id LIMIT $2"
        ).bind::<Text, _>(source_id).bind::<BigInt, _>(limit as i64).load(&mut self.connection)?;
        Ok(rows
            .into_iter()
            .map(|row| Sample {
                captured_at_ms: row.captured_at_ms,
                bytes: row.frame,
                meta: row.meta.and_then(|m| serde_json::from_str(&m).ok()),
            })
            .collect())
    }
}
