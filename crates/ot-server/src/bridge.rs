//! `opentrack bridge`: carry sync messages between nodes' NATS boundaries
//! (see `docs/sync-icd.md`), for server sites that reach each other over IP
//! and have no networking package of their own, and for tests.
//!
//! Every message a node puts on `<prefix>.out.<kind>` goes to every other
//! node's `<prefix>.in.<kind>` (only to the one named in `OT-To`, when it
//! is addressed and the bridge knows that node's site code). It can also
//! stand in for a poor link: drop, delay, duplicate, cap each node's
//! sending rate, and cut groups of nodes off from each other for a time.
//!
//! Each node's NATS gets the credentials and TLS the main connection has
//! (`--nats-*`, `OT_BRIDGE_NATS_*`, the same for every node), and a node
//! can set its own after its URL: `--node 'tls://b:4222;ca=/b/ca.pem'`.
//! Secrets are only read from files or the environment, never taken from
//! the command line of a node, and never logged.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Context;
use futures_util::StreamExt;
use ot_nats::{NatsAuth, async_nats};
use serde::Serialize;

use crate::link::HEADER_TO;

#[derive(Debug, Clone, clap::Args)]
pub struct BridgeArgs {
    /// A node's side of the link: `<nats url>`, or `<nats url>#<prefix>`
    /// when its prefix is not `ot.sync`, then this node's own credentials
    /// and TLS if they differ from `--nats-*`: `;ca=`, `;cert=`, `;key=`,
    /// `;creds=` (files), `;user=`, `;password-file=`, `;token-file=`
    /// (an empty value drops the `--nats-*` one). Give one per node.
    #[arg(long = "node", required = true)]
    pub nodes: Vec<String>,
    #[command(flatten)]
    pub auth: BridgeAuthArgs,
    /// Probability each message is lost on its way to each node.
    #[arg(long, default_value_t = 0.0)]
    pub loss: f64,
    /// Delay added to every message, ms, plus up to `--jitter-ms` more
    /// (which reorders messages).
    #[arg(long, default_value_t = 0)]
    pub delay_ms: u64,
    #[arg(long, default_value_t = 0)]
    pub jitter_ms: u64,
    /// Probability a message arrives twice.
    #[arg(long, default_value_t = 0.0)]
    pub duplicate: f64,
    /// What each node may send, kbit/s (0: no cap). Past it, messages are
    /// dropped, as on a saturated radio.
    #[arg(long, default_value_t = 0.0)]
    pub rate_kbps: f64,
    /// Cut groups of nodes off from each other for a time, seconds after
    /// the bridge starts: `<from>-<to>:<sites>|<sites>`, e.g.
    /// `60-180:AAA,BBB|CCC`. Repeatable.
    #[arg(long)]
    pub partition: Vec<String>,
    /// Write the counters here (JSON) every 5 s.
    #[arg(long)]
    pub stats: Option<PathBuf>,
    /// Seed for the losses, delays and duplicates.
    #[arg(long, default_value_t = 1)]
    pub seed: u64,
}

/// Credentials and TLS for every node's NATS, as `opentrack`'s own NATS
/// connection takes them (`--nats-*`), under `OT_BRIDGE_NATS_*`.
#[derive(Clone, Default, clap::Args)]
pub struct BridgeAuthArgs {
    /// NATS credentials file (JWT + NKey).
    #[arg(long = "nats-creds", env = "OT_BRIDGE_NATS_CREDS")]
    pub creds: Option<PathBuf>,

    /// NATS auth token (better: `--nats-token-file`).
    #[arg(
        long = "nats-token",
        env = "OT_BRIDGE_NATS_TOKEN",
        hide_env_values = true,
        conflicts_with = "token_file"
    )]
    pub token: Option<String>,

    /// A file holding the NATS auth token.
    #[arg(long = "nats-token-file", env = "OT_BRIDGE_NATS_TOKEN_FILE")]
    pub token_file: Option<PathBuf>,

    /// NATS user (with a password).
    #[arg(long = "nats-user", env = "OT_BRIDGE_NATS_USER")]
    pub user: Option<String>,

    /// NATS password (better: `--nats-password-file`).
    #[arg(
        long = "nats-password",
        env = "OT_BRIDGE_NATS_PASSWORD",
        hide_env_values = true,
        conflicts_with = "password_file"
    )]
    pub password: Option<String>,

    /// A file holding the NATS password.
    #[arg(long = "nats-password-file", env = "OT_BRIDGE_NATS_PASSWORD_FILE")]
    pub password_file: Option<PathBuf>,

    /// TLS to NATS: the CA (PEM) that signed the servers' certificates.
    /// Given, the connections must be TLS.
    #[arg(long = "nats-ca", env = "OT_BRIDGE_NATS_CA")]
    pub ca: Option<PathBuf>,

    /// Mutual TLS: this client certificate (PEM) and `--nats-key`.
    #[arg(long = "nats-cert", env = "OT_BRIDGE_NATS_CERT")]
    pub cert: Option<PathBuf>,

    #[arg(long = "nats-key", env = "OT_BRIDGE_NATS_KEY")]
    pub key: Option<PathBuf>,
}

/// Secrets never reach a log line: only whether each is set.
impl std::fmt::Debug for BridgeAuthArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BridgeAuthArgs")
            .field("creds", &self.creds)
            .field("token", &self.token.as_ref().map(|_| "<set>"))
            .field("token_file", &self.token_file)
            .field("user", &self.user)
            .field("password", &self.password.as_ref().map(|_| "<set>"))
            .field("password_file", &self.password_file)
            .field("ca", &self.ca)
            .field("cert", &self.cert)
            .field("key", &self.key)
            .finish()
    }
}

impl BridgeAuthArgs {
    /// What every node starts from, secrets read from their files.
    pub fn defaults(&self) -> anyhow::Result<NatsAuth> {
        Ok(NatsAuth {
            creds_file: nonempty(self.creds.clone()),
            token: match &self.token_file {
                Some(p) => Some(read_secret(p)?),
                None => self.token.clone().filter(|t| !t.is_empty()),
            },
            user: self.user.clone().filter(|u| !u.is_empty()),
            password: match &self.password_file {
                Some(p) => Some(read_secret(p)?),
                None => self.password.clone().filter(|p| !p.is_empty()),
            },
            tls_ca: nonempty(self.ca.clone()),
            tls_cert: nonempty(self.cert.clone()),
            tls_key: nonempty(self.key.clone()),
        })
    }
}

fn nonempty(p: Option<PathBuf>) -> Option<PathBuf> {
    p.filter(|p| !p.as_os_str().is_empty())
}

/// A token or password from its file, without the trailing newline.
fn read_secret(path: &std::path::Path) -> anyhow::Result<String> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let secret = text.trim_end_matches(['\r', '\n']).to_owned();
    anyhow::ensure!(!secret.is_empty(), "{} is empty", path.display());
    Ok(secret)
}

/// One node's side of the bridge, from its `--node`.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeSpec {
    pub url: String,
    pub prefix: String,
    pub auth: NatsAuth,
}

impl NodeSpec {
    /// `<url>[#<prefix>][;<option>=<value>...]`, over `defaults`.
    pub fn parse(s: &str, defaults: &NatsAuth) -> anyhow::Result<Self> {
        let mut parts = s.split(';');
        let head = parts.next().unwrap_or_default().trim();
        let (full_url, prefix) = head.split_once('#').unwrap_or((head, "ot.sync"));
        anyhow::ensure!(!full_url.is_empty(), "a --node has no NATS URL");
        // Credentials in the URL itself stay out of the messages.
        let url_shown = shown(full_url);
        let url = url_shown.as_str();
        anyhow::ensure!(!prefix.is_empty(), "node {url} has an empty prefix");
        let mut auth = defaults.clone();
        for opt in parts.map(str::trim).filter(|o| !o.is_empty()) {
            let (k, v) = opt.split_once('=').ok_or_else(|| {
                anyhow::anyhow!("node {url}: option {k:?} is not <name>=<value>", k = opt)
            })?;
            let (k, v) = (k.trim(), v.trim());
            let path = || (!v.is_empty()).then(|| PathBuf::from(v));
            match k {
                "ca" => auth.tls_ca = path(),
                "cert" => auth.tls_cert = path(),
                "key" => auth.tls_key = path(),
                "creds" => auth.creds_file = path(),
                "user" => auth.user = (!v.is_empty()).then(|| v.to_owned()),
                "password-file" => auth.password = path().map(|p| read_secret(&p)).transpose()?,
                "token-file" => auth.token = path().map(|p| read_secret(&p)).transpose()?,
                // A secret on the command line shows in the process list.
                "password" | "token" => anyhow::bail!(
                    "node {url}: give the {k} in a file ({k}-file=), not on the command line"
                ),
                _ => anyhow::bail!(
                    "node {url}: unknown option {k:?} (ca, cert, key, creds, user, password-file, token-file)"
                ),
            }
        }
        check_auth(url, &auth)?;
        Ok(Self {
            url: full_url.to_owned(),
            prefix: prefix.to_owned(),
            auth,
        })
    }

    /// How it connects, for the log (no secrets, no URL user info).
    fn describe(&self) -> (bool, &'static str) {
        let a = &self.auth;
        let tls = a.tls_ca.is_some() || a.tls_cert.is_some() || self.url.contains("tls://");
        let how = if a.creds_file.is_some() {
            "creds"
        } else if a.token.is_some() {
            "token"
        } else if a.user.is_some() {
            "user"
        } else {
            "none"
        };
        (tls, how)
    }
}

/// A NATS URL (or several, comma separated) without any `user:password@`.
fn shown(url: &str) -> String {
    url.split(',')
        .map(|u| match (u.find("://"), u.rfind('@')) {
            (Some(s), Some(at)) if at > s => format!("{}://***@{}", &u[..s], &u[at + 1..]),
            (None, Some(at)) => format!("***@{}", &u[at + 1..]),
            _ => u.to_owned(),
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Refuse what could only fail later, or silently connect without the
/// credentials meant: half a key pair, a user without a password (or the
/// other way round), two ways of signing in, and files that cannot be read.
fn check_auth(url: &str, a: &NatsAuth) -> anyhow::Result<()> {
    anyhow::ensure!(
        a.tls_cert.is_some() == a.tls_key.is_some(),
        "node {url}: a client certificate needs its key, and the other way round"
    );
    anyhow::ensure!(
        a.user.is_some() == a.password.is_some(),
        "node {url}: a NATS user needs a password, and the other way round"
    );
    let ways = [a.creds_file.is_some(), a.token.is_some(), a.user.is_some()];
    anyhow::ensure!(
        ways.iter().filter(|w| **w).count() <= 1,
        "node {url}: give one of a credentials file, a token or a user and password"
    );
    for p in [&a.tls_ca, &a.tls_cert, &a.tls_key, &a.creds_file]
        .into_iter()
        .flatten()
    {
        std::fs::File::open(p).with_context(|| format!("node {url}: reading {}", p.display()))?;
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
pub struct Partition {
    pub from_s: f64,
    pub to_s: f64,
    pub sides: Vec<Vec<String>>,
}

impl Partition {
    pub fn parse(s: &str) -> anyhow::Result<Self> {
        let bad = || anyhow::anyhow!("partition {s:?} is not <from>-<to>:<sites>|<sites>");
        let (span, groups) = s.split_once(':').ok_or_else(bad)?;
        let (a, b) = span.split_once('-').ok_or_else(bad)?;
        let sides: Vec<Vec<String>> = groups
            .split('|')
            .map(|g| g.split(',').map(|x| x.trim().to_owned()).collect())
            .collect();
        if sides.len() < 2 {
            return Err(bad());
        }
        Ok(Self {
            from_s: a.trim().parse().map_err(|_| bad())?,
            to_s: b.trim().parse().map_err(|_| bad())?,
            sides,
        })
    }

    /// Whether it separates `a` from `b` at `t` seconds.
    pub fn cuts(&self, t: f64, a: &str, b: &str) -> bool {
        if t < self.from_s || t >= self.to_s {
            return false;
        }
        let side = |x: &str| self.sides.iter().position(|g| g.iter().any(|s| s == x));
        matches!((side(a), side(b)), (Some(i), Some(j)) if i != j)
    }
}

/// Counters per node (by site code once known, else by its position).
#[derive(Debug, Default, Clone, Serialize)]
pub struct SideCounts {
    pub sent_messages: u64,
    pub sent_bytes: u64,
    pub report_bytes: u64,
    pub delivered: u64,
    pub lost: u64,
    pub over_rate: u64,
    pub partitioned: u64,
}

/// A small deterministic generator (xorshift64*), enough for link noise.
struct Rng(u64);

impl Rng {
    fn unit(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }
}

struct Side {
    client: async_nats::Client,
    prefix: String,
    /// Learned from the messages it sends.
    site: Option<String>,
    /// Sending allowance, bytes (token bucket), and when last topped up.
    allowance: f64,
    topped: Instant,
    counts: SideCounts,
}

pub async fn run(
    args: BridgeArgs,
    shutdown: impl std::future::Future<Output = ()>,
) -> anyhow::Result<()> {
    let partitions = args
        .partition
        .iter()
        .map(|p| Partition::parse(p))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let defaults = args.auth.defaults()?;
    let specs = args
        .nodes
        .iter()
        .map(|n| NodeSpec::parse(n, &defaults))
        .collect::<anyhow::Result<Vec<_>>>()?;
    // Nodes on one NATS with the same credentials share a connection.
    let mut clients: Vec<(String, NatsAuth, async_nats::Client)> = Vec::new();
    let mut sides = Vec::new();
    for (i, spec) in specs.iter().enumerate() {
        let (tls, how) = spec.describe();
        tracing::info!(node = i, prefix = %spec.prefix, tls, auth = how, "bridge node");
        let known = clients
            .iter()
            .find(|(u, a, _)| *u == spec.url && *a == spec.auth);
        let client = match known {
            Some((_, _, c)) => c.clone(),
            None => {
                let opts = ot_nats::connect_options(&spec.auth).await?;
                let c = ot_nats::log_destinations(opts, "bridge", &spec.url)
                    .name("opentrack-bridge")
                    .retry_on_initial_connect()
                    .connect(spec.url.as_str())
                    .await
                    .with_context(|| format!("node {i}: connecting to its NATS"))?;
                clients.push((spec.url.clone(), spec.auth.clone(), c.clone()));
                c
            }
        };
        let prefix = spec.prefix.as_str();
        sides.push(Side {
            client,
            prefix: prefix.to_owned(),
            site: None,
            allowance: 0.0,
            topped: Instant::now(),
            counts: SideCounts::default(),
        });
    }
    let mut subs = futures_util::stream::SelectAll::new();
    for (i, s) in sides.iter().enumerate() {
        let sub = s.client.subscribe(format!("{}.out.>", s.prefix)).await?;
        subs.push(sub.map(move |m| (i, m)));
    }
    tracing::info!(
        nodes = sides.len(),
        loss = args.loss,
        rate_kbps = args.rate_kbps,
        partitions = partitions.len(),
        "bridge running"
    );
    let sides = Arc::new(Mutex::new(sides));
    let started = Instant::now();
    let mut rng = Rng(args.seed.max(1));
    let rate = args.rate_kbps * 1000.0 / 8.0;
    let mut stats_tick = tokio::time::interval(Duration::from_secs(5));
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            _ = stats_tick.tick() => {
                if let Some(path) = &args.stats {
                    write_stats(path, &sides, started)?;
                }
            }
            next = subs.next() => {
                let Some((i, msg)) = next else { break };
                let kind = msg.subject.rsplit('.').next().unwrap_or("report").to_owned();
                let to = msg
                    .headers
                    .as_ref()
                    .and_then(|h| h.get(HEADER_TO))
                    .map(|v| v.as_str().to_owned());
                let t = started.elapsed().as_secs_f64();
                let mut deliveries = Vec::new();
                {
                    let mut sides = sides.lock().expect("bridge state");
                    let n = sides.len();
                    let from_site = std::str::from_utf8(msg.payload.get(4..7).unwrap_or_default())
                        .ok()
                        .map(str::to_owned);
                    let s = &mut sides[i];
                    if s.site.is_none() {
                        s.site = from_site.clone();
                    }
                    let len = msg.payload.len();
                    s.counts.sent_messages += 1;
                    s.counts.sent_bytes += len as u64;
                    if kind == "report" {
                        s.counts.report_bytes += len as u64;
                    }
                    if rate > 0.0 {
                        let now = Instant::now();
                        s.allowance = (s.allowance + now.duration_since(s.topped).as_secs_f64() * rate)
                            .min(rate.max(len as f64));
                        s.topped = now;
                        if s.allowance < len as f64 {
                            s.counts.over_rate += 1;
                            continue;
                        }
                        s.allowance -= len as f64;
                    }
                    let from = s.site.clone().unwrap_or_default();
                    for j in (0..n).filter(|j| *j != i) {
                        let dest = sides[j].site.clone();
                        if let (Some(to), Some(d)) = (&to, &dest)
                            && to != d
                        {
                            continue;
                        }
                        if let Some(d) = &dest
                            && partitions.iter().any(|p| p.cuts(t, &from, d))
                        {
                            sides[i].counts.partitioned += 1;
                            continue;
                        }
                        if rng.unit() < args.loss {
                            sides[i].counts.lost += 1;
                            continue;
                        }
                        let copies = if rng.unit() < args.duplicate { 2 } else { 1 };
                        let delay = args.delay_ms
                            + if args.jitter_ms > 0 {
                                (rng.unit() * args.jitter_ms as f64) as u64
                            } else {
                                0
                            };
                        sides[i].counts.delivered += 1;
                        let d = &sides[j];
                        deliveries.push((
                            d.client.clone(),
                            format!("{}.in.{kind}", d.prefix),
                            copies,
                            delay,
                        ));
                    }
                }
                for (client, subject, copies, delay) in deliveries {
                    let payload = msg.payload.clone();
                    let send = async move {
                        if delay > 0 {
                            tokio::time::sleep(Duration::from_millis(delay)).await;
                        }
                        for _ in 0..copies {
                            let _ = client.publish(subject.clone(), payload.clone()).await;
                        }
                    };
                    if delay > 0 {
                        tokio::spawn(send);
                    } else {
                        send.await;
                    }
                }
            }
        }
    }
    if let Some(path) = &args.stats {
        write_stats(path, &sides, started)?;
    }
    Ok(())
}

fn write_stats(
    path: &std::path::Path,
    sides: &Arc<Mutex<Vec<Side>>>,
    started: Instant,
) -> anyhow::Result<()> {
    let sides = sides.lock().expect("bridge state");
    let by: serde_json::Map<String, serde_json::Value> = sides
        .iter()
        .enumerate()
        .map(|(i, s)| {
            (
                s.site.clone().unwrap_or_else(|| format!("#{i}")),
                serde_json::to_value(&s.counts).unwrap_or_default(),
            )
        })
        .collect();
    let doc = serde_json::json!({ "elapsed_s": started.elapsed().as_secs_f64(), "nodes": by });
    std::fs::write(path, serde_json::to_vec_pretty(&doc)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_partition_cuts_only_across_its_sides_and_only_for_its_time() {
        let p = Partition::parse("60-180:AAA,BBB|CCC").unwrap();
        assert!(p.cuts(60.0, "AAA", "CCC"));
        assert!(p.cuts(100.0, "CCC", "BBB"));
        assert!(!p.cuts(100.0, "AAA", "BBB"));
        assert!(!p.cuts(59.9, "AAA", "CCC"));
        assert!(!p.cuts(180.0, "AAA", "CCC"));
        assert!(
            !p.cuts(100.0, "AAA", "DDD"),
            "a node in no group is not cut"
        );
        assert!(Partition::parse("60:AAA|BBB").is_err());
        assert!(Partition::parse("1-2:AAA").is_err());
    }

    #[derive(clap::Parser)]
    struct Cli {
        #[command(flatten)]
        bridge: BridgeArgs,
    }

    fn parse(args: &[&str]) -> Result<BridgeArgs, clap::Error> {
        use clap::Parser;
        Cli::try_parse_from(std::iter::once("bridge").chain(args.iter().copied())).map(|c| c.bridge)
    }

    /// A scratch directory with a few PEM and secret files (contents do not
    /// matter until something connects).
    fn files(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ot-bridge-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for f in ["ca.pem", "cert.pem", "key.pem", "b-ca.pem", "user.creds"] {
            std::fs::write(dir.join(f), "x").unwrap();
        }
        std::fs::write(dir.join("token"), "t0ken\n").unwrap();
        std::fs::write(dir.join("password"), "pa55\r\n").unwrap();
        std::fs::write(dir.join("empty"), "\n").unwrap();
        dir
    }

    #[test]
    fn the_nats_flags_apply_to_every_node_and_a_node_can_override_them() {
        let d = files("flags");
        let p = |f: &str| d.join(f).display().to_string();
        let args = parse(&[
            "--node",
            "tls://a:4222",
            "--node",
            &format!("tls://b:4222#site.b;ca={};cert=;key=", p("b-ca.pem")),
            "--nats-ca",
            &p("ca.pem"),
            "--nats-cert",
            &p("cert.pem"),
            "--nats-key",
            &p("key.pem"),
            "--nats-token-file",
            &p("token"),
        ])
        .unwrap();
        let defaults = args.auth.defaults().unwrap();
        assert_eq!(defaults.token.as_deref(), Some("t0ken"));
        let a = NodeSpec::parse(&args.nodes[0], &defaults).unwrap();
        assert_eq!(
            (a.url.as_str(), a.prefix.as_str()),
            ("tls://a:4222", "ot.sync")
        );
        assert_eq!(a.auth, defaults);
        assert_eq!(a.describe(), (true, "token"));
        let b = NodeSpec::parse(&args.nodes[1], &defaults).unwrap();
        assert_eq!(
            (b.url.as_str(), b.prefix.as_str()),
            ("tls://b:4222", "site.b")
        );
        assert_eq!(b.auth.tls_ca, Some(d.join("b-ca.pem")));
        assert_eq!(
            (b.auth.tls_cert.clone(), b.auth.tls_key.clone()),
            (None, None)
        );
        assert_eq!(b.auth.token.as_deref(), Some("t0ken"));
        // A node with its own sign-in drops the shared one.
        let c = NodeSpec::parse(
            &format!(
                "nats://c:4222;token-file=;user=bridge;password-file={}",
                p("password")
            ),
            &defaults,
        )
        .unwrap();
        assert_eq!(c.auth.token, None);
        assert_eq!(c.auth.user.as_deref(), Some("bridge"));
        assert_eq!(c.auth.password.as_deref(), Some("pa55"));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn the_nats_flags_come_from_the_environment_too() {
        // Only in the clap definition: a test must not set process-wide
        // variables the others could see.
        use clap::CommandFactory;
        let cmd = Cli::command();
        let env = |id: &str| {
            cmd.get_arguments()
                .find(|a| a.get_id() == id)
                .and_then(|a| a.get_env())
                .map(|e| e.to_string_lossy().into_owned())
        };
        for (id, var) in [
            ("creds", "OT_BRIDGE_NATS_CREDS"),
            ("token", "OT_BRIDGE_NATS_TOKEN"),
            ("token_file", "OT_BRIDGE_NATS_TOKEN_FILE"),
            ("user", "OT_BRIDGE_NATS_USER"),
            ("password", "OT_BRIDGE_NATS_PASSWORD"),
            ("password_file", "OT_BRIDGE_NATS_PASSWORD_FILE"),
            ("ca", "OT_BRIDGE_NATS_CA"),
            ("cert", "OT_BRIDGE_NATS_CERT"),
            ("key", "OT_BRIDGE_NATS_KEY"),
        ] {
            assert_eq!(env(id).as_deref(), Some(var), "{id}");
        }
        let hidden = |id: &str| {
            cmd.get_arguments()
                .find(|a| a.get_id() == id)
                .is_some_and(|a| a.is_hide_env_values_set())
        };
        assert!(hidden("token") && hidden("password"));
    }

    #[test]
    fn half_or_doubled_credentials_are_refused() {
        let d = files("errors");
        let p = |f: &str| d.join(f).display().to_string();
        let none = NatsAuth::default();
        let err = |s: &str, auth: &NatsAuth| NodeSpec::parse(s, auth).unwrap_err().to_string();
        assert!(
            err(&format!("tls://a:4222;cert={}", p("cert.pem")), &none).contains("needs its key")
        );
        assert!(
            err(&format!("tls://a:4222;key={}", p("key.pem")), &none).contains("needs its key")
        );
        assert!(err("nats://a:4222;user=bridge", &none).contains("needs a password"));
        assert!(
            err(
                &format!("nats://a:4222;password-file={}", p("password")),
                &none
            )
            .contains("needs a password")
        );
        assert!(
            err(
                &format!(
                    "nats://a:4222;creds={};token-file={}",
                    p("user.creds"),
                    p("token")
                ),
                &none
            )
            .contains("one of")
        );
        assert!(err("nats://a:4222;ca=/nonexistent/ca.pem", &none).contains("/nonexistent/ca.pem"));
        assert!(
            err(&format!("nats://a:4222;token-file={}", p("empty")), &none).contains("is empty")
        );
        assert!(err("nats://a:4222;password=hunter2", &none).contains("in a file"));
        assert!(!err("nats://a:4222;token=hunter2", &none).contains("hunter2"));
        assert!(err("nats://a:4222;tls", &none).contains("<name>=<value>"));
        assert!(err("nats://a:4222;cafile=x", &none).contains("unknown option"));
        assert!(err("#ot.sync", &none).contains("no NATS URL"));
        let e = err(
            "nats://u:hunter2@a:4222,nats://u:hunter2@b:4222;user=x",
            &none,
        );
        assert!(
            e.contains("nats://***@a:4222,nats://***@b:4222") && !e.contains("hunter2"),
            "{e}"
        );
        // A shared certificate a node cannot pair with a key is caught per node.
        let defaults = parse(&["--node", "tls://a:4222", "--nats-cert", &p("cert.pem")])
            .unwrap()
            .auth
            .defaults()
            .unwrap();
        assert!(err("tls://a:4222", &defaults).contains("needs its key"));
        // A literal secret and its file are one or the other.
        assert!(
            parse(&[
                "--node",
                "n",
                "--nats-token",
                "t",
                "--nats-token-file",
                &p("token")
            ])
            .is_err()
        );
        assert!(
            parse(&[
                "--node",
                "n",
                "--nats-password",
                "p",
                "--nats-password-file",
                &p("password")
            ])
            .is_err()
        );
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn the_nats_secrets_stay_out_of_debug_output() {
        let args = parse(&[
            "--node",
            "nats://a:4222",
            "--nats-token",
            "s3cret-token",
            "--nats-user",
            "bridge",
            "--nats-password",
            "s3cret-pass",
        ])
        .unwrap();
        let shown = format!("{args:?}");
        assert!(!shown.contains("s3cret"), "{shown}");
        let spec = NodeSpec::parse(&args.nodes[0], &args.auth.defaults().unwrap());
        let shown = format!("{spec:?}");
        assert!(!shown.contains("s3cret"), "{shown}");
    }

    #[test]
    fn the_noise_is_uniform_and_repeatable() {
        let (mut a, mut b) = (Rng(7), Rng(7));
        let xs: Vec<f64> = (0..1000).map(|_| a.unit()).collect();
        assert!(xs.iter().all(|x| (0.0..1.0).contains(x)));
        assert_eq!(xs[..5], (0..5).map(|_| b.unit()).collect::<Vec<_>>()[..]);
        let mean = xs.iter().sum::<f64>() / 1000.0;
        assert!((mean - 0.5).abs() < 0.05);
    }
}
