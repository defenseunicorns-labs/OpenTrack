//! The temporal track graph: typed nodes and edges in SQLite.
//!
//! Every edge carries `valid_from`/`valid_to` and the decisions that opened
//! and closed it. Nothing is deleted: ending a link closes its interval, so
//! the picture at any instant can be reconstructed and explained.

use rusqlite::{Connection, Transaction, params};
use serde::Serialize;
use serde_json::Value;

use crate::sqlite::{Result, StoreError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    SourceTrack,
    SystemTrack,
    Identifier,
    Entity,
    Group,
}

impl NodeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            NodeKind::SourceTrack => "source_track",
            NodeKind::SystemTrack => "system_track",
            NodeKind::Identifier => "identifier",
            NodeKind::Entity => "entity",
            NodeKind::Group => "group",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EdgeKind {
    /// SourceTrack → SystemTrack.
    ReportsFor,
    /// SystemTrack → SystemTrack; the source stays resolvable as an alias.
    MergedInto,
    /// SourceTrack → SystemTrack, with the score and its breakdown.
    CandidateOf,
    /// SourceTrack → SourceTrack, from a split or rejection.
    DoNotPair,
    /// SourceTrack → Identifier.
    Carries,
    /// Identifier → Entity.
    ResolvesTo,
    /// SystemTrack → Group.
    MemberOf,
    /// SystemTrack → SystemTrack: an operator's GOLD PAIR (the same object,
    /// kept as separate tracks); lower UID first.
    PairedWith,
}

impl EdgeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EdgeKind::ReportsFor => "REPORTS_FOR",
            EdgeKind::MergedInto => "MERGED_INTO",
            EdgeKind::CandidateOf => "CANDIDATE_OF",
            EdgeKind::DoNotPair => "DO_NOT_PAIR",
            EdgeKind::Carries => "CARRIES",
            EdgeKind::ResolvesTo => "RESOLVES_TO",
            EdgeKind::MemberOf => "MEMBER_OF",
            EdgeKind::PairedWith => "PAIRED_WITH",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "REPORTS_FOR" => EdgeKind::ReportsFor,
            "MERGED_INTO" => EdgeKind::MergedInto,
            "CANDIDATE_OF" => EdgeKind::CandidateOf,
            "DO_NOT_PAIR" => EdgeKind::DoNotPair,
            "CARRIES" => EdgeKind::Carries,
            "RESOLVES_TO" => EdgeKind::ResolvesTo,
            "MEMBER_OF" => EdgeKind::MemberOf,
            "PAIRED_WITH" => EdgeKind::PairedWith,
            _ => return None,
        })
    }

    /// The node kinds this edge may connect.
    fn endpoints(self) -> (NodeKind, NodeKind) {
        use NodeKind::*;
        match self {
            EdgeKind::ReportsFor | EdgeKind::CandidateOf => (SourceTrack, SystemTrack),
            EdgeKind::MergedInto | EdgeKind::PairedWith => (SystemTrack, SystemTrack),
            EdgeKind::DoNotPair => (SourceTrack, SourceTrack),
            EdgeKind::Carries => (SourceTrack, Identifier),
            EdgeKind::ResolvesTo => (Identifier, Entity),
            EdgeKind::MemberOf => (SystemTrack, Group),
        }
    }
}

/// Graph key of a source track node.
pub fn source_track_key(source_id: &str, source_track_key: &str) -> String {
    format!("{source_id}/{source_track_key}")
}

/// Return the id of the node (`kind`, `key`), creating it if needed.
pub fn upsert_node(tx: &Transaction<'_>, kind: NodeKind, key: &str, now_ms: i64) -> Result<i64> {
    Ok(tx.query_row(
        "INSERT INTO nodes (kind, key, created_at_ms) VALUES (?1, ?2, ?3)
         ON CONFLICT(kind, key) DO UPDATE SET kind = excluded.kind
         RETURNING id",
        params![kind.as_str(), key, now_ms],
        |r| r.get(0),
    )?)
}

/// Merge `attrs` (an object) into a node's attributes.
pub fn merge_node_attrs(tx: &Transaction<'_>, node: i64, attrs: &Value) -> Result<()> {
    tx.execute(
        "UPDATE nodes SET attrs = json_patch(attrs, ?2) WHERE id = ?1",
        params![node, attrs.to_string()],
    )?;
    Ok(())
}

/// Open a new edge. Endpoint kinds are checked against the edge kind.
pub fn add_edge(
    tx: &Transaction<'_>,
    kind: EdgeKind,
    src: i64,
    dst: i64,
    decision_id: i64,
    attrs: &Value,
    now_ms: i64,
) -> Result<i64> {
    let (want_src, want_dst) = kind.endpoints();
    for (node, want) in [(src, want_src), (dst, want_dst)] {
        let got: String =
            tx.query_row("SELECT kind FROM nodes WHERE id = ?1", [node], |r| r.get(0))?;
        if got != want.as_str() {
            return Err(StoreError::Conflict(format!(
                "{} cannot connect a {got} node (expected {})",
                kind.as_str(),
                want.as_str()
            )));
        }
    }
    tx.execute(
        "INSERT INTO edges (kind, src, dst, attrs, valid_from_ms, decision_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            kind.as_str(),
            src,
            dst,
            attrs.to_string(),
            now_ms,
            decision_id
        ],
    )?;
    Ok(tx.last_insert_rowid())
}

/// Close a live edge's validity interval.
pub fn end_edge(tx: &Transaction<'_>, edge_id: i64, decision_id: i64, now_ms: i64) -> Result<()> {
    let n = tx.execute(
        "UPDATE edges SET valid_to_ms = max(?2, valid_from_ms), ended_by = ?3
         WHERE id = ?1 AND valid_to_ms IS NULL",
        params![edge_id, now_ms, decision_id],
    )?;
    if n == 0 {
        return Err(StoreError::NotFound(format!("live edge {edge_id}")));
    }
    Ok(())
}

/// One edge, joined to its endpoint keys and opening decision.
#[derive(Debug, Clone, Serialize)]
pub struct EdgeRecord {
    pub id: i64,
    pub kind: EdgeKind,
    pub src_key: String,
    pub dst_key: String,
    pub attrs: Value,
    /// The source node's own attributes, e.g. the `tracker` that formed a
    /// source track from detections.
    pub src_attrs: Value,
    pub valid_from_ms: i64,
    pub valid_to_ms: Option<i64>,
    pub decision_id: i64,
    pub decision_op: String,
    pub decision_actor: String,
    /// Why the opening decision was made.
    pub decision_reason: Option<String>,
    pub ended_by: Option<i64>,
}

/// Every edge, live or ended, touching a system track or any track merged
/// into it (transitively), oldest first.
pub fn explain_system_track(conn: &Connection, uid: &str) -> Result<Vec<EdgeRecord>> {
    let mut stmt = conn.prepare(
        "WITH RECURSIVE family(id) AS (
             SELECT id FROM nodes WHERE kind = 'system_track' AND key = ?1
             UNION
             SELECT e.src FROM edges e JOIN family f ON e.dst = f.id
             WHERE e.kind = 'MERGED_INTO'
         )
         SELECT e.id, e.kind, s.key, d.key, e.attrs, e.valid_from_ms, e.valid_to_ms,
                e.decision_id, dec.op, dec.actor, e.ended_by, dec.reason, s.attrs
         FROM edges e
         JOIN nodes s ON s.id = e.src
         JOIN nodes d ON d.id = e.dst
         JOIN decisions dec ON dec.id = e.decision_id
         WHERE e.src IN (SELECT id FROM family) OR e.dst IN (SELECT id FROM family)
         ORDER BY e.valid_from_ms, e.id",
    )?;
    let rows = stmt.query_map([uid], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, i64>(5)?,
            r.get::<_, Option<i64>>(6)?,
            r.get::<_, i64>(7)?,
            r.get::<_, String>(8)?,
            r.get::<_, String>(9)?,
            r.get::<_, Option<i64>>(10)?,
            r.get::<_, Option<String>>(11)?,
            r.get::<_, String>(12)?,
        ))
    })?;
    rows.map(|row| {
        let (id, kind, src_key, dst_key, attrs, from, to, dec, op, actor, ended, reason, src_attrs) =
            row?;
        Ok(EdgeRecord {
            id,
            kind: EdgeKind::parse(&kind)
                .ok_or_else(|| StoreError::Conflict(format!("unknown edge kind {kind}")))?,
            src_key,
            dst_key,
            attrs: serde_json::from_str(&attrs)?,
            src_attrs: serde_json::from_str(&src_attrs)?,
            valid_from_ms: from,
            valid_to_ms: to,
            decision_id: dec,
            decision_op: op,
            decision_actor: actor,
            decision_reason: reason,
            ended_by: ended,
        })
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::{Db, Decision, record_decision};
    use serde_json::json;

    #[test]
    fn merge_history_is_explainable_and_temporal() {
        let mut db = Db::open_in_memory().unwrap();
        db.write(|tx| {
            let d1 = record_decision(tx, &Decision::new("engine", "create_system_track"), 1_000)?;
            let a = upsert_node(tx, NodeKind::SystemTrack, "OTK000000001", 1_000)?;
            let b = upsert_node(tx, NodeKind::SystemTrack, "OTK000000002", 1_000)?;
            let s1 = upsert_node(tx, NodeKind::SourceTrack, "ais/1", 1_000)?;
            let s2 = upsert_node(tx, NodeKind::SourceTrack, "radar/7", 1_000)?;
            add_edge(tx, EdgeKind::ReportsFor, s1, a, d1, &json!({}), 1_000)?;
            let r2 = add_edge(tx, EdgeKind::ReportsFor, s2, b, d1, &json!({}), 1_000)?;

            // Operator merges B into A: B's report moves over, B stays an alias.
            let d2 = record_decision(tx, &Decision::new("op:parker", "merge"), 2_000)?;
            end_edge(tx, r2, d2, 2_000)?;
            add_edge(tx, EdgeKind::ReportsFor, s2, a, d2, &json!({}), 2_000)?;
            add_edge(tx, EdgeKind::MergedInto, b, a, d2, &json!({}), 2_000)?;

            // Ending an already-ended edge is refused.
            assert!(end_edge(tx, r2, d2, 3_000).is_err());
            Ok(())
        })
        .unwrap();

        let why = explain_system_track(db.connection(), "OTK000000001").unwrap();
        let summary: Vec<_> = why
            .iter()
            .map(|e| {
                (
                    e.kind.as_str(),
                    e.src_key.as_str(),
                    e.dst_key.as_str(),
                    e.decision_op.as_str(),
                    e.valid_to_ms,
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                (
                    "REPORTS_FOR",
                    "ais/1",
                    "OTK000000001",
                    "create_system_track",
                    None
                ),
                (
                    "REPORTS_FOR",
                    "radar/7",
                    "OTK000000002",
                    "create_system_track",
                    Some(2_000)
                ),
                ("REPORTS_FOR", "radar/7", "OTK000000001", "merge", None),
                ("MERGED_INTO", "OTK000000002", "OTK000000001", "merge", None),
            ]
        );
    }

    #[test]
    fn edge_endpoints_are_type_checked() {
        let mut db = Db::open_in_memory().unwrap();
        let err = db
            .write(|tx| {
                let d = record_decision(tx, &Decision::new("engine", "x"), 0)?;
                let a = upsert_node(tx, NodeKind::SystemTrack, "OTK000000001", 0)?;
                let s = upsert_node(tx, NodeKind::SourceTrack, "ais/1", 0)?;
                add_edge(tx, EdgeKind::ReportsFor, a, s, d, &json!({}), 0)
            })
            .unwrap_err();
        assert!(matches!(err, StoreError::Conflict(_)), "{err}");
    }
}
