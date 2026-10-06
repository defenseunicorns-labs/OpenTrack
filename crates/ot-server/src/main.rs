//! `opentrack`: one binary, one subcommand per server role.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use clap::{Args, Parser, Subcommand};

mod api;
mod audit_api;
mod auth;
mod basemap;
mod bridge;
mod cert_status;
mod config;
mod config_backup;
mod control;
mod correlate;
mod correlation_api;
mod cot;
mod decisions_api;
mod engine;
mod fips;
mod history_api;
mod https;
mod link;
mod manage_api;
mod marking;
mod metrics;
mod ocsp;
mod plugin_cli;
mod plugins;
mod plugins_api;
mod probe;
mod profiles;
mod registry_api;
mod registry_sheet;
mod settings_api;
mod sources;
mod sync_api;
mod synthetic;
mod telemetry;
mod user_cli;
mod writer;

use config::Common;

#[derive(Parser)]
#[command(
    name = "opentrack",
    version,
    about = "Source-agnostic track management server"
)]
struct Cli {
    #[command(flatten)]
    common: Common,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create or upgrade the SQLite database, then exit.
    Migrate,
    /// Run the control plane (REST API and UI).
    Serve(ServeArgs),
    /// Run the writer (Redis outbox to NATS).
    Writer(WriterArgs),
    /// Stream the published tracks to TAK as Cursor-on-Target (the outputs
    /// Settings → TAK output configures; idle until one is enabled).
    Cot(cot::CotArgs),
    /// Run every enabled source (transports and pipelines).
    Sources,
    /// Run the engine (source tracks to system tracks, lifecycle).
    Engine(EngineArgs),
    /// Exchange tracks and decisions with other OpenTrack nodes over the
    /// node's NATS (idle until sync is enabled in Settings).
    Link(link::LinkArgs),
    /// Carry sync messages between nodes' NATS servers (server sites
    /// without a networking package, and tests: it can drop, delay,
    /// duplicate, cap and partition).
    Bridge(bridge::BridgeArgs),
    /// Run every role in one process: control plane, sources, engine,
    /// writer, TAK output, link.
    All {
        #[command(flatten)]
        serve: ServeArgs,
        #[command(flatten)]
        writer: WriterArgs,
        #[command(flatten)]
        cot: cot::CotArgs,
        #[command(flatten)]
        engine: EngineArgs,
        #[command(flatten)]
        link: link::LinkArgs,
    },
    /// Push synthetic tracks through the pipeline.
    Synthetic(synthetic::SyntheticArgs),
    /// Run a recorded scenario through the pipelines and the engine, as fast
    /// as they go, and write what the engine made of it (see
    /// scripts/benchmark/). Publishes nothing.
    Bench(BenchArgs),
    /// Check, add and list plugins.
    #[command(subcommand)]
    Plugin(plugin_cli::PluginCommand),
    /// Manage accounts (for a node without the UI).
    #[command(subcommand)]
    User(user_cli::UserCommand),
    /// Export the whole configuration, or rebuild an empty node from one.
    #[command(subcommand)]
    Config(config_backup::ConfigCommand),
    /// Exit 0 when this node's control plane answers (the image's
    /// HEALTHCHECK): `/healthz` over plain HTTP, a TCP connection under TLS.
    Health {
        #[arg(long, env = "OT_BIND", default_value = "0.0.0.0:8090")]
        bind: SocketAddr,
        // A String, not a PathBuf: docker compose passes an unset variable
        // as empty, which a PathBuf refuses.
        #[arg(long, env = "OT_TLS_CERT")]
        tls_cert: Option<String>,
    },
    /// Retire a system track: record the decision, close its graph links and
    /// publish its delete.
    Retire {
        /// UID or `tms-<UID>` document id.
        uid: String,
        /// Reason recorded in the decision log.
        #[arg(long, default_value = "retired from the command line")]
        reason: String,
    },
}

#[derive(Debug, Clone, Args)]
struct BenchArgs {
    /// Scenario directory (with scenario.json).
    scenario: PathBuf,
    /// Where the results go.
    #[arg(long)]
    out: PathBuf,
    /// Correlation settings (JSON) to use instead of the scenario's.
    #[arg(long)]
    correlation: Option<PathBuf>,
    /// Plugins to load for the run (a .wasm file or an external plugin's
    /// address), for sources and settings that name them. Repeatable.
    #[arg(long = "plugin")]
    plugins: Vec<String>,
}

#[derive(Debug, Clone, Args)]
struct ServeArgs {
    /// Address the control plane listens on.
    #[arg(long, env = "OT_BIND", default_value = "0.0.0.0:8090")]
    bind: SocketAddr,
    /// Built UI to serve at `/` (skipped if it has no index.html).
    #[arg(long, env = "OT_UI_DIR", default_value = "ui/dist")]
    ui_dir: PathBuf,
    /// Sign-in: `on`, or `off` (every caller is an admin; development only).
    #[arg(long, env = "OT_AUTH", default_value = "on", value_parser = ["on", "off"])]
    auth: String,
    /// Key session tokens are signed with (at least 32 characters). Unset:
    /// one is made on first start and kept beside the database.
    #[arg(long, env = "OT_SESSION_SECRET", hide_env_values = true)]
    session_secret: Option<String>,
    /// The first admin account, made when there are no accounts. Unset: an
    /// admin@opentrack.local account with its password in a file beside
    /// the database.
    #[arg(long, env = "OT_ADMIN_EMAIL")]
    admin_email: Option<String>,
    #[arg(long, env = "OT_ADMIN_PASSWORD", hide_env_values = true)]
    admin_password: Option<String>,
    /// Where browsers reach this server (`https://host:8090`); SAML sign-on
    /// needs it.
    #[arg(long, env = "OT_PUBLIC_URL")]
    public_url: Option<String>,
    /// Serve over TLS with this PEM certificate (chain) and `--tls-key`.
    #[arg(long, env = "OT_TLS_CERT", requires = "tls_key")]
    tls_cert: Option<String>,
    #[arg(long, env = "OT_TLS_KEY", requires = "tls_cert")]
    tls_key: Option<String>,
    /// With TLS: accept client certificates this CA (PEM) signed, as the
    /// accounts Settings → Security maps them to.
    #[arg(long, env = "OT_TLS_CLIENT_CA", requires = "tls_cert")]
    tls_client_ca: Option<String>,
    /// With a client CA: certificate revocation lists (PEM or DER files, or
    /// directories of them; comma separated). A revoked client certificate,
    /// one no list covers, or one whose list has expired is refused. The
    /// lists are reloaded when they change.
    #[arg(
        long,
        env = "OT_TLS_CLIENT_CRL",
        requires = "tls_client_ca",
        value_delimiter = ','
    )]
    tls_client_crl: Vec<String>,
    /// With a client CA: the OCSP responder to ask about client
    /// certificates instead of the one each names (its authority
    /// information access URL). Without an answer the CRLs decide; without
    /// either the certificate is refused.
    #[arg(long, env = "OT_TLS_CLIENT_OCSP_URL", requires = "tls_client_ca")]
    tls_client_ocsp_url: Option<String>,
    /// Browsers reach this server over TLS that a proxy in front of it ends
    /// (`1`): session cookies are `Secure` and HSTS is sent, as when this
    /// server serves TLS itself.
    #[arg(long, env = "OT_PUBLIC_TLS", value_parser = clap::builder::BoolishValueParser::new(), default_value_t = false)]
    public_tls: bool,
    /// Also listen here, and serve the admin routes (accounts, settings,
    /// sources and every other admin change or read) only here: the main
    /// listener answers them 404. For a management network or loopback.
    #[arg(long, env = "OT_ADMIN_BIND")]
    admin_bind: Option<String>,
    /// The admin listener's own TLS certificate and key (else the main
    /// listener's).
    #[arg(long, env = "OT_ADMIN_TLS_CERT")]
    admin_tls_cert: Option<String>,
    #[arg(long, env = "OT_ADMIN_TLS_KEY")]
    admin_tls_key: Option<String>,
    /// The admin listener's own client CA (else the main listener's). The
    /// client CRLs are the same for both.
    #[arg(long, env = "OT_ADMIN_TLS_CLIENT_CA")]
    admin_tls_client_ca: Option<String>,
}

/// The admin listener's address and TLS, when `OT_ADMIN_BIND` is set:
/// checked, with the main listener's certificate, key and client CA for
/// what it does not set. Call with empty variables already made unset.
fn admin_listener(
    args: &ServeArgs,
    crls: &[String],
) -> anyhow::Result<Option<(SocketAddr, Option<ot_source::tls::ServerTls>)>> {
    let Some(bind) = &args.admin_bind else {
        anyhow::ensure!(
            args.admin_tls_cert.is_none()
                && args.admin_tls_key.is_none()
                && args.admin_tls_client_ca.is_none(),
            "OT_ADMIN_TLS_CERT, OT_ADMIN_TLS_KEY and OT_ADMIN_TLS_CLIENT_CA need OT_ADMIN_BIND"
        );
        return Ok(None);
    };
    let bind: SocketAddr = bind
        .trim()
        .parse()
        .with_context(|| format!("OT_ADMIN_BIND {bind:?} is not an address and port"))?;
    anyhow::ensure!(
        bind != args.bind,
        "OT_ADMIN_BIND is the same address as OT_BIND"
    );
    anyhow::ensure!(
        args.admin_tls_cert.is_some() == args.admin_tls_key.is_some(),
        "OT_ADMIN_TLS_CERT and OT_ADMIN_TLS_KEY go together"
    );
    let (cert, key) = match (&args.admin_tls_cert, &args.admin_tls_key) {
        (Some(c), Some(k)) => (Some(c), Some(k)),
        _ => (args.tls_cert.as_ref(), args.tls_key.as_ref()),
    };
    let client_ca = args
        .admin_tls_client_ca
        .clone()
        .or_else(|| args.tls_client_ca.clone());
    let tls = match (cert, key) {
        (Some(cert), Some(key)) => Some(ot_source::tls::ServerTls {
            cert_file: cert.clone(),
            key_file: key.clone(),
            client_ca_file: client_ca,
            client_cert_optional: true,
            client_crl_files: if args.tls_client_ca.is_some() || args.admin_tls_client_ca.is_some()
            {
                crls.to_vec()
            } else {
                Vec::new()
            },
        }),
        _ => {
            anyhow::ensure!(
                args.admin_tls_client_ca.is_none(),
                "OT_ADMIN_TLS_CLIENT_CA needs TLS (OT_ADMIN_TLS_CERT or OT_TLS_CERT)"
            );
            None
        }
    };
    Ok(Some((bind, tls)))
}

#[derive(Debug, Clone, Args)]
struct WriterArgs {
    /// Consumer name within the writer group; unique per writer instance.
    #[arg(long, env = "OT_WRITER_CONSUMER", default_value = "writer-1")]
    consumer: String,
    /// Minimum seconds between writes of the same system track.
    #[arg(long, env = "OT_WRITE_MIN_INTERVAL_SECS", default_value_t = 5.0)]
    min_interval_secs: f64,
}

#[derive(Debug, Clone, Args)]
struct EngineArgs {
    /// Consumer name within the engine group.
    #[arg(long, env = "OT_ENGINE_CONSUMER", default_value = "engine-1")]
    engine_consumer: String,
    /// Observations before a tentative system track is confirmed.
    #[arg(long, env = "OT_CONFIRM_AFTER", default_value_t = 3)]
    confirm_after: u64,
    /// Hours without a report before a system track is dropped.
    #[arg(long, env = "OT_DROP_AFTER_HOURS", default_value_t = 6.0)]
    drop_after_hours: f64,
    /// How tracks with no shared identifier pair until an operator saves
    /// correlation settings: identifiers (never), kinematics, or
    /// kinematics-metadata (kinematics, vetoed by conflicting identifiers or
    /// domains; the default). Saved settings win; a start with a different
    /// one saved logs a warning.
    #[arg(long, env = "OT_CORRELATION", value_enum)]
    correlation: Option<correlate::Approach>,
}

impl EngineArgs {
    fn settings(&self) -> engine::EngineSettings {
        engine::EngineSettings {
            consumer: self.engine_consumer.clone(),
            confirm_after: self.confirm_after.max(1),
            drop_after: Duration::from_secs_f64(self.drop_after_hours.max(0.01) * 3600.0),
            correlation: correlate::CorrelationSettings {
                approach: self.correlation.unwrap_or_default(),
                ..Default::default()
            },
            correlation_asked: self.correlation,
            ..Default::default()
        }
    }
}

impl Command {
    /// The role, as telemetry names it.
    fn role(&self) -> &'static str {
        match self {
            Command::Migrate => "migrate",
            Command::Serve(_) => "serve",
            Command::Writer(_) => "writer",
            Command::Cot(_) => "cot",
            Command::Sources => "sources",
            Command::Engine(_) => "engine",
            Command::Link(_) => "link",
            Command::Bridge(_) => "bridge",
            Command::All { .. } => "all",
            Command::Synthetic(_) => "synthetic",
            Command::Bench(_) => "bench",
            Command::Plugin(_) => "plugin",
            Command::User(_) => "user",
            Command::Config(_) => "config",
            Command::Health { .. } => "health",
            Command::Retire { .. } => "retire",
        }
    }

    /// Whether it is a long-running server role (the rest are one-off
    /// commands, which log to standard error: their output is on
    /// standard output).
    fn is_server(&self) -> bool {
        matches!(
            self,
            Command::Serve(_)
                | Command::Writer(_)
                | Command::Cot(_)
                | Command::Sources
                | Command::Engine(_)
                | Command::Link(_)
                | Command::Bridge(_)
                | Command::All { .. }
        )
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    metrics::mark_start();
    // FIPS 140-3: the validated module, or nothing runs. First, so every TLS
    // connection (OTLP export included) uses it.
    fips::init()?;
    let cli = Cli::parse();
    let role = cli.command.role();
    let telemetry = telemetry::init(role, !cli.command.is_server(), &cli.common);
    // A site's own list of common passwords sits beside the database.
    if let Some(dir) = cli.common.sqlite.parent() {
        auth::password::set_site_dir(if dir.as_os_str().is_empty() {
            std::path::Path::new(".")
        } else {
            dir
        });
    }
    tracing::debug!("cryptography: AWS-LC FIPS module in FIPS mode");
    #[cfg(feature = "saml")]
    auth::openssl_fips();
    if cli.command.is_server() {
        telemetry.report(cli.common.clone());
    }
    let result = run(cli).await;
    if let Err(e) = &result {
        tracing::error!(role, error = format!("{e:#}"), "stopped");
    }
    telemetry.shutdown().await;
    result
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    let common = cli.common;
    match cli.command {
        Command::Migrate => {
            let db = common.open_db()?;
            println!(
                "{}: schema version {}",
                common.sqlite.display(),
                db.schema_version()?
            );
            Ok(())
        }
        Command::Serve(args) => serve(common, args).await,
        Command::Writer(args) => run_writer(common, args).await,
        Command::Cot(args) => {
            common.open_db()?;
            cot::run(common, args, shutdown_signal()).await
        }
        Command::Sources => {
            common.open_db()?;
            plugins::start(&common).await;
            sources::run(common, shutdown_signal()).await
        }
        Command::Engine(args) => run_engine(common, args).await,
        Command::Link(args) => link::run(common, args, shutdown_signal()).await,
        Command::Bridge(args) => bridge::run(args, shutdown_signal()).await,
        Command::All {
            serve: s,
            writer: w,
            cot: c,
            engine: e,
            link: l,
        } => {
            // Migrate once before the roles open the database concurrently.
            common.open_db()?;
            plugins::start(&common).await;
            tokio::try_join!(
                serve(common.clone(), s),
                run_writer(common.clone(), w),
                cot::run(common.clone(), c, shutdown_signal()),
                sources::run(common.clone(), shutdown_signal()),
                link::run(common.clone(), l, shutdown_signal()),
                run_engine(common, e),
            )?;
            Ok(())
        }
        Command::Synthetic(args) => synthetic::run(&common, args).await,
        Command::Plugin(cmd) => plugin_cli::run(&common, cmd),
        Command::User(cmd) => user_cli::run(&common, cmd),
        Command::Config(cmd) => config_backup::run(&common, cmd),
        Command::Bench(args) => {
            engine::bench::run(
                &common,
                engine::bench::BenchOptions {
                    scenario: args.scenario,
                    out: args.out,
                    correlation: args.correlation,
                    plugins: args.plugins,
                },
            )
            .await
        }
        Command::Health { bind, tls_cert } => {
            health(bind, tls_cert.is_some_and(|s| !s.trim().is_empty()))
        }
        Command::Retire { uid, reason } => {
            let uid = ot_core::Uid::from_doc_id(&uid).or_else(|_| uid.parse())?;
            let decision = common.open_db()?.retire_system_track(
                uid,
                ot_store::Decision::new("cli", "retire_system_track").reason(reason.clone()),
            )?;
            common
                .open_redis()
                .await?
                .retire_system_track(uid, &reason)
                .await?;
            println!(
                "retired {} (decision {decision}); delete queued for the writer",
                uid.doc_id()
            );
            Ok(())
        }
    }
}

async fn serve(common: Common, mut args: ServeArgs) -> anyhow::Result<()> {
    // An empty variable (as docker compose passes an unset one) is unset.
    for v in [
        &mut args.tls_cert,
        &mut args.tls_key,
        &mut args.tls_client_ca,
        &mut args.tls_client_ocsp_url,
        &mut args.public_url,
        &mut args.admin_email,
        &mut args.admin_password,
        &mut args.session_secret,
        &mut args.admin_bind,
        &mut args.admin_tls_cert,
        &mut args.admin_tls_key,
        &mut args.admin_tls_client_ca,
    ] {
        if v.as_deref().is_some_and(|s| s.trim().is_empty()) {
            *v = None;
        }
    }
    let crls: Vec<String> = std::mem::take(&mut args.tls_client_crl)
        .into_iter()
        .filter(|p| !p.trim().is_empty())
        .collect();
    let admin = admin_listener(&args, &crls)?;
    let mut db = common.open_new_db()?;
    plugins::start(&common).await;
    let disabled = args.auth == "off";
    if disabled {
        tracing::warn!(
            "AUTHENTICATION IS DISABLED (OT_AUTH=off): every caller is an admin (repeated every minute)"
        );
    } else {
        auth::bootstrap_admin(
            &common,
            &mut db,
            args.admin_email.as_deref(),
            args.admin_password.as_deref(),
        )?;
    }
    let auth_settings: auth::AuthSettings =
        serde_json::from_value(db.auth_settings()?).context("the saved sign-in settings")?;
    let secret = auth::load_secret(&common, args.session_secret.as_deref())?;
    let state = control::AppState {
        db: Arc::new(Mutex::new(db)),
        redis: common.open_redis().await?,
        nats: common.connect_nats().await?,
        auth: Arc::new(auth::Auth::new(
            secret,
            disabled,
            args.tls_cert.is_some()
                || args.public_tls
                || args
                    .public_url
                    .as_deref()
                    .is_some_and(|u| u.starts_with("https://")),
            args.public_url.clone(),
            auth_settings,
        )),
        common,
    };
    tokio::spawn(metrics::run_sampler(state.clone()));
    tokio::spawn(auth::maintenance::run(state.clone()));
    let audit_state = state.clone();
    let app = control::router(state, Some(args.ui_dir));
    let tls = match (args.tls_cert, args.tls_key) {
        (Some(cert), Some(key)) => Some(ot_source::tls::ServerTls {
            cert_file: cert,
            key_file: key,
            client_ca_file: args.tls_client_ca,
            client_cert_optional: true,
            client_crl_files: crls,
        }),
        _ => None,
    };
    // Client certificates' status (OCSP, else the CRLs) is checked after
    // the handshake, and audited (cert_status).
    let sink: cert_status::AuditSink = Arc::new(move |e| {
        let s = audit_state.clone();
        tokio::spawn(async move { auth::access::audit(&s, e).await });
    });
    let ocsp = args.tls_client_ocsp_url;
    let Some((admin_bind, admin_tls)) = admin else {
        return listen(args.bind, app, tls, (ocsp, sink), "control plane").await;
    };
    // The main listener answers the admin routes 404 (SC-7, SC-2); the
    // admin listener serves everything.
    let main = app.clone().layer(axum::Extension(auth::AdminElsewhere));
    tokio::try_join!(
        listen(
            args.bind,
            main,
            tls,
            (ocsp.clone(), sink.clone()),
            "control plane"
        ),
        listen(admin_bind, app, admin_tls, (ocsp, sink), "admin listener"),
    )?;
    Ok(())
}

/// Serve `app` on `bind`, over TLS when `tls` is given (with a client CA,
/// each client certificate's status checked after the handshake, the
/// CRLs reloaded when they change), until shutdown.
async fn listen(
    bind: SocketAddr,
    app: axum::Router,
    tls: Option<ot_source::tls::ServerTls>,
    (ocsp_url, sink): (Option<String>, cert_status::AuditSink),
    what: &'static str,
) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("binding {bind}"))?;
    if let Some(tls) = tls {
        // The handshake checks a client certificate's path only; its
        // status (OCSP, else the CRLs) is checked after (cert_status).
        let acceptor = https::Acceptor::new(cert_status::handshake_tls(&tls).acceptor()?);
        let status = match tls.client_ca_file {
            Some(_) => Some(Arc::new(cert_status::CertStatus::new(
                &tls,
                ocsp_url,
                Some(sink),
            )?)),
            None => None,
        };
        if let Some(status) = &status
            && !tls.client_crl_files.is_empty()
        {
            tokio::spawn(https::reload_on_crl_change(tls, status.clone()));
        }
        tracing::info!(addr = %bind, "{what} listening (TLS)");
        return https::serve(listener, app, acceptor, status, shutdown_signal()).await;
    }
    tracing::info!(addr = %bind, "{what} listening");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;
    Ok(())
}

async fn run_writer(common: Common, args: WriterArgs) -> anyhow::Result<()> {
    let settings = writer::WriterSettings {
        consumer: args.consumer,
        tracks_subject: common.nats.tracks_subject.clone(),
        min_interval: Duration::from_secs_f64(args.min_interval_secs.max(0.0)),
        retry_after: Duration::from_secs(5),
        batch: 500,
    };
    let nats = common.connect_nats().await?;
    tracing::info!(url = %common.nats.url, stream = %common.nats.stream, "publishing to NATS");
    let w = writer::Writer::new(
        common.open_redis().await?,
        nats,
        common.publish_context(),
        settings,
    );
    w.run(shutdown_signal()).await?;
    Ok(())
}

async fn run_engine(common: Common, args: EngineArgs) -> anyhow::Result<()> {
    common.open_db()?;
    plugins::start(&common).await;
    let settings = engine::EngineSettings {
        uid_check_nats: Some(Duration::from_secs(10)),
        ..args.settings()
    };
    engine::Engine::new(common, settings)
        .await?
        .run(shutdown_signal())
        .await
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = term => {} }
    tracing::info!("shutting down");
}

fn health(bind: SocketAddr, tls: bool) -> anyhow::Result<()> {
    use std::io::{Read, Write};
    let mut addr = bind;
    if addr.ip().is_unspecified() {
        addr.set_ip(if addr.is_ipv4() {
            std::net::Ipv4Addr::LOCALHOST.into()
        } else {
            std::net::Ipv6Addr::LOCALHOST.into()
        });
    }
    let t = std::time::Duration::from_secs(3);
    let mut c = std::net::TcpStream::connect_timeout(&addr, t)?;
    if tls {
        return Ok(());
    }
    c.set_read_timeout(Some(t))?;
    c.write_all(b"GET /healthz HTTP/1.0\r\nHost: localhost\r\n\r\n")?;
    let mut head = [0u8; 12];
    c.read_exact(&mut head)?;
    anyhow::ensure!(head.ends_with(b" 200"), "/healthz did not answer 200");
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn tls_clients_can_be_built_after_install() {
        crate::fips::init().unwrap();
        // What wss://, https:// and mqtts:// connections do first.
        let _ = rustls::ClientConfig::builder()
            .with_root_certificates(rustls::RootCertStore::empty())
            .with_no_client_auth();
    }

    #[test]
    fn health_takes_an_empty_tls_cert_as_unset() {
        // As docker compose passes OT_TLS_CERT when it isn't set.
        use clap::Parser;
        let cli = crate::Cli::try_parse_from(["opentrack", "health", "--tls-cert="]).unwrap();
        let crate::Command::Health { tls_cert, .. } = cli.command else {
            panic!("not the health command");
        };
        assert_eq!(tls_cert.as_deref(), Some(""));
    }

    fn serve_args(args: &[&str]) -> crate::ServeArgs {
        use clap::Parser;
        let cli =
            crate::Cli::try_parse_from(["opentrack", "serve"].iter().chain(args).copied()).unwrap();
        let crate::Command::Serve(mut a) = cli.command else {
            panic!("not the serve command");
        };
        // As `serve` does: an empty variable is unset.
        for v in [
            &mut a.admin_bind,
            &mut a.admin_tls_cert,
            &mut a.admin_tls_key,
            &mut a.admin_tls_client_ca,
        ] {
            if v.as_deref().is_some_and(|s| s.trim().is_empty()) {
                *v = None;
            }
        }
        a
    }

    #[test]
    fn the_admin_listener_is_optional_and_checked() {
        let crls = ["crl.pem".to_owned()];
        // Empty, as docker compose passes unset variables: no admin listener.
        let a = serve_args(&["--admin-bind=", "--admin-tls-cert=", "--admin-tls-key="]);
        assert!(crate::admin_listener(&a, &crls).unwrap().is_none());
        // Plain, like the main listener.
        let a = serve_args(&["--admin-bind=127.0.0.1:8091"]);
        let (bind, tls) = crate::admin_listener(&a, &crls).unwrap().unwrap();
        assert_eq!(bind.to_string(), "127.0.0.1:8091");
        assert!(tls.is_none());
        // The main listener's TLS, CA and CRLs, unless it has its own.
        let main_tls = [
            "--tls-cert=m.crt",
            "--tls-key=m.key",
            "--tls-client-ca=m-ca.pem",
            "--admin-bind=127.0.0.1:8091",
        ];
        let tls = crate::admin_listener(&serve_args(&main_tls), &crls)
            .unwrap()
            .unwrap()
            .1
            .unwrap();
        assert_eq!(tls.cert_file, "m.crt");
        assert_eq!(tls.client_ca_file.as_deref(), Some("m-ca.pem"));
        assert_eq!(tls.client_crl_files, crls);
        let own: Vec<&str> = main_tls
            .iter()
            .copied()
            .chain(["--admin-tls-cert=a.crt", "--admin-tls-key=a.key"])
            .collect();
        let tls = crate::admin_listener(&serve_args(&own), &crls)
            .unwrap()
            .unwrap()
            .1
            .unwrap();
        assert_eq!(
            (tls.cert_file.as_str(), tls.key_file.as_str()),
            ("a.crt", "a.key")
        );
        assert_eq!(tls.client_ca_file.as_deref(), Some("m-ca.pem"));
        // Refused: the main listener's address, a lone certificate, TLS
        // settings without the listener, a client CA without TLS.
        for bad in [
            &["--admin-bind=0.0.0.0:8090"][..],
            &["--admin-bind=not an address"],
            &["--admin-bind=127.0.0.1:8091", "--admin-tls-cert=a.crt"],
            &["--admin-tls-cert=a.crt", "--admin-tls-key=a.key"],
            &[
                "--admin-bind=127.0.0.1:8091",
                "--admin-tls-client-ca=ca.pem",
            ],
        ] {
            assert!(
                crate::admin_listener(&serve_args(bad), &crls).is_err(),
                "{bad:?}"
            );
        }
    }
}
