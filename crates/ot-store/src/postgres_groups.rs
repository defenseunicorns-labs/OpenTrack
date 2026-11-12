//! PostgreSQL group reads.

use diesel::prelude::*;
use diesel::sql_types::{BigInt, Text};
use ot_core::Uid;

use super::{PgControlStore, PostgresError};
use crate::groups::Group;

#[derive(QueryableByName)]
struct GroupRow {
    #[diesel(sql_type = Text)]
    uid: String,
    #[diesel(sql_type = Text)]
    spec: String,
    #[diesel(sql_type = BigInt)]
    created_at_ms: i64,
    #[diesel(sql_type = BigInt)]
    updated_at_ms: i64,
}

#[derive(QueryableByName)]
struct MemberRow {
    #[diesel(sql_type = Text)]
    key: String,
}

impl PgControlStore {
    pub fn groups(&mut self) -> Result<Vec<Group>, PostgresError> {
        let rows: Vec<GroupRow> = diesel::sql_query(
            "SELECT uid, spec, created_at_ms, updated_at_ms FROM track_groups
             WHERE dissolved_at_ms IS NULL ORDER BY created_at_ms",
        )
        .load(&mut self.connection)?;
        let mut groups = Vec::with_capacity(rows.len());
        for row in rows {
            let Ok(uid) = row.uid.parse::<Uid>() else {
                continue;
            };
            let members: Vec<MemberRow> = diesel::sql_query(
                "SELECT n.key FROM edges e
                 JOIN nodes n ON n.id = e.src JOIN nodes g ON g.id = e.dst
                 WHERE e.kind = 'MEMBER_OF' AND e.valid_to_ms IS NULL
                   AND g.kind = 'group' AND g.key = $1 ORDER BY n.key",
            )
            .bind::<Text, _>(uid.to_string())
            .load(&mut self.connection)?;
            groups.push(Group {
                uid,
                track_id: uid.doc_id(),
                spec: serde_json::from_str(&row.spec)?,
                members: members
                    .into_iter()
                    .filter_map(|row| row.key.parse().ok())
                    .collect(),
                created_at_ms: row.created_at_ms,
                updated_at_ms: row.updated_at_ms,
            });
        }
        Ok(groups)
    }
}
