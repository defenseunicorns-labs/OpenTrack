//! The source-worker role: runs every enabled source.
//!
//! It polls SQLite for configuration changes and (re)starts sources whose
//! revision changed. Each source is a supervised transport (reconnecting with
//! backoff) feeding its pipeline, which appends observations to the source's
//! Redis stream. The worker also persists the static-join cache, flushes
//! per-source metrics, and publishes a live status the API reads. A source
//! with a raw-output subject (set with recorded consent) also publishes each
//! of its observations, before correlation, as JSON on that NATS subject.
//! Entity fields a feed updates (a pipeline's track → entity links) are
//! written to the registry every few seconds.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use anyhow::Context;
use chrono::Utc;
use ot_source::frame::Frame;
use ot_source::pipeline::{Pipeline, StaticEntry};
use ot_source::registry::{RegistryEntry, RegistryLookup};
use ot_source::schema::{ExtensionField, ExtensionSchema};
use ot_source::source::SourceSpec;
use ot_source::transport::{self, SharedStatus};
use ot_store::RedisStore;
use serde_json::json;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::Instrument;

use crate::config::Common;

const CONFIG_POLL: Duration = Duration::from_secs(2);
const REGISTRY_POLL: Duration = Duration::from_secs(5);
const ENTITY_WRITE_EVERY: Duration = Duration::from_secs(2);
const FLUSH_EVERY: Duration = Duration::from_secs(5);
const TRACKER_HOLD_POLL: Duration = Duration::from_millis(250);
const STATUS_TTL: Duration = Duration::from_secs(30);

/// Registry snapshot shared by every source's pipeline.
#[derive(Default)]
pub struct Registry {
    map: RwLock<HashMap<(String, String), RegistryEntry>>,
    /// Entity fields feeds reported, `(source, entity, fields)`, to write.
    updates: Mutex<Vec<EntityUpdate>>,
}

type EntityUpdate = (String, String, Vec<(String, serde_json::Value)>);

impl RegistryLookup for Registry {
    fn lookup(&self, scheme: &str, value: &str) -> Option<RegistryEntry> {
        self.map
            .read()
            .ok()?
            .get(&(scheme.to_owned(), value.to_owned()))
            .cloned()
    }
}

impl Registry {
    fn replace(&self, rows: Vec<ot_store::RegistryRow>) -> usize {
        let map: HashMap<_, _> = rows
            .into_iter()
            .map(|r| {
                (
                    (r.scheme, r.value),
                    RegistryEntry {
                        entity_id: r.entity_id,
                        name: r.name,
                        expected_name: r.expected_name,
                        fields: r.fields,
                    },
                )
            })
            .collect();
        let n = map.len();
        if let Ok(mut m) = self.map.write() {
            *m = map;
        }
        n
    }

    fn queue(&self, source: &str, updates: Vec<(String, Vec<(String, serde_json::Value)>)>) {
        if let Ok(mut q) = self.updates.lock() {
            q.extend(
                updates
                    .into_iter()
                    .map(|(entity, fields)| (source.to_owned(), entity, fields)),
            );
        }
    }

    fn take_updates(&self) -> Vec<EntityUpdate> {
        self.updates
            .lock()
            .map(|mut q| std::mem::take(&mut *q))
            .unwrap_or_default()
    }
}

struct Running {
    revision: i64,
    raw_subject: Option<String>,
    handle: JoinHandle<()>,
}

pub async fn run(
    common: Common,
    shutdown: impl std::future::Future<Output = ()>,
) -> anyhow::Result<()> {
    let redis = common.open_redis().await?;
    // Only raw output uses NATS here; the client connects in the background.
    let nats = common.connect_nats().await?.client().clone();
    let registry = Arc::new(Registry::default());
    let mut running: BTreeMap<String, Running> = BTreeMap::new();
    let (mut config_version, mut registry_version) = (String::new(), String::new());
    let mut config_tick = tokio::time::interval(CONFIG_POLL);
    let mut registry_tick = tokio::time::interval(REGISTRY_POLL);
    let mut entity_tick = tokio::time::interval(ENTITY_WRITE_EVERY);
    // Enabled sources refused a start, and why: their status says so.
    let mut refused: BTreeMap<String, NotStarted> = BTreeMap::new();
    let mut refused_tick = tokio::time::interval(FLUSH_EVERY);
    tokio::pin!(shutdown);
    tracing::info!("source workers started");
    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            _ = registry_tick.tick() => {
                let c = common.clone();
                let seen = registry_version.clone();
                let res = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<(String, Vec<ot_store::RegistryRow>)>> {
                    let db = c.open_db()?;
                    let v = db.registry_version()?;
                    if v == seen { return Ok(None) }
                    Ok(Some((v, db.registry_rows()?)))
                }).await?;
                match res {
                    Ok(Some((v, rows))) => {
                        let n = registry.replace(rows);
                        tracing::info!(identifiers = n, "registry snapshot loaded");
                        registry_version = v;
                    }
                    Ok(None) => {}
                    Err(e) => tracing::warn!(error = %e, "registry refresh failed"),
                }
            }
            _ = refused_tick.tick(), if !refused.is_empty() => {
                for (id, n) in &refused {
                    if let Err(e) = redis.put_source_status(id, &n.status(id).to_string(), STATUS_TTL).await {
                        tracing::warn!(source = %id, error = %e, "source status write failed");
                    }
                }
            }
            _ = entity_tick.tick() => {
                let updates = registry.take_updates();
                if updates.is_empty() { continue }
                let c = common.clone();
                let res = tokio::task::spawn_blocking(move || -> anyhow::Result<usize> {
                    let mut db = c.open_db()?;
                    let mut changed = 0;
                    for (source, entity, fields) in updates {
                        if db.update_entity_fields(&entity, &fields, &format!("source:{source}"))? {
                            changed += 1;
                        }
                    }
                    Ok(changed)
                }).await?;
                match res {
                    Ok(0) => {}
                    Ok(n) => tracing::info!(entities = n, "entities updated from feeds"),
                    Err(e) => tracing::warn!(error = %e, "entity update failed"),
                }
            }
            _ = config_tick.tick() => {
                let c = common.clone();
                let seen = config_version.clone();
                let res = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<(String, Vec<ot_store::SourceRow>, Schemas)>> {
                    let db = c.open_db()?;
                    let v = db.sources_version()?;
                    if v == seen { return Ok(None) }
                    Ok(Some((v, db.list_sources()?, load_schemas(&db)?)))
                }).await?;
                match res {
                    Ok(Some((v, rows, schemas))) => {
                        refused = reconcile(&common, &mut running, rows, &schemas, &redis, &nats, &registry);
                        refused_tick.reset_immediately();
                        config_version = v;
                    }
                    Ok(None) => {}
                    Err(e) => tracing::warn!(error = %e, "source config refresh failed"),
                }
            }
        }
    }
    for (id, r) in running {
        r.handle.abort();
        tracing::info!(source = %id, "stopped");
    }
    Ok(())
}

/// Published extension schemas by version.
pub(crate) type Schemas = HashMap<u32, ExtensionSchema>;

pub(crate) fn load_schemas(db: &ot_store::Db) -> anyhow::Result<Schemas> {
    let mut out = HashMap::new();
    for v in db.schema_versions()? {
        if v.status != "published" {
            continue;
        }
        let fields = v
            .fields
            .into_iter()
            .map(serde_json::from_value)
            .collect::<Result<Vec<ExtensionField>, _>>()?;
        out.insert(
            v.version,
            ExtensionSchema {
                version: v.version,
                fields,
            },
        );
    }
    Ok(out)
}

/// An enabled source the worker will not start.
struct NotStarted {
    revision: i64,
    transport: &'static str,
    reason: String,
}

impl NotStarted {
    /// A status the API shows like a running source's, saying why not.
    fn status(&self, id: &str) -> serde_json::Value {
        json!({
            "source": id,
            "revision": self.revision,
            "transport": self.transport,
            "link": transport::LinkStatus::default(),
            "last_error": format!("not started: {}", self.reason),
            "not_started": self.reason,
            "totals_since_start": {},
            "updated_at": Utc::now(),
        })
    }
}

/// Start and stop sources to match the configuration; returns the enabled
/// sources it refused to start.
fn reconcile(
    common: &Common,
    running: &mut BTreeMap<String, Running>,
    rows: Vec<ot_store::SourceRow>,
    schemas: &Schemas,
    redis: &RedisStore,
    nats: &ot_nats::async_nats::Client,
    registry: &Arc<Registry>,
) -> BTreeMap<String, NotStarted> {
    let mut refused = BTreeMap::new();
    let wanted: BTreeMap<String, ot_store::SourceRow> = rows
        .into_iter()
        .filter(|r| r.enabled)
        .map(|r| (r.id.clone(), r))
        .collect();
    running.retain(|id, r| {
        let keep = wanted
            .get(id)
            .is_some_and(|w| w.revision == r.revision && w.raw_subject == r.raw_subject);
        if !keep {
            r.handle.abort();
            tracing::info!(source = %id, "stopped (disabled, removed or reconfigured)");
        }
        keep
    });
    for (id, row) in wanted {
        if running.contains_key(&id) {
            continue;
        }
        let spec: SourceSpec = match serde_json::from_value(row.spec.clone()) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(source = %id, error = %e, "stored spec no longer parses; not started");
                continue;
            }
        };
        // A file source saved before 0.4.3 may point outside the data
        // directory; it doesn't run until it is moved (ASD V-222466).
        if let Err(e) = crate::probe::file_allowed(common, &spec.transport) {
            tracing::error!(source = %id, error = %e, "not started");
            continue;
        }
        // A listener saved before 0.4.5 that does not authenticate its
        // senders doesn't run until it does, or until the risk is accepted
        // on the source (ASD V-222533).
        if let Some(reason) = spec.authentication_problem() {
            tracing::error!(source = %id, error = %reason, "not started");
            refused.insert(
                id,
                NotStarted {
                    revision: row.revision,
                    transport: spec.transport.kind(),
                    reason,
                },
            );
            continue;
        }
        let Some(schema) = schemas.get(&spec.pipeline.mapping.schema_version).cloned() else {
            tracing::error!(source = %id, version = spec.pipeline.mapping.schema_version,
                "mapping targets an unpublished schema version; not started");
            continue;
        };
        let raw = row.raw_subject.clone().map(|subject| RawOutput {
            client: nats.clone(),
            subject,
        });
        let handle = tokio::spawn(run_source(
            spec,
            schema,
            row.revision,
            redis.clone(),
            raw,
            registry.clone(),
        ));
        tracing::info!(source = %id, revision = row.revision, "started");
        running.insert(
            id,
            Running {
                revision: row.revision,
                raw_subject: row.raw_subject,
                handle,
            },
        );
    }
    refused
}

/// Where a source's raw output goes.
#[derive(Clone)]
struct RawOutput {
    client: ot_nats::async_nats::Client,
    subject: String,
}

/// Run one source forever: a supervised transport and its pipeline.
async fn run_source(
    spec: SourceSpec,
    schema: ExtensionSchema,
    revision: i64,
    redis: RedisStore,
    raw: Option<RawOutput>,
    registry: Arc<Registry>,
) {
    let id = spec.id.clone();
    let link = SharedStatus::default();
    let (tx, rx) = mpsc::channel::<Frame>(10_000);

    let transport_cfg = spec.transport.clone();
    // A protobuf codec's schema, compiled once for the transport (validated
    // with the spec, so this only fails if the spec was not).
    let proto = match transport::ProtoContext::of(&spec.pipeline.codec) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(source = %id, error = %e, "protobuf schema does not compile");
            None
        }
    };
    let link_t = link.clone();
    let tid = id.clone();
    let supervisor = tokio::spawn(async move {
        let mut backoff = Duration::from_secs(2);
        loop {
            let started = tokio::time::Instant::now();
            // The span puts the source id on the transport's connection logs.
            let res = transport::run(&transport_cfg, proto.as_ref(), tx.clone(), link_t.clone())
                .instrument(tracing::info_span!("source.transport", source = %tid))
                .await;
            if let Ok(mut s) = link_t.lock() {
                s.connected = false;
                if let Err(e) = &res {
                    s.errors += 1;
                    s.last_error = Some(format!("{e:#}"));
                }
            }
            match &res {
                Ok(()) => tracing::info!(source = %tid, "transport closed; reconnecting"),
                Err(e) => {
                    tracing::warn!(source = %tid, error = %format!("{e:#}"), ?backoff, "transport failed")
                }
            }
            if tx.is_closed() {
                return;
            }
            // A connection that lasted a while resets the backoff.
            if started.elapsed() > Duration::from_secs(60) {
                backoff = Duration::from_secs(2);
            }
            // Up to a quarter more, so sources that failed together do not
            // all come back at the same instant.
            let jitter = backoff.mul_f64(rand_fraction() * 0.25);
            tokio::time::sleep(backoff + jitter).await;
            backoff = (backoff * 2).min(Duration::from_secs(60));
        }
    });

    let mut rx = rx;
    loop {
        match pipeline_loop(
            &spec,
            &schema,
            revision,
            &mut rx,
            Outputs {
                redis: &redis,
                raw: raw.as_ref(),
            },
            &registry,
            &link,
        )
        .await
        {
            Ok(()) => break,
            Err(e) => {
                // Usually Redis; frames buffer in the channel meanwhile.
                tracing::error!(source = %id, error = %format!("{e:#}"), "pipeline failed; restarting in 5s");
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }
    supervisor.abort();
}

/// Where a pipeline writes: its Redis stream, and its raw feed if it has one.
struct Outputs<'a> {
    redis: &'a RedisStore,
    raw: Option<&'a RawOutput>,
}

async fn pipeline_loop(
    spec: &SourceSpec,
    schema: &ExtensionSchema,
    revision: i64,
    rx: &mut mpsc::Receiver<Frame>,
    out: Outputs<'_>,
    registry: &Arc<Registry>,
    link: &SharedStatus,
) -> anyhow::Result<()> {
    let Outputs { redis, raw } = out;
    let id = spec.id.as_str();
    let mut pipeline = Pipeline::new(id, spec.pipeline.clone())
        .context("invalid pipeline")?
        .with_schema(schema.clone());
    let static_ttl = Duration::from_secs(spec.pipeline.static_join.ttl_secs);

    // Warm the static cache so identity survives a restart.
    let mut loaded = 0;
    for (key, json) in redis.load_statics(id).await? {
        if let Ok(entry) = serde_json::from_str::<StaticEntry>(&json) {
            pipeline.load_static(key, entry);
            loaded += 1;
        }
    }
    tracing::info!(source = %id, statics = loaded, "pipeline ready");

    let mut flush = tokio::time::interval(FLUSH_EVERY);
    let mut last_error: Option<String> = None;
    let mut totals: HashMap<String, u64> = HashMap::new();
    let (mut raw_published, mut raw_errors) = (0u64, 0u64);
    // Scans a tracker holds for late plots are run when their hold passes.
    let mut hold = tokio::time::interval(TRACKER_HOLD_POLL);
    loop {
        let out = tokio::select! {
            frame = rx.recv() => {
                let Some(frame) = frame else { return Ok(()) };
                tracing::info_span!("source.process", source = %id)
                    .in_scope(|| pipeline.process(&frame, registry.as_ref()))
            }
            _ = hold.tick(), if spec.pipeline.tracker.is_some() => pipeline.flush(Utc::now(), false),
            _ = flush.tick() => {
                let mut counts = pipeline.take_counts().pairs();
                for (k, n) in [("raw_published", &mut raw_published), ("raw_error", &mut raw_errors)] {
                    if *n > 0 {
                        counts.push((k.to_owned(), std::mem::take(n)));
                    }
                }
                redis.incr_metrics(id, &counts).await?;
                for (k, n) in &counts {
                    *totals.entry(k.clone()).or_default() += n;
                }
                pipeline.prune(Utc::now());
                let link = link.lock().map(|l| l.clone()).unwrap_or_default();
                let status = json!({
                    "source": id,
                    "revision": revision,
                    "transport": spec.transport.kind(),
                    "link": link,
                    "last_error": last_error,
                    "totals_since_start": totals,
                    "tracker_timing": pipeline.tracker_timing(),
                    "updated_at": Utc::now(),
                });
                redis.put_source_status(id, &status.to_string(), STATUS_TTL).await?;
                continue;
            }
        };
        let mut out = out;
        // The source's security label goes on everything it reports.
        if let Some(label) = &spec.security {
            for o in &mut out.observations {
                o.security = Some(label.clone());
            }
        }
        redis
            .append_observations(id, &out.observations)
            .instrument(tracing::info_span!(
                "source.write",
                source = %id,
                observations = out.observations.len()
            ))
            .await?;
        if let Some(raw) = raw {
            for obs in &out.observations {
                let body = serde_json::to_vec(obs)?;
                match raw.client.publish(raw.subject.clone(), body.into()).await {
                    Ok(()) => raw_published += 1,
                    Err(e) => {
                        raw_errors += 1;
                        last_error = Some(format!("raw output: {e}"));
                    }
                }
            }
        }
        if !out.entity_updates.is_empty() {
            registry.queue(id, out.entity_updates);
        }
        for (key, entry) in &out.statics {
            redis
                .put_static(id, key, &serde_json::to_string(entry)?, static_ttl)
                .await?;
        }
        if out.last_error.is_some() {
            last_error = out.last_error;
        }
    }
}

/// A number in [0, 1) that differs between calls (backoff jitter; not for
/// anything that needs real randomness).
fn rand_fraction() -> f64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
    );
    (h.finish() >> 11) as f64 / (1u64 << 53) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A UDP listener saved before 0.4.5, without the risk acceptance, is
    /// not started, and its status says why; with the acceptance it runs.
    #[tokio::test]
    async fn an_unauthenticated_listener_without_the_acceptance_is_not_started() {
        let Ok(url) = std::env::var("OT_TEST_REDIS_URL") else {
            eprintln!("skipped: OT_TEST_REDIS_URL not set");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let ns = format!(
            "ot-sources-test-{}-{}",
            std::process::id(),
            Utc::now().timestamp_micros()
        );
        let common = Common {
            sqlite: dir.path().join("ot.db"),
            redis: url.clone(),
            redis_ca: None,
            redis_cert: None,
            redis_key: None,
            redis_namespace: ns.clone(),
            site: ot_core::SiteCode::new("TST").unwrap(),
            nats: crate::config::NatsArgs {
                // Nothing listens here; only raw output would use it.
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
        let redis = RedisStore::connect(&url, ot_store::Keys::new(ns))
            .await
            .unwrap();
        let nats = common.connect_nats().await.unwrap().client().clone();
        let mut spec: serde_json::Value =
            serde_json::from_str(include_str!("../../../docs/examples/gps-udp.json")).unwrap();
        spec["transport"]["bind"] = json!("127.0.0.1:0");
        let accepted = spec.clone();
        spec.as_object_mut().unwrap().remove("unauthenticated");
        let row = |spec: serde_json::Value, revision| ot_store::SourceRow {
            id: "gps".into(),
            name: "GPS".into(),
            transport: "udp".into(),
            codec: "json".into(),
            enabled: true,
            priority: 100,
            revision,
            spec,
            raw_subject: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        let schema = ExtensionSchema {
            version: accepted["pipeline"]["mapping"]["schema_version"]
                .as_u64()
                .unwrap_or(1) as u32,
            fields: Vec::new(),
        };
        let schemas: Schemas = [(schema.version, schema)].into();
        let registry = Arc::new(Registry::default());
        let mut running = BTreeMap::new();

        let refused = reconcile(
            &common,
            &mut running,
            vec![row(spec, 1)],
            &schemas,
            &redis,
            &nats,
            &registry,
        );
        assert!(running.is_empty());
        let status = refused["gps"].status("gps");
        assert!(
            status["last_error"]
                .as_str()
                .unwrap()
                .starts_with("not started: a udp source listens"),
            "{status}"
        );
        assert_eq!(status["link"]["connected"], false);

        let refused = reconcile(
            &common,
            &mut running,
            vec![row(accepted, 2)],
            &schemas,
            &redis,
            &nats,
            &registry,
        );
        assert!(refused.is_empty());
        assert!(running.contains_key("gps"));
        for (_, r) in running {
            r.handle.abort();
        }
    }
}
