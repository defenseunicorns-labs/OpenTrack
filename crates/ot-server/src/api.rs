//! REST routes for sources, the registry, metrics and system tracks.
//!
//! Authentication arrives later; until then the acting operator is taken from
//! the `X-OpenTrack-Actor` header (default `op:api`) and recorded with every
//! decision.

use std::collections::BTreeMap;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post, put};
use axum::{Extension, Json, Router};
use ot_source::frame::Frame;
use ot_source::pipeline::Pipeline;
use ot_source::schema::{ExtensionField, ExtensionSchema};
use ot_source::source::SourceSpec;
use ot_source::trace::Trace;
use ot_store::{Entity, SourceRow, SourceWrite};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::{AuthUser, Role};
use crate::control::{ApiError, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/sources", get(list_sources).post(create_source))
        .route("/sources/validate", post(validate_source))
        .route("/protobuf/describe", post(describe_protobuf))
        .route(
            "/sources/{id}",
            get(get_source).put(put_source).delete(delete_source),
        )
        .route("/sources/{id}/enable", post(enable_source))
        .route("/sources/{id}/disable", post(disable_source))
        .route(
            "/sources/{id}/raw-output",
            put(set_raw_output).delete(clear_raw_output),
        )
        .route("/sources/{id}/revisions", get(source_revisions))
        .route("/sources/{id}/metrics", get(source_metrics))
        .route("/registry", get(registry_resolve))
        .route("/registry/stats", get(registry_stats))
        .route("/registry/import", post(registry_import))
        .route("/tracks", get(list_tracks))
        .route("/probe", post(crate::probe::probe))
        .route("/sources/{id}/samples", get(crate::probe::samples))
        .route("/schema", get(schema_overview))
        .route(
            "/schema/draft",
            put(put_schema_draft).delete(discard_schema_draft),
        )
        .route("/schema/draft/publish", post(publish_schema_draft))
        .merge(crate::registry_api::routes())
        .merge(crate::settings_api::routes())
        .merge(crate::basemap::routes())
        .merge(crate::correlation_api::routes())
        .merge(crate::sync_api::routes())
        .merge(crate::manage_api::routes())
        .merge(crate::plugins_api::routes())
        .merge(crate::profiles::routes())
        .merge(crate::config_backup::routes())
}

/// Who is making a request, as the decision log records it: the signed-in
/// account, set by [`crate::auth::layer`] (a client cannot set it).
pub(crate) fn actor(headers: &HeaderMap) -> String {
    headers
        .get(crate::auth::ACTOR_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.len() <= 128)
        .map_or_else(|| "op:api".to_owned(), str::to_owned)
}

/// Parse and validate a source spec, including against the published
/// extension schema version its mapping targets. Returns the spec, its
/// normalised JSON (defaults filled in) and that schema.
async fn parse_spec(
    s: &AppState,
    body: Value,
) -> Result<(SourceSpec, Value, ExtensionSchema), ApiError> {
    let spec: SourceSpec = serde_json::from_value(body)
        .map_err(|e| ApiError::unprocessable(format!("invalid source spec: {e}")))?;
    crate::probe::file_allowed(&s.common, &spec.transport).map_err(ApiError::unprocessable)?;
    let schema = published_schema(s, spec.pipeline.mapping.schema_version).await?;
    spec.validate_against(schema.as_ref())
        .map_err(|e| ApiError::unprocessable(e.to_string()))?;
    let normalised = serde_json::to_value(&spec).map_err(|e| ApiError::internal(e.to_string()))?;
    Ok((
        spec,
        normalised,
        schema.expect("validated against a published schema"),
    ))
}

/// A published extension schema version, typed.
pub(crate) async fn published_schema(
    s: &AppState,
    version: u32,
) -> Result<Option<ExtensionSchema>, ApiError> {
    let v = s.with_db(move |db| db.schema_version_get(version)).await?;
    v.filter(|v| v.status == "published")
        .map(to_extension_schema)
        .transpose()
}

pub(crate) fn to_extension_schema(v: ot_store::SchemaVersion) -> Result<ExtensionSchema, ApiError> {
    let fields = v
        .fields
        .into_iter()
        .map(serde_json::from_value)
        .collect::<Result<Vec<ExtensionField>, _>>()
        .map_err(|e| {
            ApiError::internal(format!("stored schema {} is unreadable: {e}", v.version))
        })?;
    Ok(ExtensionSchema {
        version: v.version,
        fields,
    })
}

async fn with_status(s: &AppState, row: SourceRow) -> Value {
    let status = s
        .redis
        .get_source_status(&row.id)
        .await
        .ok()
        .flatten()
        .and_then(|j| serde_json::from_str::<Value>(&j).ok());
    let mut v = serde_json::to_value(&row).unwrap_or_default();
    v["status"] = status.unwrap_or(Value::Null);
    v
}

/// Whether the caller may see a source's secrets: only admins, who can
/// change sources (see `ot_source::secrets`). No account: no.
pub fn sees_secrets(u: Option<&Extension<AuthUser>>) -> bool {
    u.is_some_and(|u| u.role >= Role::Admin)
}

/// Hide a source row's secrets from a caller who may not see them.
fn for_caller(mut v: Value, secrets: bool) -> Value {
    if !secrets && let Some(spec) = v.get_mut("spec") {
        ot_source::secrets::redact_spec(spec);
    }
    v
}

async fn list_sources(
    State(s): State<AppState>,
    u: Option<Extension<AuthUser>>,
) -> Result<Json<Value>, ApiError> {
    let secrets = sees_secrets(u.as_ref());
    let rows = s.with_db(|db| db.list_sources()).await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        out.push(for_caller(with_status(&s, r).await, secrets));
    }
    Ok(Json(json!({ "sources": out })))
}

async fn get_source(
    State(s): State<AppState>,
    u: Option<Extension<AuthUser>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let row = s
        .with_db(move |db| db.get_source(&id))
        .await?
        .ok_or_else(|| ApiError::not_found("source"))?;
    Ok(Json(for_caller(
        with_status(&s, row).await,
        sees_secrets(u.as_ref()),
    )))
}

async fn save(
    s: &AppState,
    spec: SourceSpec,
    normalised: Value,
    actor: String,
) -> Result<SourceRow, ApiError> {
    if spec.unauthenticated.is_some() {
        // The decision records the acceptance; the log says it plainly.
        tracing::warn!(source = %spec.id, transport = spec.transport.kind(), %actor,
            "listening source saved with unauthenticated senders accepted");
    }
    s.with_db(move |db| {
        db.put_source(
            &SourceWrite {
                id: &spec.id,
                name: &spec.name,
                transport: spec.transport.kind(),
                codec: spec.pipeline.codec.name(),
                priority: spec.priority,
                spec: &normalised,
            },
            &actor,
        )
    })
    .await
}

async fn create_source(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let (spec, normalised, _) = parse_spec(&s, body).await?;
    let id = spec.id.clone();
    if s.with_db(move |db| db.get_source(&id)).await?.is_some() {
        return Err(ApiError::conflict(format!(
            "source {} already exists; use PUT to update",
            spec.id
        )));
    }
    let row = save(&s, spec, normalised, actor(&headers)).await?;
    Ok((StatusCode::CREATED, Json(with_status(&s, row).await)))
}

async fn put_source(
    State(s): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let (spec, normalised, _) = parse_spec(&s, body).await?;
    if spec.id != id {
        return Err(ApiError::unprocessable(format!(
            "body id {:?} does not match path id {id:?} (source ids never change)",
            spec.id
        )));
    }
    let row = save(&s, spec, normalised, actor(&headers)).await?;
    Ok(Json(with_status(&s, row).await))
}

async fn delete_source(
    State(s): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let actor = actor(&headers);
    s.with_db(move |db| db.delete_source(&id, &actor)).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn toggle(
    s: AppState,
    id: String,
    enabled: bool,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let actor = actor(&headers);
    let row = s
        .with_db(move |db| db.set_source_enabled(&id, enabled, &actor))
        .await?;
    Ok(Json(with_status(&s, row).await))
}

async fn enable_source(
    State(s): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    toggle(s, id, true, headers).await
}

async fn disable_source(
    State(s): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    toggle(s, id, false, headers).await
}

#[derive(Deserialize)]
struct RawOutput {
    /// NATS subject for this source's raw tracks; `opentrack.raw.<source>`
    /// when omitted.
    #[serde(default)]
    subject: Option<String>,
    /// Must be true: the admin confirms this source's raw data is published.
    consent: bool,
}

/// A concrete (wildcard-free) NATS subject outside the system tracks prefix.
pub(crate) fn check_raw_subject(subject: &str, tracks_prefix: &str) -> Result<(), String> {
    let valid = !subject.is_empty()
        && subject.split('.').all(|t| {
            !t.is_empty() && t != "*" && t != ">" && !t.chars().any(|c| c.is_whitespace())
        });
    if !valid {
        return Err(format!(
            "{subject:?} is not a valid NATS subject (dot-separated tokens, no wildcards or spaces)"
        ));
    }
    if subject == tracks_prefix || subject.starts_with(&format!("{tracks_prefix}.")) {
        return Err(format!(
            "raw output needs its own subject, outside the system tracks subjects ({tracks_prefix}.>)"
        ));
    }
    Ok(())
}

async fn set_raw_output(
    State(s): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<RawOutput>,
) -> Result<Json<Value>, ApiError> {
    if !body.consent {
        return Err(ApiError::unprocessable(
            "raw output publishes this source's tracks as received; set consent to true to confirm",
        ));
    }
    let c = match body.subject.as_deref().map(str::trim) {
        Some(sub) if !sub.is_empty() => sub.to_owned(),
        _ => format!("opentrack.raw.{id}"),
    };
    check_raw_subject(&c, &s.common.nats.tracks_subject).map_err(ApiError::unprocessable)?;
    let actor = actor(&headers);
    let row = s
        .with_db(move |db| db.set_raw_output(&id, Some(&c), &actor))
        .await?;
    Ok(Json(with_status(&s, row).await))
}

async fn clear_raw_output(
    State(s): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let actor = actor(&headers);
    let row = s
        .with_db(move |db| db.set_raw_output(&id, None, &actor))
        .await?;
    Ok(Json(with_status(&s, row).await))
}

async fn source_revisions(
    State(s): State<AppState>,
    u: Option<Extension<AuthUser>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let secrets = sees_secrets(u.as_ref());
    let revs = s.with_db(move |db| db.source_revisions(&id)).await?;
    let revs: Vec<Value> = revs
        .into_iter()
        .map(|r| for_caller(serde_json::to_value(r).unwrap_or_default(), secrets))
        .collect();
    Ok(Json(json!({ "revisions": revs })))
}

#[derive(Deserialize)]
struct MetricsQuery {
    minutes: Option<i64>,
}

async fn source_metrics(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<MetricsQuery>,
) -> Result<Json<Value>, ApiError> {
    let minutes = q.minutes.unwrap_or(60).clamp(1, 7 * 24 * 60);
    let series = s.redis.metrics_range(&id, minutes).await?;
    let mut totals: BTreeMap<String, u64> = BTreeMap::new();
    let points: Vec<Value> = series
        .into_iter()
        .map(|(minute, counts)| {
            for (k, v) in &counts {
                *totals.entry(k.clone()).or_default() += v;
            }
            json!({ "minute": minute, "counts": counts })
        })
        .collect();
    Ok(Json(
        json!({ "source": id, "minutes": minutes, "totals": totals, "series": points }),
    ))
}

#[derive(Deserialize)]
struct ValidateBody {
    spec: Value,
    /// Optional sample frames (strings) to run through the pipeline.
    #[serde(default)]
    samples: Vec<String>,
    /// Also run the stored probe samples of this source id.
    #[serde(default)]
    stored_samples_of: Option<String>,
    /// Follow the first this many decoded records stage by stage (0: no
    /// trace; at most [`ot_source::trace::MAX_SAMPLES`]).
    #[serde(default)]
    trace: Option<usize>,
}

/// Validate a source spec and optionally dry-run sample frames through its
/// pipeline, using the live registry. Nothing is saved or published.
async fn validate_source(
    State(s): State<AppState>,
    Json(body): Json<ValidateBody>,
) -> Result<Json<Value>, ApiError> {
    let (spec, normalised, schema) = parse_spec(&s, body.spec).await?;
    let mut frames: Vec<Frame> = body
        .samples
        .iter()
        .map(|s| Frame::new(s.clone().into_bytes()))
        .collect();
    if let Some(id) = body.stored_samples_of.clone() {
        let stored = s
            .with_db(move |db| db.probe_samples(&id, ot_store::probe::DEFAULT_SAMPLE_CAP))
            .await?;
        frames.extend(stored.into_iter().map(|s| {
            let meta = match s.meta {
                Some(Value::Object(m)) => m,
                _ => Default::default(),
            };
            Frame::new(s.bytes).with_meta(meta)
        }));
    }
    if frames.is_empty() {
        return Ok(Json(json!({ "valid": true, "spec": normalised })));
    }
    let rows = s.with_db(|db| db.registry_rows()).await?;
    let registry: BTreeMap<(String, String), ot_source::registry::RegistryEntry> = rows
        .into_iter()
        .map(|r| {
            (
                (r.scheme, r.value),
                ot_source::registry::RegistryEntry {
                    entity_id: r.entity_id,
                    name: r.name,
                    expected_name: r.expected_name,
                    fields: r.fields,
                },
            )
        })
        .collect();
    let mut pipeline = Pipeline::new(spec.id.clone(), spec.pipeline.clone())
        .map_err(|e| ApiError::unprocessable(e.to_string()))?
        .with_schema(schema);
    let mut trace = body.trace.filter(|n| *n > 0).map(Trace::new);
    let mut observations = Vec::new();
    let mut errors = Vec::new();
    for frame in frames.iter().take(1000) {
        let out = match trace.as_mut() {
            Some(t) => pipeline.process_traced(frame, &registry, t),
            None => pipeline.process(frame, &registry),
        };
        observations.extend(out.observations);
        if let Some(e) = out.last_error {
            errors.push(e);
        }
    }
    // Scans still held by a tracker stage.
    let last = frames
        .last()
        .map_or_else(chrono::Utc::now, |f| f.received_at);
    let flushed = match trace.as_mut() {
        Some(t) => pipeline.flush_traced(last, true, t),
        None => pipeline.flush(last, true),
    };
    observations.extend(flushed.observations);
    let counts: BTreeMap<String, u64> = pipeline.take_counts().pairs().into_iter().collect();
    let mut body = json!({
        "valid": true,
        "counts": counts,
        "errors": errors,
        "observations": observations,
    });
    if let Some(t) = trace {
        body["trace"] = traced_with_publish(&s, t).await?;
    }
    Ok(Json(body))
}

/// A trace as JSON, each sample ending with a `publish` stage: what it
/// emitted, as the upsert message a new system track seeded from it would
/// publish (its UID is correlation's to assign, so it is left out).
async fn traced_with_publish(s: &AppState, trace: Trace) -> Result<Value, ApiError> {
    let version = s.with_db(|db| db.latest_published_schema()).await?;
    let output = published_schema(s, version).await?;
    let ctx = s.common.publish_context();
    let now = chrono::Utc::now();
    let uid = ot_core::Uid::new(s.common.site, 1).map_err(|e| ApiError::internal(e.to_string()))?;
    let placeholder = json!("assigned by correlation");
    let mut samples = Vec::with_capacity(trace.samples.len());
    for sample in &trace.samples {
        let items: Vec<Value> = sample
            .emitted
            .iter()
            .map(|obs| {
                let mut t = ot_core::SystemTrack::from_first_observation(uid, obs.clone());
                if let Some(schema) = &output {
                    t.attributes = ot_source::schema::resolve_attributes(schema, &t).0;
                }
                let mut m = serde_json::to_value(ot_core::wire::to_message(&t, &ctx, now))
                    .unwrap_or(Value::Null);
                m["uid"] = placeholder.clone();
                m["track_id"] = placeholder.clone();
                ot_source::trace::capped(m)
            })
            .collect();
        let mut v = serde_json::to_value(sample).map_err(|e| ApiError::internal(e.to_string()))?;
        if let Some(stages) = v["stages"].as_array_mut() {
            stages.push(json!({ "id": "publish", "items": items }));
        }
        samples.push(v);
    }
    Ok(json!({ "frames": trace.frames, "samples": samples }))
}

#[derive(Deserialize)]
struct ResolveQuery {
    scheme: String,
    value: String,
}

async fn registry_resolve(
    State(s): State<AppState>,
    Query(q): Query<ResolveQuery>,
) -> Result<Json<Value>, ApiError> {
    let e = s
        .with_db(move |db| db.registry_resolve(&q.scheme, &q.value))
        .await?
        .ok_or_else(|| ApiError::not_found("identifier"))?;
    Ok(Json(serde_json::to_value(e).unwrap_or_default()))
}

async fn registry_stats(State(s): State<AppState>) -> Result<Json<Value>, ApiError> {
    Ok(Json(s.with_db(|db| db.registry_counts()).await?))
}

#[derive(Deserialize)]
struct ImportBody {
    label: String,
    entities: Vec<Entity>,
}

async fn registry_import(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ImportBody>,
) -> Result<Json<Value>, ApiError> {
    let actor = actor(&headers);
    let counts = s
        .with_db(move |db| db.registry_import(&body.entities, &actor, &body.label))
        .await?;
    Ok(Json(serde_json::to_value(counts).unwrap_or_default()))
}

#[derive(Deserialize)]
struct TracksQuery {
    source: Option<String>,
    limit: Option<usize>,
}

/// Live system tracks, newest report first.
async fn list_tracks(
    State(s): State<AppState>,
    Query(q): Query<TracksQuery>,
) -> Result<Json<Value>, ApiError> {
    let mut tracks = s.redis.list_system_tracks().await?;
    if let Some(src) = &q.source {
        tracks.retain(|t| t.contributors.iter().any(|c| &c.source_id == src));
    }
    tracks.sort_by_key(|t| std::cmp::Reverse(t.last_seen));
    let total = tracks.len();
    let ctx = s.common.publish_context();
    let now = chrono::Utc::now();
    let items: Vec<Value> = tracks
        .into_iter()
        .take(q.limit.unwrap_or(500).min(10_000))
        .map(|t| {
            // The GOLD fields exactly as published.
            let m = ot_core::wire::to_message(&t, &ctx, now);
            json!({
                "uid": t.uid,
                "track_id": m.track_id,
                "kind": m.kind,
                "members": m.members,
                "groups": m.groups,
                "paired_with": m.paired_with,
                "entity_id": t.entity_id,
                "notices": t.notices.len(),
                "state": t.state,
                "class": m.class,
                "gold_name": m.name,
                "marking": t.view.security.as_ref().map(ot_core::SecurityLabel::marking),
                "domain": m.domain,
                "affiliation": m.affiliation,
                "force_code": m.force_code,
                "track_type": m.track_type,
                "sidc": m.sidc,
                "name": t.view.name,
                "callsign": t.view.callsign,
                "classification": t.view.classification.cot_type_or_derived(),
                "identifiers": t.view.identifiers,
                "latitude": t.view.position.latitude,
                "longitude": t.view.position.longitude,
                "course_deg": t.view.kinematics.course_deg,
                "speed_mps": t.view.kinematics.speed_mps,
                "last_seen": t.last_seen,
                "observation_count": t.observation_count,
                "published": t.is_published(),
                "confidence": t.confidence(),
                "filtered": t.filtered,
                "sources": t.contributors.iter().map(|c| format!("{}/{}", c.source_id, c.source_track_key)).collect::<Vec<_>>(),
                "registry": t.view.ext.get("registry"),
            })
        })
        .collect();
    Ok(Json(json!({ "total": total, "tracks": items })))
}

/// The whole schema: fixed core fields, every extension version, and which
/// sources target which version.
async fn schema_overview(State(s): State<AppState>) -> Result<Json<Value>, ApiError> {
    let versions = s.with_db(|db| db.schema_versions()).await?;
    let sources = s.with_db(|db| db.list_sources()).await?;
    let usage: Vec<Value> = sources
        .iter()
        .map(|r| {
            json!({
                "source": r.id,
                "enabled": r.enabled,
                "schema_version": r.spec.pointer("/pipeline/mapping/schema_version").cloned().unwrap_or(json!(1)),
            })
        })
        .collect();
    let latest = versions
        .iter()
        .filter(|v| v.status == "published")
        .map(|v| v.version)
        .max();
    let builtins: Vec<Value> = ot_source::schema::Builtin::ALL
        .iter()
        .map(|b| json!({ "name": b, "type": b.kind(), "reads": b.reads() }))
        .collect();
    Ok(Json(json!({
        // Always published (the OTH-GOLD minimum); the schema adds `attributes`.
        "published_core": ["track_id", "class", "name", "domain", "affiliation", "force_code",
                           "track_type", "sidc", "time", "lat", "lon"],
        "builtins": builtins,
        // Where each always-published field comes from, for the pipeline view.
        "gold_sources": ot_core::wire::GOLD_SOURCES
            .iter()
            .map(|(k, v)| (k.to_string(), json!(v)))
            .collect::<serde_json::Map<_, _>>(),
        "core": ot_source::mapping::target_fields().collect::<Vec<_>>(),
        "reserved_extension_keys": ot_source::schema::RESERVED_KEYS,
        "latest_published": latest,
        "versions": versions,
        "sources": usage,
    })))
}

#[derive(Deserialize)]
struct DraftBody {
    fields: Vec<Value>,
    #[serde(default)]
    notes: Option<String>,
}

async fn put_schema_draft(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<DraftBody>,
) -> Result<Json<Value>, ApiError> {
    let fields: Vec<ExtensionField> = body
        .fields
        .iter()
        .cloned()
        .map(serde_json::from_value)
        .collect::<Result<_, _>>()
        .map_err(|e| ApiError::unprocessable(format!("invalid field definition: {e}")))?;
    ExtensionSchema {
        version: 0,
        fields: fields.clone(),
    }
    .validate()
    .map_err(|e| ApiError::unprocessable(e.to_string()))?;
    let normalised: Vec<Value> = fields
        .iter()
        .map(|f| serde_json::to_value(f).expect("field serialises"))
        .collect();
    let actor = actor(&headers);
    let draft = s
        .with_db(move |db| db.put_schema_draft(&normalised, body.notes.as_deref(), &actor))
        .await?;
    Ok(Json(serde_json::to_value(draft).unwrap_or_default()))
}

#[derive(Deserialize)]
struct ProtoFiles {
    /// `.proto` files, name → contents.
    files: std::collections::BTreeMap<String, String>,
}

/// Compile a producer's `.proto` files and say what they define (methods,
/// messages and their fields), for the source editor; or why they do not
/// compile (`file:line: what`).
async fn describe_protobuf(Json(req): Json<ProtoFiles>) -> Result<Json<Value>, ApiError> {
    let set = tokio::task::spawn_blocking(move || ot_source::proto::ProtoSet::compile(&req.files))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .map_err(|e| ApiError::unprocessable(e.to_string()))?;
    Ok(Json(
        serde_json::to_value(set.describe()).unwrap_or_default(),
    ))
}

async fn publish_schema_draft(
    State(s): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let (actor, site) = (actor(&headers), s.common.site);
    let v = s
        .with_db(move |db| {
            let v = db.publish_schema_draft(&actor)?;
            crate::settings_api::share_profile(
                db,
                site,
                &actor,
                &serde_json::json!({ "op": "profile_schema", "fields": v.fields, "notes": v.notes }),
            )?;
            Ok(v)
        })
        .await?;
    Ok(Json(serde_json::to_value(v).unwrap_or_default()))
}

async fn discard_schema_draft(
    State(s): State<AppState>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let actor = actor(&headers);
    s.with_db(move |db| db.discard_schema_draft(&actor)).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use crate::config::Common;
    use crate::control::{AppState, router};

    const AIS: &str = include_str!("../../../docs/examples/aisstream.json");
    const ADSB: &str = include_str!("../../../docs/examples/adsb-lol.json");
    const SCHEMA: &str = include_str!("../../../docs/examples/schema.json");

    #[test]
    fn worked_examples_stay_valid() {
        let schema: Value = serde_json::from_str(SCHEMA).unwrap();
        let schema = ot_source::schema::ExtensionSchema {
            version: 2,
            fields: serde_json::from_value(schema["fields"].clone()).unwrap(),
        };
        schema.validate().unwrap();
        for (name, text) in [("aisstream", AIS), ("adsb-lol", ADSB)] {
            let spec: ot_source::source::SourceSpec =
                serde_json::from_str(text).unwrap_or_else(|e| panic!("{name}: {e}"));
            spec.validate_against(Some(&schema))
                .unwrap_or_else(|e| panic!("{name}: {e}"));
        }
        // Examples that use only the core schema.
        for (name, text) in [
            (
                "stanag4607",
                include_str!("../../../docs/examples/stanag4607.json"),
            ),
            (
                "sapient",
                include_str!("../../../docs/examples/sapient.json"),
            ),
            (
                "gps-udp",
                include_str!("../../../docs/examples/gps-udp.json"),
            ),
            (
                "autoferry track",
                include_str!("../../../docs/examples/autoferry/demo-autoferry-track.json"),
            ),
            (
                "autoferry lidar",
                include_str!("../../../docs/examples/autoferry/demo-autoferry-lidar.json"),
            ),
            (
                "autoferry radar",
                include_str!("../../../docs/examples/autoferry/demo-autoferry-radar.json"),
            ),
        ] {
            let spec: ot_source::source::SourceSpec =
                serde_json::from_str(text).unwrap_or_else(|e| panic!("{name}: {e}"));
            spec.validate().unwrap_or_else(|e| panic!("{name}: {e}"));
        }
    }

    async fn publish_example_schema(app: &axum::Router) {
        let (st, body) = call(
            app,
            "PUT",
            "/api/v1/schema/draft",
            Some(serde_json::from_str(SCHEMA).unwrap()),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{body}");
        let (st, body) = call(app, "POST", "/api/v1/schema/draft/publish", None).await;
        assert_eq!(
            (st, body["version"].as_u64()),
            (StatusCode::OK, Some(2)),
            "{body}"
        );
    }

    /// The router over an in-memory database and an isolated Redis
    /// namespace; `None` (test skipped) without `OT_TEST_REDIS_URL`.
    async fn app() -> Option<(axum::Router, ot_store::RedisStore)> {
        app_with_ui(None).await
    }

    async fn app_with_ui(
        ui: Option<std::path::PathBuf>,
    ) -> Option<(axum::Router, ot_store::RedisStore)> {
        let url = std::env::var("OT_TEST_REDIS_URL").ok()?;
        // Unique per test: two starting in the same microsecond must not
        // share (and purge) a namespace.
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let ns = format!(
            "ot-api-test-{}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_micros(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let common = Common {
            sqlite: ":memory:".into(),
            redis: url.clone(),
            redis_ca: None,
            redis_cert: None,
            redis_key: None,
            redis_namespace: ns.clone(),
            site: ot_core::SiteCode::new("TST").unwrap(),
            nats: crate::config::NatsArgs {
                // Nothing listens here: status reports it, nothing blocks.
                url: "nats://127.0.0.1:9".into(),
                creds: None,
                token: None,
                user: None,
                password: None,
                ca: None,
                cert: None,
                key: None,
                stream: "TRACKS".into(),
                tracks_subject: "tracks".into(),
                max_age_hours: 24.0,
            },
            profiles_dir: "profiles/trackers".into(),
            obs_window_secs: 600,
            shared_db: Default::default(),
        };
        let redis = ot_store::RedisStore::connect(&url, ot_store::Keys::new(ns))
            .await
            .unwrap();
        let state = AppState {
            db: Arc::new(Mutex::new(ot_store::Db::open_in_memory().unwrap())),
            redis: redis.clone(),
            nats: common.connect_nats().await.unwrap(),
            auth: Arc::new(crate::auth::Auth::off()),
            common,
        };
        Some((router(state, ui), redis))
    }

    async fn call(
        app: &axum::Router,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut req = Request::builder()
            .method(method)
            .uri(uri)
            .header("x-opentrack-actor", "op:test");
        let body = match body {
            Some(b) => {
                req = req.header("content-type", "application/json");
                Body::from(b.to_string())
            }
            None => Body::empty(),
        };
        let res = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    #[tokio::test]
    async fn a_listener_needs_authentication_or_a_recorded_acceptance() {
        let Some((app, _redis)) = app().await else {
            eprintln!("skipped: OT_TEST_REDIS_URL not set");
            return;
        };
        publish_example_schema(&app).await;
        let mut spec: Value =
            serde_json::from_str(include_str!("../../../docs/examples/gps-udp.json")).unwrap();
        spec["id"] = json!("udp-auth");
        let accepted = spec.clone();
        spec.as_object_mut().unwrap().remove("unauthenticated");
        let (st, body) = call(&app, "POST", "/api/v1/sources", Some(spec.clone())).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("UDP cannot carry TLS"),
            "{body}"
        );
        let (st, body) = call(&app, "POST", "/api/v1/sources", Some(accepted)).await;
        assert_eq!(st, StatusCode::CREATED, "{body}");
        // Moved to mutual TLS, the acceptance goes: a decision whose before
        // and after (the two revisions) show the change.
        spec["transport"] = json!({"type": "tcp_server", "bind": "127.0.0.1:0",
            "tls": {"cert_file": "s.pem", "key_file": "s.key", "client_ca_file": "ca.pem"}});
        let (st, body) = call(&app, "PUT", "/api/v1/sources/udp-auth", Some(spec)).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        let (_, revs) = call(&app, "GET", "/api/v1/sources/udp-auth/revisions", None).await;
        let rev = |n: i64| {
            revs["revisions"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["revision"] == n)
                .cloned()
                .unwrap()
        };
        assert_eq!(rev(1)["spec"]["unauthenticated"], "accepted", "{revs}");
        assert!(rev(2)["spec"].get("unauthenticated").is_none(), "{revs}");
        assert!(rev(2)["decision_id"].is_i64(), "{revs}");
        let (_, d) = call(&app, "GET", "/api/v1/decisions?op=update_source", None).await;
        assert_eq!(d["decisions"][0]["evidence"]["source"], "udp-auth", "{d}");
    }

    #[tokio::test]
    async fn source_lifecycle_through_the_api() {
        let Some((app, redis)) = app().await else {
            eprintln!("skipped: OT_TEST_REDIS_URL not set");
            return;
        };
        let spec: Value = serde_json::from_str(ADSB).unwrap();

        // Its extension schema is not published yet.
        let (st, body) = call(&app, "POST", "/api/v1/sources", Some(spec.clone())).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            body["error"].as_str().unwrap().contains("schema version 2"),
            "{body}"
        );
        publish_example_schema(&app).await;

        let (st, body) = call(&app, "POST", "/api/v1/sources", Some(spec.clone())).await;
        assert_eq!(st, StatusCode::CREATED, "{body}");
        assert_eq!(
            (body["revision"].as_i64(), body["enabled"].as_bool()),
            (Some(1), Some(false))
        );
        assert_eq!(body["transport"], "http_poll");

        let (st, _) = call(&app, "POST", "/api/v1/sources", Some(spec.clone())).await;
        assert_eq!(st, StatusCode::CONFLICT);

        let (st, body) = call(&app, "PUT", "/api/v1/sources/other-id", Some(spec.clone())).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{body}");

        let mut typo = spec.clone();
        typo["pipeline"]["mapping"]["rules"][1]["fields"]["kinematic.speed_mps"] = json!("gs");
        let (st, body) = call(&app, "PUT", "/api/v1/sources/adsb-lol", Some(typo)).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("kinematic.speed_mps"),
            "{body}"
        );

        let mut undeclared = spec.clone();
        undeclared["pipeline"]["mapping"]["rules"][1]["fields"]["ext.tail_art"] = json!("r");
        let (st, body) = call(&app, "PUT", "/api/v1/sources/adsb-lol", Some(undeclared)).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            body["error"].as_str().unwrap().contains("ext.tail_art"),
            "{body}"
        );

        let mut edited = spec.clone();
        edited["name"] = json!("adsb.lol (edited)");
        let (st, body) = call(&app, "PUT", "/api/v1/sources/adsb-lol", Some(edited)).await;
        assert_eq!((st, body["revision"].as_i64()), (StatusCode::OK, Some(2)));

        let (_, body) = call(&app, "POST", "/api/v1/sources/adsb-lol/enable", None).await;
        assert_eq!(body["enabled"], true);
        let (_, body) = call(&app, "GET", "/api/v1/sources/adsb-lol/revisions", None).await;
        assert_eq!(body["revisions"].as_array().unwrap().len(), 2);
        let (_, body) = call(&app, "GET", "/api/v1/sources", None).await;
        assert_eq!(body["sources"][0]["name"], "adsb.lol (edited)");

        // Raw output needs explicit consent and a subject outside `tracks.>`.
        for bad in [
            json!({"consent": false}),
            json!({"subject": "tracks.raw", "consent": true}),
            json!({"subject": "tracks", "consent": true}),
            json!({"subject": "raw.*", "consent": true}),
            json!({"subject": "raw..adsb", "consent": true}),
        ] {
            let (st, _) = call(
                &app,
                "PUT",
                "/api/v1/sources/adsb-lol/raw-output",
                Some(bad.clone()),
            )
            .await;
            assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{bad}");
        }
        let (st, body) = call(
            &app,
            "PUT",
            "/api/v1/sources/adsb-lol/raw-output",
            Some(json!({"consent": true})),
        )
        .await;
        assert_eq!(
            (st, body["raw_subject"].as_str()),
            (StatusCode::OK, Some("opentrack.raw.adsb-lol"))
        );
        let (st, body) = call(
            &app,
            "PUT",
            "/api/v1/sources/adsb-lol/raw-output",
            Some(json!({"subject": "raw.adsb", "consent": true})),
        )
        .await;
        assert_eq!(
            (st, body["raw_subject"].as_str()),
            (StatusCode::OK, Some("raw.adsb"))
        );

        // Status reports NATS as down without blocking the API: 503, with
        // the body saying which dependency.
        let (st, body) = call(&app, "GET", "/api/v1/status", None).await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["nats"]["ok"], false);
        assert_eq!(body["sqlite"]["ok"], true);
        assert_eq!(body["redis"]["ok"], true);
        assert_eq!(body["service"], "opentrack");
        // Liveness is not readiness: the process answers.
        let (st, _) = call(&app, "GET", "/healthz", None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["nats"]["stream"], "TRACKS");

        let (st, _) = call(&app, "DELETE", "/api/v1/sources/adsb-lol", None).await;
        assert_eq!(st, StatusCode::NO_CONTENT);
        let (st, _) = call(&app, "GET", "/api/v1/sources/adsb-lol", None).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        redis.purge_namespace().await.unwrap();
    }

    #[tokio::test]
    async fn schema_workspace() {
        let Some((app, redis)) = app().await else {
            eprintln!("skipped: OT_TEST_REDIS_URL not set");
            return;
        };
        let (_, body) = call(&app, "GET", "/api/v1/schema", None).await;
        assert_eq!(body["latest_published"], 1);
        assert!(
            body["core"]
                .as_array()
                .unwrap()
                .contains(&json!("position.latitude"))
        );

        let (st, _) = call(
            &app,
            "PUT",
            "/api/v1/schema/draft",
            Some(json!({"fields": [{"key": "registry", "type": "json"}]})),
        )
        .await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "reserved key");
        let (st, _) = call(
            &app,
            "PUT",
            "/api/v1/schema/draft",
            Some(json!({"fields": [{"key": "mode", "type": "enum"}]})),
        )
        .await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "enum without values");

        publish_example_schema(&app).await;
        let (_, body) = call(&app, "GET", "/api/v1/schema", None).await;
        assert_eq!(body["latest_published"], 2);
        assert_eq!(body["versions"][1]["fields"].as_array().unwrap().len(), 8);
        let (st, _) = call(&app, "POST", "/api/v1/schema/draft/publish", None).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "no draft left to publish");
        redis.purge_namespace().await.unwrap();
    }

    #[tokio::test]
    async fn dry_run_and_registry_with_any_identifier_scheme() {
        let Some((app, redis)) = app().await else {
            eprintln!("skipped: OT_TEST_REDIS_URL not set");
            return;
        };
        let (st, body) = call(&app, "POST", "/api/v1/registry/import", Some(json!({
            "label": "test",
            "entities": [{"id": "cvn75", "name": "USS HARRY S TRUMAN", "fields": {"cot": "a-f-S-C-A"},
                "identifiers": [{"scheme": "mmsi", "value": "338000001"}, {"scheme": "imo", "value": "9876543"},
                                {"scheme": "elnot", "value": "NL504"}]}]
        }))).await;
        assert_eq!(
            (st, body["identifiers"].as_u64()),
            (StatusCode::OK, Some(3)),
            "{body}"
        );
        let (st, body) = call(
            &app,
            "GET",
            "/api/v1/registry?scheme=elnot&value=NL504",
            None,
        )
        .await;
        assert_eq!((st, body["id"].as_str()), (StatusCode::OK, Some("cvn75")));

        // Dry run: a CoT event carrying an ELNOT resolves through the registry.
        let spec = json!({
            "id": "cot-test", "name": "CoT test",
            "transport": {"type": "udp", "bind": "127.0.0.1:0"}, "unauthenticated": "accepted",
            "pipeline": {
                "codec": {"type": "cot_xml"},
                "mapping": {"rules": [{"name": "event", "key": "event.@uid",
                    "identifiers": [{"scheme": "elnot", "value": "event.detail.elnot.@code"}],
                    "fields": {"position.latitude": "event.point.@lat", "position.longitude": "event.point.@lon",
                               "classification.cot_type": "event.@type", "name": "event.detail.contact.@callsign"}}]},
                "registry": {"apply": {"classification.cot_type": "cot", "platform.name": "name"}}
            }
        });
        let frame = r#"<event uid="E1" type="a-u-S" time="2026-09-25T00:00:00Z"><point lat="10" lon="20"/>
            <detail><contact callsign="HARRY S TRUMAN"/><elnot code="NL504"/></detail></event>"#;
        let (st, body) = call(
            &app,
            "POST",
            "/api/v1/sources/validate",
            Some(json!({"spec": spec, "samples": [frame]})),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{body}");
        let o = &body["observations"][0];
        assert_eq!(
            o["identifiers"][0],
            json!({"scheme": "elnot", "value": "NL504"})
        );
        assert_eq!(o["ext"]["registry"]["scheme"], "elnot");
        assert_eq!(o["ext"]["registry"]["grade"], "name");
        assert_eq!(o["classification"]["cot_type"], "a-f-S-C-A");
        redis.purge_namespace().await.unwrap();
    }

    #[tokio::test]
    async fn dry_run_traces_the_first_records_stage_by_stage() {
        let Some((app, redis)) = app().await else {
            eprintln!("skipped: OT_TEST_REDIS_URL not set");
            return;
        };
        let spec = json!({
            "id": "trace-test", "name": "Trace test",
            "transport": {"type": "udp", "bind": "127.0.0.1:0"}, "unauthenticated": "accepted",
            "pipeline": {
                "codec": {"type": "json"},
                "mapping": {"rules": [{"name": "pos", "key": "id",
                    "fields": {"position.latitude": "lat", "position.longitude": "lon", "name": "name"}}]},
                "filter": {"drop_if": {"path": "name", "eq": "NOISE"}}
            }
        });
        let mut samples = vec![
            json!({"id": "A", "lat": 10, "lon": 20, "name": "alpha"}).to_string(),
            json!({"id": "B", "lat": 11, "lon": 21, "name": "NOISE"}).to_string(),
            "not json".to_owned(),
        ];
        samples.extend((0..12).map(|i| json!({"id": i, "lat": 1, "lon": 2}).to_string()));
        let validate = |trace: Value| {
            call(
                &app,
                "POST",
                "/api/v1/sources/validate",
                Some(json!({"spec": spec, "samples": samples, "trace": trace})),
            )
        };
        let (st, plain) = validate(Value::Null).await;
        assert_eq!(st, StatusCode::OK, "{plain}");
        assert!(plain.get("trace").is_none(), "off unless asked");
        let (_, zero) = validate(json!(0)).await;
        assert!(zero.get("trace").is_none());
        // Frames are stamped as they arrive, so compare all but the times.
        let keys = |b: &Value| -> Vec<Value> {
            b["observations"]
                .as_array()
                .unwrap()
                .iter()
                .map(|o| o["source_track_key"].clone())
                .collect()
        };

        let (st, body) = validate(json!(3)).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        for k in ["valid", "counts", "errors"] {
            assert_eq!(body[k], plain[k], "{k} unchanged by tracing");
        }
        assert_eq!(keys(&body), keys(&plain));
        let frames = body["trace"]["frames"].as_array().unwrap();
        let t = body["trace"]["samples"].as_array().unwrap();
        assert_eq!((t.len(), frames.len()), (3, 3), "one record per frame");
        assert_eq!(t[0]["frame"], 0);
        assert_eq!(frames[0]["format"], "json");
        assert_eq!(frames[0]["content"]["name"], "alpha");
        let ids: Vec<&str> = t[0]["stages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["decode", "map", "registry", "filter", "publish"]);
        let publish = &t[0]["stages"][4]["items"][0];
        assert_eq!(publish["name"], "ALPHA", "the published message");
        assert_eq!(publish["op"], "upsert");
        assert_eq!(publish["track_id"], "assigned by correlation");
        assert_eq!(t[1]["stages"][3]["dropped"], json!(["drop_if matched"]));
        assert_eq!(t[1]["stages"][4]["items"], json!([]));
        assert_eq!(
            frames[2],
            json!({"format": "text", "bytes": 8, "content": "not json"})
        );
        assert!(
            t[2]["stages"][0]["dropped"][0]
                .as_str()
                .unwrap()
                .starts_with("decode error")
        );

        let (_, body) = validate(json!(50)).await;
        assert_eq!(
            body["trace"]["samples"].as_array().unwrap().len(),
            10,
            "capped"
        );
        redis.purge_namespace().await.unwrap();
    }

    /// Probe a live MQTT topic, keep the samples, and preview a mapping that
    /// takes the track key from the topic. Needs `OT_TEST_MQTT_URL` too.
    #[tokio::test]
    async fn mqtt_probe_keeps_topics_for_the_preview() {
        let Ok(mqtt) = std::env::var("OT_TEST_MQTT_URL") else {
            eprintln!("skipped: OT_TEST_MQTT_URL not set");
            return;
        };
        let Some((app, redis)) = app().await else {
            eprintln!("skipped: OT_TEST_REDIS_URL not set");
            return;
        };
        let run_id = format!("otapi{}", chrono::Utc::now().timestamp_micros());
        let (host, port) = mqtt
            .trim_start_matches("mqtt://")
            .rsplit_once(':')
            .map(|(h, p)| (h.to_owned(), p.parse::<u16>().unwrap()))
            .unwrap();
        let (client, mut events) = rumqttc::AsyncClient::new(
            rumqttc::MqttOptions::new(format!("{run_id}-pub"), host, port),
            16,
        );
        let pump = tokio::spawn(async move { while events.poll().await.is_ok() {} });
        let topic = format!("{run_id}/366123456/pos");
        let publisher = tokio::spawn(async move {
            loop {
                let _ = client
                    .publish(
                        topic.clone(),
                        rumqttc::QoS::AtMostOnce,
                        false,
                        r#"{"lat":32.7,"lon":-117.2}"#,
                    )
                    .await;
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        });

        let transport = json!({"type": "mqtt", "url": mqtt, "topics": [format!("{run_id}/+/pos")]});
        let (st, body) = call(
            &app,
            "POST",
            "/api/v1/probe",
            Some(json!({
                "transport": transport, "codec": {"type": "json"},
                "max_frames": 3, "max_secs": 10, "save_as": "mqtt-test"
            })),
        )
        .await;
        publisher.abort();
        pump.abort();
        assert_eq!(st, StatusCode::OK, "{body}");
        assert_eq!(body["frames"], 3, "{body}");
        assert_eq!(body["samples_saved"], 3);
        assert_eq!(
            body["sample_frames"][0]["meta"]["topic_levels"][1],
            "366123456"
        );
        // Inference sees the topic like any other field.
        assert!(
            body["fields"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f["path"] == "_frame.topic"),
            "{body}"
        );

        let spec = json!({
            "id": "mqtt-test", "name": "MQTT test", "transport": transport,
            "pipeline": {
                "codec": {"type": "json"},
                "mapping": {"rules": [{"name": "pos", "key": "_frame.topic_levels[1]",
                    "identifiers": [{"scheme": "mmsi", "value": "_frame.topic_levels[1]"}],
                    "fields": {"position.latitude": "lat", "position.longitude": "lon"}}]}
            }
        });
        let (st, body) = call(
            &app,
            "POST",
            "/api/v1/sources/validate",
            Some(json!({"spec": spec, "stored_samples_of": "mqtt-test"})),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{body}");
        let o = &body["observations"][0];
        assert_eq!(o["source_track_key"], "366123456", "{body}");
        assert_eq!(
            o["identifiers"][0],
            json!({"scheme": "mmsi", "value": "366123456"})
        );
        redis.purge_namespace().await.unwrap();
    }

    /// Raw request and response bodies (spreadsheets).
    async fn call_raw(
        app: &axum::Router,
        method: &str,
        uri: &str,
        body: Vec<u8>,
    ) -> (StatusCode, Vec<u8>) {
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .header("x-opentrack-actor", "op:test")
            .body(Body::from(body))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        (
            status,
            res.into_body().collect().await.unwrap().to_bytes().to_vec(),
        )
    }

    #[tokio::test]
    async fn settings_banner_exports_and_purge_confirmation() {
        let Some((app, _redis)) = app().await else {
            eprintln!("skipped: OT_TEST_REDIS_URL not set");
            return;
        };
        let (st, body) = call(&app, "GET", "/api/v1/settings", None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["settings"]["banner"]["enabled"], false);
        assert_eq!(body["site_code"], "TST");
        let (_, b) = call(&app, "GET", "/api/v1/public/banner", None).await;
        assert_eq!(b["enabled"], false);

        let bad = json!({"banner": {"enabled": true, "text": "SECRET", "background": "red", "color": "#ffffff"}});
        let (st, _) = call(&app, "PUT", "/api/v1/settings", Some(bad)).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "colours are #rrggbb");
        let good = json!({"site_name": "Garden Island", "banner": {"enabled": true, "text": "SECRET", "background": "#c8102e", "color": "#ffffff"}});
        let (st, body) = call(&app, "PUT", "/api/v1/settings", Some(good)).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert_eq!(body["settings"]["site_name"], "Garden Island");
        // Shaped as OpenStare's /api/public/banner.
        let (_, b) = call(&app, "GET", "/api/v1/public/banner", None).await;
        assert_eq!(
            b,
            json!({"enabled": true, "text": "SECRET", "background": "#c8102e", "color": "#ffffff"})
        );
        for (uri, starts) in [
            ("/api/v1/export/tracks?format=geojson", "{"),
            (
                "/api/v1/export/tracks?format=csv",
                "SECRET\nclassification,track_id,",
            ),
            ("/api/v1/export/config", "{"),
            ("/api/v1/audit?format=csv", "SECRET\nseq,"),
            ("/api/v1/registry/export?format=csv", "SECRET\nentity_id,"),
        ] {
            let (st, bytes) = call_raw(&app, "GET", uri, Vec::new()).await;
            assert_eq!(st, StatusCode::OK, "{uri}");
            let text = String::from_utf8_lossy(&bytes);
            assert!(text.starts_with(starts), "{uri}: {text}");
        }
        // A labelled track and an unlabelled one (the banner's SECRET):
        // the file takes the highest, and each labelled track its own.
        let track = |n: u64, security: Value| {
            let t: chrono::DateTime<chrono::Utc> = chrono::Utc::now();
            let obs = serde_json::from_value(json!({
                "schema_version": 1, "source_id": "ais", "source_track_key": format!("k{n}"),
                "observed_at": t, "received_at": t, "security": security,
                "position": { "latitude": 32.68, "longitude": -117.23 }
            }))
            .unwrap();
            let uid = ot_core::Uid::new(ot_core::SiteCode::new("TST").unwrap(), n).unwrap();
            ot_core::SystemTrack::from_first_observation(uid, obs)
        };
        let labelled = track(
            1,
            json!({"classification": "UNCLASSIFIED", "restrictions": ["FOUO"], "sharing": "GBR, USA"}),
        );
        _redis.put_system_track(&labelled, true).await.unwrap();
        _redis
            .put_system_track(&track(2, Value::Null), true)
            .await
            .unwrap();
        let (_, bytes) =
            call_raw(&app, "GET", "/api/v1/export/tracks?format=csv", Vec::new()).await;
        let text = String::from_utf8(bytes).unwrap();
        let mut r = csv::ReaderBuilder::new()
            .has_headers(false)
            .flexible(true)
            .from_reader(text.as_bytes());
        let rows: Vec<Vec<String>> = r
            .records()
            .map(|x| x.unwrap().iter().map(str::to_owned).collect())
            .collect();
        assert_eq!(rows[0], ["(S//FOUO/REL TO USA, GBR)"], "{text}");
        assert_eq!(rows[1][0], "classification");
        let mut marks: Vec<&str> = rows[2..].iter().map(|r| r[0].as_str()).collect();
        marks.sort();
        assert_eq!(marks, ["", "(U//FOUO/REL TO USA, GBR)"]);
        let (_, bytes) = call_raw(
            &app,
            "GET",
            "/api/v1/export/tracks?format=geojson",
            Vec::new(),
        )
        .await;
        let g: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(g["security"]["marking"], "(S//FOUO/REL TO USA, GBR)");
        let f = g["features"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["id"] == labelled.uid.doc_id())
            .unwrap();
        assert_eq!(f["properties"]["marking"], "(U//FOUO/REL TO USA, GBR)");
        assert_eq!(
            f["properties"]["security"]["classification"],
            "UNCLASSIFIED"
        );
        let (st, _) = call(
            &app,
            "POST",
            "/api/v1/admin/purge",
            Some(json!({"confirm": "nope"})),
        )
        .await;
        assert_eq!(
            st,
            StatusCode::UNPROCESSABLE_ENTITY,
            "the site code must be typed"
        );
    }

    #[tokio::test]
    async fn tak_outputs_are_saved_checked_and_reported() {
        let Some((app, _redis)) = app().await else {
            eprintln!("skipped: OT_TEST_REDIS_URL not set");
            return;
        };
        let bad =
            json!({"tak": {"outputs": [{"id": "x", "delivery": {"kind": "multicast", "ttl": 0}}]}});
        let (st, body) = call(&app, "PUT", "/api/v1/settings", Some(bad)).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        let good = json!({"tak": {"outputs": [
            {"id": "sa", "delivery": {"kind": "multicast"}},
            {"id": "eud", "enabled": false, "delivery": {"kind": "listen", "bind": "0.0.0.0:8089",
                "tls": {"cert_file": "/etc/ot/tak.pem", "key_file": "/etc/ot/tak.key"}}},
        ]}});
        let (st, body) = call(&app, "PUT", "/api/v1/settings", Some(good)).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert_eq!(
            body["settings"]["tak"]["outputs"][0]["delivery"]["group"],
            "239.2.3.1"
        );
        let (st, body) = call(&app, "GET", "/api/v1/tak/status", None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["running"], false);
        let outs = body["outputs"].as_array().unwrap();
        assert_eq!(
            (
                outs[0]["id"].as_str(),
                outs[0]["state"].as_str(),
                outs[0]["encrypted"].as_bool()
            ),
            (Some("sa"), Some("not started"), Some(false))
        );
        assert_eq!(
            (outs[1]["state"].as_str(), outs[1]["encrypted"].as_bool()),
            (Some("off"), Some(true))
        );
    }

    #[tokio::test]
    async fn the_full_configuration_exports_and_only_an_empty_node_imports() {
        let Some((app, _redis)) = app().await else {
            eprintln!("skipped: OT_TEST_REDIS_URL not set");
            return;
        };
        let (st, status) = call(&app, "GET", "/api/v1/import/config", None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(status["empty"], true, "{status}");
        let good = json!({"site_name": "Garden Island"});
        let (st, _) = call(&app, "PUT", "/api/v1/settings", Some(good)).await;
        assert_eq!(st, StatusCode::OK);
        let (_, status) = call(&app, "GET", "/api/v1/import/config", None).await;
        assert_eq!(status["empty"], false);

        // A sensitive download: an attachment no cache keeps.
        let req = Request::builder()
            .uri("/api/v1/export/config")
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let h = res.headers();
        assert!(
            h[header::CONTENT_DISPOSITION]
                .to_str()
                .unwrap()
                .starts_with("attachment; filename=\"opentrack-config-TST-")
        );
        assert!(
            h[header::CACHE_CONTROL]
                .to_str()
                .unwrap()
                .contains("no-store")
        );
        let file: Value =
            serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert_eq!(file["format"], "opentrack-config");
        assert_eq!(file["version"], 2);
        assert_eq!(file["app_settings"]["site_name"], "Garden Island");
        let (_, decisions) = call(&app, "GET", "/api/v1/decisions?op=export_config", None).await;
        assert_eq!(
            decisions["decisions"][0]["actor"], "anonymous",
            "{decisions}"
        );

        // This node has settings now: an import is refused, saying so.
        let (st, body) = call(&app, "POST", "/api/v1/import/config", Some(file)).await;
        assert_eq!(st, StatusCode::CONFLICT, "{body}");
        assert!(
            body["present"][0]
                .as_str()
                .unwrap()
                .contains("instance settings")
        );
        // Whatever the file: a configured node is never overwritten.
        let (st, _) = call(&app, "POST", "/api/v1/import/config", Some(json!({"x": 1}))).await;
        assert_eq!(st, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn the_registry_round_trips_through_a_spreadsheet() {
        let Some((app, _redis)) = app().await else {
            eprintln!("skipped: OT_TEST_REDIS_URL not set");
            return;
        };
        // An earlier sheet's registry and card columns import as attributes.
        let csv = "name,domain,id:mmsi,registry:flag,attr:crew:number,card:owner\n\
                   TED STEVENS,surface,338000001,US,12,MSC\nOTHER,,366000002,,,\n";
        // A dry run writes nothing.
        let (st, body) = call_raw(
            &app,
            "POST",
            "/api/v1/registry/import-sheet?format=csv",
            csv.into(),
        )
        .await;
        let plan: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(st, StatusCode::OK, "{plan}");
        assert_eq!(plan["counts"]["create"], 2);
        assert_eq!(plan["applied"], false);
        let (_, list) = call(&app, "GET", "/api/v1/registry/entities", None).await;
        assert_eq!(list["total"], 0);

        let (st, body) = call_raw(
            &app,
            "POST",
            "/api/v1/registry/import-sheet?format=csv&apply=true",
            csv.into(),
        )
        .await;
        let done: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            (st, &done["applied"]),
            (StatusCode::OK, &json!(true)),
            "{done}"
        );
        let (_, list) = call(&app, "GET", "/api/v1/registry/entities?q=338", None).await;
        assert_eq!(list["total"], 1, "{list}");
        let ted = &list["entities"][0];
        assert_eq!(ted["name"], "TED STEVENS");
        assert_eq!(ted["domain"], "surface");
        assert_eq!(
            ted["attributes"],
            json!([{"key": "flag", "type": "text", "value": "US"},
                   {"key": "crew", "type": "number", "value": 12},
                   {"key": "owner", "type": "text", "value": "MSC"}])
        );

        // The same sheet again changes nothing; a conflicting row is refused whole.
        let (_, body) = call_raw(
            &app,
            "POST",
            "/api/v1/registry/import-sheet?format=csv",
            csv.into(),
        )
        .await;
        let again: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(again["counts"]["unchanged"], 2, "{again}");
        let bad = "entity_id,id:mmsi\nent-x,338000001\n";
        let (st, _) = call_raw(
            &app,
            "POST",
            "/api/v1/registry/import-sheet?format=csv&apply=true",
            bad.into(),
        )
        .await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);

        for format in ["csv", "xlsx"] {
            let (st, bytes) = call_raw(
                &app,
                "GET",
                &format!("/api/v1/registry/export?format={format}"),
                Vec::new(),
            )
            .await;
            assert_eq!(st, StatusCode::OK);
            let sheet = crate::registry_sheet::read(
                &bytes,
                crate::registry_sheet::Format::parse(format).unwrap(),
            )
            .unwrap();
            assert_eq!(sheet.rows.len(), 2, "{format}");
            assert!(sheet.header.contains(&"attr:crew:number".to_owned()));
        }
    }

    #[tokio::test]
    async fn schema_drafts_and_entities_through_the_api() {
        let Some((app, redis)) = app().await else {
            eprintln!("skipped: OT_TEST_REDIS_URL not set");
            return;
        };
        let (st, body) = call(
            &app,
            "PUT",
            "/api/v1/schema/draft",
            Some(json!({"fields": [
                {"key": "contact_phone", "type": "string", "description": "Ship's contact number"},
                {"key": "speed_mps", "type": "number", "builtin": "speed_mps"}
            ]})),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert_eq!(body["fields"][1]["builtin"], "speed_mps");
        let (st, _) = call(&app, "POST", "/api/v1/schema/draft/publish", None).await;
        assert_eq!(st, StatusCode::OK);
        // A linked field must declare its built-in's type.
        let (st, _) = call(
            &app,
            "PUT",
            "/api/v1/schema/draft",
            Some(json!({"fields": [
                {"key": "speed", "type": "string", "builtin": "speed_mps"}
            ]})),
        )
        .await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        call(&app, "DELETE", "/api/v1/schema/draft", None).await;

        let (st, body) = call(
            &app,
            "POST",
            "/api/v1/registry/entities",
            Some(json!({
                "name": "TED STEVENS", "domain": "surface", "affiliation": "friend",
                "identifiers": [{"scheme": "MMSI", "value": "338924210"}, {"scheme": "imo", "value": "9876543"}],
                "attributes": [{"key": "contact_phone", "type": "text", "value": "+1 555 0100"},
                               {"key": "length", "type": "number", "value": "210"}]
            })),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{body}");
        let id = body["entity"]["id"].as_str().unwrap().to_owned();
        assert_eq!(body["entity"]["identifiers"][0]["scheme"], "mmsi");
        assert_eq!(body["entity"]["attributes"][1]["value"], 210);

        // An identifier belongs to one entity; an entity needs one.
        let (st, _) = call(
            &app,
            "POST",
            "/api/v1/registry/entities",
            Some(json!({"identifiers": [{"scheme": "mmsi", "value": "338924210"}]})),
        )
        .await;
        assert_eq!(st, StatusCode::CONFLICT);
        let (st, _) = call(
            &app,
            "POST",
            "/api/v1/registry/entities",
            Some(json!({"name": "x"})),
        )
        .await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);

        // Edit: remove the IMO, add an ELNOT; bad values are refused.
        let mut e = body["entity"].clone();
        e["identifiers"] = json!([{"scheme": "mmsi", "value": "338924210"}, {"scheme": "elnot", "value": "NL504"}]);
        e["affiliation"] = json!("neutral");
        let (st, body) = call(
            &app,
            "PUT",
            &format!("/api/v1/registry/entities/{id}"),
            Some(e.clone()),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert_eq!(body["entity"]["identifiers"][1]["value"], "NL504");
        assert_eq!(body["revisions"].as_array().unwrap().len(), 2);
        let (_, found) = call(
            &app,
            "GET",
            "/api/v1/registry?scheme=imo&value=9876543",
            None,
        )
        .await;
        assert_ne!(found["id"], id);
        for bad in [
            json!({"domain": "sky"}),
            json!({"attributes": [{"key": "name", "type": "text", "value": "x"}]}),
            json!({"attributes": [{"key": "n", "type": "number", "value": "ten"}]}),
        ] {
            let mut b = e.clone();
            for (k, v) in bad.as_object().unwrap() {
                b[k] = v.clone();
            }
            let (st, _) = call(
                &app,
                "PUT",
                &format!("/api/v1/registry/entities/{id}"),
                Some(b),
            )
            .await;
            assert!(st.is_client_error(), "{bad}: {st}");
        }

        let (_, fields) = call(&app, "GET", "/api/v1/registry/fields", None).await;
        assert!(
            fields["minimum"]
                .as_array()
                .unwrap()
                .contains(&json!("affiliation"))
        );
        assert_eq!(fields["attributes"][0]["key"], "contact_phone");

        let (st, _) = call(
            &app,
            "DELETE",
            &format!("/api/v1/registry/entities/{id}"),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::NO_CONTENT);
        let (st, _) = call(
            &app,
            "GET",
            &format!("/api/v1/registry/entities/{id}"),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        let (_, body) = call(&app, "GET", "/api/v1/schema", None).await;
        assert!(
            body["builtins"]
                .as_array()
                .unwrap()
                .iter()
                .any(|b| b["name"] == "state")
        );
        redis.purge_namespace().await.unwrap();
    }

    #[tokio::test]
    async fn correlation_settings_and_operator_commands() {
        let Some((app, redis)) = app().await else {
            eprintln!("skipped: OT_TEST_REDIS_URL not set");
            return;
        };
        let (st, body) = call(&app, "GET", "/api/v1/correlation/settings", None).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert_eq!(body["saved"], false);
        assert_eq!(body["settings"]["mode"], "automatic");
        assert_eq!(body["version"], crate::correlate::VERSION);
        let (st, _) = call(
            &app,
            "PUT",
            "/api/v1/correlation/settings",
            Some(json!({"split": {"m": 9, "n": 6}})),
        )
        .await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        let (st, body) = call(
            &app,
            "PUT",
            "/api/v1/correlation/settings",
            Some(json!({"mode": "suggest", "kinematic": {"m": 3}})),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{body}");
        let (_, body) = call(&app, "GET", "/api/v1/correlation/settings", None).await;
        assert_eq!(body["saved"], true);
        assert_eq!(body["settings"]["mode"], "suggest");
        assert_eq!(body["settings"]["kinematic"]["m"], 3);
        let (_, body) = call(&app, "GET", "/api/v1/correlation/decisions", None).await;
        assert_eq!(body["decisions"][0]["op"], "correlation_settings");
        let (_, body) = call(
            &app,
            "GET",
            "/api/v1/correlation/suggestions?status=open",
            None,
        )
        .await;
        assert_eq!(body["suggestions"], json!([]));

        // Commands go to the engine and come back with its answer.
        let engine = redis.clone();
        let responder = tokio::spawn(async move {
            loop {
                for cmd in engine.pop_commands(10).await.unwrap() {
                    let answer = if cmd["op"] == "merge" {
                        json!({"ok": true, "result": {"merged_into": cmd["into"]}})
                    } else {
                        json!({"ok": false, "error": "no such track"})
                    };
                    engine
                        .put_command_result(cmd["id"].as_str().unwrap(), &answer)
                        .await
                        .unwrap();
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        });
        let (st, body) = call(
            &app,
            "POST",
            "/api/v1/tracks/merge",
            Some(json!({"from": "tms-TST000000001", "into": "tms-TST000000002"})),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert_eq!(body["merged_into"], "tms-TST000000002");
        let (st, body) = call(
            &app,
            "POST",
            "/api/v1/tracks/tms-TST000000001/split",
            Some(json!({"source_track": "radar/r1"})),
        )
        .await;
        assert_eq!(st, StatusCode::CONFLICT);
        assert_eq!(body["error"], "no such track");
        let (st, _) = call(
            &app,
            "POST",
            "/api/v1/correlation/suggestions/1/maybe",
            None,
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        responder.abort();
        redis.purge_namespace().await.unwrap();
    }

    #[tokio::test]
    async fn system_metrics() {
        let Some((app, redis)) = app().await else {
            eprintln!("skipped: OT_TEST_REDIS_URL not set");
            return;
        };
        // One delete waiting for the writer, and some counted pipeline work.
        redis
            .ensure_outbox_group(crate::writer::GROUP)
            .await
            .unwrap();
        let uid = ot_core::Uid::new(ot_core::SiteCode::new("TST").unwrap(), 7).unwrap();
        redis.retire_system_track(uid, "test").await.unwrap();
        redis
            .incr_metrics(crate::metrics::ENGINE, &[("observations".into(), 5)])
            .await
            .unwrap();
        redis
            .set_gauges(crate::metrics::SYSTEM, &[("cpu_milli", 120)])
            .await
            .unwrap();

        let (st, body) = call(&app, "GET", "/api/v1/metrics?minutes=10", None).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        let series = body["series"].as_array().unwrap();
        assert_eq!(series.len(), 10);
        assert_eq!(series[9]["engine"]["observations"], 5, "{body}");
        assert_eq!(body["recent"]["engine"]["observations"], 5);
        let live = &body["live"];
        assert_eq!(live["outbox"], json!({"pending": 0, "lag": 1}), "{body}");
        assert_eq!(live["tracks"], 0);
        assert_eq!(live["cpu_milli"], 120);
        assert!(live["redis_bytes"].as_u64().unwrap() > 0);
        redis.purge_namespace().await.unwrap();
    }

    #[tokio::test]
    async fn ui_assets_never_fall_back_to_the_page() {
        let dir = std::env::temp_dir().join(format!("ot-ui-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(dir.join("index.html"), "<html>page</html>").unwrap();
        std::fs::write(dir.join("assets/app-abc.js"), "console.log(1)").unwrap();
        let Some((app, redis)) = app_with_ui(Some(dir.clone())).await else {
            eprintln!("skipped: OT_TEST_REDIS_URL not set");
            return;
        };
        let get = |uri: &str| {
            axum::http::Request::builder()
                .uri(uri)
                .body(axum::body::Body::empty())
                .unwrap()
        };
        let res = app
            .clone()
            .oneshot(get("/assets/app-abc.js"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert!(
            res.headers()["cache-control"]
                .to_str()
                .unwrap()
                .contains("immutable")
        );
        // A chunk from an older build: 404, not the page.
        let res = app
            .clone()
            .oneshot(get("/assets/app-old.js"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        // Client-side routes get the page, uncached.
        let res = app.clone().oneshot(get("/sources")).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()["cache-control"], "no-cache");
        std::fs::remove_dir_all(dir).ok();
        redis.purge_namespace().await.unwrap();
    }
}
