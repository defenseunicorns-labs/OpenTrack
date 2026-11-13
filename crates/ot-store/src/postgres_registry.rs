//! PostgreSQL registry lookups used by the engine.

use std::collections::BTreeSet;

use diesel::prelude::*;
use diesel::sql_types::{BigInt, Nullable, Text};
use serde::Serialize;
use serde_json::{Value, json};

use super::{PgControlStore, PostgresError, VersionText, record_decision};
use crate::Decision;
use crate::registry::{Entity, EntityRevision, RegistryIdentifier, RegistryRow, new_entity_id};

fn enum_text<T: Serialize>(value: &Option<T>) -> Option<String> {
    value
        .as_ref()
        .and_then(|v| serde_json::to_value(v).ok())
        .and_then(|v| v.as_str().map(str::to_owned))
}

fn write_entity_row(
    connection: &mut PgConnection,
    e: &Entity,
    now: i64,
) -> Result<(), PostgresError> {
    diesel::sql_query(
        "INSERT INTO registry_entities
         (id, name, status, class_name, domain, affiliation, track_type, cot_type,
          sidc, attributes, source, created_at_ms, updated_at_ms, publish)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $12, $13)
         ON CONFLICT (id) DO UPDATE SET name = excluded.name, status = excluded.status,
           class_name = excluded.class_name, domain = excluded.domain,
           affiliation = excluded.affiliation, track_type = excluded.track_type,
           cot_type = excluded.cot_type, sidc = excluded.sidc, attributes = excluded.attributes,
           source = coalesce(excluded.source, registry_entities.source),
           publish = excluded.publish, updated_at_ms = excluded.updated_at_ms",
    )
    .bind::<Text, _>(&e.id)
    .bind::<Nullable<Text>, _>(e.name.as_deref())
    .bind::<Text, _>(&e.status)
    .bind::<Nullable<Text>, _>(e.class_name.as_deref())
    .bind::<Nullable<Text>, _>(enum_text(&e.domain))
    .bind::<Nullable<Text>, _>(enum_text(&e.affiliation))
    .bind::<Nullable<Text>, _>(enum_text(&e.track_type))
    .bind::<Nullable<Text>, _>(e.cot_type.as_deref())
    .bind::<Nullable<Text>, _>(e.sidc.as_deref())
    .bind::<Text, _>(serde_json::to_string(&e.attributes)?)
    .bind::<Nullable<Text>, _>(e.source.as_deref())
    .bind::<BigInt, _>(now)
    .bind::<Nullable<Text>, _>(enum_text(&e.publish))
    .execute(connection)?;
    Ok(())
}

fn write_entity_revision(
    connection: &mut PgConnection,
    e: &Entity,
    now: i64,
    decision_id: i64,
) -> Result<(), PostgresError> {
    let snapshot = Entity {
        updated_at_ms: now,
        ..e.clone()
    };
    diesel::sql_query(
        "INSERT INTO entity_revisions (entity_id, entity, saved_at_ms, decision_id)
         VALUES ($1, $2, $3, $4)",
    )
    .bind::<Text, _>(&e.id)
    .bind::<Text, _>(serde_json::to_string(&snapshot)?)
    .bind::<BigInt, _>(now)
    .bind::<BigInt, _>(decision_id)
    .execute(connection)?;
    Ok(())
}

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
    pub fn save_entity(&mut self, entity: &Entity, actor: &str) -> Result<Entity, PostgresError> {
        let mut e = entity.clone();
        e.validate()
            .map_err(|err| PostgresError::Conflict(err.to_string()))?;
        let before = if e.id.trim().is_empty() {
            e.id = new_entity_id();
            None
        } else {
            e.id = e.id.trim().to_owned();
            self.entity(&e.id)?
        };
        if before.is_none() && e.source.is_none() {
            e.source = Some(actor.to_owned());
        }
        let now = crate::sqlite::now_ms();
        self.connection.transaction::<(), PostgresError, _>(|connection| {
            for ident in &e.identifiers {
                let holders: Vec<IdRow> = diesel::sql_query(
                    "SELECT entity_id AS id FROM registry_identifiers WHERE scheme = $1 AND value = $2",
                )
                .bind::<Text, _>(&ident.scheme)
                .bind::<Text, _>(&ident.value)
                .load(connection)?;
                if let Some(holder) = holders.first().filter(|holder| holder.id != e.id) {
                    return Err(PostgresError::Conflict(format!(
                        "{} already belongs to entity {}", ident.label(), holder.id,
                    )));
                }
            }
            let op = if before.is_some() { "edit_entity" } else { "create_entity" };
            let mut decision = Decision::new(actor, op)
                .evidence(json!({"entity": e.id, "name": e.name}))
                .after(serde_json::to_value(&e)?);
            if let Some(previous) = &before {
                decision = decision.before(serde_json::to_value(previous)?);
            }
            let decision_id = record_decision(connection, &decision)?;
            write_entity_row(connection, &e, now)?;
            let kept: BTreeSet<(&str, &str)> = e.identifiers.iter()
                .map(|ident| (ident.scheme.as_str(), ident.value.as_str())).collect();
            for old in before.iter().flat_map(|previous| &previous.identifiers) {
                if !kept.contains(&(old.scheme.as_str(), old.value.as_str())) {
                    diesel::sql_query(
                        "DELETE FROM registry_identifiers WHERE scheme = $1 AND value = $2 AND entity_id = $3",
                    )
                    .bind::<Text, _>(&old.scheme)
                    .bind::<Text, _>(&old.value)
                    .bind::<Text, _>(&e.id)
                    .execute(connection)?;
                }
            }
            for ident in &e.identifiers {
                diesel::sql_query(
                    "INSERT INTO registry_identifiers
                     (scheme, value, entity_id, expected_name, source, added_at_ms)
                     VALUES ($1, $2, $3, $4, $5, $6)
                     ON CONFLICT (scheme, value) DO UPDATE
                     SET expected_name = excluded.expected_name
                     WHERE registry_identifiers.entity_id = excluded.entity_id",
                )
                .bind::<Text, _>(&ident.scheme)
                .bind::<Text, _>(&ident.value)
                .bind::<Text, _>(&e.id)
                .bind::<Nullable<Text>, _>(ident.expected_name.as_deref())
                .bind::<Text, _>(ident.source.as_deref().unwrap_or(actor))
                .bind::<BigInt, _>(now)
                .execute(connection)?;
            }
            write_entity_revision(connection, &e, now, decision_id)
        })?;
        self.entity(&e.id)?
            .ok_or_else(|| PostgresError::NotFound(format!("entity {}", e.id)))
    }

    pub fn delete_entity(&mut self, id: &str, actor: &str) -> Result<(), PostgresError> {
        let before = self
            .entity(id)?
            .ok_or_else(|| PostgresError::NotFound(format!("entity {id}")))?;
        self.connection
            .transaction::<(), PostgresError, _>(|connection| {
                record_decision(
                    connection,
                    &Decision::new(actor, "delete_entity")
                        .evidence(json!({"entity": id, "name": before.name}))
                        .before(serde_json::to_value(&before)?),
                )?;
                diesel::sql_query("DELETE FROM registry_identifiers WHERE entity_id = $1")
                    .bind::<Text, _>(id)
                    .execute(connection)?;
                diesel::sql_query("DELETE FROM registry_entities WHERE id = $1")
                    .bind::<Text, _>(id)
                    .execute(connection)?;
                Ok(())
            })
    }

    pub fn update_entity_fields(
        &mut self,
        id: &str,
        updates: &[(String, Value)],
        actor: &str,
    ) -> Result<bool, PostgresError> {
        let Some(before) = self.entity(id)? else {
            return Ok(false);
        };
        let mut e = before.clone();
        let mut changed = Vec::new();
        for (key, value) in updates {
            if let Ok(true) = e.set_field(key, value) {
                changed.push(key.clone());
            }
        }
        if changed.is_empty() {
            return Ok(false);
        }
        e.validate()
            .map_err(|err| PostgresError::Conflict(err.to_string()))?;
        let now = crate::sqlite::now_ms();
        self.connection
            .transaction::<(), PostgresError, _>(|connection| {
                let decision_id = record_decision(
                    connection,
                    &Decision::new(actor, "update_entity")
                        .evidence(json!({"entity": id, "changed": changed}))
                        .before(serde_json::to_value(&before)?)
                        .after(serde_json::to_value(&e)?),
                )?;
                write_entity_row(connection, &e, now)?;
                write_entity_revision(connection, &e, now, decision_id)
            })?;
        Ok(true)
    }

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
