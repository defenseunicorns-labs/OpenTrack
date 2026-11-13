//! PostgreSQL pending sync log maintenance.

use diesel::prelude::*;
use diesel::sql_types::{BigInt, Nullable, Text};
use ot_core::SiteCode;
use ot_sync::{Entry, GlobalId, Hlc};

use super::{PgControlStore, PostgresError};
use crate::sync::PeerKey;

#[derive(QueryableByName)]
struct NumberRow {
    #[diesel(sql_type = Nullable<BigInt>)]
    value: Option<i64>,
}

#[derive(QueryableByName)]
struct PeerKeyRow {
    #[diesel(sql_type = Text)]
    site: String,
    #[diesel(sql_type = Text)]
    public_key: String,
    #[diesel(sql_type = Text)]
    pinned_by: String,
    #[diesel(sql_type = BigInt)]
    pinned_at_ms: i64,
}

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

#[derive(QueryableByName)]
struct StatusCountRow {
    #[diesel(sql_type = Text)]
    status: String,
    #[diesel(sql_type = BigInt)]
    count: i64,
}

#[derive(QueryableByName)]
struct HeadRow {
    #[diesel(sql_type = Text)]
    site: String,
    #[diesel(sql_type = BigInt)]
    seq: i64,
}

impl PgControlStore {
    pub fn sync_counts(
        &mut self,
    ) -> Result<std::collections::BTreeMap<String, u64>, PostgresError> {
        let rows: Vec<StatusCountRow> =
            diesel::sql_query("SELECT status, count(*) AS count FROM sync_log GROUP BY status")
                .load(&mut self.connection)?;
        Ok(rows
            .into_iter()
            .map(|row| (row.status, row.count as u64))
            .collect())
    }

    pub fn sync_heads(&mut self) -> Result<Vec<(SiteCode, u64)>, PostgresError> {
        let rows: Vec<HeadRow> = diesel::sql_query(
            "SELECT site, max(seq) AS seq FROM sync_log GROUP BY site ORDER BY site",
        )
        .load(&mut self.connection)?;
        Ok(rows
            .into_iter()
            .filter_map(|row| Some((row.site.parse().ok()?, row.seq as u64)))
            .collect())
    }

    pub fn sync_last_hlc(&mut self) -> Result<Hlc, PostgresError> {
        let row: NumberRow = diesel::sql_query("SELECT max(hlc) AS value FROM sync_log")
            .get_result(&mut self.connection)?;
        Ok(Hlc::from_i64(row.value.unwrap_or(0)))
    }

    pub fn sync_append_own(
        &mut self,
        site: SiteCode,
        actor: &str,
        role: &str,
        command: &serde_json::Value,
        now_ms: u64,
    ) -> Result<Entry, PostgresError> {
        self.connection
            .transaction::<Entry, PostgresError, _>(|connection| {
                let _: super::SchemaVersion = diesel::sql_query(
                    "SELECT 1::bigint AS version FROM pg_advisory_xact_lock(6869400123459)",
                )
                .get_result(connection)?;
                let seq: NumberRow = diesel::sql_query(
                "SELECT (coalesce(max(seq), 0) + 1)::bigint AS value FROM sync_log WHERE site = $1",
            ).bind::<Text, _>(site.as_str()).get_result(connection)?;
                let last: NumberRow = diesel::sql_query("SELECT max(hlc) AS value FROM sync_log")
                    .get_result(connection)?;
                let seq = seq.value.unwrap_or(1);
                let hlc =
                    ot_sync::Clock::after(Hlc::from_i64(last.value.unwrap_or(0))).tick(now_ms);
                diesel::sql_query(
                    "INSERT INTO sync_log
                 (site, seq, hlc, actor, role, command, received_at_ms, status)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, 'applied')",
                )
                .bind::<Text, _>(site.as_str())
                .bind::<BigInt, _>(seq)
                .bind::<BigInt, _>(hlc.as_i64())
                .bind::<Text, _>(actor)
                .bind::<Text, _>(role)
                .bind::<Text, _>(command.to_string())
                .bind::<BigInt, _>(now_ms as i64)
                .execute(connection)?;
                Ok(Entry {
                    id: GlobalId {
                        site,
                        seq: seq as u64,
                    },
                    hlc,
                    actor: actor.to_owned(),
                    role: role.to_owned(),
                    command: command.clone(),
                })
            })
    }

    pub fn sync_next_seq(&mut self, site: SiteCode) -> Result<u64, PostgresError> {
        let row: NumberRow =
            diesel::sql_query("SELECT max(seq) AS value FROM sync_log WHERE site = $1")
                .bind::<Text, _>(site.as_str())
                .get_result(&mut self.connection)?;
        Ok(row.value.unwrap_or(0) as u64 + 1)
    }

    pub fn sync_peer_keys(&mut self) -> Result<Vec<PeerKey>, PostgresError> {
        let rows: Vec<PeerKeyRow> = diesel::sql_query(
            "SELECT site, public_key, pinned_by, pinned_at_ms FROM sync_peer_keys ORDER BY site",
        )
        .load(&mut self.connection)?;
        Ok(rows
            .into_iter()
            .map(|row| PeerKey {
                site: row.site,
                public_key: row.public_key,
                pinned_by: row.pinned_by,
                pinned_at_ms: row.pinned_at_ms,
            })
            .collect())
    }

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
