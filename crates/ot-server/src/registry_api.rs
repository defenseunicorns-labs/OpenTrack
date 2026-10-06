//! The registry over the API: browse, create, edit and delete entities, and
//! move the whole registry in and out as a spreadsheet (see
//! [`crate::registry_sheet`]).

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use ot_core::Uid;
use ot_store::{Entity, RegistryIdentifier};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::api::actor;
use crate::control::{ApiError, AppState};
use crate::registry_sheet::{self, Format, Planned};

/// Largest sheet an import accepts.
const MAX_SHEET_BYTES: usize = 32 * 1024 * 1024;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/registry/entities", get(list).post(create))
        .route(
            "/registry/entities/{id}",
            get(view).put(save).delete(delete),
        )
        .route("/registry/fields", get(fields))
        .route("/registry/export", get(export))
        .route(
            "/registry/import-sheet",
            post(import).layer(DefaultBodyLimit::max(MAX_SHEET_BYTES)),
        )
}

#[derive(Deserialize)]
struct ListQuery {
    #[serde(default)]
    q: String,
    limit: Option<usize>,
    #[serde(default)]
    offset: usize,
}

/// A page of entities (all, or those whose name or identifier matches `q`).
async fn list(
    State(s): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Value>, ApiError> {
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    let (entities, total) = s
        .with_db(move |db| db.list_entities(&q.q, limit, q.offset))
        .await?;
    Ok(Json(json!({ "entities": entities, "total": total })))
}

/// Entity field names a pipeline can link: the OTH-GOLD minimum, and every
/// attribute key in use (with how many entities have it).
async fn fields(State(s): State<AppState>) -> Result<Json<Value>, ApiError> {
    let keys = s.with_db(|db| db.attribute_keys()).await?;
    Ok(Json(json!({
        "minimum": ot_store::registry::MINIMUM,
        "attributes": keys
            .into_iter()
            .map(|(k, n)| json!({"key": k, "entities": n}))
            .collect::<Vec<_>>(),
    })))
}

/// The entity, its history, and the live tracks it speaks for (with what
/// their feeds reported where the entity replaced it).
async fn entity_view(s: &AppState, id: String) -> Result<Value, ApiError> {
    let key = id.clone();
    let (entity, revisions) = s
        .with_db(move |db| Ok((db.entity(&key)?, db.entity_revisions(&key, 50)?)))
        .await?;
    let entity = entity.ok_or_else(|| ApiError::not_found(format!("entity {id}")))?;
    let tracks: Vec<Value> = s
        .redis
        .list_system_tracks()
        .await?
        .into_iter()
        .filter(|t| t.entity_id.as_deref() == Some(id.as_str()))
        .map(|t| {
            json!({
                "uid": t.uid, "track_id": t.uid.doc_id(), "state": t.state,
                "source_id": t.view.source_id, "last_seen": t.last_seen,
                "notices": t.notices,
            })
        })
        .collect();
    Ok(json!({ "entity": entity, "revisions": revisions, "tracks": tracks }))
}

async fn view(State(s): State<AppState>, Path(id): Path<String>) -> Result<Json<Value>, ApiError> {
    Ok(Json(entity_view(&s, id).await?))
}

async fn save(
    State(s): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(mut entity): Json<Entity>,
) -> Result<Json<Value>, ApiError> {
    entity.id = id.clone();
    let actor = actor(&headers);
    let key = id.clone();
    s.with_db(move |db| {
        if db.entity(&key)?.is_none() {
            return Err(ot_store::StoreError::NotFound(format!("entity {key}")));
        }
        db.save_entity(&entity, &actor)
    })
    .await?;
    Ok(Json(entity_view(&s, id).await?))
}

async fn delete(
    State(s): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let actor = actor(&headers);
    s.with_db(move |db| db.delete_entity(&id, &actor)).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct CreateBody {
    /// Seed the entity's name and identifiers from this live track.
    #[serde(default)]
    from_track: Option<String>,
    #[serde(flatten)]
    entity: Entity,
}

async fn create(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateBody>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let mut entity = body.entity;
    entity.id = String::new();
    if let Some(raw) = &body.from_track {
        let uid: Uid = Uid::from_doc_id(raw)
            .or_else(|_| raw.parse())
            .map_err(|e| ApiError::bad_request(e.to_string()))?;
        let t = s
            .redis
            .get_system_track(uid)
            .await?
            .ok_or_else(|| ApiError::not_found(format!("live system track {uid}")))?;
        if let Some(e) = &t.entity_id {
            return Err(ApiError::conflict(format!(
                "track {uid} already resolves to entity {e}"
            )));
        }
        entity.name = entity.name.or(t.view.name.clone());
        for i in &t.view.identifiers {
            if !entity
                .identifiers
                .iter()
                .any(|x| x.scheme == i.scheme && x.value == i.value)
            {
                entity.identifiers.push(RegistryIdentifier {
                    scheme: i.scheme.clone(),
                    value: i.value.clone(),
                    expected_name: t.view.name.clone(),
                    source: None,
                });
            }
        }
    }
    if entity.identifiers.is_empty() {
        return Err(ApiError::unprocessable(
            "an entity needs at least one identifier (scheme and value), so tracks can find it",
        ));
    }
    let actor = actor(&headers);
    let saved = s.with_db(move |db| db.save_entity(&entity, &actor)).await?;
    Ok((StatusCode::CREATED, Json(entity_view(&s, saved.id).await?)))
}

#[derive(Deserialize)]
struct FormatQuery {
    #[serde(default = "xlsx")]
    format: String,
}

fn xlsx() -> String {
    "xlsx".into()
}

fn format_of(q: &FormatQuery) -> Result<Format, ApiError> {
    Format::parse(&q.format)
        .ok_or_else(|| ApiError::bad_request(format!("format {:?}: use xlsx or csv", q.format)))
}

/// The whole registry as a spreadsheet download.
async fn export(
    State(s): State<AppState>,
    Query(q): Query<FormatQuery>,
) -> Result<Response, ApiError> {
    let format = format_of(&q)?;
    let (all, _) = s.with_db(|db| db.list_entities("", usize::MAX, 0)).await?;
    // Marked with the banner's text: entities carry no labels.
    let marking = crate::marking::Marker::load(&s).await?.file([]);
    let bytes = tokio::task::spawn_blocking(move || {
        let mut sheet = registry_sheet::export(&all);
        sheet.marking = Some(marking);
        registry_sheet::write(&sheet, format)
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?
    .map_err(ApiError::internal)?;
    let name = format!(
        "opentrack-registry-{}.{}",
        chrono::Utc::now().format("%Y%m%d-%H%M"),
        format.extension()
    );
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(format.content_type()),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{name}\""))
            .map_err(|e| ApiError::internal(e.to_string()))?,
    );
    Ok((StatusCode::OK, headers, bytes).into_response())
}

#[derive(Deserialize)]
struct ImportQuery {
    #[serde(default = "xlsx")]
    format: String,
    /// Write the changes; otherwise only report what they would be.
    #[serde(default)]
    apply: bool,
    /// Recorded with the import decision (e.g. the file name).
    #[serde(default)]
    label: Option<String>,
}

/// Plan (and with `apply`, make) the changes a spreadsheet describes. Nothing
/// is written while any row has an error.
async fn import(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<ImportQuery>,
    body: Bytes,
) -> Result<Response, ApiError> {
    let format = format_of(&FormatQuery {
        format: q.format.clone(),
    })?;
    let sheet = registry_sheet::read(&body, format).map_err(ApiError::unprocessable)?;
    let planned = s
        .with_db(move |db| {
            // Lookups fail only on a database error: remember the first.
            let failure = std::cell::RefCell::new(None);
            fn keep<T>(
                failure: &std::cell::RefCell<Option<ot_store::StoreError>>,
                r: ot_store::sqlite::Result<Option<T>>,
            ) -> Option<T> {
                r.unwrap_or_else(|e| {
                    failure.borrow_mut().get_or_insert(e);
                    None
                })
            }
            let db: &ot_store::Db = db;
            let result = registry_sheet::plan(
                &sheet,
                &mut |id| keep(&failure, db.entity(id)),
                &mut |scheme, value| {
                    keep(&failure, db.registry_resolve(scheme, value)).map(|e| e.id)
                },
                &mut ot_store::new_entity_id,
            );
            match failure.into_inner() {
                Some(e) => Err(e),
                None => Ok(result),
            }
        })
        .await?;
    let rows: Vec<Planned> = planned.map_err(ApiError::unprocessable)?;
    let count = |a: &str| rows.iter().filter(|r| r.action == a).count();
    let counts = json!({
        "create": count("create"), "update": count("update"),
        "unchanged": count("unchanged"), "error": count("error"),
    });
    let errors = count("error");
    if !q.apply || errors > 0 {
        let status = if q.apply {
            StatusCode::UNPROCESSABLE_ENTITY
        } else {
            StatusCode::OK
        };
        return Ok((
            status,
            Json(json!({ "applied": false, "counts": counts, "rows": rows })),
        )
            .into_response());
    }
    let actor = actor(&headers);
    let label = q.label.unwrap_or_else(|| "spreadsheet import".into());
    let entities: Vec<Entity> = rows.iter().filter_map(|r| r.entity.clone()).collect();
    let imported = s
        .with_db(move |db| db.registry_import(&entities, &actor, &label))
        .await?;
    Ok((
        StatusCode::OK,
        Json(json!({ "applied": true, "counts": counts, "registry": imported, "rows": rows })),
    )
        .into_response())
}
