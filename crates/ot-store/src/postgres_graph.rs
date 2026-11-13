//! PostgreSQL track graph reads used by the engine and API.

use diesel::prelude::*;
use diesel::sql_types::{BigInt, Nullable, Text};
use ot_core::{SiteCode, Uid};
use serde_json::{Value, json};

use super::{PgControlStore, PostgresError, record_decision};
use crate::Decision;
use crate::graph::{self, EdgeKind, EdgeRecord, NodeKind};

#[derive(QueryableByName)]
struct IdRow {
    #[diesel(sql_type = BigInt)]
    id: i64,
}

#[derive(QueryableByName)]
struct SequenceRow {
    #[diesel(sql_type = BigInt)]
    sequence: i64,
}

#[derive(QueryableByName)]
struct KindRow {
    #[diesel(sql_type = Text)]
    kind: String,
}

#[derive(QueryableByName)]
struct MovedEdge {
    #[diesel(sql_type = BigInt)]
    src: i64,
    #[diesel(sql_type = Text)]
    attrs: String,
}

#[derive(QueryableByName)]
struct CarriedEdge {
    #[diesel(sql_type = Text)]
    kind: String,
    #[diesel(sql_type = BigInt)]
    other: i64,
}

#[derive(QueryableByName)]
struct ExistsRow {
    #[diesel(sql_type = diesel::sql_types::Bool)]
    exists: bool,
}

pub(super) fn upsert_node(
    connection: &mut diesel::pg::PgConnection,
    kind: NodeKind,
    key: &str,
    now: i64,
) -> Result<i64, PostgresError> {
    let row: IdRow = diesel::sql_query(
        "INSERT INTO nodes (kind, key, created_at_ms) VALUES ($1, $2, $3)
         ON CONFLICT (kind, key) DO UPDATE SET kind = excluded.kind RETURNING id",
    )
    .bind::<Text, _>(kind.as_str())
    .bind::<Text, _>(key)
    .bind::<BigInt, _>(now)
    .get_result(connection)?;
    Ok(row.id)
}

pub(super) fn live_system_node(
    connection: &mut diesel::pg::PgConnection,
    uid: Uid,
) -> Result<i64, PostgresError> {
    let rows: Vec<IdRow> = diesel::sql_query(
        "SELECT id FROM nodes WHERE kind = 'system_track' AND key = $1
         AND retired_at_ms IS NULL",
    )
    .bind::<Text, _>(uid.to_string())
    .load(connection)?;
    rows.into_iter()
        .next()
        .map(|row| row.id)
        .ok_or_else(|| PostgresError::NotFound(format!("live system track {uid}")))
}

pub(super) fn add_edge(
    connection: &mut diesel::pg::PgConnection,
    kind: EdgeKind,
    src: i64,
    dst: i64,
    decision_id: i64,
    attrs: &Value,
    now: i64,
) -> Result<i64, PostgresError> {
    let (want_src, want_dst) = match kind {
        EdgeKind::ReportsFor | EdgeKind::CandidateOf => {
            (NodeKind::SourceTrack, NodeKind::SystemTrack)
        }
        EdgeKind::MergedInto | EdgeKind::PairedWith => {
            (NodeKind::SystemTrack, NodeKind::SystemTrack)
        }
        EdgeKind::DoNotPair => (NodeKind::SourceTrack, NodeKind::SourceTrack),
        EdgeKind::Carries => (NodeKind::SourceTrack, NodeKind::Identifier),
        EdgeKind::ResolvesTo => (NodeKind::Identifier, NodeKind::Entity),
        EdgeKind::MemberOf => (NodeKind::SystemTrack, NodeKind::Group),
    };
    for (node, want) in [(src, want_src), (dst, want_dst)] {
        let row: KindRow = diesel::sql_query("SELECT kind FROM nodes WHERE id = $1")
            .bind::<BigInt, _>(node)
            .get_result(connection)?;
        if row.kind != want.as_str() {
            return Err(PostgresError::Conflict(format!(
                "{} cannot connect a {} node (expected {})",
                kind.as_str(),
                row.kind,
                want.as_str()
            )));
        }
    }
    let row: IdRow = diesel::sql_query(
        "INSERT INTO edges (kind, src, dst, attrs, valid_from_ms, decision_id)
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
    )
    .bind::<Text, _>(kind.as_str())
    .bind::<BigInt, _>(src)
    .bind::<BigInt, _>(dst)
    .bind::<Text, _>(attrs.to_string())
    .bind::<BigInt, _>(now)
    .bind::<BigInt, _>(decision_id)
    .get_result(connection)?;
    Ok(row.id)
}

fn open_system_track(
    connection: &mut diesel::pg::PgConnection,
    uid: Uid,
    source_id: &str,
    source_track_key: &str,
    decision: &Decision,
) -> Result<i64, PostgresError> {
    let now = crate::sqlite::now_ms();
    let decision_id = record_decision(connection, decision)?;
    let sys = upsert_node(connection, NodeKind::SystemTrack, &uid.to_string(), now)?;
    diesel::sql_query("UPDATE nodes SET retired_at_ms = NULL WHERE id = $1")
        .bind::<BigInt, _>(sys)
        .execute(connection)?;
    let src = upsert_node(
        connection,
        NodeKind::SourceTrack,
        &graph::source_track_key(source_id, source_track_key),
        now,
    )?;
    add_edge(
        connection,
        EdgeKind::ReportsFor,
        src,
        sys,
        decision_id,
        &json!({"pairing": "auto", "confidence": 1.0}),
        now,
    )?;
    Ok(decision_id)
}

#[derive(QueryableByName)]
struct ReportRow {
    #[diesel(sql_type = Text)]
    source: String,
    #[diesel(sql_type = Text)]
    uid: String,
}

#[derive(QueryableByName)]
struct PairRow {
    #[diesel(sql_type = Text)]
    a: String,
    #[diesel(sql_type = Text)]
    b: String,
}

#[derive(QueryableByName)]
struct EdgeRow {
    #[diesel(sql_type = BigInt)]
    id: i64,
    #[diesel(sql_type = Text)]
    kind: String,
    #[diesel(sql_type = Text)]
    src_key: String,
    #[diesel(sql_type = Text)]
    dst_key: String,
    #[diesel(sql_type = Text)]
    attrs: String,
    #[diesel(sql_type = Text)]
    src_attrs: String,
    #[diesel(sql_type = BigInt)]
    valid_from_ms: i64,
    #[diesel(sql_type = Nullable<BigInt>)]
    valid_to_ms: Option<i64>,
    #[diesel(sql_type = BigInt)]
    decision_id: i64,
    #[diesel(sql_type = Text)]
    decision_op: String,
    #[diesel(sql_type = Text)]
    decision_actor: String,
    #[diesel(sql_type = Nullable<Text>)]
    decision_reason: Option<String>,
    #[diesel(sql_type = Nullable<BigInt>)]
    ended_by: Option<i64>,
}

#[derive(QueryableByName)]
struct DecisionRow {
    #[diesel(sql_type = BigInt)]
    id: i64,
    #[diesel(sql_type = BigInt)]
    at_ms: i64,
    #[diesel(sql_type = Text)]
    actor: String,
    #[diesel(sql_type = Text)]
    op: String,
    #[diesel(sql_type = Nullable<Text>)]
    reason: Option<String>,
    #[diesel(sql_type = Text)]
    evidence: String,
}

impl PgControlStore {
    pub fn do_not_pairs(&mut self) -> Result<Vec<(String, String)>, PostgresError> {
        let rows: Vec<PairRow> = diesel::sql_query(
            "SELECT a.key AS a, b.key AS b FROM edges e
             JOIN nodes a ON a.id = e.src JOIN nodes b ON b.id = e.dst
             WHERE e.kind = 'DO_NOT_PAIR' AND e.valid_to_ms IS NULL",
        )
        .load(&mut self.connection)?;
        Ok(rows.into_iter().map(|row| (row.a, row.b)).collect())
    }
    pub fn create_system_track(
        &mut self,
        site: SiteCode,
        source_id: &str,
        source_track_key: &str,
        decision: &Decision,
    ) -> Result<(Uid, i64), PostgresError> {
        self.connection
            .transaction::<(Uid, i64), PostgresError, _>(|connection| {
                let row: SequenceRow = diesel::sql_query(
                    "INSERT INTO uid_sequences (site, next_sequence) VALUES ($1, 2)
                 ON CONFLICT (site) DO UPDATE SET next_sequence = uid_sequences.next_sequence + 1
                 RETURNING next_sequence - 1 AS sequence",
                )
                .bind::<Text, _>(site.as_str())
                .get_result(connection)?;
                let uid = Uid::new(site, row.sequence as u64)?;
                let decision_id =
                    open_system_track(connection, uid, source_id, source_track_key, decision)?;
                Ok((uid, decision_id))
            })
    }

    pub fn adopt_system_track(
        &mut self,
        uid: Uid,
        source_id: &str,
        source_track_key: &str,
        decision: &Decision,
    ) -> Result<i64, PostgresError> {
        self.connection
            .transaction::<i64, PostgresError, _>(|connection| {
                open_system_track(connection, uid, source_id, source_track_key, decision)
            })
    }

    pub fn pair_source_track(
        &mut self,
        source_id: &str,
        source_track_key: &str,
        uid: Uid,
        attrs: &Value,
        decision: &Decision,
    ) -> Result<i64, PostgresError> {
        self.connection
            .transaction::<i64, PostgresError, _>(|connection| {
                let now = crate::sqlite::now_ms();
                let sys = live_system_node(connection, uid)?;
                let decision_id = record_decision(connection, decision)?;
                let src = upsert_node(
                    connection,
                    NodeKind::SourceTrack,
                    &graph::source_track_key(source_id, source_track_key),
                    now,
                )?;
                diesel::sql_query(
                    "UPDATE edges SET valid_to_ms = greatest($2, valid_from_ms), ended_by = $3
                 WHERE src = $1 AND kind = 'REPORTS_FOR' AND valid_to_ms IS NULL",
                )
                .bind::<BigInt, _>(src)
                .bind::<BigInt, _>(now)
                .bind::<BigInt, _>(decision_id)
                .execute(connection)?;
                add_edge(
                    connection,
                    EdgeKind::ReportsFor,
                    src,
                    sys,
                    decision_id,
                    attrs,
                    now,
                )?;
                Ok(decision_id)
            })
    }

    pub fn note_source_track(
        &mut self,
        source_id: &str,
        source_track_key: &str,
        attrs: &Value,
    ) -> Result<(), PostgresError> {
        self.connection
            .transaction::<(), PostgresError, _>(|connection| {
                let now = crate::sqlite::now_ms();
                let node = upsert_node(
                    connection,
                    NodeKind::SourceTrack,
                    &graph::source_track_key(source_id, source_track_key),
                    now,
                )?;
                diesel::sql_query(
                    "UPDATE nodes SET attrs = (attrs::jsonb || $2::jsonb)::text WHERE id = $1",
                )
                .bind::<BigInt, _>(node)
                .bind::<Text, _>(attrs.to_string())
                .execute(connection)?;
                Ok(())
            })
    }

    pub fn end_source_track(
        &mut self,
        source_id: &str,
        source_track_key: &str,
        decision: &Decision,
    ) -> Result<Option<i64>, PostgresError> {
        self.connection
            .transaction::<Option<i64>, PostgresError, _>(|connection| {
                let key = graph::source_track_key(source_id, source_track_key);
                let rows: Vec<IdRow> = diesel::sql_query(
                    "SELECT n.id FROM nodes n JOIN edges e ON e.src = n.id
                 WHERE n.kind = 'source_track' AND n.key = $1
                   AND e.kind = 'REPORTS_FOR' AND e.valid_to_ms IS NULL LIMIT 1",
                )
                .bind::<Text, _>(&key)
                .load(connection)?;
                let Some(src) = rows.into_iter().next().map(|row| row.id) else {
                    return Ok(None);
                };
                let now = crate::sqlite::now_ms();
                let decision_id = record_decision(connection, decision)?;
                diesel::sql_query(
                    "UPDATE edges SET valid_to_ms = greatest($2, valid_from_ms), ended_by = $3
                 WHERE src = $1 AND kind = 'REPORTS_FOR' AND valid_to_ms IS NULL",
                )
                .bind::<BigInt, _>(src)
                .bind::<BigInt, _>(now)
                .bind::<BigInt, _>(decision_id)
                .execute(connection)?;
                Ok(Some(decision_id))
            })
    }

    pub fn retire_system_track(
        &mut self,
        uid: Uid,
        decision: &Decision,
    ) -> Result<i64, PostgresError> {
        self.connection
            .transaction::<i64, PostgresError, _>(|connection| {
                let now = crate::sqlite::now_ms();
                let node = live_system_node(connection, uid)?;
                let decision_id = record_decision(connection, decision)?;
                diesel::sql_query(
                    "UPDATE edges SET valid_to_ms = greatest($2, valid_from_ms), ended_by = $3
                 WHERE (src = $1 OR dst = $1) AND valid_to_ms IS NULL",
                )
                .bind::<BigInt, _>(node)
                .bind::<BigInt, _>(now)
                .bind::<BigInt, _>(decision_id)
                .execute(connection)?;
                diesel::sql_query("UPDATE nodes SET retired_at_ms = $2 WHERE id = $1")
                    .bind::<BigInt, _>(node)
                    .bind::<BigInt, _>(now)
                    .execute(connection)?;
                Ok(decision_id)
            })
    }

    pub fn merge_system_tracks(
        &mut self,
        from: Uid,
        into: Uid,
        decision: &Decision,
    ) -> Result<i64, PostgresError> {
        if from == into {
            return Err(PostgresError::Conflict(format!(
                "cannot merge {from} into itself"
            )));
        }
        self.connection
            .transaction::<i64, PostgresError, _>(|connection| {
                let now = crate::sqlite::now_ms();
                let from_node = live_system_node(connection, from)?;
                let into_node = live_system_node(connection, into)?;
                let decision_id = record_decision(connection, decision)?;
                let carried: Vec<CarriedEdge> = diesel::sql_query(
                    "SELECT kind, CASE WHEN src = $1 THEN dst ELSE src END AS other
                 FROM edges WHERE (src = $1 OR dst = $1)
                   AND kind IN ('MEMBER_OF', 'PAIRED_WITH') AND valid_to_ms IS NULL",
                )
                .bind::<BigInt, _>(from_node)
                .load(connection)?;
                let moved: Vec<MovedEdge> = diesel::sql_query(
                    "SELECT src, attrs FROM edges WHERE dst = $1
                 AND kind = 'REPORTS_FOR' AND valid_to_ms IS NULL",
                )
                .bind::<BigInt, _>(from_node)
                .load(connection)?;
                diesel::sql_query(
                    "UPDATE edges SET valid_to_ms = greatest($2, valid_from_ms), ended_by = $3
                 WHERE (src = $1 OR dst = $1) AND valid_to_ms IS NULL",
                )
                .bind::<BigInt, _>(from_node)
                .bind::<BigInt, _>(now)
                .bind::<BigInt, _>(decision_id)
                .execute(connection)?;
                for edge in moved {
                    let attrs = serde_json::from_str(&edge.attrs).unwrap_or(Value::Null);
                    add_edge(
                        connection,
                        EdgeKind::ReportsFor,
                        edge.src,
                        into_node,
                        decision_id,
                        &attrs,
                        now,
                    )?;
                }
                add_edge(
                    connection,
                    EdgeKind::MergedInto,
                    from_node,
                    into_node,
                    decision_id,
                    &json!({}),
                    now,
                )?;
                for edge in carried {
                    let pairing = edge.kind == "PAIRED_WITH";
                    if pairing && edge.other == into_node {
                        continue;
                    }
                    let exists: ExistsRow = diesel::sql_query(
                        "SELECT EXISTS (SELECT 1 FROM edges WHERE kind = $1
                     AND valid_to_ms IS NULL AND
                     ((src = $2 AND dst = $3) OR (src = $3 AND dst = $2))) AS exists",
                    )
                    .bind::<Text, _>(&edge.kind)
                    .bind::<BigInt, _>(into_node)
                    .bind::<BigInt, _>(edge.other)
                    .get_result(connection)?;
                    if !exists.exists {
                        let kind = if pairing {
                            EdgeKind::PairedWith
                        } else {
                            EdgeKind::MemberOf
                        };
                        add_edge(
                            connection,
                            kind,
                            into_node,
                            edge.other,
                            decision_id,
                            &json!({}),
                            now,
                        )?;
                    }
                }
                diesel::sql_query("UPDATE nodes SET retired_at_ms = $2 WHERE id = $1")
                    .bind::<BigInt, _>(from_node)
                    .bind::<BigInt, _>(now)
                    .execute(connection)?;
                Ok(decision_id)
            })
    }
    pub fn live_reports(&mut self) -> Result<Vec<(String, Uid)>, PostgresError> {
        let rows: Vec<ReportRow> = diesel::sql_query(
            "SELECT s.key AS source, d.key AS uid FROM edges e
             JOIN nodes s ON s.id = e.src JOIN nodes d ON d.id = e.dst
             WHERE e.kind = 'REPORTS_FOR' AND e.valid_to_ms IS NULL",
        )
        .load(&mut self.connection)?;
        rows.into_iter()
            .map(|row| Ok((row.source, row.uid.parse()?)))
            .collect()
    }

    pub fn explain(&mut self, uid: Uid) -> Result<Vec<EdgeRecord>, PostgresError> {
        let rows: Vec<EdgeRow> = diesel::sql_query(
            "WITH RECURSIVE family(id) AS (
                 SELECT id FROM nodes WHERE kind = 'system_track' AND key = $1
                 UNION SELECT e.src FROM edges e JOIN family f ON e.dst = f.id
                 WHERE e.kind = 'MERGED_INTO'
             )
             SELECT e.id, e.kind, s.key AS src_key, d.key AS dst_key, e.attrs,
                    s.attrs AS src_attrs, e.valid_from_ms, e.valid_to_ms,
                    e.decision_id, dec.op AS decision_op, dec.actor AS decision_actor,
                    dec.reason AS decision_reason, e.ended_by
             FROM edges e JOIN nodes s ON s.id = e.src
             JOIN nodes d ON d.id = e.dst JOIN decisions dec ON dec.id = e.decision_id
             WHERE e.src IN (SELECT id FROM family) OR e.dst IN (SELECT id FROM family)
             ORDER BY e.valid_from_ms, e.id",
        )
        .bind::<Text, _>(uid.to_string())
        .load(&mut self.connection)?;
        rows.into_iter()
            .map(|row| {
                Ok(EdgeRecord {
                    id: row.id,
                    kind: EdgeKind::parse(&row.kind).ok_or_else(|| {
                        PostgresError::Conflict(format!("unknown edge kind {}", row.kind))
                    })?,
                    src_key: row.src_key,
                    dst_key: row.dst_key,
                    attrs: serde_json::from_str(&row.attrs)?,
                    src_attrs: serde_json::from_str(&row.src_attrs)?,
                    valid_from_ms: row.valid_from_ms,
                    valid_to_ms: row.valid_to_ms,
                    decision_id: row.decision_id,
                    decision_op: row.decision_op,
                    decision_actor: row.decision_actor,
                    decision_reason: row.decision_reason,
                    ended_by: row.ended_by,
                })
            })
            .collect()
    }

    pub fn decision(&mut self, id: i64) -> Result<Option<Value>, PostgresError> {
        let rows: Vec<DecisionRow> = diesel::sql_query(
            "SELECT id, at_ms, actor, op, reason, evidence FROM decisions WHERE id = $1",
        )
        .bind::<BigInt, _>(id)
        .load(&mut self.connection)?;
        rows.into_iter()
            .next()
            .map(|row| {
                Ok(json!({
                    "id": row.id,
                    "at_ms": row.at_ms,
                    "actor": row.actor,
                    "op": row.op,
                    "reason": row.reason,
                    "evidence": serde_json::from_str::<Value>(&row.evidence)?,
                }))
            })
            .transpose()
    }
}
