//! gRPC for protobuf sources, on HTTP/2 (hyper), with no generated code:
//! the producer's `.proto` (see [`crate::proto`]) says what the methods and
//! messages are.
//!
//! * [`subscribe`]: OpenTrack calls a producer's streaming method and reads
//!   the messages it streams back.
//! * [`serve`]: OpenTrack accepts calls producers make to any method of
//!   their `.proto` it is told to accept, and reads the messages they send.
//!
//! On the wire a gRPC message is a 5-byte prefix (a compressed flag and a
//! big-endian length) and the message; the call's outcome is the
//! `grpc-status` trailer. Both sides take gzip.
//!
//! Hardening: a size limit on every message (compressed and not), TLS with
//! mutual certificates, a bearer token for producers, keepalive pings,
//! validation of every message pushed against the schema, and backpressure:
//! when the pipeline falls behind, OpenTrack stops reading and HTTP/2 flow
//! control slows the producer down; nothing is dropped.

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::io::Read;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::{Buf, Bytes, BytesMut};
use http::{HeaderMap, HeaderValue, Request, Response, StatusCode};
use http_body_util::{BodyExt, Full, StreamBody};
use hyper::body::{Frame as BodyFrame, Incoming};
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use tokio::sync::{Semaphore, mpsc};

use crate::frame::Frame;
use crate::proto::ProtoSet;

/// Largest message accepted, by default (gRPC's own default).
pub const DEFAULT_MAX_MESSAGE: usize = 4 << 20;

/// gRPC status codes OpenTrack uses.
pub mod code {
    pub const OK: u32 = 0;
    pub const INVALID_ARGUMENT: u32 = 3;
    pub const RESOURCE_EXHAUSTED: u32 = 8;
    pub const UNIMPLEMENTED: u32 = 12;
    pub const INTERNAL: u32 = 13;
    pub const UNAVAILABLE: u32 = 14;
    pub const UNAUTHENTICATED: u32 = 16;
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum GrpcError {
    #[error("gRPC status {code}: {message}")]
    Status { code: u32, message: String },
    #[error("a {size}-byte message is over the {max}-byte limit")]
    TooLarge { size: usize, max: usize },
    #[error("{0}")]
    Protocol(String),
}

impl GrpcError {
    fn code(&self) -> u32 {
        match self {
            GrpcError::Status { code, .. } => *code,
            GrpcError::TooLarge { .. } => code::RESOURCE_EXHAUSTED,
            GrpcError::Protocol(_) => code::INTERNAL,
        }
    }
}

/// One message with its 5-byte prefix, uncompressed.
pub fn frame_message(message: &[u8]) -> Bytes {
    let mut b = BytesMut::with_capacity(5 + message.len());
    b.extend_from_slice(&[0]);
    b.extend_from_slice(&(message.len() as u32).to_be_bytes());
    b.extend_from_slice(message);
    b.freeze()
}

/// Splits a body's bytes into messages as they arrive.
#[derive(Debug)]
pub struct Deframer {
    buf: BytesMut,
    max: usize,
    /// The call's `grpc-encoding` is gzip.
    gzip: bool,
}

impl Deframer {
    pub fn new(max: usize, gzip: bool) -> Self {
        Self {
            buf: BytesMut::new(),
            max,
            gzip,
        }
    }

    pub fn push(&mut self, chunk: &[u8]) {
        self.buf.extend_from_slice(chunk);
    }

    /// The next whole message, if one has arrived.
    pub fn next_message(&mut self) -> Result<Option<Vec<u8>>, GrpcError> {
        if self.buf.len() < 5 {
            return Ok(None);
        }
        let compressed = self.buf[0];
        let len = u32::from_be_bytes([self.buf[1], self.buf[2], self.buf[3], self.buf[4]]) as usize;
        if len > self.max {
            return Err(GrpcError::TooLarge {
                size: len,
                max: self.max,
            });
        }
        if self.buf.len() < 5 + len {
            return Ok(None);
        }
        self.buf.advance(5);
        let body = self.buf.split_to(len);
        match compressed {
            0 => Ok(Some(body.to_vec())),
            1 if self.gzip => {
                // Read one byte past the limit to tell "exactly at" from "over".
                let mut out = Vec::new();
                flate2::read::GzDecoder::new(&body[..])
                    .take(self.max as u64 + 1)
                    .read_to_end(&mut out)
                    .map_err(|e| GrpcError::Protocol(format!("gzip: {e}")))?;
                if out.len() > self.max {
                    return Err(GrpcError::TooLarge {
                        size: out.len(),
                        max: self.max,
                    });
                }
                Ok(Some(out))
            }
            1 => Err(GrpcError::Protocol(
                "a compressed message without a grpc-encoding OpenTrack reads (gzip)".into(),
            )),
            f => Err(GrpcError::Protocol(format!("bad message flag {f}"))),
        }
    }

    /// Bytes of an unfinished message left over.
    pub fn pending(&self) -> usize {
        self.buf.len()
    }
}

fn gzip_encoded(headers: &HeaderMap) -> Result<bool, GrpcError> {
    match headers.get("grpc-encoding").and_then(|v| v.to_str().ok()) {
        None | Some("identity") => Ok(false),
        Some("gzip") => Ok(true),
        Some(other) => Err(GrpcError::Status {
            code: code::UNIMPLEMENTED,
            message: format!("grpc-encoding {other} is not supported (gzip, identity)"),
        }),
    }
}

fn status_of(headers: &HeaderMap) -> Option<(u32, String)> {
    let code = headers.get("grpc-status")?.to_str().ok()?.parse().ok()?;
    let message = headers
        .get("grpc-message")
        .and_then(|v| v.to_str().ok())
        .map(percent_decode)
        .unwrap_or_default();
    Some((code, message))
}

/// `grpc-message` is percent-encoded.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn percent_encode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if (0x20..0x7f).contains(&b) && b != b'%' {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

// --- OpenTrack calls the producer --------------------------------------------

/// A call to a producer's streaming method.
pub struct Subscription {
    /// `http://host:port` or `https://host:port`.
    pub url: String,
    /// `/package.Service/Method`.
    pub path: String,
    /// The request message, encoded.
    pub request: Vec<u8>,
    /// Extra request headers (gRPC metadata), e.g. `authorization`.
    pub metadata: BTreeMap<String, String>,
    /// TLS for `https://` (ALPN is set to h2 here).
    pub tls: Option<(rustls::ClientConfig, rustls::pki_types::ServerName<'static>)>,
    pub max_message: usize,
    /// HTTP/2 keepalive ping interval (none: no pings).
    pub keepalive: Option<Duration>,
    pub connect_timeout: Duration,
}

/// How a subscription ended cleanly: the producer closed the stream.
#[derive(Debug, Clone, PartialEq)]
pub struct Ended {
    pub messages: u64,
}

/// Call the method and hand each message it streams back to `on_message`
/// (`on_open` once the producer has accepted the call). Returns when the
/// producer ends the call: `Ok` with status OK, an error otherwise (the
/// caller reconnects).
pub async fn subscribe<F, Fut>(
    call: &Subscription,
    on_open: impl FnOnce(),
    mut on_message: F,
) -> anyhow::Result<Ended>
where
    F: FnMut(Vec<u8>) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    let uri: http::Uri = format!("{}{}", call.url.trim_end_matches('/'), call.path).parse()?;
    let host = uri
        .host()
        .ok_or_else(|| anyhow::anyhow!("gRPC url {} has no host", call.url))?
        .to_owned();
    let https = uri.scheme_str() == Some("https");
    let port = uri.port_u16().unwrap_or(if https { 443 } else { 80 });
    let tcp = tokio::time::timeout(
        call.connect_timeout,
        tokio::net::TcpStream::connect((host.as_str(), port)),
    )
    .await
    .map_err(|_| anyhow::anyhow!("connecting to {host}:{port} timed out"))??;
    match tcp.peer_addr() {
        Ok(peer) => {
            tracing::info!(component = "grpc", destination = %format!("{host}:{port}"), peer_addr = %peer, "connected")
        }
        Err(e) => {
            tracing::info!(component = "grpc", destination = %format!("{host}:{port}"), error = %e, "connected; peer address unknown")
        }
    }
    tcp.set_nodelay(true)?;
    let mut builder = hyper::client::conn::http2::Builder::new(TokioExecutor::new());
    builder.timer(TokioTimer::new());
    if let Some(k) = call.keepalive {
        builder
            .keep_alive_interval(k)
            .keep_alive_timeout(k.max(Duration::from_secs(10)))
            .keep_alive_while_idle(true);
    }
    // The body carries the request; an empty body type for simplicity.
    let request = frame_message(&call.request);
    let mut req = Request::post(uri.clone())
        .header("content-type", "application/grpc")
        .header("te", "trailers")
        .header("grpc-accept-encoding", "gzip,identity")
        .header(
            "user-agent",
            concat!("opentrack/", env!("CARGO_PKG_VERSION")),
        );
    for (k, v) in &call.metadata {
        req = req.header(k.as_str(), v.as_str());
    }
    let req = req.body(Full::new(request))?;
    let response = match &call.tls {
        Some((config, name)) => {
            if !https {
                anyhow::bail!("TLS settings need an https:// gRPC url");
            }
            let mut config = config.clone();
            config.alpn_protocols = vec![b"h2".to_vec()];
            let stream = tokio_rustls::TlsConnector::from(Arc::new(config))
                .connect(name.clone(), tcp)
                .await
                .map_err(|e| anyhow::anyhow!("TLS handshake with {host}:{port}: {e}"))?;
            let (mut sender, conn) = builder.handshake(TokioIo::new(stream)).await?;
            tokio::spawn(async move {
                if let Err(e) = conn.await {
                    tracing::debug!(error = %e, "gRPC connection ended");
                }
            });
            sender.send_request(req).await?
        }
        None => {
            if https {
                // Default trust: the system roots.
                let config = crate::tls::ClientTls::default().client_config()?;
                let name = crate::tls::server_name(&host)?;
                let mut config = config;
                config.alpn_protocols = vec![b"h2".to_vec()];
                let stream = tokio_rustls::TlsConnector::from(Arc::new(config))
                    .connect(name, tcp)
                    .await
                    .map_err(|e| anyhow::anyhow!("TLS handshake with {host}:{port}: {e}"))?;
                let (mut sender, conn) = builder.handshake(TokioIo::new(stream)).await?;
                tokio::spawn(async move {
                    let _ = conn.await;
                });
                sender.send_request(req).await?
            } else {
                let (mut sender, conn) = builder.handshake(TokioIo::new(tcp)).await?;
                tokio::spawn(async move {
                    let _ = conn.await;
                });
                sender.send_request(req).await?
            }
        }
    };
    read_response(response, call.max_message, on_open, &mut on_message).await
}

async fn read_response<F, Fut>(
    response: Response<Incoming>,
    max: usize,
    on_open: impl FnOnce(),
    on_message: &mut F,
) -> anyhow::Result<Ended>
where
    F: FnMut(Vec<u8>) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    let (parts, mut body) = response.into_parts();
    if parts.status != StatusCode::OK {
        anyhow::bail!("gRPC call answered HTTP {}", parts.status);
    }
    // Trailers-only: the call failed before any message.
    if let Some((code, message)) = status_of(&parts.headers) {
        if code != code::OK {
            return Err(GrpcError::Status { code, message }.into());
        }
        return Ok(Ended { messages: 0 });
    }
    let ct = parts
        .headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !ct.starts_with("application/grpc") {
        anyhow::bail!("not a gRPC response (content-type {ct:?})");
    }
    let mut deframer = Deframer::new(max, gzip_encoded(&parts.headers)?);
    on_open();
    let mut messages = 0u64;
    while let Some(frame) = body.frame().await {
        let frame = frame?;
        if let Some(data) = frame.data_ref() {
            deframer.push(data);
            while let Some(m) = deframer.next_message()? {
                messages += 1;
                on_message(m).await?;
            }
        } else if let Some(trailers) = frame.trailers_ref() {
            return match status_of(trailers) {
                Some((code::OK, _)) => Ok(Ended { messages }),
                Some((code, message)) => Err(GrpcError::Status { code, message }.into()),
                None => Err(GrpcError::Protocol("gRPC trailers without grpc-status".into()).into()),
            };
        }
    }
    Err(GrpcError::Protocol("the stream ended without a gRPC status".into()).into())
}

// --- Producers call OpenTrack ------------------------------------------------

/// A method producers may call, and the message type they send.
#[derive(Clone)]
pub struct Accepted {
    pub input: prost_reflect::MessageDescriptor,
}

/// What the server accepts.
pub struct Service {
    pub schema: Arc<ProtoSet>,
    /// `/package.Service/Method` → what to expect.
    pub methods: BTreeMap<String, Accepted>,
    /// `authorization: Bearer <token>` producers must send, if set.
    pub token: Option<String>,
    pub max_message: usize,
    pub max_connections: usize,
    pub keepalive: Option<Duration>,
    /// Told of every call refused or failed (`peer: why`), so the source's
    /// status shows a producer in trouble.
    pub on_problem: Option<Arc<dyn Fn(String) + Send + Sync>>,
}

impl Service {
    fn problem(&self, peer: &Peer, what: &str) {
        tracing::debug!(peer = %peer.addr, %what, "gRPC call refused");
        if let Some(f) = &self.on_problem {
            f(format!(
                "{}: {what}",
                peer.subject
                    .as_deref()
                    .map_or_else(|| peer.addr.to_string(), str::to_owned)
            ));
        }
    }
}

/// A connection's peer, as frames record it.
#[derive(Debug, Clone)]
pub struct Peer {
    pub addr: SocketAddr,
    /// The client certificate's subject (mutual TLS).
    pub subject: Option<String>,
}

type RespBody = StreamBody<
    futures_util::stream::Iter<std::vec::IntoIter<Result<BodyFrame<Bytes>, Infallible>>>,
>;

fn reply(code: u32, message: &str, body: Option<Bytes>) -> Response<RespBody> {
    let mut trailers = HeaderMap::new();
    trailers.insert("grpc-status", HeaderValue::from(code));
    if !message.is_empty()
        && let Ok(v) = HeaderValue::from_str(&percent_encode(message))
    {
        trailers.insert("grpc-message", v);
    }
    let mut frames = Vec::new();
    if let Some(b) = body {
        frames.push(Ok(BodyFrame::data(b)));
    }
    frames.push(Ok(BodyFrame::trailers(trailers)));
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/grpc")
        .header("grpc-accept-encoding", "gzip,identity")
        .body(StreamBody::new(futures_util::stream::iter(frames)))
        .expect("a valid response")
}

/// Handle one call: check it, read its messages into `tx` (waiting when the
/// pipeline is behind), and answer.
async fn handle(
    req: Request<Incoming>,
    service: Arc<Service>,
    peer: Peer,
    tx: mpsc::Sender<Frame>,
    counts: Arc<ServeCounts>,
) -> Result<Response<RespBody>, Infallible> {
    use std::sync::atomic::Ordering::Relaxed;
    let path = req.uri().path().to_owned();
    let fail = |code: u32, what: String| {
        service.problem(&peer, &what);
        Ok(reply(code, &what, None))
    };
    let Some(accepted) = service.methods.get(&path).cloned() else {
        counts.rejected.fetch_add(1, Relaxed);
        return fail(code::UNIMPLEMENTED, format!("{path} is not accepted here"));
    };
    let ct = req
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if req.method() != http::Method::POST || !ct.starts_with("application/grpc") {
        counts.rejected.fetch_add(1, Relaxed);
        return fail(code::INTERNAL, "not a gRPC call".into());
    }
    if let Some(token) = &service.token {
        let given = req
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));
        if !given.is_some_and(|g| constant_time_eq(g.as_bytes(), token.as_bytes())) {
            counts.rejected.fetch_add(1, Relaxed);
            return fail(
                code::UNAUTHENTICATED,
                "a valid bearer token is required".into(),
            );
        }
    }
    let gzip = match gzip_encoded(req.headers()) {
        Ok(g) => g,
        Err(e) => return fail(e.code(), e.to_string()),
    };
    let mut deframer = Deframer::new(service.max_message, gzip);
    let mut body = req.into_body();
    let origin = peer.addr.to_string();
    let mut received = 0u64;
    loop {
        let frame = match body.frame().await {
            None => break,
            Some(Ok(f)) => f,
            Some(Err(e)) => {
                counts.failed.fetch_add(1, Relaxed);
                return fail(code::UNAVAILABLE, format!("reading the call: {e}"));
            }
        };
        let Some(data) = frame.data_ref() else {
            continue;
        };
        deframer.push(data);
        loop {
            let message = match deframer.next_message() {
                Ok(Some(m)) => m,
                Ok(None) => break,
                Err(e) => {
                    counts.failed.fetch_add(1, Relaxed);
                    return fail(e.code(), e.to_string());
                }
            };
            // Tell the producer now, not by a silent drop later.
            if let Err(e) =
                prost_reflect::DynamicMessage::decode(accepted.input.clone(), &message[..])
            {
                counts.failed.fetch_add(1, Relaxed);
                return fail(
                    code::INVALID_ARGUMENT,
                    format!(
                        "message {} is not a valid {}: {e}",
                        received + 1,
                        accepted.input.full_name()
                    ),
                );
            }
            let mut f = Frame::new(message);
            f.origin = Some(origin.clone());
            f.meta
                .insert("method".into(), serde_json::Value::String(path.clone()));
            if let Some(s) = &peer.subject {
                f.meta
                    .insert("peer_subject".into(), serde_json::Value::String(s.clone()));
            }
            // Waits while the pipeline is behind: the producer is slowed by
            // HTTP/2 flow control, and nothing is dropped.
            if tx.send(f).await.is_err() {
                return Ok(reply(code::UNAVAILABLE, "the source is stopping", None));
            }
            received += 1;
            counts.messages.fetch_add(1, Relaxed);
        }
    }
    if deframer.pending() > 0 {
        counts.failed.fetch_add(1, Relaxed);
        return Ok(reply(
            code::INTERNAL,
            "the call ended inside a message",
            None,
        ));
    }
    counts.calls.fetch_add(1, Relaxed);
    // The response: the method's output message left at its defaults (an
    // empty message, whatever its type).
    Ok(reply(code::OK, "", Some(frame_message(&[]))))
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Counters for the source's status.
#[derive(Debug, Default)]
pub struct ServeCounts {
    pub calls: std::sync::atomic::AtomicU64,
    pub messages: std::sync::atomic::AtomicU64,
    pub rejected: std::sync::atomic::AtomicU64,
    pub failed: std::sync::atomic::AtomicU64,
    pub connections_refused: std::sync::atomic::AtomicU64,
}

/// Accept producers' calls on `listener` until it fails; each message they
/// send becomes a frame on `tx`.
pub async fn serve(
    listener: tokio::net::TcpListener,
    acceptor: Option<tokio_rustls::TlsAcceptor>,
    service: Arc<Service>,
    tx: mpsc::Sender<Frame>,
    counts: Arc<ServeCounts>,
) -> anyhow::Result<()> {
    let slots = Arc::new(Semaphore::new(service.max_connections.max(1)));
    loop {
        let (stream, addr) = listener.accept().await?;
        let Ok(permit) = slots.clone().try_acquire_owned() else {
            counts
                .connections_refused
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            tracing::warn!(%addr, "gRPC connection refused: too many connections");
            drop(stream);
            continue;
        };
        let (service, tx, counts, acceptor) = (
            service.clone(),
            tx.clone(),
            counts.clone(),
            acceptor.clone(),
        );
        tokio::spawn(async move {
            let _permit = permit;
            let _ = stream.set_nodelay(true);
            let mut http = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new());
            let mut h2 = http.http2();
            h2.timer(TokioTimer::new());
            if let Some(k) = service.keepalive {
                h2.keep_alive_interval(k)
                    .keep_alive_timeout(k.max(Duration::from_secs(10)));
            }
            let http = http.http2_only();
            let result = match acceptor {
                Some(acceptor) => match acceptor.accept(stream).await {
                    Ok(tls) => {
                        let subject = tls
                            .get_ref()
                            .1
                            .peer_certificates()
                            .and_then(|c| c.first())
                            .and_then(|c| crate::tls::subject(c.as_ref()));
                        let peer = Peer { addr, subject };
                        http.serve_connection(
                            TokioIo::new(tls),
                            hyper::service::service_fn(move |req| {
                                handle(
                                    req,
                                    service.clone(),
                                    peer.clone(),
                                    tx.clone(),
                                    counts.clone(),
                                )
                            }),
                        )
                        .await
                    }
                    Err(e) => {
                        tracing::warn!(%addr, error = %e, "gRPC TLS client rejected");
                        service.problem(
                            &Peer {
                                addr,
                                subject: None,
                            },
                            &format!("TLS: {e}"),
                        );
                        return;
                    }
                },
                None => {
                    let peer = Peer {
                        addr,
                        subject: None,
                    };
                    http.serve_connection(
                        TokioIo::new(stream),
                        hyper::service::service_fn(move |req| {
                            handle(
                                req,
                                service.clone(),
                                peer.clone(),
                                tx.clone(),
                                counts.clone(),
                            )
                        }),
                    )
                    .await
                }
            };
            if let Err(e) = result {
                tracing::debug!(%addr, error = %e, "gRPC connection ended");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::proto::tests::files;

    static PROBLEMS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

    #[test]
    fn messages_are_found_in_any_split_of_the_bytes() {
        let a = frame_message(b"hello");
        let b = frame_message(b"");
        let all: Vec<u8> = [a.as_ref(), b.as_ref()].concat();
        let mut d = Deframer::new(100, false);
        let mut got = Vec::new();
        for byte in &all {
            d.push(std::slice::from_ref(byte));
            while let Some(m) = d.next_message().unwrap() {
                got.push(m);
            }
        }
        assert_eq!(got, vec![b"hello".to_vec(), Vec::new()]);
        assert_eq!(d.pending(), 0);
    }

    #[test]
    fn oversize_and_bombs_are_refused() {
        let mut d = Deframer::new(4, false);
        d.push(&frame_message(b"12345"));
        assert!(matches!(
            d.next_message(),
            Err(GrpcError::TooLarge { size: 5, max: 4 })
        ));
        // 1 MB of zeros, gzipped to ~1 KB, against a 1 KB limit.
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
        std::io::Write::write_all(&mut gz, &vec![0u8; 1 << 20]).unwrap();
        let small = gz.finish().unwrap();
        let mut framed = vec![1];
        framed.extend_from_slice(&(small.len() as u32).to_be_bytes());
        framed.extend_from_slice(&small);
        let mut d = Deframer::new(4096, true);
        d.push(&framed);
        assert!(matches!(d.next_message(), Err(GrpcError::TooLarge { .. })));
        // Compressed without the call saying gzip.
        let mut d = Deframer::new(1 << 21, false);
        d.push(&framed);
        assert!(matches!(d.next_message(), Err(GrpcError::Protocol(_))));
    }

    #[test]
    fn status_messages_survive_percent_encoding() {
        let s = "message 3 is not a valid acme.Track: 100% wrong\nsorry";
        assert_eq!(percent_decode(&percent_encode(s)), s);
    }

    async fn server(
        token: Option<&str>,
    ) -> (
        SocketAddr,
        mpsc::Receiver<Frame>,
        Arc<ServeCounts>,
        Arc<ProtoSet>,
    ) {
        let schema = ProtoSet::compile(&files()).unwrap();
        let m = schema.method("acme.tracks.v1.TrackFeed/Push").unwrap();
        let service = Arc::new(Service {
            methods: BTreeMap::from([(
                format!("/{}", m.name),
                Accepted {
                    input: schema.message(&m.input).unwrap(),
                },
            )]),
            schema: schema.clone(),
            token: token.map(str::to_owned),
            max_message: 1 << 16,
            max_connections: 4,
            keepalive: None,
            on_problem: Some(Arc::new(|p: String| PROBLEMS.lock().unwrap().push(p))),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        // A small channel, so backpressure is exercised.
        let (tx, rx) = mpsc::channel(2);
        let counts = Arc::new(ServeCounts::default());
        tokio::spawn(serve(listener, None, service, tx, counts.clone()));
        (addr, rx, counts, schema)
    }

    /// A producer pushing a stream of batches, as a raw HTTP/2 client.
    async fn push(
        addr: SocketAddr,
        path: &str,
        bodies: Vec<Vec<u8>>,
        token: Option<&str>,
    ) -> (u32, String) {
        let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
        let (mut sender, conn) = hyper::client::conn::http2::Builder::new(TokioExecutor::new())
            .handshake(TokioIo::new(tcp))
            .await
            .unwrap();
        tokio::spawn(conn);
        let body: Vec<u8> = bodies
            .iter()
            .flat_map(|b| frame_message(b).to_vec())
            .collect();
        let mut req = Request::post(format!("http://{addr}{path}"))
            .header("content-type", "application/grpc")
            .header("te", "trailers");
        if let Some(t) = token {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        let resp = sender
            .send_request(req.body(Full::new(Bytes::from(body))).unwrap())
            .await
            .unwrap();
        let (_, mut b) = resp.into_parts();
        while let Some(f) = b.frame().await {
            if let Some(t) = f.unwrap().trailers_ref() {
                return status_of(t).unwrap();
            }
        }
        panic!("no trailers");
    }

    #[tokio::test]
    async fn producers_push_batches_and_bad_ones_are_told_why() {
        let (addr, mut rx, counts, schema) = server(Some("s3cret")).await;
        let batch = schema.message("acme.tracks.v1.TrackBatch").unwrap();
        let one = |id: &str| {
            schema
                .encode(&batch, &json!({"tracks": [{"track_id": id}]}))
                .unwrap()
        };
        // Three batches through a channel of two: the server waits for the
        // pipeline instead of dropping.
        let reader = tokio::spawn(async move {
            let mut got = Vec::new();
            for _ in 0..3 {
                tokio::time::sleep(Duration::from_millis(20)).await;
                let f = rx.recv().await.unwrap();
                assert_eq!(f.meta["method"], "/acme.tracks.v1.TrackFeed/Push");
                got.push(f.bytes);
            }
            (got, rx)
        });
        let (code, msg) = push(
            addr,
            "/acme.tracks.v1.TrackFeed/Push",
            vec![one("A"), one("B"), one("C")],
            Some("s3cret"),
        )
        .await;
        assert_eq!((code, msg.as_str()), (code::OK, ""));
        let (got, _rx) = reader.await.unwrap();
        assert_eq!(got.len(), 3);
        assert_eq!(
            schema.decode(&batch, &got[2]).unwrap()["tracks"][0]["track_id"],
            "C"
        );

        let (code, _) = push(
            addr,
            "/acme.tracks.v1.TrackFeed/Push",
            vec![one("A")],
            Some("wrong"),
        )
        .await;
        assert_eq!(code, code::UNAUTHENTICATED);
        let (code, msg) = push(
            addr,
            "/acme.tracks.v1.TrackFeed/Nope",
            vec![one("A")],
            Some("s3cret"),
        )
        .await;
        assert_eq!(code, code::UNIMPLEMENTED, "{msg}");
        let (code, msg) = push(
            addr,
            "/acme.tracks.v1.TrackFeed/Push",
            vec![vec![0x0a, 0xff]],
            Some("s3cret"),
        )
        .await;
        assert_eq!(code, code::INVALID_ARGUMENT);
        assert!(msg.contains("acme.tracks.v1.TrackBatch"), "{msg}");
        let (code, _) = push(
            addr,
            "/acme.tracks.v1.TrackFeed/Push",
            vec![vec![0; 1 << 17]],
            Some("s3cret"),
        )
        .await;
        assert_eq!(code, code::RESOURCE_EXHAUSTED);
        use std::sync::atomic::Ordering::Relaxed;
        assert_eq!(counts.messages.load(Relaxed), 3);
        assert_eq!(counts.rejected.load(Relaxed), 2);
        assert_eq!(counts.failed.load(Relaxed), 2);
        let problems = PROBLEMS.lock().unwrap().clone();
        assert_eq!(problems.len(), 4, "{problems:?}");
        assert!(
            problems[0].ends_with(": a valid bearer token is required"),
            "{problems:?}"
        );
    }

    /// A producer that streams `n` batches to whoever calls
    /// `TrackFeed/Stream` with the right token, then ends the call OK (or
    /// fails every call with `fail`).
    async fn producer(n: usize, fail: Option<(u32, &'static str)>) -> SocketAddr {
        let schema = ProtoSet::compile(&files()).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let schema = schema.clone();
                tokio::spawn(async move {
                    let svc = hyper::service::service_fn(move |req: Request<Incoming>| {
                        let schema = schema.clone();
                        async move {
                            assert_eq!(req.uri().path(), "/acme.tracks.v1.TrackFeed/Stream");
                            let authorised = req
                                .headers()
                                .get("authorization")
                                .and_then(|v| v.to_str().ok())
                                == Some("Bearer t0ken");
                            let sub = schema.message("acme.tracks.v1.Subscribe").unwrap();
                            let mut body = req.into_body();
                            let mut d = Deframer::new(1 << 16, false);
                            while let Some(f) = body.frame().await {
                                if let Some(data) = f.unwrap().data_ref() {
                                    d.push(data);
                                }
                            }
                            let request = schema
                                .decode(&sub, &d.next_message().unwrap().unwrap())
                                .unwrap();
                            if let Some((code, msg)) = fail {
                                return Ok::<_, Infallible>(reply(code, msg, None));
                            }
                            if !authorised {
                                return Ok(reply(code::UNAUTHENTICATED, "no token", None));
                            }
                            let batch = schema.message("acme.tracks.v1.TrackBatch").unwrap();
                            let area = request["area"].as_str().unwrap().to_owned();
                            let mut all = Vec::new();
                            for i in 0..n {
                                let m = schema
                                    .encode(
                                        &batch,
                                        &json!({"tracks": [{"track_id": format!("{area}-{i}")}]}),
                                    )
                                    .unwrap();
                                all.extend_from_slice(&frame_message(&m));
                            }
                            Ok(reply(code::OK, "", Some(Bytes::from(all))))
                        }
                    });
                    let _ = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
                        .http2_only()
                        .serve_connection(TokioIo::new(stream), svc)
                        .await;
                });
            }
        });
        addr
    }

    fn call(addr: SocketAddr, token: &str) -> Subscription {
        let schema = ProtoSet::compile(&files()).unwrap();
        let sub = schema.message("acme.tracks.v1.Subscribe").unwrap();
        Subscription {
            url: format!("http://{addr}"),
            path: "/acme.tracks.v1.TrackFeed/Stream".into(),
            request: schema.encode(&sub, &json!({"area": "solent"})).unwrap(),
            metadata: BTreeMap::from([("authorization".into(), format!("Bearer {token}"))]),
            tls: None,
            max_message: 1 << 16,
            keepalive: Some(Duration::from_secs(5)),
            connect_timeout: Duration::from_secs(5),
        }
    }

    #[tokio::test]
    async fn a_producers_stream_is_read_and_its_errors_are_reported() {
        let addr = producer(3, None).await;
        let schema = ProtoSet::compile(&files()).unwrap();
        let batch = schema.message("acme.tracks.v1.TrackBatch").unwrap();
        let mut got = Vec::new();
        let mut opened = false;
        let ended = subscribe(
            &call(addr, "t0ken"),
            || opened = true,
            |m| {
                got.push(schema.decode(&batch, &m).unwrap()["tracks"][0]["track_id"].clone());
                async { Ok(()) }
            },
        )
        .await
        .unwrap();
        assert!(opened);
        assert_eq!(ended.messages, 3);
        assert_eq!(
            got,
            vec![json!("solent-0"), json!("solent-1"), json!("solent-2")]
        );

        let e = subscribe(&call(addr, "wrong"), || {}, |_| async { Ok(()) })
            .await
            .unwrap_err();
        assert!(e.to_string().contains("status 16"), "{e}");

        let addr = producer(0, Some((7, "not allowed: área restricted"))).await;
        let e = subscribe(&call(addr, "t0ken"), || {}, |_| async { Ok(()) })
            .await
            .unwrap_err();
        assert_eq!(e.to_string(), "gRPC status 7: not allowed: área restricted");

        // Nobody listening: a connection error, which the supervisor retries.
        let closed = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap()
            .local_addr()
            .unwrap();
        assert!(
            subscribe(&call(closed, "t0ken"), || {}, |_| async { Ok(()) })
                .await
                .is_err()
        );
    }
}
