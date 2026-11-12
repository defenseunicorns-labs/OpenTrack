//! PostgreSQL registry lookups used by the engine.

use diesel::prelude::*;
use diesel::sql_types::{BigInt, Nullable, Text};
use serde_json::{Value, json};

use super::{PgControlStore, PostgresError, VersionText};
use crate::registry::{Entity, RegistryIdentifier, RegistryRow};

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
