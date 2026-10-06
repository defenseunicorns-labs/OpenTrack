//! The link role: this node's side of the boundary with whatever carries
//! sync messages between nodes (see `docs/multi-node.md` and
//! `docs/sync-icd.md`). OpenTrack does not do the networking: it publishes
//! what it has to say on the node's own NATS, `<prefix>.out.<kind>`, and
//! hears other nodes on `<prefix>.in.<kind>`; a networking package (or
//! `opentrack bridge`, for server sites) moves the bytes.
//!
//! Nothing here depends on the link delivering everything. Decision logs
//! converge by anti-entropy: every few seconds each node says which
//! decisions it holds from each site (`summary`), and a node that hears of
//! decisions it lacks asks whoever said so (`want`), whichever node made
//! them.
//!
//! Every message is signed with this node's key (`sync.key`, beside the
//! database) and accepted only if it verifies with the key an admin pinned
//! for the sender's site code in Settings → Nodes (`docs/sync-icd.md`,
//! *Signature*). A signed message is accepted once, and only while its clock
//! is within [`FRESH`] of this node's, so a recorded one cannot be replayed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use ot_core::SiteCode;
use ot_sync::sign::{NodeKey, PublicKey, SignError};
use ot_sync::wire::{self, Body, Kind, Message, Summary, Want};
use ot_sync::{Entry, Hlc};
use serde_json::json;

use crate::config::Common;
use crate::settings_api::{SyncSettings, sync_settings};

/// Header on an `out` message meant for one node, with its site code.
pub const HEADER_TO: &str = "OT-To";

#[derive(Debug, Clone, clap::Args)]
pub struct LinkArgs {
    /// Subject prefix of the sync boundary: `<prefix>.out.<kind>` and
    /// `<prefix>.in.<kind>`.
    #[arg(long, env = "OT_SYNC_PREFIX", default_value = "ot.sync")]
    pub sync_prefix: String,
    /// Seconds between summaries.
    #[arg(long, env = "OT_SYNC_SUMMARY_SECS", default_value_t = 5.0)]
    pub summary_secs: f64,
}

/// A message to send: its kind, the node it is for (none: everyone), and
/// its bytes.
#[derive(Debug, Clone, PartialEq)]
pub struct Out {
    pub kind: Kind,
    pub to: Option<SiteCode>,
    pub bytes: Vec<u8>,
}

/// Ranges asked for per request, and how long before asking a site again.
const MOST_RANGES: usize = 16;
const ASK_AGAIN: Duration = Duration::from_secs(5);

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct LinkCounts {
    pub received: u64,
    pub unreadable: u64,
    /// From a site not in the trusted list.
    pub untrusted: u64,
    pub reports_in: u64,
    pub decisions_in: u64,
    pub decisions_out: u64,
    pub wants_in: u64,
    pub wants_out: u64,
    /// From a trusted site with no key pinned for it.
    pub no_key: u64,
    /// Signed with another key, or the signature does not verify.
    pub bad_signature: u64,
    /// Clock further than [`FRESH`] from this node's.
    pub stale: u64,
    /// Already accepted once.
    pub replayed: u64,
}

/// How far a message's clock may be from this node's for it to be
/// accepted. Nodes' clocks must agree to within this (NTP, GPS); a message
/// delayed longer is dropped, and what matters is asked for again.
pub const FRESH: Duration = Duration::from_secs(300);
/// Refused messages are logged (and, for a bad signature, audited) at most
/// this often per site.
const REPORT_EVERY: Duration = Duration::from_secs(60);

/// The link's state, apart from NATS.
pub struct Link {
    common: Common,
    settings: SyncSettings,
    /// This node's own entries sent so far (by sequence).
    sent: u64,
    /// When each site's missing decisions were last asked for.
    asked: HashMap<SiteCode, Instant>,
    /// When each peer was last heard.
    pub heard: HashMap<SiteCode, Instant>,
    /// What each peer last said about its tracks' identity and attributes
    /// (sent only when they change), put on every report of theirs.
    identity: HashMap<(SiteCode, ot_core::Uid), Identity>,
    pub counts: LinkCounts,
    redis: Option<ot_store::RedisStore>,
    /// This node's signing key.
    key: NodeKey,
    /// The keys pinned for peers (Settings → Nodes).
    keys: HashMap<SiteCode, PublicKey>,
    /// Signatures accepted within the last [`FRESH`] × 2, when.
    seen: HashMap<[u8; 16], Instant>,
    /// When each site's refusals were last logged, and how many since.
    refused: HashMap<SiteCode, (Instant, u64)>,
    /// When a bad signature from each site was last audited.
    audited: HashMap<SiteCode, Instant>,
}

/// Where this node's signing key lives: `sync.key` beside the database.
pub fn key_path(common: &Common) -> PathBuf {
    common
        .sqlite
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .join("sync.key")
}

/// This node's signing key, made on first use (in the FIPS module) and kept
/// in `sync.key`, readable by its owner only. A damaged file is an error,
/// never silently replaced: that would change the node's identity.
pub fn load_node_key(common: &Common) -> anyhow::Result<NodeKey> {
    use anyhow::Context;
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD as B64;
    let path = key_path(common);
    let read = |path: &Path| -> anyhow::Result<NodeKey> {
        let text = std::fs::read_to_string(path)?;
        let der = B64
            .decode(text.trim())
            .map_err(|_| SignError::BadPrivateKey)?;
        Ok(NodeKey::from_pkcs8(&der)?)
    };
    match read(&path) {
        Ok(k) => return Ok(k),
        Err(e)
            if e.downcast_ref::<std::io::Error>()
                .is_none_or(|e| e.kind() != std::io::ErrorKind::NotFound) =>
        {
            return Err(e).with_context(|| format!("reading {}", path.display()));
        }
        Err(_) => {}
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let der = NodeKey::generate()?;
    // Two roles may start together: only one makes the file, the other
    // reads it.
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    match opts.open(&path) {
        Ok(mut f) => {
            use std::io::Write;
            f.write_all(format!("{}\n", B64.encode(&der)).as_bytes())
                .with_context(|| format!("writing {}", path.display()))?;
            tracing::info!(path = %path.display(), "made the sync signing key");
            Ok(NodeKey::from_pkcs8(&der)?)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            read(&path).with_context(|| format!("reading {}", path.display()))
        }
        Err(e) => Err(e).with_context(|| format!("writing {}", path.display())),
    }
}

#[derive(Debug, Default, Clone)]
struct Identity {
    identifiers: Vec<ot_core::schema::Identifier>,
    attrs: Option<serde_json::Value>,
}

/// A peer not heard for this long is asked for a snapshot when it returns.
const SILENT: Duration = Duration::from_secs(30);

impl Link {
    pub async fn new(common: Common) -> anyhow::Result<Self> {
        let site = common.site;
        let c = common.clone();
        let (sent, key) = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
            Ok((c.open_db()?.sync_next_seq(site)? - 1, load_node_key(&c)?))
        })
        .await??;
        let mut link = Self {
            common,
            settings: SyncSettings::default(),
            sent,
            asked: HashMap::new(),
            heard: HashMap::new(),
            identity: HashMap::new(),
            counts: LinkCounts::default(),
            redis: None,
            key,
            keys: HashMap::new(),
            seen: HashMap::new(),
            refused: HashMap::new(),
            audited: HashMap::new(),
        };
        link.refresh().await?;
        Ok(link)
    }

    pub fn enabled(&self) -> bool {
        self.settings.enabled
    }

    async fn db<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut ot_store::Db) -> ot_store::sqlite::Result<T> + Send + 'static,
    ) -> anyhow::Result<T> {
        let c = self.common.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<T> {
            let mut db = c.open_db()?;
            Ok(f(&mut db)?)
        })
        .await?
    }

    /// Reload the settings (peers, on or off) and the pinned keys.
    pub async fn refresh(&mut self) -> anyhow::Result<()> {
        let (saved, pinned) = self
            .db(|db| Ok((db.app_settings()?, db.sync_peer_keys()?)))
            .await?;
        self.settings = sync_settings(&saved);
        self.keys = pinned
            .into_iter()
            .filter_map(|k| {
                let key = k.public_key.parse().ok();
                if key.is_none() {
                    tracing::warn!(site = %k.site, "the key pinned for this site is not a key");
                }
                Some((k.site.parse().ok()?, key?))
            })
            .collect();
        let now = Instant::now();
        self.seen
            .retain(|_, at| now.duration_since(*at) < FRESH * 2);
        Ok(())
    }

    /// This node's signing key.
    pub fn key(&self) -> &NodeKey {
        &self.key
    }

    /// A message to send, signed: once, whoever it is for.
    fn out(&self, kind: Kind, to: Option<SiteCode>, bytes: Vec<u8>) -> Out {
        Out {
            kind,
            to,
            bytes: self.key.sign(bytes),
        }
    }

    async fn redis(&mut self) -> anyhow::Result<ot_store::RedisStore> {
        if self.redis.is_none() {
            self.redis = Some(self.common.open_redis().await?);
        }
        Ok(self.redis.clone().expect("opened above"))
    }

    /// Messages the engine queued (reports, attributes, releases).
    pub async fn engine_messages(&mut self) -> anyhow::Result<Vec<Out>> {
        let redis = self.redis().await?;
        let mut outs = Vec::new();
        loop {
            let batch = redis.pop_sync_out(500).await?;
            let n = batch.len();
            outs.extend(batch.into_iter().filter_map(|bytes| {
                let kind = bytes.get(3).copied().and_then(kind_of)?;
                Some(self.out(kind, None, bytes))
            }));
            if n < 500 {
                return Ok(outs);
            }
        }
    }

    fn now_hlc(&self) -> Hlc {
        Hlc::new(chrono::Utc::now().timestamp_millis() as u64, 0)
    }

    fn trusted(&self, site: SiteCode) -> bool {
        self.settings.peers.iter().any(|p| p == site.as_str())
    }

    /// This node's entries made since the last call, for everyone.
    pub async fn new_decisions(&mut self) -> anyhow::Result<Vec<Out>> {
        let (site, after) = (self.common.site, self.sent);
        let entries: Vec<Entry> = self.db(move |db| db.sync_since(site, after, 255)).await?;
        let Some(last) = entries.last() else {
            return Ok(Vec::new());
        };
        self.sent = last.id.seq;
        self.counts.decisions_out += entries.len() as u64;
        Ok(wire::encode_decisions(site, self.now_hlc(), &entries)
            .into_iter()
            .map(|bytes| self.out(Kind::Decision, None, bytes))
            .collect())
    }

    /// How the link is going, for the Settings page.
    pub async fn write_status(&mut self) -> anyhow::Result<()> {
        let now = chrono::Utc::now();
        let heard: serde_json::Map<String, serde_json::Value> = self
            .heard
            .iter()
            .map(|(site, at)| {
                let ago = chrono::Duration::from_std(at.elapsed()).unwrap_or_default();
                (site.to_string(), json!(now - ago))
            })
            .collect();
        let status = json!({ "at": now, "heard": heard, "counts": self.counts });
        self.redis().await?.put_sync_status("link", &status).await?;
        Ok(())
    }

    /// What this node holds, for everyone.
    pub async fn summary(&self) -> anyhow::Result<Out> {
        let heads = self.db(|db| db.sync_heads()).await?;
        let m = Message::new(
            self.common.site,
            self.now_hlc(),
            Body::Summary(Summary {
                heads,
                reporting: 0,
            }),
        );
        Ok(self.out(Kind::Summary, None, m.encode()))
    }

    /// Handle a message another node sent; returns what to send back.
    pub async fn handle(&mut self, bytes: &[u8]) -> anyhow::Result<Vec<Out>> {
        self.counts.received += 1;
        let site = match wire::sender(bytes) {
            Ok(s) => s,
            Err(e) => {
                self.counts.unreadable += 1;
                tracing::debug!(error = %e, "unreadable sync message");
                return Ok(Vec::new());
            }
        };
        if site == self.common.site {
            return Ok(Vec::new());
        }
        if !self.trusted(site) {
            self.counts.untrusted += 1;
            return Ok(Vec::new());
        }
        let Some(key) = self.keys.get(&site).copied() else {
            self.counts.no_key += 1;
            self.refuse(site, "no key is pinned for this site in Settings → Nodes");
            return Ok(Vec::new());
        };
        let plain = match key.verify(bytes) {
            Ok(p) => p,
            Err(e) => {
                self.counts.bad_signature += 1;
                self.refuse(site, &e.to_string());
                self.audit_bad_signature(site, &e).await;
                return Ok(Vec::new());
            }
        };
        let m = match Message::decode(plain) {
            Ok(m) => m,
            Err(e) => {
                self.counts.unreadable += 1;
                tracing::debug!(error = %e, "unreadable sync message");
                return Ok(Vec::new());
            }
        };
        let now_ms = chrono::Utc::now().timestamp_millis() as u64;
        if m.hlc.ms().abs_diff(now_ms) > FRESH.as_millis() as u64 {
            self.counts.stale += 1;
            self.refuse(site, "its clock is more than 5 minutes from this node's");
            return Ok(Vec::new());
        }
        let sig: [u8; 16] = bytes[bytes.len() - ot_sync::sign::SIGNATURE..][..16]
            .try_into()
            .expect("verified messages carry a signature");
        if self.seen.insert(sig, Instant::now()).is_some() {
            self.counts.replayed += 1;
            return Ok(Vec::new());
        }
        let returning = self
            .heard
            .insert(m.site, Instant::now())
            .is_none_or(|at| at.elapsed() > SILENT);
        let mut outs = Vec::new();
        if returning {
            // A node new to this one (or back): ask for its whole picture.
            let want = Message::new(
                self.common.site,
                self.now_hlc(),
                Body::Want(Want {
                    decisions: Vec::new(),
                    snapshot: true,
                }),
            );
            outs.push(self.out(Kind::Want, Some(m.site), want.encode()));
        }
        outs.extend(self.dispatch(m).await?);
        Ok(outs)
    }

    /// Log a refused message, at most every [`REPORT_EVERY`] per site.
    fn refuse(&mut self, site: SiteCode, why: &str) {
        let now = Instant::now();
        let (at, n) = self.refused.entry(site).or_insert((now - REPORT_EVERY, 0));
        *n += 1;
        if now.duration_since(*at) >= REPORT_EVERY {
            tracing::warn!(site = %site, refused = *n, reason = why, "sync messages refused");
            *at = now;
            *n = 0;
        }
    }

    /// A trusted site's message that fails its signature may be someone
    /// else using its name: audit it, at most every [`REPORT_EVERY`].
    async fn audit_bad_signature(&mut self, site: SiteCode, e: &SignError) {
        if self
            .audited
            .get(&site)
            .is_some_and(|at| at.elapsed() < REPORT_EVERY)
        {
            return;
        }
        self.audited.insert(site, Instant::now());
        let event =
            ot_store::audit::AuditEvent::new(format!("sync:{site}"), "sync_signature_refused")
                .failure()
                .detail(json!({
                    "site": site.as_str(),
                    "error": e.to_string(),
                    "refused_so_far": self.counts.bad_signature,
                }));
        if let Err(err) = self.db(move |db| db.audit(&event)).await {
            tracing::warn!(error = %format!("{err:#}"), "refused sync message not audited");
        }
    }

    async fn dispatch(&mut self, m: Message) -> anyhow::Result<Vec<Out>> {
        match m.body {
            Body::Decisions(entries) => {
                self.counts.decisions_in += entries.len() as u64;
                let redis = self.redis().await?;
                for entry in entries {
                    // Only trusted sites' decisions, whoever relays them.
                    if entry.id.site != self.common.site && self.trusted(entry.id.site) {
                        redis
                            .push_command(&json!({ "op": "sync_entry", "entry": entry }))
                            .await?;
                    }
                }
                Ok(Vec::new())
            }
            Body::Summary(s) => self.wants_for(m.site, s).await,
            Body::Want(w) => {
                self.counts.wants_in += 1;
                if w.snapshot {
                    self.redis()
                        .await?
                        .push_command(&json!({ "op": "sync_snapshot" }))
                        .await?;
                }
                self.answer(m.site, w).await
            }
            Body::Reports(reports) => {
                self.counts.reports_in += reports.len() as u64;
                let obs: Vec<ot_core::Observation> = reports
                    .into_iter()
                    .map(|r| self.observation(m.site, r))
                    .collect();
                self.redis()
                    .await?
                    .append_observations(&format!("peer:{}", m.site), &obs)
                    .await?;
                Ok(Vec::new())
            }
            Body::Attrs(items) => {
                for (uid, v) in items {
                    self.identity.entry((m.site, uid)).or_default().attrs = Some(v);
                }
                Ok(Vec::new())
            }
            Body::Release(uids) => {
                let uids: Vec<String> = uids.iter().map(|u| u.to_string()).collect();
                self.redis()
                    .await?
                    .push_command(
                        &json!({ "op": "sync_release", "site": m.site.as_str(), "uids": uids }),
                    )
                    .await?;
                Ok(Vec::new())
            }
        }
    }

    /// A peer's report as an observation of source `peer:<site>`, keyed by
    /// the track's UID, with the identity the peer last gave it.
    fn observation(&mut self, site: SiteCode, r: wire::Report) -> ot_core::Observation {
        use ot_core::schema::{Ellipse, Identifier, Uncertainty};
        let key = (site, r.uid);
        if r.state == ot_core::TrackState::Dropped {
            self.identity.remove(&key);
        } else if let Some(ids) = &r.identifiers {
            self.identity.entry(key).or_default().identifiers = ids
                .iter()
                .map(|(s, v)| Identifier::new(s.clone(), v.clone()))
                .collect();
        }
        let who = self.identity.get(&key).cloned().unwrap_or_default();
        let at = chrono::DateTime::from_timestamp_millis(r.time_ms).unwrap_or_default();
        let known = |m: f64| m.is_finite().then_some(m);
        let uncertainty = known(r.error.major_m).map(|major| Uncertainty {
            ellipse: Some(Ellipse {
                semi_major_m: major,
                semi_minor_m: known(r.error.minor_m).unwrap_or(major).min(major),
                orientation_deg: r.error.orientation_deg,
            }),
            circular_error_m: None,
            vertical_error_m: None,
            covariance: None,
        });
        let mut v = json!({
            "schema_version": 1,
            "source_id": format!("peer:{site}"),
            "source_track_key": r.uid.to_string(),
            "observed_at": at,
            "received_at": chrono::Utc::now(),
            "position": {"latitude": r.lat, "longitude": r.lon, "altitude_hae_m": r.alt_m},
            "kinematics": {"course_deg": r.course_deg, "speed_mps": r.speed_mps},
            "classification": {"domain": r.domain},
            "state": r.state,
            "identifiers": who.identifiers,
        });
        if let Some(a) = &who.attrs {
            for k in ["name", "callsign"] {
                if a[k].is_string() {
                    v[k] = a[k].clone();
                }
            }
            if let Some(c) = a["classification"].as_object() {
                let mut c = c.clone();
                if r.domain.is_some() {
                    c.insert("domain".into(), json!(r.domain));
                }
                v["classification"] = serde_json::Value::Object(c);
            }
            if a["attributes"].is_object() {
                v["ext"] = a["attributes"].clone();
            }
        }
        let mut obs: ot_core::Observation =
            serde_json::from_value(strip_nulls(v)).expect("a peer report is an observation");
        obs.uncertainty = uncertainty;
        obs.origin = r
            .origin_ms
            .and_then(chrono::DateTime::from_timestamp_millis);
        obs
    }

    /// Ask `from` for the decisions its summary shows this node lacks.
    async fn wants_for(&mut self, from: SiteCode, s: Summary) -> anyhow::Result<Vec<Out>> {
        let mut decisions = Vec::new();
        for (site, head) in s.heads {
            if site == self.common.site || !self.trusted(site) {
                continue;
            }
            if self
                .asked
                .get(&site)
                .is_some_and(|at| at.elapsed() < ASK_AGAIN)
            {
                continue;
            }
            let missing = self
                .db(move |db| db.sync_missing(site, head, MOST_RANGES))
                .await?;
            if !missing.is_empty() {
                self.asked.insert(site, Instant::now());
                decisions.push((site, missing));
            }
        }
        if decisions.is_empty() {
            return Ok(Vec::new());
        }
        self.counts.wants_out += 1;
        let m = Message::new(
            self.common.site,
            self.now_hlc(),
            Body::Want(Want {
                decisions,
                snapshot: false,
            }),
        );
        Ok(vec![self.out(Kind::Want, Some(from), m.encode())])
    }

    /// Send `to` the decisions it asked for that this node holds.
    async fn answer(&mut self, to: SiteCode, w: Want) -> anyhow::Result<Vec<Out>> {
        let mut entries = Vec::new();
        for (site, ranges) in w.decisions {
            for (a, b) in ranges.into_iter().take(MOST_RANGES) {
                let b = b.min(a + 1000);
                entries.extend(self.db(move |db| db.sync_range(site, a, b)).await?);
            }
        }
        self.counts.decisions_out += entries.len() as u64;
        Ok(
            wire::encode_decisions(self.common.site, self.now_hlc(), &entries)
                .into_iter()
                .map(|bytes| self.out(Kind::Decision, Some(to), bytes))
                .collect(),
        )
    }
}

fn kind_of(b: u8) -> Option<Kind> {
    [
        Kind::Report,
        Kind::Attrs,
        Kind::Decision,
        Kind::Summary,
        Kind::Want,
        Kind::Release,
    ]
    .into_iter()
    .find(|k| *k as u8 == b)
}

/// Drop null members (absent optional fields).
fn strip_nulls(v: serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(m) => serde_json::Value::Object(
            m.into_iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k, strip_nulls(v)))
                .collect(),
        ),
        other => other,
    }
}

/// Run the link against the node's NATS until shutdown.
pub async fn run(
    common: Common,
    args: LinkArgs,
    shutdown: impl std::future::Future<Output = ()>,
) -> anyhow::Result<()> {
    let nats = ot_nats::Nats::connect(common.nats_settings()).await?;
    let client = nats.client().clone();
    let mut sub = client
        .subscribe(format!("{}.in.>", args.sync_prefix))
        .await?;
    let mut link = Link::new(common).await?;
    tracing::info!(
        prefix = %args.sync_prefix,
        enabled = link.enabled(),
        key = %link.key().public().fingerprint_text(),
        "link running"
    );
    let send = |outs: Vec<Out>| {
        let client = client.clone();
        let prefix = args.sync_prefix.clone();
        async move {
            for o in outs {
                let subject = format!("{prefix}.out.{}", o.kind.name());
                let mut headers = ot_nats::async_nats::HeaderMap::new();
                if let Some(to) = o.to {
                    headers.insert(HEADER_TO, to.as_str());
                }
                if let Err(e) = client
                    .publish_with_headers(subject, headers, o.bytes.into())
                    .await
                {
                    tracing::warn!(error = %e, "sync publish failed");
                }
            }
        }
    };
    let summary_every = Duration::from_secs_f64(args.summary_secs.max(0.5));
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    let (mut next_summary, mut next_refresh) = (Instant::now(), Instant::now());
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            msg = sub.next() => {
                let Some(msg) = msg else { break };
                if !link.enabled() {
                    continue;
                }
                match link.handle(&msg.payload).await {
                    Ok(outs) => send(outs).await,
                    Err(e) => tracing::warn!(error = %format!("{e:#}"), "sync message failed"),
                }
            }
            _ = tick.tick() => {
                let now = Instant::now();
                if now >= next_refresh {
                    next_refresh = now + Duration::from_secs(5);
                    if let Err(e) = link.refresh().await {
                        tracing::warn!(error = %format!("{e:#}"), "sync settings refresh failed");
                    }
                }
                if !link.enabled() {
                    continue;
                }
                match link.new_decisions().await {
                    Ok(outs) => send(outs).await,
                    Err(e) => tracing::warn!(error = %format!("{e:#}"), "sending decisions failed"),
                }
                match link.engine_messages().await {
                    Ok(outs) => send(outs).await,
                    Err(e) => tracing::warn!(error = %format!("{e:#}"), "sending tracks failed"),
                }
                if now >= next_summary {
                    next_summary = now + summary_every;
                    if let Err(e) = link.write_status().await {
                        tracing::warn!(error = %format!("{e:#}"), "sync status failed");
                    }
                    match link.summary().await {
                        Ok(o) => send(vec![o]).await,
                        Err(e) => tracing::warn!(error = %format!("{e:#}"), "summary failed"),
                    }
                }
            }
        }
    }
    // Bounded: with NATS unreachable a flush would wait forever.
    let _ = tokio::time::timeout(Duration::from_secs(2), client.flush()).await;
    tracing::info!(counts = ?link.counts, "link stopped");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A link for site `site` on a throwaway database (no Redis: summaries
    /// need only the database), trusting `peers`.
    async fn link(site: &str, peers: &[&str]) -> (Link, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let mut common = crate::config_backup::tests::common(dir.path());
        common.site = site.parse().unwrap();
        let v = json!({ "sync": { "enabled": true, "peers": peers } });
        common
            .open_db()
            .unwrap()
            .put_app_settings(&v, "test")
            .unwrap();
        (Link::new(common).await.unwrap(), dir)
    }

    fn pin(to: &Link, site: &str, key: &PublicKey) {
        let site: SiteCode = site.parse().unwrap();
        to.common
            .open_db()
            .unwrap()
            .sync_pin_key(site, &key.to_string(), "admin")
            .unwrap();
    }

    fn audited(l: &Link) -> i64 {
        l.common
            .open_db()
            .unwrap()
            .connection()
            .query_row(
                "SELECT count(*) FROM audit WHERE op = 'sync_signature_refused'",
                [],
                |r| r.get(0),
            )
            .unwrap()
    }

    #[tokio::test]
    async fn the_key_is_made_once_and_kept_private() {
        let (a, _d) = link("AAA", &[]).await;
        let again = load_node_key(&a.common).unwrap();
        assert_eq!(again.public(), a.key().public());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(key_path(&a.common))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // A damaged key is an error, not a new identity.
        std::fs::write(key_path(&a.common), "not a key").unwrap();
        assert!(load_node_key(&a.common).is_err());
    }

    #[tokio::test]
    async fn only_signed_fresh_messages_from_pinned_keys_are_accepted() {
        let (a, _da) = link("AAA", &["BBB"]).await;
        let (mut b, _db) = link("BBB", &["AAA", "CCC"]).await;
        let (c, _dc) = link("CCC", &["BBB"]).await;
        let summary = a.summary().await.unwrap();

        // Trusted, but no key pinned yet (as after an upgrade).
        b.handle(&summary.bytes).await.unwrap();
        assert_eq!(b.counts.no_key, 1);

        // Pinned: accepted, and B (new to A) asks for A's picture.
        pin(&b, "AAA", &a.key().public());
        pin(&b, "CCC", &c.key().public());
        b.refresh().await.unwrap();
        let back = b.handle(&summary.bytes).await.unwrap();
        assert_eq!(back.len(), 1);
        assert!(b.heard.contains_key(&"AAA".parse().unwrap()));

        // The same message again is a replay.
        assert!(b.handle(&summary.bytes).await.unwrap().is_empty());
        assert_eq!(b.counts.replayed, 1);

        // A changed byte, or C claiming to be A: refused and audited once.
        let mut tampered = a.summary().await.unwrap().bytes;
        tampered[16] ^= 1;
        b.handle(&tampered).await.unwrap();
        let mut forged = c.summary().await.unwrap().bytes;
        forged[4..7].copy_from_slice(b"AAA");
        b.handle(&forged).await.unwrap();
        assert_eq!(b.counts.bad_signature, 2);
        assert_eq!(audited(&b), 1);

        // A site not trusted at all, and a message with no signature.
        let (d, _dd) = link("DDD", &[]).await;
        b.handle(&d.summary().await.unwrap().bytes).await.unwrap();
        assert_eq!(b.counts.untrusted, 1);
        let plain = Message::new(
            a.common.site,
            a.now_hlc(),
            Body::Summary(Summary::default()),
        )
        .encode();
        b.handle(&plain).await.unwrap();
        assert_eq!(b.counts.bad_signature, 3);

        // Signed properly but recorded long ago: stale.
        let old = Message::new(
            a.common.site,
            Hlc::new(a.now_hlc().ms() - 600_000, 0),
            Body::Summary(Summary::default()),
        );
        b.handle(&a.key().sign(old.encode())).await.unwrap();
        assert_eq!(b.counts.stale, 1);

        // Version 1 (unsigned, from before the upgrade) is unreadable.
        let mut v1 = plain.clone();
        v1[2] = 1;
        b.handle(&v1).await.unwrap();
        assert_eq!(b.counts.unreadable, 1);
    }
}
