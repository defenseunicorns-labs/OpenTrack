//! PostgreSQL registry lookups used by the engine.

use diesel::prelude::*;
use diesel::sql_types::{BigInt, Nullable, Text};
use serde_json::{Value, json};

use super::{PgControlStore, PostgresError, VersionText};
use crate::registry::{Entity, EntityRevision, RegistryIdentifier, RegistryRow};

#[derive(QueryableByName)]
struct IdRow {
    #[diesel(sql_type = Text)]
    id: String,
}

#[derive(QueryableByName)]
struct CountRow {
    #[diesel(sql_type = BigInt)]
    n: i64,
}

#[derive(QueryableByName)]
struct KeyRow {
    #[diesel(sql_type = Text)]
    key: String,
    #[diesel(sql_type = BigInt)]
    n: i64,
}

#[derive(QueryableByName)]
struct RevisionRow {
    #[diesel(sql_type = BigInt)]
    id: i64,
    #[diesel(sql_type = Text)]
    entity: String,
    #[diesel(sql_type = BigInt)]
    saved_at_ms: i64,
    #[diesel(sql_type = Nullable<BigInt>)]
    decision_id: Option<i64>,
    #[diesel(sql_type = Nullable<Text>)]
    actor: Option<String>,
}

#[derive(QueryableByName)]
struct EntityRow {
    #[diesel(sql_type = Text)]
    id: String,
    #[diesel(sql_type = Nullable<Text>)]
    name: Option<String>,
    #[diesel(sql_type = Text)]
    status: String,
    #[diesel(sql_type = Nullable<Text>)]
    class_name: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    domain: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    affiliation: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    track_type: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    cot_type: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    sidc: Option<String>,
    #[diesel(sql_type = Text)]
    attributes: String,
    #[diesel(sql_type = Nullable<Text>)]
    source: Option<String>,
    #[diesel(sql_type = BigInt)]
    updated_at_ms: i64,
    #[diesel(sql_type = Nullable<Text>)]
    publish: Option<String>,
}

#[derive(QueryableByName)]
struct IdentifierRow {
    #[diesel(sql_type = Text)]
    scheme: String,
    #[diesel(sql_type = Text)]
    value: String,
    #[diesel(sql_type = Text)]
    entity_id: String,
    #[diesel(sql_type = Nullable<Text>)]
    expected_name: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    source: Option<String>,
}

impl PgControlStore {
    pub fn registry_resolve(
        &mut self,
        scheme: &str,
        value: &str,
    ) -> Result<Option<Entity>, PostgresError> {
        let rows: Vec<IdRow> = diesel::sql_query(
            "SELECT entity_id AS id FROM registry_identifiers WHERE scheme = $1 AND value = $2",
        )
        .bind::<Text, _>(scheme)
        .bind::<Text, _>(value)
        .load(&mut self.connection)?;
        match rows.into_iter().next() {
            Some(row) => self.entity(&row.id),
            None => Ok(None),
        }
    }

    pub fn list_entities(
        &mut self,
        q: &str,
        limit: usize,
        offset: usize,
    ) -> Result<(Vec<Entity>, usize), PostgresError> {
        let q = q.trim();
        let pattern = format!("%{}%", q.replace(['%', '_'], ""));
        let filter = "($1 = '%%' OR e.name ILIKE $1 OR e.id = $2 OR EXISTS (
            SELECT 1 FROM registry_identifiers i WHERE i.entity_id = e.id AND i.value ILIKE $1))";
        let total: CountRow = diesel::sql_query(format!(
            "SELECT count(*)::bigint AS n FROM registry_entities e WHERE {filter}"
        ))
        .bind::<Text, _>(&pattern)
        .bind::<Text, _>(q)
        .get_result(&mut self.connection)?;
        let ids: Vec<IdRow> = diesel::sql_query(format!(
            "SELECT e.id FROM registry_entities e WHERE {filter}
             ORDER BY e.name IS NULL, lower(e.name), e.id LIMIT $3 OFFSET $4"
        ))
        .bind::<Text, _>(&pattern)
        .bind::<Text, _>(q)
        .bind::<BigInt, _>(limit.min(i64::MAX as usize) as i64)
        .bind::<BigInt, _>(offset.min(i64::MAX as usize) as i64)
        .load(&mut self.connection)?;
        let page = ids
            .into_iter()
            .map(|row| self.entity(&row.id))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .collect();
        Ok((page, total.n as usize))
    }

    pub fn attribute_keys(&mut self) -> Result<Vec<(String, i64)>, PostgresError> {
        let rows: Vec<KeyRow> = diesel::sql_query(
            "SELECT a.value ->> 'key' AS key, count(*)::bigint AS n
             FROM registry_entities e, jsonb_array_elements(e.attributes::jsonb) AS a(value)
             GROUP BY key ORDER BY key",
        )
        .load(&mut self.connection)?;
        Ok(rows.into_iter().map(|row| (row.key, row.n)).collect())
    }

    pub fn entity_revisions(
        &mut self,
        id: &str,
        limit: usize,
    ) -> Result<Vec<EntityRevision>, PostgresError> {
        let rows: Vec<RevisionRow> = diesel::sql_query(
            "SELECT r.id, r.entity, r.saved_at_ms, r.decision_id, d.actor
             FROM entity_revisions r LEFT JOIN decisions d ON d.id = r.decision_id
             WHERE r.entity_id = $1 ORDER BY r.id DESC LIMIT $2",
        )
        .bind::<Text, _>(id)
        .bind::<BigInt, _>(limit.min(i64::MAX as usize) as i64)
        .load(&mut self.connection)?;
        rows.into_iter()
            .map(|row| {
                Ok(EntityRevision {
                    id: row.id,
                    entity: serde_json::from_str(&row.entity)?,
                    saved_at_ms: row.saved_at_ms,
                    decision_id: row.decision_id,
                    actor: row.actor,
                })
            })
            .collect()
    }

    pub fn registry_counts(&mut self) -> Result<Value, PostgresError> {
        #[derive(QueryableByName)]
        struct Counts {
            #[diesel(sql_type = BigInt)]
            active: i64,
            #[diesel(sql_type = BigInt)]
            retired: i64,
            #[diesel(sql_type = BigInt)]
            identifiers: i64,
        }
        let counts: Counts = diesel::sql_query(
            "SELECT (SELECT count(*) FROM registry_entities WHERE status = 'active')::bigint AS active,
                    (SELECT count(*) FROM registry_entities WHERE status = 'retired')::bigint AS retired,
                    (SELECT count(*) FROM registry_identifiers)::bigint AS identifiers",
        )
        .get_result(&mut self.connection)?;
        Ok(
            json!({"active": counts.active, "retired": counts.retired, "identifiers": counts.identifiers}),
        )
    }

    pub fn entity(&mut self, id: &str) -> Result<Option<Entity>, PostgresError> {
        let rows: Vec<EntityRow> = diesel::sql_query(
            "SELECT id, name, status, class_name, domain, affiliation, track_type,
             cot_type, sidc, attributes, source, updated_at_ms, publish
             FROM registry_entities WHERE id = $1",
        )
        .bind::<Text, _>(id)
        .load(&mut self.connection)?;
        let Some(row) = rows.into_iter().next() else {
            return Ok(None);
        };
        let identifiers: Vec<IdentifierRow> = diesel::sql_query(
            "SELECT scheme, value, entity_id, expected_name, source
             FROM registry_identifiers WHERE entity_id = $1
             ORDER BY added_at_ms, scheme, value",
        )
        .bind::<Text, _>(id)
        .load(&mut self.connection)?;
        let identifiers: Vec<RegistryIdentifier> = identifiers
            .into_iter()
            .map(|row| RegistryIdentifier {
                scheme: row.scheme,
                value: row.value,
                expected_name: row.expected_name,
                source: row.source,
            })
            .collect();
        let attributes = serde_json::from_str::<Value>(&row.attributes)?;
        let entity = serde_json::from_value(json!({
            "id": row.id,
            "name": row.name,
            "status": row.status,
            "class_name": row.class_name,
            "domain": row.domain,
            "affiliation": row.affiliation,
            "track_type": row.track_type,
            "cot_type": row.cot_type,
            "sidc": row.sidc,
            "attributes": attributes,
            "source": row.source,
            "updated_at_ms": row.updated_at_ms,
            "publish": row.publish,
            "identifiers": identifiers,
        }))?;
        Ok(Some(entity))
    }

    pub fn registry_rows(&mut self) -> Result<Vec<RegistryRow>, PostgresError> {
        let identifiers: Vec<IdentifierRow> = diesel::sql_query(
            "SELECT i.scheme, i.value, i.entity_id, i.expected_name, i.source
             FROM registry_identifiers i
             JOIN registry_entities e ON e.id = i.entity_id
             WHERE e.status = 'active' ORDER BY e.id, i.scheme, i.value",
        )
        .load(&mut self.connection)?;
        let mut rows = Vec::with_capacity(identifiers.len());
        let mut current: Option<Entity> = None;
        for identifier in identifiers {
            if current
                .as_ref()
                .is_none_or(|entity| entity.id != identifier.entity_id)
            {
                current = self.entity(&identifier.entity_id)?;
            }
            let Some(entity) = &current else { continue };
            rows.push(RegistryRow {
                scheme: identifier.scheme,
                value: identifier.value,
                entity_id: identifier.entity_id,
                name: entity.name.clone(),
                expected_name: identifier.expected_name,
                fields: entity.fields_map(),
            });
        }
        Ok(rows)
    }

    pub fn registry_version(&mut self) -> Result<String, PostgresError> {
        let row: VersionText = diesel::sql_query(
            "SELECT ((SELECT count(*) FROM registry_entities)::text || ':' ||
                    (SELECT coalesce(max(updated_at_ms), 0) FROM registry_entities)::text || ':' ||
                    (SELECT count(*) FROM registry_identifiers)::text || ':' ||
                    (SELECT coalesce(max(added_at_ms), 0) FROM registry_identifiers)::text)
                    AS version",
        )
        .get_result(&mut self.connection)?;
        Ok(row.version)
    }
}
