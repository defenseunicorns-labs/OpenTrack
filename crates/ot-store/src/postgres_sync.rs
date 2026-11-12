//! PostgreSQL pending sync log maintenance.

use diesel::prelude::*;
use diesel::sql_types::{BigInt, Text};
use ot_sync::{Entry, GlobalId, Hlc};

use super::{PgControlStore, PostgresError};

#[derive(QueryableByName)]
struct PendingRow {
    #[diesel(sql_type = Text)]
    site: String,
    #[diesel(sql_type = BigInt)]
    seq: i64,
    #[diesel(sql_type = BigInt)]
    hlc: i64,
    #[diesel(sql_type = Text)]
    actor: String,
    #[diesel(sql_type = Text)]
    role: String,
    #[diesel(sql_type = Text)]
    command: String,
}

impl PgControlStore {
    pub fn sync_pending(&mut self) -> Result<Vec<Entry>, PostgresError> {
        let rows: Vec<PendingRow> = diesel::sql_query(
            "SELECT site, seq, hlc, actor, role, command FROM sync_log
             WHERE status = 'pending' ORDER BY hlc",
        )
        .load(&mut self.connection)?;
        rows.into_iter()
            .map(|row| {
                Ok(Entry {
                    id: GlobalId {
                        site: row.site.parse()?,
                        seq: row.seq as u64,
                    },
                    hlc: Hlc::from_i64(row.hlc),
                    actor: row.actor,
                    role: row.role,
                    command: serde_json::from_str(&row.command)?,
                })
            })
            .collect()
    }

    pub fn sync_expire_pending(&mut self, before_ms: i64) -> Result<usize, PostgresError> {
        let changed = diesel::sql_query(
            "UPDATE sync_log SET status = 'failed', detail = 'its tracks never arrived'
             WHERE status = 'pending' AND received_at_ms < $1",
        )
        .bind::<BigInt, _>(before_ms)
        .execute(&mut self.connection)?;
        Ok(changed)
    }
}
