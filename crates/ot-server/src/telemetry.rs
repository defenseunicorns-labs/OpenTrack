//! Logs, traces and metrics: to standard output as always, and over
//! OpenTelemetry (OTLP) to the deployment's collector when one is set.
//!
//! OpenTrack keeps no log store of its own beyond the audit table: the
//! collector and the back end behind it (SIEM, log store) take the records
//! from here. Everything is configured with the standard `OTEL_*` variables:
//!
//! - `OTEL_EXPORTER_OTLP_ENDPOINT` (or `OTEL_EXPORTER_OTLP_{LOGS,TRACES,METRICS}_ENDPOINT`)
//!   turns export on; without one, nothing is exported.
//! - `OTEL_EXPORTER_OTLP_PROTOCOL` (and per signal): `grpc` or
//!   `http/protobuf` (the default).
//! - `OTEL_EXPORTER_OTLP_CERTIFICATE`, `..._CLIENT_CERTIFICATE`,
//!   `..._CLIENT_KEY` (and per signal): PEM files for TLS and mutual TLS.
//!   TLS always runs on the process's FIPS provider.
//! - `OTEL_EXPORTER_OTLP_HEADERS`, `..._TIMEOUT`, `..._COMPRESSION`,
//!   `OTEL_SERVICE_NAME`, `OTEL_RESOURCE_ATTRIBUTES`, `OTEL_TRACES_SAMPLER`,
//!   `OTEL_METRIC_EXPORT_INTERVAL`, `OTEL_{LOGS,TRACES,METRICS}_EXPORTER=none`
//!   and `OTEL_SDK_DISABLED` work as the specification says.
//!
//! Each exporter is watched: the last success and failure of every signal
//! are kept in [`Health`], logged when export starts or stops failing, and
//! written to Redis by each server role so `/status` (the Overview's
//! Telemetry row) shows them. A collector outage never stops OpenTrack:
//! records that cannot be sent are dropped from the export (never queued
//! without bound), standard output keeps every log line, and the audit
//! table keeps every audit record (`GET /api/v1/audit` fills the gap).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use opentelemetry::KeyValue;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::{WithHttpConfig, WithTonicConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::logs::{LogBatch, LogExporter, SdkLoggerProvider};
use opentelemetry_sdk::metrics::data::ResourceMetrics;
use opentelemetry_sdk::metrics::exporter::PushMetricExporter;
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider, Temporality};
use opentelemetry_sdk::trace::{Sampler, SdkTracerProvider, SpanData, SpanExporter};
use serde_json::{Value, json};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::config::Common;

/// The three signals, as the `OTEL_*` variables name them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Signal {
    Logs,
    Traces,
    Metrics,
}

impl Signal {
    const ALL: [Signal; 3] = [Signal::Logs, Signal::Traces, Signal::Metrics];

    fn name(self) -> &'static str {
        match self {
            Signal::Logs => "logs",
            Signal::Traces => "traces",
            Signal::Metrics => "metrics",
        }
    }

    fn env(self) -> &'static str {
        match self {
            Signal::Logs => "LOGS",
            Signal::Traces => "TRACES",
            Signal::Metrics => "METRICS",
        }
    }
}

/// Where and how one signal is exported, from the environment.
#[derive(Debug, Clone, PartialEq)]
struct Target {
    endpoint: String,
    grpc: bool,
    ca_file: Option<String>,
    cert_file: Option<String>,
    key_file: Option<String>,
}

/// A variable, per signal first, then the general one; empty is unset.
fn otlp_var(env: &impl Fn(&str) -> Option<String>, signal: Signal, name: &str) -> Option<String> {
    env(&format!("OTEL_EXPORTER_OTLP_{}_{name}", signal.env()))
        .or_else(|| env(&format!("OTEL_EXPORTER_OTLP_{name}")))
        .filter(|v| !v.trim().is_empty())
}

/// The signal's export target, `Ok(None)` when it is not exported.
fn target(env: &impl Fn(&str) -> Option<String>, signal: Signal) -> Result<Option<Target>, String> {
    if env("OTEL_SDK_DISABLED").is_some_and(|v| v.trim().eq_ignore_ascii_case("true")) {
        return Ok(None);
    }
    if env(&format!("OTEL_{}_EXPORTER", signal.env()))
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("none"))
    {
        return Ok(None);
    }
    let Some(endpoint) = otlp_var(env, signal, "ENDPOINT") else {
        return Ok(None);
    };
    let grpc = match otlp_var(env, signal, "PROTOCOL").as_deref().map(str::trim) {
        None | Some("http/protobuf") => false,
        Some("grpc") => true,
        Some(other) => {
            return Err(format!(
                "OTEL_EXPORTER_OTLP_PROTOCOL `{other}` is not supported: use `grpc` or `http/protobuf`"
            ));
        }
    };
    Ok(Some(Target {
        endpoint,
        grpc,
        ca_file: otlp_var(env, signal, "CERTIFICATE"),
        cert_file: otlp_var(env, signal, "CLIENT_CERTIFICATE"),
        key_file: otlp_var(env, signal, "CLIENT_KEY"),
    }))
}

/// How one signal's export is going.
#[derive(Debug, Clone, Default)]
struct SignalHealth {
    endpoint: String,
    protocol: &'static str,
    last_ok_ms: Option<i64>,
    /// The latest failure, and since when export has been failing; cleared
    /// by the next success.
    error: Option<String>,
    failing_since_ms: Option<i64>,
    /// Export could not be set up (bad variable, unreadable certificate).
    setup_error: Option<String>,
}

/// Every exported signal's health, shared with the exporters.
#[derive(Debug, Default)]
pub struct Health {
    signals: Mutex<BTreeMap<Signal, SignalHealth>>,
}

impl Health {
    fn with<R>(&self, f: impl FnOnce(&mut BTreeMap<Signal, SignalHealth>) -> R) -> R {
        f(&mut self.signals.lock().unwrap_or_else(|p| p.into_inner()))
    }

    fn add(&self, signal: Signal, t: &Target) {
        self.with(|s| {
            s.insert(
                signal,
                SignalHealth {
                    endpoint: t.endpoint.clone(),
                    protocol: if t.grpc { "grpc" } else { "http/protobuf" },
                    ..Default::default()
                },
            );
        });
    }

    fn setup_failed(&self, signal: Signal, endpoint: String, error: String) {
        tracing::error!(signal = signal.name(), %endpoint, %error, "OpenTelemetry export not started");
        self.with(|s| {
            s.insert(
                signal,
                SignalHealth {
                    endpoint,
                    setup_error: Some(error),
                    ..Default::default()
                },
            );
        });
    }

    /// Note an export's result; log when export starts or stops failing.
    fn record(&self, signal: Signal, result: &OTelSdkResult) {
        let now = chrono::Utc::now().timestamp_millis();
        let change = self.with(|s| {
            let h = s.entry(signal).or_default();
            match result {
                Ok(()) => {
                    h.last_ok_ms = Some(now);
                    h.failing_since_ms = None;
                    h.error.take().map(|_| None)
                }
                Err(e) => {
                    let first = h.failing_since_ms.is_none();
                    h.failing_since_ms.get_or_insert(now);
                    h.error = Some(e.to_string());
                    first.then(|| Some(e.to_string()))
                }
            }
        });
        // Target `opentrack::telemetry`, which is never exported itself: a
        // failing collector would otherwise be fed its own failures.
        match change {
            Some(Some(error)) => tracing::warn!(
                signal = signal.name(),
                %error,
                "OpenTelemetry export failing: records are dropped from the export until it recovers (standard output keeps the logs, the audit table the audit records)"
            ),
            Some(None) => {
                tracing::info!(signal = signal.name(), "OpenTelemetry export recovered");
                if let Some(endpoint) = self.with(|s| s.get(&signal).map(|h| h.endpoint.clone())) {
                    log_destination(
                        signal,
                        &endpoint,
                        "OpenTelemetry collector reconnected (resolved address)",
                    );
                }
            }
            None => {}
        }
    }

    /// For each failing signal, whether its collector takes connections
    /// again: a signal only exports when it has something to send, so an
    /// idle one would otherwise stay failing after the collector is back.
    /// The next export confirms it (or fails it again).
    async fn probe(&self) {
        let failing: Vec<(Signal, String)> = self.with(|s| {
            s.iter()
                .filter(|(_, h)| h.failing_since_ms.is_some())
                .map(|(sig, h)| (*sig, h.endpoint.clone()))
                .collect()
        });
        for (signal, endpoint) in failing {
            let Some(addr) = host_port(&endpoint) else {
                continue;
            };
            let connect = tokio::net::TcpStream::connect(addr);
            if let Ok(Ok(_)) = tokio::time::timeout(Duration::from_secs(2), connect).await {
                self.record(signal, &Ok(()));
            }
        }
    }

    /// Whether any signal is exported (or was meant to be).
    pub fn configured(&self) -> bool {
        self.with(|s| !s.is_empty())
    }

    /// This process's status, as `/status` shows it.
    pub fn status(&self, role: &str) -> Value {
        self.with(|s| {
            let signals: serde_json::Map<String, Value> = s
                .iter()
                .map(|(sig, h)| {
                    let error = h.setup_error.clone().or_else(|| h.error.clone());
                    let mut v = json!({
                        "ok": error.is_none(),
                        "endpoint": h.endpoint,
                        "protocol": h.protocol,
                        "last_ok_ms": h.last_ok_ms,
                        "failing_since_ms": h.failing_since_ms,
                    });
                    if let Some(e) = error {
                        v["error"] = json!(e);
                    }
                    (sig.name().to_owned(), v)
                })
                .collect();
            json!({
                "role": role,
                "host": hostname(),
                "pid": std::process::id(),
                "at_ms": chrono::Utc::now().timestamp_millis(),
                "ok": signals.values().all(|v| v["ok"] == true),
                "signals": signals,
            })
        })
    }
}

/// Log the addresses a signal's collector resolves to (ASD STIG V-222470).
/// The exporters keep their sockets to themselves, so this resolves the
/// endpoint's host; on a thread of its own, since exports may call it from
/// anywhere and resolution blocks.
fn log_destination(signal: Signal, endpoint: &str, message: &'static str) {
    let Some(addr) = host_port(endpoint) else {
        return;
    };
    let signal = signal.name();
    std::thread::spawn(move || {
        use std::net::ToSocketAddrs;
        let peer_addr = match addr.to_socket_addrs() {
            Ok(addrs) => ot_core::netlog::addr_list(addrs),
            Err(e) => format!("unresolved ({e})"),
        };
        tracing::info!(component = "otlp", signal, server = %addr, %peer_addr, "{message}");
    });
}

/// `host:port` of an endpoint URL (the scheme's port when it has none).
fn host_port(endpoint: &str) -> Option<String> {
    let uri: axum::http::Uri = endpoint.trim().parse().ok()?;
    let host = uri.host()?.trim_start_matches('[').trim_end_matches(']');
    let port = uri
        .port_u16()
        .unwrap_or(if uri.scheme_str() == Some("https") {
            443
        } else {
            80
        });
    Some(if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    })
}

fn hostname() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|h| !h.is_empty())
        .or_else(|| {
            std::fs::read_to_string("/proc/sys/kernel/hostname")
                .ok()
                .map(|h| h.trim().to_owned())
        })
        .unwrap_or_else(|| "unknown".into())
}

/// An exporter whose results go to [`Health`].
#[derive(Debug)]
struct Watched<E> {
    inner: E,
    signal: Signal,
    health: Arc<Health>,
}

impl<E: LogExporter> LogExporter for Watched<E> {
    async fn export(&self, batch: LogBatch<'_>) -> OTelSdkResult {
        let r = self.inner.export(batch).await;
        self.health.record(self.signal, &r);
        r
    }
    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        self.inner.shutdown_with_timeout(timeout)
    }
    fn set_resource(&mut self, resource: &Resource) {
        self.inner.set_resource(resource);
    }
}

impl<E: SpanExporter> SpanExporter for Watched<E> {
    async fn export(&self, batch: Vec<SpanData>) -> OTelSdkResult {
        let r = self.inner.export(batch).await;
        self.health.record(self.signal, &r);
        r
    }
    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        self.inner.shutdown_with_timeout(timeout)
    }
    fn force_flush(&self) -> OTelSdkResult {
        self.inner.force_flush()
    }
    fn set_resource(&mut self, resource: &Resource) {
        self.inner.set_resource(resource);
    }
}

impl<E: PushMetricExporter> PushMetricExporter for Watched<E> {
    async fn export(&self, metrics: &ResourceMetrics) -> OTelSdkResult {
        let r = self.inner.export(metrics).await;
        self.health.record(self.signal, &r);
        r
    }
    fn force_flush(&self) -> OTelSdkResult {
        self.inner.force_flush()
    }
    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        self.inner.shutdown_with_timeout(timeout)
    }
    fn temporality(&self) -> Temporality {
        self.inner.temporality()
    }
}

/// The TLS client configuration for an HTTP target: the process's FIPS
/// provider, the target's CA (or the system roots) and client certificate.
fn http_client(t: &Target) -> anyhow::Result<reqwest::blocking::Client> {
    let tls = ot_source::tls::ClientTls {
        ca_file: t.ca_file.clone(),
        cert_file: t.cert_file.clone(),
        key_file: t.key_file.clone(),
        ..Default::default()
    }
    .client_config()?;
    // The blocking client runs its own runtime, which may not be built from
    // inside Tokio's.
    std::thread::spawn(move || {
        reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(10))
            .tls_backend_preconfigured(tls)
            .build()
    })
    .join()
    .map_err(|_| anyhow::anyhow!("building the HTTP client panicked"))?
    .map_err(Into::into)
}

/// The tonic TLS configuration for an `https` gRPC target (TLS runs on the
/// process's FIPS provider).
fn grpc_tls(
    t: &Target,
) -> anyhow::Result<Option<opentelemetry_otlp::tonic_types::transport::ClientTlsConfig>> {
    use opentelemetry_otlp::tonic_types::transport::{Certificate, ClientTlsConfig, Identity};
    if !t
        .endpoint
        .trim_start()
        .to_ascii_lowercase()
        .starts_with("https://")
    {
        return Ok(None);
    }
    let read = |what: &str, path: &str| {
        std::fs::read(path).map_err(|e| anyhow::anyhow!("{what} {path}: {e}"))
    };
    let mut tls = ClientTlsConfig::new();
    tls = match &t.ca_file {
        Some(ca) => tls.ca_certificate(Certificate::from_pem(read("CA certificate", ca)?)),
        None => tls.with_native_roots(),
    };
    match (&t.cert_file, &t.key_file) {
        (Some(c), Some(k)) => {
            tls = tls.identity(Identity::from_pem(
                read("client certificate", c)?,
                read("client key", k)?,
            ))
        }
        (None, None) => {}
        _ => anyhow::bail!("a client certificate needs its key (and the other way round)"),
    }
    Ok(Some(tls))
}

/// Build one signal's OTLP exporter for `t`.
macro_rules! exporter {
    ($kind:ident, $t:expr) => {{
        let t: &Target = $t;
        let build = || -> anyhow::Result<_> {
            if t.grpc {
                let mut b = opentelemetry_otlp::$kind::builder().with_tonic();
                if let Some(tls) = grpc_tls(t)? {
                    b = b.with_tls_config(tls);
                }
                Ok(b.build()?)
            } else {
                Ok(opentelemetry_otlp::$kind::builder()
                    .with_http()
                    .with_http_client(http_client(t)?)
                    .build()?)
            }
        };
        build()
    }};
}

/// Targets whose own events must not be exported as logs: the exporters'
/// transports, and this module (its "export failing" warnings).
fn not_exported(target: &str) -> bool {
    const SKIP: &[&str] = &[
        "opentelemetry",
        "opentrack::telemetry",
        "h2",
        "hyper",
        "tonic",
        "tower",
        "reqwest",
        "rustls",
    ];
    SKIP.iter().any(|s| {
        target == *s
            || target
                .strip_prefix(s)
                .is_some_and(|r| r.starts_with("::") || r.starts_with('_'))
    })
}

/// This process's telemetry; [`shutdown`](Telemetry::shutdown) it before exit.
pub struct Telemetry {
    pub health: Arc<Health>,
    role: &'static str,
    logs: Option<SdkLoggerProvider>,
    traces: Option<SdkTracerProvider>,
    metrics: Option<SdkMeterProvider>,
}

/// Set up logging for `role`: standard output (standard error for the
/// one-off commands, whose output is on standard output), and OTLP export
/// of each signal whose endpoint is set. Call once, inside the runtime and
/// after [`crate::fips::init`].
pub fn init(role: &'static str, to_stderr: bool, common: &Common) -> Telemetry {
    let env = |k: &str| std::env::var(k).ok();
    let health = Arc::new(Health::default());
    let resource = {
        let name = env("OTEL_SERVICE_NAME")
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| "opentrack".into());
        Resource::builder_empty()
            .with_service_name(name)
            .with_attributes([
                KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
                KeyValue::new(
                    "service.instance.id",
                    format!("{}:{}", hostname(), std::process::id()),
                ),
                KeyValue::new("opentrack.node_id", common.node_id()),
                KeyValue::new("opentrack.role", role),
            ])
            .with_detector(Box::new(
                opentelemetry_sdk::resource::EnvResourceDetector::new(),
            ))
            .build()
    };

    let mut setup_errors = Vec::new();
    let mut targets = BTreeMap::new();
    for s in Signal::ALL {
        match target(&env, s) {
            Ok(Some(t)) => {
                targets.insert(s, t);
            }
            Ok(None) => {}
            Err(e) => setup_errors.push((s, otlp_var(&env, s, "ENDPOINT").unwrap_or_default(), e)),
        }
    }
    let mut build_errors = Vec::new();
    let mut failed = |s: Signal, t: &Target, e: anyhow::Error| {
        build_errors.push((s, t.endpoint.clone(), format!("{e:#}")));
    };

    let logs = targets
        .get(&Signal::Logs)
        .and_then(|t| match exporter!(LogExporter, t) {
            Ok(e) => {
                health.add(Signal::Logs, t);
                let e = Watched {
                    inner: e,
                    signal: Signal::Logs,
                    health: health.clone(),
                };
                Some(
                    SdkLoggerProvider::builder()
                        .with_resource(resource.clone())
                        .with_batch_exporter(e)
                        .build(),
                )
            }
            Err(e) => {
                failed(Signal::Logs, t, e);
                None
            }
        });
    let traces = targets
        .get(&Signal::Traces)
        .and_then(|t| match exporter!(SpanExporter, t) {
            Ok(e) => {
                health.add(Signal::Traces, t);
                let e = Watched {
                    inner: e,
                    signal: Signal::Traces,
                    health: health.clone(),
                };
                let mut b = SdkTracerProvider::builder()
                    .with_resource(resource.clone())
                    .with_batch_exporter(e);
                // Unless told otherwise, one trace in ten: the pipeline makes
                // many a second.
                if env("OTEL_TRACES_SAMPLER").is_none_or(|v| v.trim().is_empty()) {
                    b = b.with_sampler(Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(
                        0.1,
                    ))));
                }
                Some(b.build())
            }
            Err(e) => {
                failed(Signal::Traces, t, e);
                None
            }
        });
    let metrics = targets
        .get(&Signal::Metrics)
        .and_then(|t| match exporter!(MetricExporter, t) {
            Ok(e) => {
                health.add(Signal::Metrics, t);
                let e = Watched {
                    inner: e,
                    signal: Signal::Metrics,
                    health: health.clone(),
                };
                let p = SdkMeterProvider::builder()
                    .with_resource(resource.clone())
                    .with_reader(PeriodicReader::builder(e).build())
                    .build();
                opentelemetry::global::set_meter_provider(p.clone());
                Some(p)
            }
            Err(e) => {
                failed(Signal::Metrics, t, e);
                None
            }
        });

    let filter = || {
        tracing_subscriber::EnvFilter::try_from_env("OT_LOG")
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"))
    };
    let json = env("OT_LOG_FORMAT").as_deref() == Some("json");
    let stdout: Box<dyn Layer<_> + Send + Sync> = match (json, to_stderr) {
        (true, true) => tracing_subscriber::fmt::layer()
            .json()
            .with_writer(std::io::stderr)
            .boxed(),
        (true, false) => tracing_subscriber::fmt::layer().json().boxed(),
        (false, true) => tracing_subscriber::fmt::layer()
            .with_writer(std::io::stderr)
            .boxed(),
        (false, false) => tracing_subscriber::fmt::layer().boxed(),
    };
    let otel_logs = logs.as_ref().map(|p| {
        opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(p).with_filter(
            tracing_subscriber::filter::filter_fn(|m| !not_exported(m.target())),
        )
    });
    let otel_traces = traces
        .as_ref()
        .map(|p| tracing_opentelemetry::layer().with_tracer(p.tracer("opentrack")));
    tracing_subscriber::registry()
        .with(filter())
        .with(stdout)
        .with(otel_logs)
        .with(otel_traces)
        .init();

    for (s, endpoint, e) in setup_errors.into_iter().chain(build_errors) {
        health.setup_failed(s, endpoint, e);
    }
    if health.configured() {
        let exported: Vec<_> = Signal::ALL
            .iter()
            .filter(|s| targets.contains_key(s))
            .map(|s| format!("{} → {}", s.name(), targets[s].endpoint))
            .collect();
        tracing::info!(role, signals = %exported.join(", "), "exporting over OpenTelemetry");
        for (s, t) in &targets {
            log_destination(
                *s,
                &t.endpoint,
                "OpenTelemetry collector (resolved address)",
            );
        }
    }
    Telemetry {
        health,
        role,
        logs,
        traces,
        metrics,
    }
}

/// How often each server role writes its export status to Redis, and how
/// old a status may be before `/status` ignores it (the process is gone).
const REPORT_EVERY: Duration = Duration::from_secs(10);
const STATUS_STALE: Duration = Duration::from_secs(60);
/// A status this old is removed.
const STATUS_FORGET: Duration = Duration::from_secs(24 * 3600);

impl Telemetry {
    /// For a server role with export configured: write this process's export
    /// status to Redis every few seconds, for `/status`.
    pub fn report(&self, common: Common) {
        if !self.health.configured() {
            return;
        }
        let (health, role) = (self.health.clone(), self.role);
        let process = format!("{role}@{}:{}", hostname(), std::process::id());
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(REPORT_EVERY);
            let mut redis = None;
            loop {
                tick.tick().await;
                if redis.is_none() {
                    redis = common.open_redis().await.ok();
                }
                health.probe().await;
                if let Some(r) = &redis
                    && let Err(e) = r.put_telemetry_status(&process, &health.status(role)).await
                {
                    tracing::debug!(error = %e, "telemetry status not stored");
                }
            }
        });
    }

    /// Export what is still queued, then stop.
    pub async fn shutdown(self) {
        let Telemetry {
            logs,
            traces,
            metrics,
            ..
        } = self;
        // The providers block while their exporters finish, which the
        // exporters need the runtime's other threads for.
        let _ = tokio::task::spawn_blocking(move || {
            if let Some(p) = metrics {
                let _ = p.shutdown();
            }
            if let Some(p) = traces {
                let _ = p.shutdown();
            }
            if let Some(p) = logs {
                let _ = p.shutdown();
            }
        })
        .await;
    }
}

/// Every process's export status for `/status`: `configured` when any
/// server role exports, `ok` when every live one's signals are, the
/// processes themselves, and the first error.
pub async fn status(redis: &ot_store::redis_store::RedisStore) -> Value {
    let all = match redis.telemetry_status().await {
        Ok(a) => a,
        Err(e) => return json!({ "configured": false, "ok": false, "error": e.to_string() }),
    };
    let now = chrono::Utc::now().timestamp_millis();
    let age = |v: &Value| now - v["at_ms"].as_i64().unwrap_or(0);
    let forget: Vec<String> = all
        .iter()
        .filter(|(_, v)| age(v) > STATUS_FORGET.as_millis() as i64)
        .map(|(k, _)| k.clone())
        .collect();
    let _ = redis.forget_telemetry_status(&forget).await;
    let live: Vec<&Value> = all
        .values()
        .filter(|v| age(v) <= STATUS_STALE.as_millis() as i64)
        .collect();
    if live.is_empty() {
        return json!({ "configured": false, "ok": true });
    }
    let error = live.iter().find_map(|p| {
        p["signals"].as_object()?.iter().find_map(|(sig, s)| {
            s["error"].as_str().map(|e| {
                format!(
                    "{} ({}, {sig}): {e}",
                    p["role"].as_str().unwrap_or("?"),
                    p["host"].as_str().unwrap_or("?")
                )
            })
        })
    });
    let endpoints: std::collections::BTreeSet<String> = live
        .iter()
        .filter_map(|p| p["signals"].as_object())
        .flat_map(|s| s.values())
        .filter_map(|s| {
            Some(format!(
                "{} {}",
                s["protocol"].as_str()?,
                s["endpoint"].as_str()?
            ))
        })
        .collect();
    let mut v = json!({
        "configured": true,
        "ok": error.is_none(),
        "endpoints": endpoints,
        "processes": live,
    });
    if let Some(e) = error {
        v["error"] = json!(e);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let vars: BTreeMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| vars.get(k).cloned()
    }

    #[test]
    fn no_endpoint_no_export() {
        assert_eq!(target(&env(&[]), Signal::Logs), Ok(None));
    }

    #[test]
    fn a_signals_own_variables_come_before_the_general_ones() {
        let e = env(&[
            ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://collector:4318"),
            ("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT", "https://traces:4317"),
            ("OTEL_EXPORTER_OTLP_TRACES_PROTOCOL", "grpc"),
            ("OTEL_EXPORTER_OTLP_CERTIFICATE", "/ca.pem"),
        ]);
        let logs = target(&e, Signal::Logs).unwrap().unwrap();
        assert_eq!(logs.endpoint, "http://collector:4318");
        assert!(!logs.grpc, "http/protobuf is the default");
        let traces = target(&e, Signal::Traces).unwrap().unwrap();
        assert_eq!(traces.endpoint, "https://traces:4317");
        assert!(traces.grpc);
        assert_eq!(traces.ca_file.as_deref(), Some("/ca.pem"));
    }

    #[test]
    fn exporter_none_and_sdk_disabled_turn_export_off() {
        let base = [("OTEL_EXPORTER_OTLP_ENDPOINT", "http://c:4318")];
        let off = env(&[base[0], ("OTEL_METRICS_EXPORTER", "none")]);
        assert_eq!(target(&off, Signal::Metrics), Ok(None));
        assert!(target(&off, Signal::Logs).unwrap().is_some());
        let disabled = env(&[base[0], ("OTEL_SDK_DISABLED", "true")]);
        assert_eq!(target(&disabled, Signal::Logs), Ok(None));
    }

    #[test]
    fn an_unsupported_protocol_is_an_error() {
        let e = env(&[
            ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://c:4318"),
            ("OTEL_EXPORTER_OTLP_PROTOCOL", "http/json"),
        ]);
        assert!(target(&e, Signal::Logs).unwrap_err().contains("http/json"));
    }

    #[test]
    fn health_fails_on_an_error_and_recovers_on_success() {
        let h = Health::default();
        let t = Target {
            endpoint: "http://c:4318".into(),
            grpc: false,
            ca_file: None,
            cert_file: None,
            key_file: None,
        };
        h.add(Signal::Logs, &t);
        assert_eq!(h.status("all")["ok"], true);
        h.record(
            Signal::Logs,
            &Err(opentelemetry_sdk::error::OTelSdkError::InternalFailure(
                "refused".into(),
            )),
        );
        let s = h.status("all");
        assert_eq!(s["ok"], false);
        assert!(
            s["signals"]["logs"]["error"]
                .as_str()
                .unwrap()
                .contains("refused")
        );
        assert!(s["signals"]["logs"]["failing_since_ms"].is_i64());
        h.record(Signal::Logs, &Ok(()));
        let s = h.status("all");
        assert_eq!(s["ok"], true);
        assert!(s["signals"]["logs"]["failing_since_ms"].is_null());
        assert!(s["signals"]["logs"]["last_ok_ms"].is_i64());
    }

    #[test]
    fn endpoints_resolve_to_host_and_port() {
        assert_eq!(
            host_port("http://collector:4318").as_deref(),
            Some("collector:4318")
        );
        assert_eq!(
            host_port("https://otel.example.mil/v1/logs").as_deref(),
            Some("otel.example.mil:443")
        );
        assert_eq!(
            host_port("http://[::1]:4317").as_deref(),
            Some("[::1]:4317")
        );
        assert_eq!(host_port("not a url"), None);
    }

    #[test]
    fn the_exporters_own_transports_are_not_exported() {
        assert!(not_exported("h2::codec"));
        assert!(not_exported("opentelemetry_sdk"));
        assert!(not_exported("opentrack::telemetry"));
        assert!(!not_exported("audit"));
        assert!(!not_exported("opentrack::engine"));
        assert!(!not_exported("hyperion"));
    }
}
