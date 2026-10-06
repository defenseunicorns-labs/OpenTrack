//! Benchmark runs: a recorded scenario through the real pipelines and the
//! engine, as fast as they go, on the scenario's own clock.
//!
//! A scenario is a directory (built by `scripts/benchmark/`):
//!
//! ```text
//! scenario.json    {"name", "description", "sources": [{"spec": <source spec>,
//!                   "frames": "<file>"}], "engine": {...}, "correlation": {...},
//!                   "step_secs": 1, "sample_secs": 1}
//! <frames>.jsonl   one frame per line, as the transport would deliver it:
//!                  {"t": <time>, "json": ...} | {"t", "text": "..."} | {"t", "hex": "..."}
//! truth.jsonl      read by the scorer, not here
//! ```
//!
//! Each source's frames go through its pipeline (codec, mapping, tracker
//! stage) at their recorded time, and the observations through the engine,
//! one step of scenario time at a time. Nothing is published: there is no
//! writer, and the engine works in a Redis namespace of its own, removed at
//! the end. The run writes, to the output directory:
//!
//! - `tracks.jsonl`: every system track every `sample_secs`
//! - `source_tracks.jsonl`: every observation the pipelines produced
//! - `plots.jsonl`: every plot a tracker stage was given (what the sensor saw)
//! - `associations.jsonl`: where each detection went (detection sources)
//! - `trace.jsonl`: the engine's trace (`scripts/benchmark/replay/replay-video.py` renders it)
//! - `run.json`: counts and timings

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use chrono::{DateTime, Utc};
use ot_core::Observation;
use ot_source::frame::Frame;
use ot_source::pipeline::Pipeline;
use ot_source::source::SourceSpec;
use ot_store::SourceWrite;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Engine, EngineSettings};
use crate::config::Common;
use crate::correlate::CorrelationSettings;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Scenario {
    name: String,
    #[serde(default)]
    description: Option<String>,
    sources: Vec<ScenarioSource>,
    #[serde(default)]
    engine: EngineOverrides,
    /// Correlation settings (unset fields take the defaults).
    #[serde(default)]
    correlation: Option<Value>,
    /// Scenario seconds the engine processes at a time.
    #[serde(default = "one")]
    step_secs: f64,
    /// Scenario seconds between samples of the system tracks.
    #[serde(default = "one")]
    sample_secs: f64,
    /// Truth file, for the scorer.
    #[serde(default)]
    #[allow(dead_code)]
    truth: Option<String>,
    /// Anything else the builder records (sensor settings, provenance).
    #[serde(default)]
    #[allow(dead_code)]
    notes: Option<Value>,
}

fn one() -> f64 {
    1.0
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScenarioSource {
    spec: Value,
    frames: String,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct EngineOverrides {
    confirm_after: Option<u64>,
    drop_after_secs: Option<f64>,
    stale_air_secs: Option<f64>,
    stale_surface_secs: Option<f64>,
    stale_other_secs: Option<f64>,
}

#[derive(Deserialize)]
struct FrameLine {
    t: DateTime<Utc>,
    #[serde(default)]
    json: Option<Value>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    hex: Option<String>,
}

fn unhex(s: &str) -> anyhow::Result<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        bail!("odd hex length");
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).context("bad hex"))
        .collect()
}

fn read_frames(path: &Path) -> anyhow::Result<Vec<(DateTime<Utc>, Vec<u8>)>> {
    let text = std::fs::read_to_string(path).with_context(|| path.display().to_string())?;
    let mut out = Vec::new();
    for (n, line) in text
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
    {
        let f: FrameLine =
            serde_json::from_str(line).with_context(|| format!("{}:{}", path.display(), n + 1))?;
        let bytes = match (f.json, f.text, f.hex) {
            (Some(v), None, None) => serde_json::to_vec(&v)?,
            (None, Some(t), None) => t.into_bytes(),
            (None, None, Some(h)) => {
                unhex(&h).with_context(|| format!("{}:{}", path.display(), n + 1))?
            }
            _ => bail!(
                "{}:{}: a frame has one of json, text or hex",
                path.display(),
                n + 1
            ),
        };
        out.push((f.t, bytes));
    }
    Ok(out)
}

pub struct BenchOptions {
    pub scenario: PathBuf,
    pub out: PathBuf,
    /// Correlation settings to use instead of the scenario's (for sweeps).
    pub correlation: Option<PathBuf>,
    /// Plugins to load first (files or addresses).
    pub plugins: Vec<String>,
}

struct Writers {
    tracks: std::io::BufWriter<std::fs::File>,
    source_tracks: std::io::BufWriter<std::fs::File>,
    plots: std::io::BufWriter<std::fs::File>,
}

fn writer(dir: &Path, name: &str) -> anyhow::Result<std::io::BufWriter<std::fs::File>> {
    Ok(std::io::BufWriter::new(std::fs::File::create(
        dir.join(name),
    )?))
}

pub async fn run(common: &Common, opts: BenchOptions) -> anyhow::Result<()> {
    let wall = Instant::now();
    let dir = &opts.scenario;
    let scenario: Scenario = serde_json::from_str(
        &std::fs::read_to_string(dir.join("scenario.json"))
            .with_context(|| format!("{}/scenario.json", dir.display()))?,
    )
    .context("scenario.json")?;
    std::fs::create_dir_all(&opts.out)?;
    let mut plugins = Vec::new();
    for arg in &opts.plugins {
        // An external plugin's secret comes from OT_PLUGIN_SECRET.
        let secret = crate::plugin_cli::secret_arg(None)?;
        let source = crate::plugin_cli::source_arg(common, arg, secret)?;
        let p = ot_plugin::load(&source, &ot_plugin::Grants::default())
            .with_context(|| format!("plugin {arg}"))?;
        plugins
            .push(json!({"name": p.manifest().name, "version": p.manifest().version, "from": arg}));
        ot_source::plugin::install(p).map_err(anyhow::Error::msg)?;
    }

    // Sources, their pipelines and their frames, in one timeline.
    let mut specs = Vec::new();
    let mut pipelines = BTreeMap::new();
    let mut timeline: Vec<(DateTime<Utc>, usize, Vec<u8>)> = Vec::new();
    for s in &scenario.sources {
        let spec: SourceSpec =
            serde_json::from_value(s.spec.clone()).context("a scenario source spec")?;
        // A bench replays recorded frames and never opens the transport,
        // so a listener's sender authentication is not its concern.
        match spec.validate() {
            Ok(()) | Err(ot_source::source::SourceError::Unauthenticated(_)) => {}
            Err(e) => bail!("source {}: {e}", spec.id),
        }
        let pipeline = Pipeline::new(spec.id.clone(), spec.pipeline.clone())
            .map_err(|e| anyhow::anyhow!("source {}: {e}", spec.id))?
            .keep_plots(true);
        let i = specs.len();
        for (t, bytes) in read_frames(&dir.join(&s.frames))? {
            timeline.push((t, i, bytes));
        }
        pipelines.insert(i, pipeline);
        specs.push((spec, s.spec.clone()));
    }
    timeline.sort_by_key(|f| (f.0, f.1));
    let (Some(first), Some(last)) = (timeline.first().map(|f| f.0), timeline.last().map(|f| f.0))
    else {
        bail!("the scenario has no frames");
    };

    // A database and Redis namespace of the run's own.
    let tmp = std::env::temp_dir().join(format!(
        "ot-bench-{}-{}",
        std::process::id(),
        Utc::now().timestamp_micros()
    ));
    std::fs::create_dir_all(&tmp)?;
    let mut c = common.clone();
    c.sqlite = tmp.join("bench.db");
    c.redis_namespace = format!(
        "ot-bench-{}-{}",
        std::process::id(),
        Utc::now().timestamp_micros()
    );
    {
        let mut db = c.open_db()?;
        for (spec, raw) in &specs {
            db.put_source(
                &SourceWrite {
                    id: &spec.id,
                    name: &spec.name,
                    transport: spec.transport.kind(),
                    codec: spec.pipeline.codec.name(),
                    priority: spec.priority,
                    spec: raw,
                },
                "bench",
            )?;
            db.set_source_enabled(&spec.id, true, "bench")?;
        }
    }
    let mut settings = EngineSettings::default();
    let o = &scenario.engine;
    if let Some(n) = o.confirm_after {
        settings.confirm_after = n.max(1);
    }
    let secs = |s: f64| Duration::from_secs_f64(s.max(0.001));
    if let Some(s) = o.drop_after_secs {
        settings.drop_after = secs(s);
    }
    if let Some(s) = o.stale_air_secs {
        settings.stale_air = secs(s);
    }
    if let Some(s) = o.stale_surface_secs {
        settings.stale_surface = secs(s);
    }
    if let Some(s) = o.stale_other_secs {
        settings.stale_other = secs(s);
    }
    let correlation = match &opts.correlation {
        Some(p) => Some(serde_json::from_str::<Value>(&std::fs::read_to_string(p)?)?),
        None => scenario.correlation.clone(),
    };
    if let Some(v) = correlation {
        settings.correlation =
            serde_json::from_value::<CorrelationSettings>(v).context("correlation settings")?;
    }
    let correlation_used = serde_json::to_value(&settings.correlation)?;

    let mut e = Engine::new(c.clone(), settings).await?;
    e.recording = true;
    e.read_block = Duration::ZERO;
    e.sim_now = Some(first);
    e.refresh_sources().await?;
    e.refresh_correlation().await?;
    e.refresh_attributes().await?;

    let result = replay(
        &mut e,
        &scenario,
        &mut pipelines,
        &timeline,
        first,
        last,
        &opts.out,
    )
    .await;
    let _ = e.redis.purge_namespace().await;
    let _ = std::fs::remove_dir_all(&tmp);
    let (frames, observations, timing, errors) = result?;

    let mut associations = writer(&opts.out, "associations.jsonl")?;
    for (key, uid) in &e.associations {
        writeln!(
            associations,
            "{}",
            json!({"key": key, "track": uid.map(|u| u.doc_id())})
        )?;
    }
    associations.flush()?;
    let mut trace = writer(&opts.out, "trace.jsonl")?;
    for v in &e.trace {
        writeln!(trace, "{v}")?;
    }
    trace.flush()?;
    let run = json!({
        "scenario": scenario.name,
        "description": scenario.description,
        "opentrack": env!("CARGO_PKG_VERSION"),
        "correlation_version": crate::correlate::VERSION,
        "start": first, "end": last,
        "scenario_secs": (last - first).num_milliseconds() as f64 / 1000.0,
        "wall_secs": wall.elapsed().as_secs_f64(),
        "timing_secs": {
            "pipelines": timing.pipelines.as_secs_f64(),
            "engine": timing.engine.as_secs_f64(),
            "sampling": timing.sampling.as_secs_f64(),
        },
        "frames": frames,
        "observations": observations,
        "sources": specs.iter().map(|(s, _)| json!({
            "id": s.id, "reports": s.reports,
            "tracker": s.pipeline.tracker.as_ref().map(|t| serde_json::to_value(t).unwrap_or(Value::Null)),
        })).collect::<Vec<_>>(),
        "correlation": correlation_used,
        "plugins": plugins,
        "errors": errors.iter().map(|(s, (n, last))| (s.clone(), json!({"count": n, "last": last}))).collect::<serde_json::Map<_, _>>(),
    });
    std::fs::write(
        opts.out.join("run.json"),
        serde_json::to_string_pretty(&run)?,
    )?;
    eprintln!(
        "{}: {frames} frames, {observations} observations, {:.0} s of scenario in {:.1} s",
        scenario.name,
        run["scenario_secs"].as_f64().unwrap_or(0.0),
        run["wall_secs"].as_f64().unwrap_or(0.0)
    );
    Ok(())
}

const REAP_EVERY_SECS: i64 = 10;

fn specs_id(p: &Pipeline) -> String {
    p.source_id().to_owned()
}

/// Wall time spent in each part of a run.
#[derive(Default)]
struct Timing {
    pipelines: Duration,
    engine: Duration,
    sampling: Duration,
}

#[allow(clippy::too_many_arguments)]
async fn replay(
    e: &mut Engine,
    scenario: &Scenario,
    pipelines: &mut BTreeMap<usize, Pipeline>,
    timeline: &[(DateTime<Utc>, usize, Vec<u8>)],
    first: DateTime<Utc>,
    last: DateTime<Utc>,
    out: &Path,
) -> anyhow::Result<(usize, usize, Timing, BTreeMap<String, (u64, String)>)> {
    let registry = BTreeMap::new();
    let mut w = Writers {
        tracks: writer(out, "tracks.jsonl")?,
        source_tracks: writer(out, "source_tracks.jsonl")?,
        plots: writer(out, "plots.jsonl")?,
    };
    let ms = |s: f64| chrono::Duration::milliseconds((s.max(0.01) * 1000.0) as i64);
    let (step, sample) = (ms(scenario.step_secs), ms(scenario.sample_secs));
    // Let tracks age out after the last frame, so their ends are scored.
    let tail = chrono::Duration::seconds(30);
    let (mut now, mut next_sample, mut next_reap) = (first, first, first);
    let (mut i, mut observations) = (0, 0);
    let mut timing = Timing::default();
    let mut errors: BTreeMap<String, (u64, String)> = BTreeMap::new();
    while now <= last + tail {
        now += step;
        let clock = Instant::now();
        let mut batch: Vec<Observation> = Vec::new();
        while i < timeline.len() && timeline[i].0 < now {
            let (t, src, bytes) = &timeline[i];
            let mut frame = Frame::new(bytes.clone());
            frame.received_at = *t;
            let p = pipelines.get_mut(src).expect("pipeline");
            let out = p.process(&frame, &registry);
            if let Some(e) = &out.last_error {
                let entry = errors.entry(specs_id(p)).or_insert((0, String::new()));
                entry.0 += 1;
                entry.1.clone_from(e);
            }
            for o in &out.plots {
                writeln!(
                    w.plots,
                    "{}",
                    json!({
                        "t": o.observed_at, "source": o.source_id,
                        "lat": o.position.latitude, "lon": o.position.longitude,
                    })
                )?;
            }
            batch.extend(out.observations);
            i += 1;
        }
        let end = now > last;
        for p in pipelines.values_mut() {
            let out = p.flush(now, end);
            if let Some(e) = &out.last_error {
                let entry = errors.entry(specs_id(p)).or_insert((0, String::new()));
                entry.0 += 1;
                entry.1.clone_from(e);
            }
            batch.extend(out.observations);
        }
        for o in &batch {
            writeln!(
                w.source_tracks,
                "{}",
                json!({
                    "t": o.observed_at, "source": o.source_id, "key": o.source_track_key,
                    "lat": o.position.latitude, "lon": o.position.longitude,
                    "alt": o.position.altitude_hae_m,
                    "course": o.kinematics.course_deg, "speed": o.kinematics.speed_mps,
                    "state": o.state,
                })
            )?;
        }
        observations += batch.len();
        timing.pipelines += clock.elapsed();
        let clock = Instant::now();
        let mut by_source: BTreeMap<&str, Vec<Observation>> = BTreeMap::new();
        for o in &batch {
            by_source
                .entry(o.source_id.as_str())
                .or_default()
                .push(o.clone());
        }
        for (source, obs) in by_source {
            e.redis.append_observations(source, &obs).await?;
        }
        e.sim_now = Some(now);
        while e.pump(false).await? > 0 {}
        if now >= next_reap {
            e.reap().await?;
            next_reap = now + chrono::Duration::seconds(REAP_EVERY_SECS);
        }
        timing.engine += clock.elapsed();
        let clock = Instant::now();
        while next_sample <= now {
            sample_tracks(e, next_sample, &mut w.tracks)?;
            next_sample += sample;
        }
        timing.sampling += clock.elapsed();
    }
    w.tracks.flush()?;
    w.source_tracks.flush()?;
    w.plots.flush()?;
    for (source, (n, last)) in &errors {
        eprintln!("{source}: {n} errors; the last: {last}");
    }
    Ok((timeline.len(), observations, timing, errors))
}

fn sample_tracks(e: &Engine, t: DateTime<Utc>, w: &mut impl Write) -> anyhow::Result<()> {
    for track in e.tracks.values() {
        if track.first_seen > t {
            continue;
        }
        let v = &track.view;
        writeln!(
            w,
            "{}",
            json!({
                "t": t, "uid": track.uid.doc_id(), "kind": track.kind, "state": track.state,
                "published": track.published, "observed_at": v.observed_at,
                "lat": v.position.latitude, "lon": v.position.longitude,
                "alt": v.position.altitude_hae_m,
                "course": v.kinematics.course_deg, "speed": v.kinematics.speed_mps,
                "cep": v.uncertainty.as_ref().and_then(|u| u.circular_error_m),
                "domain": v.classification.domain,
                "name": v.name,
                "contributors": track.contributors.iter()
                    .map(|c| format!("{}/{}", c.source_id, c.source_track_key))
                    .collect::<Vec<_>>(),
            })
        )?;
    }
    Ok(())
}
