//! PostgreSQL pending sync log maintenance.

use diesel::prelude::*;
use diesel::sql_types::{BigInt, Nullable, Text};
use ot_core::SiteCode;
use ot_sync::{Entry, GlobalId, Hlc};

use super::{PgControlStore, PostgresError, record_decision};
use crate::sync::{PeerKey, SyncStatus};

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

impl PendingRow {
    fn into_entry(self) -> Result<Entry, PostgresError> {
        Ok(Entry {
            id: GlobalId {
                site: self.site.parse()?,
                seq: self.seq as u64,
            },
            hlc: Hlc::from_i64(self.hlc),
            actor: self.actor,
            role: self.role,
            command: serde_json::from_str(&self.command)?,
        })
    }
}

#[derive(QueryableByName)]
struct EntryStatusRow {
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
    #[diesel(sql_type = Text)]
    status: String,
}

impl EntryStatusRow {
    fn into_entry(self) -> Result<(Entry, SyncStatus), PostgresError> {
        let status = SyncStatus::parse(&self.status);
        let row = PendingRow {
            site: self.site,
            seq: self.seq,
            hlc: self.hlc,
            actor: self.actor,
            role: self.role,
            command: self.command,
        };
        Ok((row.into_entry()?, status))
    }
}

#[derive(QueryableByName)]
struct SeqRow {
    #[diesel(sql_type = BigInt)]
    seq: i64,
}

#[derive(QueryableByName)]
struct TextRow {
    #[diesel(sql_type = Nullable<Text>)]
    value: Option<String>,
}

#[derive(QueryableByName)]
struct SyncDecisionRow {
    #[diesel(sql_type = Nullable<Text>)]
    sync_id: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    part: Option<String>,
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

    pub fn sync_missing(
        &mut self,
        site: SiteCode,
        upto: u64,
        most: usize,
    ) -> Result<Vec<(u64, u64)>, PostgresError> {
        let rows: Vec<SeqRow> = diesel::sql_query(
            "SELECT seq FROM sync_log WHERE site = $1 AND seq <= $2 ORDER BY seq",
        )
        .bind::<Text, _>(site.as_str())
        .bind::<BigInt, _>(upto as i64)
        .load(&mut self.connection)?;
        let mut out = Vec::new();
        let mut next = 1u64;
        for seq in rows.into_iter().map(|r| r.seq as u64).chain([upto + 1]) {
            if seq > next {
                out.push((next, seq - 1));
                if out.len() == most {
                    break;
                }
            }
            next = seq + 1;
        }
        Ok(out)
    }

    pub fn sync_range(
        &mut self,
        site: SiteCode,
        from: u64,
        to: u64,
    ) -> Result<Vec<Entry>, PostgresError> {
        let rows: Vec<PendingRow> = diesel::sql_query(
            "SELECT site, seq, hlc, actor, role, command FROM sync_log WHERE site = $1 AND seq BETWEEN $2 AND $3 ORDER BY seq"
        ).bind::<Text, _>(site.as_str()).bind::<BigInt, _>(from as i64).bind::<BigInt, _>(to as i64).load(&mut self.connection)?;
        rows.into_iter().map(PendingRow::into_entry).collect()
    }

    pub fn sync_record(
        &mut self,
        e: &Entry,
        status: SyncStatus,
        detail: Option<&str>,
    ) -> Result<bool, PostgresError> {
        let changed = diesel::sql_query(
            "INSERT INTO sync_log (site, seq, hlc, actor, role, command, received_at_ms, status, detail)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9) ON CONFLICT (site, seq) DO NOTHING"
        ).bind::<Text, _>(e.id.site.as_str())
        .bind::<BigInt, _>(e.id.seq as i64).bind::<BigInt, _>(e.hlc.as_i64())
        .bind::<Text, _>(&e.actor).bind::<Text, _>(&e.role)
        .bind::<Text, _>(e.command.to_string())
        .bind::<BigInt, _>(crate::sqlite::now_ms())
        .bind::<Text, _>(status.as_str())
        .bind::<Nullable<Text>, _>(detail)
        .execute(&mut self.connection)?;
        Ok(changed == 1)
    }

    pub fn sync_set_status(
        &mut self,
        id: GlobalId,
        status: SyncStatus,
        detail: Option<&str>,
    ) -> Result<(), PostgresError> {
        diesel::sql_query(
            "UPDATE sync_log SET status = $3, detail = $4 WHERE site = $1 AND seq = $2",
        )
        .bind::<Text, _>(id.site.as_str())
        .bind::<BigInt, _>(id.seq as i64)
        .bind::<Text, _>(status.as_str())
        .bind::<Nullable<Text>, _>(detail)
        .execute(&mut self.connection)?;
        Ok(())
    }

    pub fn sync_entry(
        &mut self,
        id: GlobalId,
    ) -> Result<Option<(Entry, SyncStatus)>, PostgresError> {
        let row: Option<EntryStatusRow> = diesel::sql_query(
            "SELECT site, seq, hlc, actor, role, command, status FROM sync_log WHERE site = $1 AND seq = $2"
        ).bind::<Text, _>(id.site.as_str()).bind::<BigInt, _>(id.seq as i64)
        .get_result(&mut self.connection).optional()?;
        row.map(EntryStatusRow::into_entry).transpose()
    }

    pub fn sync_applied_after(&mut self, hlc: Hlc) -> Result<Vec<Entry>, PostgresError> {
        let rows: Vec<PendingRow> = diesel::sql_query(
            "SELECT site, seq, hlc, actor, role, command FROM sync_log WHERE status = 'applied' AND hlc > $1 ORDER BY hlc"
        ).bind::<BigInt, _>(hlc.as_i64()).load(&mut self.connection)?;
        rows.into_iter().map(PendingRow::into_entry).collect()
    }

    pub fn sync_since(
        &mut self,
        site: SiteCode,
        after: u64,
        limit: usize,
    ) -> Result<Vec<Entry>, PostgresError> {
        let rows: Vec<PendingRow> = diesel::sql_query(
            "SELECT site, seq, hlc, actor, role, command FROM sync_log WHERE site = $1 AND seq > $2 ORDER BY seq LIMIT $3"
        ).bind::<Text, _>(site.as_str()).bind::<BigInt, _>(after as i64)
        .bind::<BigInt, _>(limit as i64).load(&mut self.connection)?;
        rows.into_iter().map(PendingRow::into_entry).collect()
    }

    pub fn sync_undone_by(&mut self, id: GlobalId) -> Result<Option<GlobalId>, PostgresError> {
        let row: Option<HeadRow> = diesel::sql_query(
            "SELECT site, seq FROM sync_log WHERE command::jsonb ->> 'op' = 'undo'
             AND command::jsonb ->> 'decision' = $1 ORDER BY hlc LIMIT 1",
        )
        .bind::<Text, _>(id.to_string())
        .get_result(&mut self.connection)
        .optional()?;
        row.map(|r| {
            Ok(GlobalId {
                site: r.site.parse()?,
                seq: r.seq as u64,
            })
        })
        .transpose()
    }

    pub fn max_decision_id(&mut self) -> Result<i64, PostgresError> {
        let row: NumberRow = diesel::sql_query("SELECT max(id) AS value FROM decisions")
            .get_result(&mut self.connection)?;
        Ok(row.value.unwrap_or(0))
    }

    pub fn tag_decisions(
        &mut self,
        after: i64,
        actor: &str,
        id: GlobalId,
        parts: &[String],
    ) -> Result<Vec<i64>, PostgresError> {
        self.connection.transaction::<Vec<i64>, PostgresError, _>(|connection| {
            let rows: Vec<SeqRow> = diesel::sql_query(
                "SELECT id AS seq FROM decisions WHERE id > $1 AND actor = $2 ORDER BY id"
            ).bind::<BigInt, _>(after).bind::<Text, _>(actor).load(connection)?;
            let ids: Vec<i64> = rows.into_iter().map(|r| r.seq).collect();
            let matching_parts = (parts.len() == ids.len() && !parts.is_empty()).then_some(parts);
            for (index, decision) in ids.iter().enumerate() {
                let mut tag = serde_json::json!({ "sync": id.to_string() });
                if let Some(parts) = matching_parts { tag["sync_part"] = serde_json::Value::String(parts[index].clone()); }
                diesel::sql_query("UPDATE decisions SET evidence = (evidence::jsonb || $2::jsonb)::text WHERE id = $1")
                    .bind::<BigInt, _>(*decision).bind::<Text, _>(tag.to_string()).execute(connection)?;
            }
            Ok(ids)
        })
    }

    pub fn sync_of_decision(
        &mut self,
        decision: i64,
    ) -> Result<Option<(GlobalId, Option<String>)>, PostgresError> {
        let row: Option<SyncDecisionRow> = diesel::sql_query(
            "SELECT sync_id, evidence::jsonb ->> 'sync_part' AS part FROM decisions WHERE id = $1",
        )
        .bind::<BigInt, _>(decision)
        .get_result(&mut self.connection)
        .optional()?;
        row.and_then(|r| r.sync_id.map(|s| (s, r.part)))
            .map(|(s, part)| Ok((s.parse()?, part)))
            .transpose()
    }

    pub fn decision_of_sync(
        &mut self,
        id: GlobalId,
        part: Option<&str>,
    ) -> Result<Option<i64>, PostgresError> {
        let row: Option<SeqRow> = diesel::sql_query(
            "SELECT id AS seq FROM decisions WHERE sync_id = $1
             AND ($2::text IS NULL OR evidence::jsonb ->> 'sync_part' = $2) ORDER BY id LIMIT 1",
        )
        .bind::<Text, _>(id.to_string())
        .bind::<Nullable<Text>, _>(part)
        .get_result(&mut self.connection)
        .optional()?;
        Ok(row.map(|r| r.seq))
    }

    pub fn sync_pin_key(
        &mut self,
        site: SiteCode,
        public_key: &str,
        actor: &str,
    ) -> Result<i64, PostgresError> {
        self.connection.transaction::<i64, PostgresError, _>(|connection| {
            let row: Option<TextRow> = diesel::sql_query(
                "SELECT public_key AS value FROM sync_peer_keys WHERE site = $1"
            ).bind::<Text, _>(site.as_str()).get_result(connection).optional()?;
            let before = row.and_then(|r| r.value);
            let decision = crate::Decision::new(actor, "sync_key_pin")
                .evidence(serde_json::json!({ "site": site.as_str() }))
                .before(serde_json::json!({ "public_key": before }))
                .after(serde_json::json!({ "public_key": public_key }));
            let id = record_decision(connection, &decision)?;
            diesel::sql_query(
                "INSERT INTO sync_peer_keys (site, public_key, pinned_by, pinned_at_ms) VALUES ($1,$2,$3,$4)
                 ON CONFLICT (site) DO UPDATE SET public_key = excluded.public_key,
                 pinned_by = excluded.pinned_by, pinned_at_ms = excluded.pinned_at_ms"
            ).bind::<Text, _>(site.as_str()).bind::<Text, _>(public_key)
            .bind::<Text, _>(actor).bind::<BigInt, _>(crate::sqlite::now_ms()).execute(connection)?;
            Ok(id)
        })
    }

    pub fn sync_unpin_key(
        &mut self,
        site: SiteCode,
        actor: &str,
    ) -> Result<Option<i64>, PostgresError> {
        self.connection
            .transaction::<Option<i64>, PostgresError, _>(|connection| {
                let row: Option<TextRow> = diesel::sql_query(
                    "SELECT public_key AS value FROM sync_peer_keys WHERE site = $1",
                )
                .bind::<Text, _>(site.as_str())
                .get_result(connection)
                .optional()?;
                let Some(before) = row.and_then(|r| r.value) else {
                    return Ok(None);
                };
                let decision = crate::Decision::new(actor, "sync_key_remove")
                    .evidence(serde_json::json!({ "site": site.as_str() }))
                    .before(serde_json::json!({ "public_key": before }));
                let id = record_decision(connection, &decision)?;
                diesel::sql_query("DELETE FROM sync_peer_keys WHERE site = $1")
                    .bind::<Text, _>(site.as_str())
                    .execute(connection)?;
                Ok(Some(id))
            })
    }
}
