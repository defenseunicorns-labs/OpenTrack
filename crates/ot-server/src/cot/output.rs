//! The live picture as CoT, and the three ways it leaves: a connection to a
//! TAK Server, UDP multicast, and a listener TAK clients connect to.
//!
//! The [`Hub`] holds every published track's latest event data and
//! broadcasts each change. Each output runs a fan-out task that renders a
//! change once and hands the bytes to every connection's bounded queue,
//! and re-sends each live track every half of its stale time. A connection
//! (a TAK Server link, a client of the listener, the multicast socket) gets
//! the whole picture first, then its queue. A connection whose queue fills
//! (a slow client) is dropped, so it never holds up the others; one that
//! comes back gets the picture again.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use anyhow::bail;
use bytes::Bytes;
use chrono::Utc;
use ot_core::Uid;
use ot_source::transport::MulticastInterface;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinSet;
use tokio::time::Instant;

use super::event::{self, CotTrack};
use super::settings::{Delivery, TakOutput};

/// Changes buffered for an output's fan-out before it must catch up by
/// sending the whole picture again.
const BROADCAST: usize = 16_384;
/// Events queued for one connection before it counts as too slow and is
/// dropped.
pub const CLIENT_QUEUE: usize = 8_192;
/// A write that takes longer than this ends the connection.
const WRITE_TIMEOUT: Duration = Duration::from_secs(20);
/// Stream writes are gathered up to about this size.
const WRITE_BATCH: usize = 64 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// A change to the picture.
#[derive(Debug, Clone)]
pub enum Change {
    Upsert(Arc<CotTrack>),
    /// The track ended; the type it was last sent as, when it was.
    Delete {
        uid: Uid,
        last_type: Option<String>,
    },
}

/// Every published track, as CoT, and the stream of changes to it.
pub struct Hub {
    picture: RwLock<HashMap<Uid, Arc<CotTrack>>>,
    tx: broadcast::Sender<Change>,
}

impl Default for Hub {
    fn default() -> Self {
        Self {
            picture: RwLock::new(HashMap::new()),
            tx: broadcast::channel(BROADCAST).0,
        }
    }
}

impl Hub {
    fn read(&self) -> std::sync::RwLockReadGuard<'_, HashMap<Uid, Arc<CotTrack>>> {
        self.picture.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<Uid, Arc<CotTrack>>> {
        self.picture.write().unwrap_or_else(|e| e.into_inner())
    }

    /// Replace the picture without telling anyone (at start).
    pub fn load(&self, tracks: impl IntoIterator<Item = CotTrack>) {
        *self.write() = tracks.into_iter().map(|t| (t.uid, Arc::new(t))).collect();
    }

    pub fn upsert(&self, t: CotTrack) {
        let t = Arc::new(t);
        // The picture first, so a connection that took its snapshot before
        // this is registered to hear the change.
        self.write().insert(t.uid, t.clone());
        let _ = self.tx.send(Change::Upsert(t));
    }

    /// Remove a track; `false` if it was not in the picture.
    pub fn delete(&self, uid: Uid) -> bool {
        let Some(old) = self.write().remove(&uid) else {
            return false;
        };
        let _ = self.tx.send(Change::Delete {
            uid,
            last_type: Some(old.cot_type.clone()),
        });
        true
    }

    pub fn get(&self, uid: Uid) -> Option<Arc<CotTrack>> {
        self.read().get(&uid).cloned()
    }

    pub fn snapshot(&self) -> Vec<Arc<CotTrack>> {
        self.read().values().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.read().len()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Change> {
        self.tx.subscribe()
    }
}

/// How an output renders events.
#[derive(Debug, Clone, Copy)]
pub struct Render {
    pub stale: Duration,
    pub remarks: bool,
}

impl Render {
    pub fn of(o: &TakOutput) -> Self {
        Self {
            stale: Duration::from_secs_f64(o.stale_secs.max(1.0)),
            remarks: o.remarks,
        }
    }

    /// The track's event, or `None` once it is past its stale time (its
    /// reports stopped): it is not sent again until it reports.
    pub fn event(&self, t: &CotTrack) -> Option<Bytes> {
        event::event_xml(t, Utc::now(), self.stale, self.remarks).map(Bytes::from)
    }

    pub fn delete(&self, uid: Uid, last_type: Option<&str>) -> Bytes {
        Bytes::from(event::delete_xml(uid, last_type, Utc::now()))
    }

    /// Each live track is sent again this long after it was last sent, well
    /// before TAK lets it go stale.
    pub fn refresh_every(&self) -> Duration {
        (self.stale / 2).max(Duration::from_secs(1))
    }
}

/// When each track sent is due to be sent again. No I/O, so it can be
/// tested alone.
#[derive(Debug, Default)]
pub struct Refresh {
    every: Duration,
    due: HashMap<Uid, Instant>,
}

impl Refresh {
    pub fn new(every: Duration) -> Self {
        Self {
            every,
            due: HashMap::new(),
        }
    }

    pub fn sent(&mut self, uid: Uid, now: Instant) {
        self.due.insert(uid, now + self.every);
    }

    pub fn remove(&mut self, uid: Uid) {
        self.due.remove(&uid);
    }

    /// Tracks due again, forgotten until they are sent.
    pub fn take_due(&mut self, now: Instant) -> Vec<Uid> {
        let due: Vec<Uid> = self
            .due
            .iter()
            .filter(|(_, at)| **at <= now)
            .map(|(u, _)| *u)
            .collect();
        for u in &due {
            self.due.remove(u);
        }
        due
    }

    pub fn tracked(&self) -> impl Iterator<Item = Uid> + '_ {
        self.due.keys().copied()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.due.len()
    }
}

/// An output's counters and state, for status and metrics.
#[derive(Debug, Default)]
pub struct Counters {
    pub sent: AtomicU64,
    pub errors: AtomicU64,
    /// Connections dropped for falling behind.
    pub dropped: AtomicU64,
    pub clients: AtomicU64,
    state: Mutex<(String, Option<String>)>,
}

impl Counters {
    pub fn set_state(&self, state: &str) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).0 = state.to_owned();
    }

    /// Count an error and keep its text.
    pub fn error(&self, text: String) {
        self.errors.fetch_add(1, Ordering::Relaxed);
        self.state.lock().unwrap_or_else(|e| e.into_inner()).1 = Some(text);
    }

    pub fn state(&self) -> (String, Option<String>) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn status(&self) -> serde_json::Value {
        let (state, last_error) = self.state();
        serde_json::json!({
            "state": state,
            "last_error": last_error,
            "sent": self.sent.load(Ordering::Relaxed),
            "errors": self.errors.load(Ordering::Relaxed),
            "dropped": self.dropped.load(Ordering::Relaxed),
            "clients": self.clients.load(Ordering::Relaxed),
        })
    }
}

/// The connections an output feeds.
type Clients = Arc<Mutex<Vec<Client>>>;

struct Client {
    peer: String,
    tx: mpsc::Sender<Bytes>,
}

/// Counts a connection while it lasts.
struct Connected(Arc<Counters>);

impl Connected {
    fn new(c: &Arc<Counters>) -> Self {
        c.clients.fetch_add(1, Ordering::Relaxed);
        Self(c.clone())
    }
}

impl Drop for Connected {
    fn drop(&mut self) {
        self.0.clients.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Everything a connection needs.
#[derive(Clone)]
struct Ctx {
    id: String,
    hub: Arc<Hub>,
    clients: Clients,
    render: Render,
    counters: Arc<Counters>,
}

impl Ctx {
    /// Register a connection: from now on it hears every change.
    fn register(&self, peer: &str) -> mpsc::Receiver<Bytes> {
        let (tx, rx) = mpsc::channel(CLIENT_QUEUE);
        self.clients
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(Client {
                peer: peer.to_owned(),
                tx,
            });
        rx
    }

    /// Hand an event to every connection; drop those too far behind.
    /// Rendered only when some connection is there to take it.
    fn send_all(&self, render: impl FnOnce() -> Option<Bytes>) {
        let mut clients = self.clients.lock().unwrap_or_else(|e| e.into_inner());
        if clients.is_empty() {
            return;
        }
        let Some(bytes) = render() else { return };
        clients.retain(|c| match c.tx.try_send(bytes.clone()) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                tracing::warn!(output = %self.id, peer = %c.peer,
                    queue = CLIENT_QUEUE, "TAK connection too slow; dropped");
                self.counters.dropped.fetch_add(1, Ordering::Relaxed);
                self.counters
                    .error(format!("{} was too slow and was dropped", c.peer));
                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        });
    }
}

/// Run one output until the task is aborted.
pub async fn run(output: TakOutput, hub: Arc<Hub>, counters: Arc<Counters>) {
    let ctx = Ctx {
        id: output.id.clone(),
        hub,
        clients: Arc::new(Mutex::new(Vec::new())),
        render: Render::of(&output),
        counters,
    };
    if !output.delivery.encrypted() {
        tracing::warn!(output = %output.id, kind = output.delivery.kind(),
            "TAK output without TLS: anyone on the network path can read the picture");
    }
    tokio::join!(fan_out(ctx.clone()), connections(output.delivery, ctx));
}

async fn fan_out(ctx: Ctx) {
    let mut rx = ctx.hub.subscribe();
    let mut refresh = Refresh::new(ctx.render.refresh_every());
    // Connections get the picture when they connect: due again from now.
    let now = Instant::now();
    for t in ctx.hub.snapshot() {
        if ctx.render.event(&t).is_some() {
            refresh.sent(t.uid, now);
        }
    }
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            change = rx.recv() => match change {
                Ok(Change::Upsert(t)) => match ctx.render.event(&t) {
                    Some(bytes) => {
                        ctx.send_all(|| Some(bytes));
                        refresh.sent(t.uid, Instant::now());
                    }
                    // Its reports stopped long enough ago that TAK has let it go.
                    None => refresh.remove(t.uid),
                },
                Ok(Change::Delete { uid, last_type }) => {
                    ctx.send_all(|| Some(ctx.render.delete(uid, last_type.as_deref())));
                    refresh.remove(uid);
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(output = %ctx.id, missed = n, "TAK output fell behind; sending the picture again");
                    let now = Instant::now();
                    let live = ctx.hub.snapshot();
                    let gone: Vec<Uid> = refresh
                        .tracked()
                        .filter(|u| !live.iter().any(|t| t.uid == *u))
                        .collect();
                    for uid in gone {
                        ctx.send_all(|| Some(ctx.render.delete(uid, None)));
                        refresh.remove(uid);
                    }
                    for t in live {
                        if let Some(bytes) = ctx.render.event(&t) {
                            ctx.send_all(|| Some(bytes));
                            refresh.sent(t.uid, now);
                        }
                    }
                }
                Err(broadcast::error::RecvError::Closed) => return,
            },
            _ = tick.tick() => {
                let now = Instant::now();
                for uid in refresh.take_due(now) {
                    // Re-sent until its stale time; after that it waits for a report.
                    if let Some(bytes) = ctx.hub.get(uid).and_then(|t| ctx.render.event(&t)) {
                        ctx.send_all(|| Some(bytes));
                        refresh.sent(uid, now);
                    }
                }
            }
        }
    }
}

async fn connections(delivery: Delivery, ctx: Ctx) {
    match delivery {
        Delivery::TakServer { host, port, tls } => tak_server(host, port, tls, ctx).await,
        Delivery::Multicast {
            group,
            port,
            ttl,
            interface,
        } => multicast(group, port, ttl, interface, ctx).await,
        Delivery::Listen { bind, tls } => listen(bind, tls, ctx).await,
    }
}

/// Write the picture, then the queue, until either side ends. Anything the
/// peer sends (TAK clients send their own position and pings) is read and
/// dropped.
async fn serve_stream<S>(stream: S, peer: String, ctx: Ctx) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut queue = ctx.register(&peer);
    let _connected = Connected::new(&ctx.counters);
    let (mut rd, mut wr) = tokio::io::split(stream);
    let mut batch: Vec<u8> = Vec::with_capacity(WRITE_BATCH);
    let mut n = 0u64;
    for t in ctx.hub.snapshot() {
        let Some(bytes) = ctx.render.event(&t) else {
            continue;
        };
        batch.extend_from_slice(&bytes);
        n += 1;
        if batch.len() >= WRITE_BATCH {
            write(&mut wr, &mut batch, &mut n, &ctx.counters).await?;
        }
    }
    write(&mut wr, &mut batch, &mut n, &ctx.counters).await?;
    let mut buf = vec![0u8; 8192];
    loop {
        tokio::select! {
            item = queue.recv() => {
                let Some(bytes) = item else {
                    // Dropped for falling behind, or the output stopped.
                    return Ok(());
                };
                batch.extend_from_slice(&bytes);
                n += 1;
                while batch.len() < WRITE_BATCH {
                    match queue.try_recv() {
                        Ok(b) => {
                            batch.extend_from_slice(&b);
                            n += 1;
                        }
                        Err(_) => break,
                    }
                }
                write(&mut wr, &mut batch, &mut n, &ctx.counters).await?;
            }
            read = rd.read(&mut buf) => match read {
                Ok(0) => return Ok(()),
                Ok(_) => {}
                Err(e) => return Err(e.into()),
            }
        }
    }
}

async fn write<W: AsyncWrite + Unpin>(
    w: &mut W,
    batch: &mut Vec<u8>,
    n: &mut u64,
    counters: &Counters,
) -> anyhow::Result<()> {
    if batch.is_empty() {
        return Ok(());
    }
    tokio::time::timeout(WRITE_TIMEOUT, async {
        w.write_all(batch).await?;
        w.flush().await
    })
    .await
    .map_err(|_| anyhow::anyhow!("write timed out"))??;
    counters.sent.fetch_add(*n, Ordering::Relaxed);
    batch.clear();
    *n = 0;
    Ok(())
}

fn next_backoff(b: Duration) -> Duration {
    (b * 2).min(MAX_BACKOFF)
}

/// Keep a connection to a TAK Server's streaming input, sending the picture
/// on every (re)connect.
async fn tak_server(host: String, port: u16, tls: Option<ot_source::tls::ClientTls>, ctx: Ctx) {
    let host = host.trim().to_owned();
    let peer = format!("{host}:{port}");
    let mut backoff = Duration::from_secs(1);
    loop {
        ctx.counters.set_state("connecting");
        let result: anyhow::Result<()> = async {
            let tcp = tokio::time::timeout(
                CONNECT_TIMEOUT,
                tokio::net::TcpStream::connect((host.as_str(), port)),
            )
            .await
            .map_err(|_| anyhow::anyhow!("connecting to {peer} timed out"))??;
            tcp.set_nodelay(true).ok();
            let peer_addr = tcp.peer_addr().map(|a| a.to_string());
            match &tls {
                Some(t) => {
                    let name = match t.server_name()? {
                        Some(n) => n,
                        None => server_name(&host)?,
                    };
                    let connector = tokio_rustls::TlsConnector::from(Arc::new(t.client_config()?));
                    let stream =
                        tokio::time::timeout(HANDSHAKE_TIMEOUT, connector.connect(name, tcp))
                            .await
                            .map_err(|_| {
                                anyhow::anyhow!("TLS handshake with {peer} timed out")
                            })??;
                    connected(&ctx, &peer, &peer_addr, &mut backoff);
                    serve_stream(stream, peer.clone(), ctx.clone()).await
                }
                None => {
                    connected(&ctx, &peer, &peer_addr, &mut backoff);
                    serve_stream(tcp, peer.clone(), ctx.clone()).await
                }
            }
        }
        .await;
        match result {
            Ok(()) => tracing::info!(output = %ctx.id, %peer, "TAK Server connection ended"),
            Err(e) => {
                let text = format!("{e:#}");
                tracing::warn!(output = %ctx.id, %peer, error = %text, ?backoff, "TAK Server connection failed");
                ctx.counters.error(text);
            }
        }
        ctx.counters.set_state("disconnected");
        tokio::time::sleep(backoff).await;
        backoff = next_backoff(backoff);
    }
}

fn server_name(host: &str) -> anyhow::Result<rustls::pki_types::ServerName<'static>> {
    let name = host.trim_start_matches('[').trim_end_matches(']');
    rustls::pki_types::ServerName::try_from(name.to_owned())
        .map_err(|_| anyhow::anyhow!("{host:?} is not a valid TLS server name"))
}

/// `peer` is the configured host:port; `peer_addr` the address the socket
/// actually reached (ASD STIG V-222470).
fn connected(ctx: &Ctx, peer: &str, peer_addr: &std::io::Result<String>, backoff: &mut Duration) {
    let peer_addr = match peer_addr {
        Ok(a) => a.clone(),
        Err(e) => format!("unknown ({e})"),
    };
    tracing::info!(output = %ctx.id, component = "tak_server", %peer, %peer_addr, "connected to TAK Server; sending the picture");
    ctx.counters.set_state("connected");
    *backoff = Duration::from_secs(1);
}

/// The UDP socket a multicast output sends from.
pub fn udp_socket(
    dest: SocketAddr,
    ttl: u32,
    interface: Option<&MulticastInterface>,
) -> anyhow::Result<tokio::net::UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let socket = Socket::new(Domain::for_address(dest), Type::DGRAM, Some(Protocol::UDP))?;
    let local = match dest.ip() {
        IpAddr::V4(ip) => {
            let from = match interface {
                None => None,
                Some(MulticastInterface::Address(a)) => Some(*a),
                Some(other) => bail!("an IPv4 destination needs an interface address, not {other}"),
            };
            if ip.is_multicast() {
                socket.set_multicast_ttl_v4(ttl)?;
                // Listeners on this host (and tests) hear it too.
                socket.set_multicast_loop_v4(true)?;
                if let Some(i) = from {
                    socket.set_multicast_if_v4(&i)?;
                }
            } else {
                socket.set_ttl_v4(ttl)?;
            }
            SocketAddr::V4(SocketAddrV4::new(from.unwrap_or(Ipv4Addr::UNSPECIFIED), 0))
        }
        IpAddr::V6(ip) => {
            if ip.is_multicast() {
                socket.set_multicast_hops_v6(ttl)?;
                socket.set_multicast_loop_v6(true)?;
                // IPv6 picks the outgoing interface by index; a name is
                // looked up now, so a missing one is an output error.
                if let Some(i) = interface {
                    socket.set_multicast_if_v6(i.index()?)?;
                }
            } else {
                socket.set_unicast_hops_v6(ttl)?;
            }
            SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, 0, 0, 0))
        }
    };
    socket.bind(&local.into())?;
    socket.set_nonblocking(true)?;
    Ok(tokio::net::UdpSocket::from_std(socket.into())?)
}

/// One event per datagram to the group, the picture first.
async fn multicast(group: String, port: u16, ttl: u32, interface: Option<String>, ctx: Ctx) {
    let mut backoff = Duration::from_secs(1);
    loop {
        let result: anyhow::Result<()> = async {
            let ip: IpAddr = group.trim().parse()?;
            let iface = interface
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::parse::<MulticastInterface>)
                .transpose()
                .map_err(anyhow::Error::msg)?;
            let dest = SocketAddr::new(ip, port);
            let socket = udp_socket(dest, ttl, iface.as_ref())?;
            tracing::info!(output = %ctx.id, component = "cot_multicast", peer_addr = %dest, "sending to the multicast group");
            let mut queue = ctx.register(&dest.to_string());
            let _connected = Connected::new(&ctx.counters);
            ctx.counters.set_state("sending");
            backoff = Duration::from_secs(1);
            for t in ctx.hub.snapshot() {
                let Some(bytes) = ctx.render.event(&t) else {
                    continue;
                };
                socket.send_to(&bytes, dest).await?;
                ctx.counters.sent.fetch_add(1, Ordering::Relaxed);
            }
            while let Some(bytes) = queue.recv().await {
                socket.send_to(&bytes, dest).await?;
                ctx.counters.sent.fetch_add(1, Ordering::Relaxed);
            }
            Ok(())
        }
        .await;
        if let Err(e) = result {
            let text = format!("{e:#}");
            tracing::warn!(output = %ctx.id, error = %text, "TAK multicast failed");
            ctx.counters.error(text);
            ctx.counters.set_state("error");
        }
        tokio::time::sleep(backoff).await;
        backoff = next_backoff(backoff);
    }
}

/// Accept TAK clients: each gets the picture, then the stream.
async fn listen(bind: String, tls: Option<ot_source::tls::ServerTls>, ctx: Ctx) {
    let mut backoff = Duration::from_secs(1);
    loop {
        let result: anyhow::Result<()> = async {
            let acceptor = tls.as_ref().map(|t| t.acceptor()).transpose()?;
            let listener = tokio::net::TcpListener::bind(bind.trim()).await?;
            tracing::info!(output = %ctx.id, addr = %bind, tls = acceptor.is_some(), "listening for TAK clients");
            ctx.counters.set_state("listening");
            backoff = Duration::from_secs(1);
            accept_loop(listener, tls.clone(), acceptor, ctx.clone()).await
        }
        .await;
        if let Err(e) = result {
            let text = format!("{e:#}");
            tracing::warn!(output = %ctx.id, addr = %bind, error = %text, "TAK listener failed");
            ctx.counters.error(text);
            ctx.counters.set_state("error");
        }
        tokio::time::sleep(backoff).await;
        backoff = next_backoff(backoff);
    }
}

async fn accept_loop(
    listener: tokio::net::TcpListener,
    tls: Option<ot_source::tls::ServerTls>,
    mut acceptor: Option<tokio_rustls::TlsAcceptor>,
    ctx: Ctx,
) -> anyhow::Result<()> {
    // Dropped with the output: every client ends with it.
    let mut clients = JoinSet::new();
    let mut stamp = tls.as_ref().map(|t| t.crl_stamp());
    let mut crl_check = tokio::time::interval(Duration::from_secs(60));
    crl_check.tick().await;
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (tcp, peer) = match accepted {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!(output = %ctx.id, error = %e, "accept failed");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                };
                tcp.set_nodelay(true).ok();
                let (ctx, acceptor) = (ctx.clone(), acceptor.clone());
                clients.spawn(async move {
                    let peer = peer.to_string();
                    let result = match acceptor {
                        Some(a) => match tokio::time::timeout(HANDSHAKE_TIMEOUT, a.accept(tcp)).await {
                            Ok(Ok(s)) => {
                                let who = s
                                    .get_ref()
                                    .1
                                    .peer_certificates()
                                    .and_then(|c| c.first())
                                    .and_then(|c| ot_source::tls::subject(c));
                                tracing::info!(output = %ctx.id, %peer, client = who.as_deref().unwrap_or("-"),
                                    "TAK client connected; sending the picture");
                                serve_stream(s, peer.clone(), ctx.clone()).await
                            }
                            Ok(Err(e)) => Err(anyhow::anyhow!("TLS handshake with {peer}: {e}")),
                            Err(_) => Err(anyhow::anyhow!("TLS handshake with {peer} timed out")),
                        },
                        None => {
                            tracing::info!(output = %ctx.id, %peer, "TAK client connected; sending the picture");
                            serve_stream(tcp, peer.clone(), ctx.clone()).await
                        }
                    };
                    match result {
                        Ok(()) => tracing::info!(output = %ctx.id, %peer, "TAK client left"),
                        Err(e) => {
                            let text = format!("{e:#}");
                            tracing::warn!(output = %ctx.id, %peer, error = %text, "TAK client connection failed");
                            ctx.counters.error(text);
                        }
                    }
                });
            }
            Some(_) = clients.join_next(), if !clients.is_empty() => {}
            _ = crl_check.tick() => {
                let (Some(t), Some(old)) = (&tls, &mut stamp) else { continue };
                let now = t.crl_stamp();
                if now != *old {
                    *old = now;
                    match t.acceptor() {
                        Ok(a) => {
                            acceptor = Some(a);
                            tracing::info!(output = %ctx.id, "client certificate revocation lists reloaded");
                        }
                        Err(e) => {
                            let text = format!("client CRLs changed but did not load; the previous ones stay: {e:#}");
                            tracing::error!(output = %ctx.id, error = %text);
                            ctx.counters.error(text);
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cot::event::tests::uid;

    #[test]
    fn refresh_resends_each_track_at_its_own_time() {
        let t0 = Instant::now();
        let mut r = Refresh::new(Duration::from_secs(30));
        r.sent(uid(1), t0);
        r.sent(uid(2), t0 + Duration::from_secs(10));
        assert!(r.take_due(t0 + Duration::from_secs(29)).is_empty());
        assert_eq!(r.take_due(t0 + Duration::from_secs(30)), vec![uid(1)]);
        // Until it is sent again it is not due twice.
        assert!(r.take_due(t0 + Duration::from_secs(31)).is_empty());
        r.sent(uid(1), t0 + Duration::from_secs(31));
        // A new update pushes the next refresh back.
        r.sent(uid(2), t0 + Duration::from_secs(35));
        assert!(r.take_due(t0 + Duration::from_secs(40)).is_empty());
        r.remove(uid(1));
        assert_eq!(r.take_due(t0 + Duration::from_secs(65)), vec![uid(2)]);
        assert_eq!(r.len(), 0);
    }

    #[test]
    fn refresh_is_half_the_stale_time() {
        let r = |s: f64| {
            Render {
                stale: Duration::from_secs_f64(s),
                remarks: true,
            }
            .refresh_every()
        };
        assert_eq!(r(60.0), Duration::from_secs(30));
        assert_eq!(r(1.0), Duration::from_secs(1));
    }

    #[test]
    fn a_slow_connection_is_dropped_without_holding_up_the_others() {
        let ctx = Ctx {
            id: "eud".into(),
            hub: Arc::new(Hub::default()),
            clients: Arc::new(Mutex::new(Vec::new())),
            render: Render {
                stale: Duration::from_secs(60),
                remarks: false,
            },
            counters: Arc::new(Counters::default()),
        };
        let _slow = ctx.register("slow");
        let mut fast = ctx.register("fast");
        for i in 0..=CLIENT_QUEUE {
            ctx.send_all(|| Some(Bytes::from(format!("{i}"))));
            // The fast one keeps up.
            assert!(fast.try_recv().is_ok());
        }
        assert_eq!(ctx.counters.dropped.load(Ordering::Relaxed), 1);
        assert_eq!(ctx.counters.errors.load(Ordering::Relaxed), 1);
        let peers: Vec<String> = ctx
            .clients
            .lock()
            .unwrap()
            .iter()
            .map(|c| c.peer.clone())
            .collect();
        assert_eq!(peers, ["fast"]);
        assert!(ctx.counters.state().1.unwrap().contains("slow"));
    }

    #[test]
    fn the_hub_broadcasts_changes_and_deletes_only_what_it_held() {
        let hub = Hub::default();
        let mut rx = hub.subscribe();
        let t = CotTrack::of(&crate::cot::event::tests::track(1, serde_json::json!({})));
        hub.upsert(t.clone());
        assert_eq!(hub.len(), 1);
        assert!(matches!(rx.try_recv(), Ok(Change::Upsert(u)) if u.uid == t.uid));
        assert!(!hub.delete(uid(9)));
        assert!(hub.delete(t.uid));
        match rx.try_recv() {
            Ok(Change::Delete { uid: u, last_type }) => {
                assert_eq!((u, last_type.as_deref()), (t.uid, Some("a-u-G")));
            }
            other => panic!("{other:?}"),
        }
        assert!(rx.try_recv().is_err());
        assert!(hub.snapshot().is_empty());
    }
}
