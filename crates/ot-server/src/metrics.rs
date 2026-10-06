//! System metrics for the Overview.
//!
//! The pipeline already counts per minute in Redis: each source's stages
//! (frames, emitted, errors, ...), the engine (`_engine`) and the writer
//! (`_writer`). This module adds a sampler that records gauges beside them
//! every [`SAMPLE_EVERY`] under `_system` (process memory and CPU, Redis
//! memory, stream backlogs, live tracks, the NATS stream), and `/metrics`,
//! which returns all of it as one per-minute series plus the live values.

use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use ot_store::GroupBacklog;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::control::{ApiError, AppState};

/// Metrics "source" names for the roles that are not feeds.
pub const SYSTEM: &str = "_system";
pub const ENGINE: &str = "_engine";
pub const WRITER: &str = "_writer";
/// The `cot` role (TAK output): `sent`, `errors`, `dropped` (connections
/// too slow), each also per output as `<counter>:<id>`; gauges `clients`,
/// `clients:<id>` and `tracks`.
pub const COT: &str = "_cot";

const SAMPLE_EVERY: Duration = Duration::from_secs(15);
/// Window for the per-source rates the topology shows.
const RECENT_MINUTES: usize = 5;

static STARTED: OnceLock<Instant> = OnceLock::new();

/// Mark the process start, for uptime. Call once, early in `main`.
pub fn mark_start() {
    STARTED.get_or_init(Instant::now);
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/metrics", get(metrics))
}

/// Memory and CPU of this process, from `/proc` (Linux only).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct ProcessStats {
    rss_bytes: u64,
    cpu_secs: f64,
    threads: u64,
}

/// Kernel clock ticks per second; 100 on every mainstream Linux build.
const CLK_TCK: f64 = 100.0;

fn process_stats() -> Option<ProcessStats> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    parse_proc(&status, &stat)
}

fn parse_proc(status: &str, stat: &str) -> Option<ProcessStats> {
    let field = |name: &str| {
        status
            .lines()
            .find_map(|l| l.strip_prefix(name))
            .and_then(|v| v.split_whitespace().next())
            .and_then(|v| v.parse::<u64>().ok())
    };
    // Fields after the parenthesised command name; utime and stime are the
    // 14th and 15th fields overall, so the 12th and 13th after it.
    let rest = &stat[stat.rfind(')')? + 1..];
    let f: Vec<&str> = rest.split_whitespace().collect();
    let ticks = f.get(11)?.parse::<u64>().ok()? + f.get(12)?.parse::<u64>().ok()?;
    Some(ProcessStats {
        rss_bytes: field("VmRSS:")? * 1024,
        cpu_secs: ticks as f64 / CLK_TCK,
        threads: field("Threads:").unwrap_or(0),
    })
}

/// What is true right now, computed on demand.
#[derive(Debug, Default, Serialize)]
struct Live {
    tracks: u64,
    by_state: BTreeMap<String, u64>,
    by_domain: BTreeMap<String, u64>,
    with_entity: u64,
    notices: u64,
    /// The writer's backlog on the outbox.
    outbox: GroupBacklog,
    /// The engine's backlog over every enabled source's observation stream.
    observations: GroupBacklog,
    redis_bytes: u64,
    rss_bytes: u64,
    threads: u64,
    nats_messages: Option<u64>,
    nats_bytes: Option<u64>,
    sqlite_bytes: u64,
    uptime_secs: u64,
}

async fn live(s: &AppState) -> Result<Live, ApiError> {
    let mut out = Live::default();
    let ctx = s.common.publish_context();
    let now = chrono::Utc::now();
    for t in s.redis.list_system_tracks().await? {
        out.tracks += 1;
        let state = serde_json::to_value(t.state)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        *out.by_state.entry(state).or_default() += 1;
        let m = ot_core::wire::to_message(&t, &ctx, now);
        *out.by_domain.entry(m.domain).or_default() += 1;
        out.with_entity += u64::from(t.entity_id.is_some());
        out.notices += t.notices.len() as u64;
    }
    out.outbox = s.redis.outbox_backlog(crate::writer::GROUP).await?;
    let sources = s.with_db(|db| db.list_sources()).await?;
    for r in sources.iter().filter(|r| r.enabled) {
        let b = s.redis.obs_backlog(&r.id, crate::engine::GROUP).await?;
        out.observations.pending += b.pending;
        out.observations.lag += b.lag;
    }
    out.redis_bytes = s.redis.used_memory().await?;
    if let Some(p) = process_stats() {
        out.rss_bytes = p.rss_bytes;
        out.threads = p.threads;
    }
    if let Ok(n) = tokio::time::timeout(Duration::from_secs(3), s.nats.status()).await {
        out.nats_messages = n.stream_messages;
        out.nats_bytes = n.stream_bytes;
    }
    out.sqlite_bytes = ["", "-wal"]
        .iter()
        .filter_map(|suffix| {
            let mut p = s.common.sqlite.clone().into_os_string();
            p.push(suffix);
            std::fs::metadata(p).ok().map(|m| m.len())
        })
        .sum();
    out.uptime_secs = STARTED.get().map(|t| t.elapsed().as_secs()).unwrap_or(0);
    Ok(out)
}

/// Record gauges under `_system` every [`SAMPLE_EVERY`], so the Overview has
/// history for them. Runs with the control plane.
pub async fn run_sampler(s: AppState) {
    let mut tick = tokio::time::interval(SAMPLE_EVERY);
    let mut last: Option<(Instant, f64)> = None;
    loop {
        tick.tick().await;
        let proc = process_stats();
        // CPU is a rate: seconds of CPU per second of wall time, in thousandths
        // of a core.
        let cpu_milli = match (last, proc) {
            (Some((at, secs)), Some(p)) => {
                let wall = at.elapsed().as_secs_f64();
                (wall > 0.0).then(|| ((p.cpu_secs - secs).max(0.0) / wall * 1000.0).round() as u64)
            }
            _ => None,
        };
        if let Some(p) = proc {
            last = Some((Instant::now(), p.cpu_secs));
        }
        let l = match live(&s).await {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(error = %e.message, "metrics sample failed");
                continue;
            }
        };
        let state = |k: &str| l.by_state.get(k).copied().unwrap_or(0);
        let mut gauges = vec![
            ("tracks", l.tracks),
            ("tracks_tentative", state("tentative")),
            ("tracks_confirmed", state("confirmed")),
            ("tracks_lost", state("lost")),
            ("tracks_with_entity", l.with_entity),
            ("notices", l.notices),
            ("outbox_pending", l.outbox.pending),
            ("outbox_lag", l.outbox.lag),
            ("obs_pending", l.observations.pending),
            ("obs_lag", l.observations.lag),
            ("redis_bytes", l.redis_bytes),
            ("rss_bytes", l.rss_bytes),
            ("threads", l.threads),
            ("sqlite_bytes", l.sqlite_bytes),
        ];
        if let Some(c) = cpu_milli {
            gauges.push(("cpu_milli", c));
        }
        if let Some(n) = l.nats_messages {
            gauges.push(("nats_messages", n));
        }
        if let Some(n) = l.nats_bytes {
            gauges.push(("nats_bytes", n));
        }
        if let Err(e) = s.redis.set_gauges(SYSTEM, &gauges).await {
            tracing::warn!(error = %e, "metrics sample not stored");
        }
    }
}

#[derive(Deserialize)]
struct MetricsQuery {
    minutes: Option<i64>,
}

type Minute = std::collections::HashMap<String, u64>;

/// Stage counters summed over sources. Breakdowns (`rejected:<reason>`,
/// `grade:<grade>`) stay per source.
fn add_counters(into: &mut BTreeMap<String, u64>, from: &Minute) {
    for (k, v) in from {
        if !k.contains(':') {
            *into.entry(k.clone()).or_default() += v;
        }
    }
}

fn recent(series: &[(i64, Minute)]) -> BTreeMap<String, u64> {
    let mut out = BTreeMap::new();
    for (_, m) in series.iter().rev().take(RECENT_MINUTES) {
        add_counters(&mut out, m);
    }
    out
}

/// Every counter and gauge per minute, oldest first, plus live values and
/// each source's totals over the last few minutes.
async fn metrics(
    State(s): State<AppState>,
    Query(q): Query<MetricsQuery>,
) -> Result<Json<Value>, ApiError> {
    let minutes = q.minutes.unwrap_or(60).clamp(1, 24 * 60);
    let sources = s.with_db(|db| db.list_sources()).await?;
    let mut by_source = Vec::with_capacity(sources.len());
    for r in &sources {
        by_source.push((r.id.clone(), s.redis.metrics_range(&r.id, minutes).await?));
    }
    let engine = s.redis.metrics_range(ENGINE, minutes).await?;
    let writer = s.redis.metrics_range(WRITER, minutes).await?;
    let cot = s.redis.metrics_range(COT, minutes).await?;
    let system = s.redis.metrics_range(SYSTEM, minutes).await?;

    let series: Vec<Value> = (0..engine.len())
        .map(|i| {
            let mut ingest = BTreeMap::new();
            let mut emitted = BTreeMap::new();
            for (id, rows) in &by_source {
                let m = &rows[i].1;
                add_counters(&mut ingest, m);
                emitted.insert(id.as_str(), m.get("emitted").copied().unwrap_or(0));
            }
            json!({
                "minute": engine[i].0,
                "ingest": ingest,
                "emitted_by_source": emitted,
                "engine": engine[i].1,
                "writer": writer[i].1,
                "cot": cot[i].1,
                "system": system[i].1,
            })
        })
        .collect();

    let mut live = serde_json::to_value(live(&s).await?).unwrap_or_default();
    // CPU needs two samples, so it comes from the sampler.
    live["cpu_milli"] = json!(system.iter().rev().find_map(|(_, m)| m.get("cpu_milli")));
    Ok(Json(json!({
        "minutes": minutes,
        "recent_minutes": RECENT_MINUTES,
        "series": series,
        "recent": {
            "sources": by_source.iter().map(|(id, rows)| (id.clone(), recent(rows))).collect::<BTreeMap<_, _>>(),
            "engine": recent(&engine),
            "writer": recent(&writer),
            "cot": recent(&cot),
        },
        "live": live,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_proc() {
        let status = "Name:\topentrack\nVmRSS:\t   51200 kB\nThreads:\t17\n";
        // pid (comm with spaces) state ppid ... utime=250 stime=50
        let stat = "42 (open track) S 1 42 42 0 -1 4194560 100 0 0 0 250 50 0 0 20 0 17 0 1000";
        let p = parse_proc(status, stat).unwrap();
        assert_eq!(p.rss_bytes, 51200 * 1024);
        assert_eq!(p.threads, 17);
        assert!((p.cpu_secs - 3.0).abs() < 1e-9);
        if cfg!(target_os = "linux") {
            assert!(process_stats().unwrap().rss_bytes > 0);
        }
    }

    #[test]
    fn sums_stage_counters_but_not_breakdowns() {
        let mut into = BTreeMap::new();
        let m: Minute = [
            ("frames".to_owned(), 3),
            ("rejected:dup".to_owned(), 1),
            ("rejected".to_owned(), 1),
        ]
        .into_iter()
        .collect();
        add_counters(&mut into, &m);
        add_counters(&mut into, &m);
        assert_eq!(into.get("frames"), Some(&6));
        assert_eq!(into.get("rejected"), Some(&2));
        assert!(!into.contains_key("rejected:dup"));
    }
}
