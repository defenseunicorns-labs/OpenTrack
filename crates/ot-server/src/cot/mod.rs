//! The `cot` role: the published picture as Cursor-on-Target for TAK.
//!
//! It reads the Redis outbox in a consumer group of its own
//! ([`GROUP`], beside the NATS writer's), so a slow TAK link never holds up
//! NATS, and publishes exactly what the writer publishes: tracks the
//! engine's publish rule let out, each upsert as an event, each end as a
//! delete. Updates of a track are coalesced (at most one per minimum
//! interval, urgent ones at once) as the writer does.
//!
//! The outputs are configured in Settings → TAK output (saved with the
//! instance settings as `tak`) and re-read every few seconds: an output
//! added, changed, turned off or removed is started, restarted or stopped
//! without restarting the role. See [`output`] for the deliveries and
//! [`event`] for the events.

pub mod api;
pub mod event;
pub mod output;
pub mod settings;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use ot_core::Uid;
use ot_store::{OutboxOp, RedisStore};
use serde_json::json;
use tokio::time::Instant;

use crate::config::Common;
use crate::writer::Schedule;
use event::CotTrack;
use output::{Counters, Hub};
use settings::{TakOutput, TakSettings, tak_settings};

/// Consumer group of the `cot` role on the outbox.
pub const GROUP: &str = "track-cot";

/// Metrics bucket (per-minute counters and gauges).
pub const METRICS_SOURCE: &str = crate::metrics::COT;

const SETTINGS_EVERY: Duration = Duration::from_secs(5);
const STATUS_EVERY: Duration = Duration::from_secs(2);
const METRICS_EVERY: Duration = Duration::from_secs(15);
/// The status expires this long after the role last wrote it.
const STATUS_TTL: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, clap::Args)]
pub struct CotArgs {
    /// Consumer name within the `cot` group on the outbox. Run one `cot`
    /// role per node: two would split the tracks between them.
    #[arg(long, env = "OT_COT_CONSUMER", default_value = "cot-1")]
    pub cot_consumer: String,
    /// Minimum seconds between two events of the same track (identity and
    /// classification changes go at once).
    #[arg(long, env = "OT_COT_MIN_INTERVAL_SECS", default_value_t = 2.0)]
    pub cot_min_interval_secs: f64,
}

/// Turns outbox entries into changes to the [`Hub`]'s picture.
pub struct Feeder {
    redis: RedisStore,
    consumer: String,
    schedule: Schedule,
    pub hub: Arc<Hub>,
    pub upserts: u64,
    pub deletes: u64,
}

impl Feeder {
    /// Join the group (created at the end of the outbox: the picture is read
    /// from the live tracks), then load that picture.
    pub async fn start(
        redis: RedisStore,
        consumer: String,
        min_interval: Duration,
    ) -> anyhow::Result<Self> {
        redis.ensure_outbox_group_from(GROUP, "$").await?;
        let hub = Arc::new(Hub::default());
        let live: Vec<CotTrack> = redis
            .list_system_tracks()
            .await?
            .iter()
            .filter(|t| t.is_published())
            .map(CotTrack::of)
            .collect();
        tracing::info!(tracks = live.len(), "TAK picture loaded");
        hub.load(live);
        Ok(Self {
            redis,
            consumer,
            schedule: Schedule::new(min_interval),
            hub,
            upserts: 0,
            deletes: 0,
        })
    }

    /// One read-and-apply cycle. With `pending`, re-read this consumer's
    /// unacknowledged entries (after a restart) instead of waiting.
    pub async fn pump(&mut self, pending: bool) -> anyhow::Result<()> {
        let now = Instant::now();
        let block = self
            .schedule
            .next_due()
            .map(|at| at.saturating_duration_since(now))
            .unwrap_or(Duration::from_secs(1))
            .clamp(Duration::from_millis(1), Duration::from_secs(1));
        let entries = self
            .redis
            .read_outbox(GROUP, &self.consumer, 500, block, pending)
            .await?;
        let now = Instant::now();
        let mut acks = Vec::new();
        let mut ended: Vec<(Uid, String)> = Vec::new();
        for entry in entries {
            match entry.op {
                OutboxOp::Publish { uid, urgent } => {
                    self.schedule.offer(uid, entry.id, urgent, now)
                }
                OutboxOp::Tombstone { uid, .. } => ended.push((uid, entry.id)),
                // TAK has no history to correct.
                OutboxOp::HistoryDelete { .. } => acks.push(entry.id),
            }
        }
        if !ended.is_empty() {
            let uids: Vec<Uid> = ended.iter().map(|(u, _)| *u).collect();
            let states = self.redis.get_system_tracks(&uids).await?;
            for ((uid, id), state) in ended.into_iter().zip(states) {
                // Published again since (a withdrawn track back in): the
                // delete is out of date.
                if !state.is_some_and(|t| t.is_published()) && self.hub.delete(uid) {
                    self.deletes += 1;
                }
                acks.push(id);
            }
        }
        let due = self.schedule.take_due(now);
        let uids: Vec<Uid> = due.iter().map(|(u, _)| *u).collect();
        let tracks = self.redis.get_system_tracks(&uids).await?;
        for ((uid, ids), track) in due.into_iter().zip(tracks) {
            // Gone or withdrawn since it was queued: its delete is in the outbox.
            if let Some(t) = track.filter(|t| t.is_published()) {
                self.hub.upsert(CotTrack::of(&t));
                self.upserts += 1;
                self.schedule.written(uid, now);
            }
            acks.extend(ids);
        }
        self.redis.ack_outbox(GROUP, &acks).await?;
        self.schedule.prune(now);
        Ok(())
    }
}

/// A metric's field and value.
type Metric = (String, u64);

/// A running output.
struct Running {
    config: TakOutput,
    counters: Arc<Counters>,
    task: tokio::task::JoinHandle<()>,
}

/// Starts, restarts and stops outputs as the settings change.
#[derive(Default)]
pub struct Outputs {
    running: HashMap<String, Running>,
    /// Counters as last put in the metrics: (sent, errors, dropped).
    flushed: HashMap<String, (u64, u64, u64)>,
}

impl Outputs {
    pub fn apply(&mut self, settings: &TakSettings, hub: &Arc<Hub>) {
        let wanted: HashMap<&str, &TakOutput> = settings
            .outputs
            .iter()
            .filter(|o| o.enabled && o.check().is_ok())
            .map(|o| (o.id.as_str(), o))
            .collect();
        self.running.retain(|id, r| {
            let keep = wanted.get(id.as_str()).is_some_and(|o| **o == r.config);
            if !keep {
                tracing::info!(output = %id, "TAK output stopped");
                r.task.abort();
            }
            keep
        });
        for (id, o) in wanted {
            if self.running.contains_key(id) {
                continue;
            }
            tracing::info!(output = %id, kind = o.delivery.kind(), "TAK output started");
            let counters = Arc::new(Counters::default());
            let task = tokio::spawn(output::run(o.clone(), hub.clone(), counters.clone()));
            self.running.insert(
                id.to_owned(),
                Running {
                    config: o.clone(),
                    counters,
                    task,
                },
            );
        }
    }

    #[cfg(test)]
    pub fn counters(&self, id: &str) -> Option<Arc<Counters>> {
        self.running.get(id).map(|r| r.counters.clone())
    }

    pub fn stop_all(&mut self) {
        for r in self.running.values() {
            r.task.abort();
        }
        self.running.clear();
    }

    fn status(&self, hub: &Hub) -> serde_json::Value {
        let outputs: serde_json::Map<String, serde_json::Value> = self
            .running
            .iter()
            .map(|(id, r)| {
                let mut s = r.counters.status();
                s["kind"] = json!(r.config.delivery.kind());
                (id.clone(), s)
            })
            .collect();
        json!({ "at": chrono::Utc::now(), "tracks": hub.len(), "outputs": outputs })
    }

    /// Counter increments since the last call, and the gauges now.
    fn metrics(&mut self, hub: &Hub) -> (Vec<Metric>, Vec<Metric>) {
        let (mut counts, mut gauges) = (Vec::new(), Vec::new());
        let (mut sent, mut errors, mut dropped, mut clients) = (0, 0, 0, 0);
        for (id, r) in &self.running {
            let c = &r.counters;
            let now = (
                c.sent.load(Ordering::Relaxed),
                c.errors.load(Ordering::Relaxed),
                c.dropped.load(Ordering::Relaxed),
            );
            let before = self.flushed.insert(id.clone(), now).unwrap_or_default();
            let d = (
                now.0.saturating_sub(before.0),
                now.1.saturating_sub(before.1),
                now.2.saturating_sub(before.2),
            );
            for (name, n) in [("sent", d.0), ("errors", d.1), ("dropped", d.2)] {
                if n > 0 {
                    counts.push((format!("{name}:{id}"), n));
                }
            }
            (sent, errors, dropped) = (sent + d.0, errors + d.1, dropped + d.2);
            let n = c.clients.load(Ordering::Relaxed);
            clients += n;
            gauges.push((format!("clients:{id}"), n));
        }
        self.flushed.retain(|id, _| self.running.contains_key(id));
        for (name, n) in [("sent", sent), ("errors", errors), ("dropped", dropped)] {
            if n > 0 {
                counts.push((name.to_owned(), n));
            }
        }
        gauges.push(("clients".into(), clients));
        gauges.push(("tracks".into(), hub.len() as u64));
        (counts, gauges)
    }
}

async fn load_settings(common: &Common) -> anyhow::Result<TakSettings> {
    let c = common.clone();
    tokio::task::spawn_blocking(move || -> anyhow::Result<TakSettings> {
        Ok(tak_settings(&c.open_db()?.app_settings()?))
    })
    .await?
}

/// Run the role until `shutdown` resolves.
pub async fn run(
    common: Common,
    args: CotArgs,
    shutdown: impl std::future::Future<Output = ()>,
) -> anyhow::Result<()> {
    let redis = common.open_redis().await?;
    let mut feeder = Feeder::start(
        redis.clone(),
        args.cot_consumer.clone(),
        Duration::from_secs_f64(args.cot_min_interval_secs.max(0.0)),
    )
    .await?;
    let mut outputs = Outputs::default();
    tracing::info!(consumer = %args.cot_consumer, "TAK output role running");
    tokio::pin!(shutdown);
    let mut pending = true;
    let mut backoff = Duration::from_millis(500);
    let far = Instant::now() - Duration::from_secs(3600);
    let (mut settings_at, mut status_at, mut metrics_at) = (far, far, Instant::now());
    loop {
        let now = Instant::now();
        if now.duration_since(settings_at) >= SETTINGS_EVERY {
            settings_at = now;
            match load_settings(&common).await {
                Ok(s) => outputs.apply(&s, &feeder.hub),
                Err(e) => {
                    tracing::warn!(error = %format!("{e:#}"), "TAK output settings not read")
                }
            }
        }
        if now.duration_since(status_at) >= STATUS_EVERY {
            status_at = now;
            if let Err(e) = redis
                .put_cot_status(&outputs.status(&feeder.hub), STATUS_TTL)
                .await
            {
                tracing::debug!(error = %e, "TAK output status not stored");
            }
        }
        if now.duration_since(metrics_at) >= METRICS_EVERY {
            metrics_at = now;
            let (counts, gauges) = outputs.metrics(&feeder.hub);
            let gauges: Vec<(&str, u64)> = gauges.iter().map(|(k, v)| (k.as_str(), *v)).collect();
            if let Err(e) = async {
                redis.incr_metrics(METRICS_SOURCE, &counts).await?;
                redis.set_gauges(METRICS_SOURCE, &gauges).await
            }
            .await
            {
                tracing::debug!(error = %e, "TAK output metrics not stored");
            }
        }
        tokio::select! {
            _ = &mut shutdown => break,
            res = feeder.pump(pending) => match res {
                Ok(()) => {
                    pending = false;
                    backoff = Duration::from_millis(500);
                }
                Err(e) => {
                    tracing::warn!(error = %format!("{e:#}"), ?backoff, "TAK output cycle failed; retrying");
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(10));
                }
            },
        }
    }
    outputs.stop_all();
    tracing::info!(
        upserts = feeder.upserts,
        deletes = feeder.deletes,
        "TAK output role stopped"
    );
    Ok(())
}

#[cfg(test)]
mod tests;
