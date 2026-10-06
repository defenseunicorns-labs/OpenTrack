//! Transports: getting frames off the wire.
//!
//! Each transport runs until its connection ends or fails; the worker's
//! supervisor restarts it with backoff. Framing is part of the transport's
//! configuration. Secrets are never stored: any string setting may reference
//! an environment variable as `${env:NAME}`, resolved when the source starts.
//! Stream and URL transports can run over TLS, mutual TLS included (see
//! [`crate::tls`]).

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, bail};
use bytes::BytesMut;
use chrono::Utc;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio_tungstenite::Connector;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use crate::frame::{Frame, Framer, Framing};
use crate::path::Path;
pub use crate::tls::{ClientTls, ServerTls};

/// The local interface a multicast group is joined (or sent) on. IPv4
/// names it by one of its addresses; IPv6 by index, so a name is looked up
/// when the socket opens (a missing interface is then a source error, not a
/// bad setting). In JSON it is a string: `"192.0.2.10"`, `"eth0"` or `"2"`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum MulticastInterface {
    /// An IPv4 address of this host (IPv4 groups).
    Address(Ipv4Addr),
    /// An interface index (IPv6 groups; 0: the system's choice).
    Index(u32),
    /// An interface name such as `eth0` (IPv6 groups).
    Name(String),
}

impl MulticastInterface {
    /// The index an IPv6 join or send needs, looking a name up now.
    pub fn index(&self) -> anyhow::Result<u32> {
        match self {
            MulticastInterface::Index(i) => Ok(*i),
            MulticastInterface::Name(name) => interface_index(name),
            MulticastInterface::Address(a) => {
                bail!("an IPv6 group needs an interface name or index, not the address {a}")
            }
        }
    }
}

#[cfg(unix)]
fn interface_index(name: &str) -> anyhow::Result<u32> {
    nix::net::if_::if_nametoindex(name).with_context(|| format!("no network interface {name:?}"))
}

#[cfg(not(unix))]
fn interface_index(name: &str) -> anyhow::Result<u32> {
    bail!("interface names are not supported on this platform; give {name:?}'s index")
}

impl std::str::FromStr for MulticastInterface {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        let s = s.trim();
        if let Ok(a) = s.parse::<Ipv4Addr>() {
            return Ok(MulticastInterface::Address(a));
        }
        if let Ok(i) = s.parse::<u32>() {
            return Ok(MulticastInterface::Index(i));
        }
        if s.parse::<Ipv6Addr>().is_ok() {
            return Err(format!(
                "multicast interface {s:?}: IPv6 joins by interface name or index, not address"
            ));
        }
        // Linux allows 15 bytes; names never hold spaces, '/' or ':'
        // (an IPv6 address was caught above).
        let ok = !s.is_empty()
            && s.len() <= 15
            && s.bytes()
                .all(|b| b.is_ascii_graphic() && !b"/:%".contains(&b));
        if !ok {
            return Err(format!(
                "multicast interface {s:?}: an IPv4 address, an interface name or an index"
            ));
        }
        Ok(MulticastInterface::Name(s.to_string()))
    }
}

impl TryFrom<String> for MulticastInterface {
    type Error = String;

    fn try_from(s: String) -> Result<Self, String> {
        s.parse()
    }
}

impl From<MulticastInterface> for String {
    fn from(i: MulticastInterface) -> String {
        i.to_string()
    }
}

impl std::fmt::Display for MulticastInterface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MulticastInterface::Address(a) => a.fmt(f),
            MulticastInterface::Index(i) => i.fmt(f),
            MulticastInterface::Name(n) => f.write_str(n),
        }
    }
}

/// Settings of a multicast join or send the types alone cannot catch: the
/// group is multicast, and the interface is given the way its family
/// needs. Shared with the TAK output.
pub fn check_multicast(
    group: IpAddr,
    interface: Option<&MulticastInterface>,
) -> Result<(), String> {
    if !group.is_multicast() {
        return Err(format!("{group} is not a multicast group"));
    }
    match (group, interface) {
        (IpAddr::V4(_), Some(i @ (MulticastInterface::Index(_) | MulticastInterface::Name(_)))) => {
            Err(format!(
                "an IPv4 group is joined by an interface address of this host, not {i:?}"
            ))
        }
        (IpAddr::V6(_), Some(MulticastInterface::Address(a))) => Err(format!(
            "an IPv6 group is joined by interface name or index, not the address {a}"
        )),
        _ => Ok(()),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TransportConfig {
    /// Connect out to a TCP server.
    TcpClient {
        host: String,
        port: u16,
        #[serde(default = "lines")]
        framing: Framing,
        /// Sent once after connecting (e.g. a login or subscribe line).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        send_on_connect: Option<String>,
        /// Connect with TLS (mutual TLS with a client certificate).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tls: Option<ClientTls>,
    },
    /// Listen for TCP clients; every connection feeds the same source.
    TcpServer {
        bind: String,
        #[serde(default = "lines")]
        framing: Framing,
        /// Accept TLS only; with `client_ca_file`, only clients holding a
        /// certificate that CA signed (mutual TLS).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tls: Option<ServerTls>,
    },
    /// One frame per datagram.
    Udp {
        bind: String,
        /// Join this multicast group (IPv4 or IPv6, the bind's family).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        multicast_group: Option<IpAddr>,
        /// Where to join it: an interface address for an IPv4 group, an
        /// interface name or index for an IPv6 one (default: the system's
        /// choice).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        multicast_interface: Option<MulticastInterface>,
    },
    /// Poll an HTTP endpoint; one frame per response body.
    HttpPoll {
        url: String,
        #[serde(default = "five")]
        interval_secs: f64,
        #[serde(default = "get")]
        method: String,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        headers: BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<String>,
        #[serde(default = "twenty")]
        timeout_secs: f64,
        /// TLS for `https://`: a private CA, a client certificate, …
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tls: Option<ClientTls>,
    },
    /// WebSocket client; one frame per message.
    Websocket {
        url: String,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        headers: BTreeMap<String, String>,
        /// Sent after connecting: a JSON value (sent as text) or a string.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subscribe: Option<Value>,
        /// If a JSON message has a non-null value here, the server reported
        /// an error in-band: reconnect.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error_path: Option<Path>,
        #[serde(default = "twenty")]
        ping_secs: f64,
        /// TLS for `wss://`: a private CA, a client certificate, …
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tls: Option<ClientTls>,
    },
    /// MQTT 3.1.1 client; one frame per PUBLISH. The message's topic is
    /// available to mappings as `_frame.topic` (and `_frame.topic_levels`,
    /// the topic split on `/`).
    Mqtt {
        /// `mqtt://host[:1883]` or `mqtts://host[:8883]` (TLS).
        url: String,
        /// Topic filters; `+` and `#` wildcards allowed.
        topics: Vec<String>,
        /// Subscription QoS: 0 (at most once) or 1 (at least once).
        #[serde(default)]
        qos: u8,
        /// Defaults to a unique id per connection. Required for a persistent
        /// session (`clean_session: false`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        client_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        username: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        password: Option<String>,
        #[serde(default = "yes")]
        clean_session: bool,
        #[serde(default = "thirty")]
        keepalive_secs: f64,
        /// PEM file of the CA to trust for `mqtts://` (default: system roots).
        /// Older form of `tls.ca_file`, still accepted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ca_file: Option<String>,
        /// TLS for `mqtts://`: a private CA, a client certificate, …
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tls: Option<ClientTls>,
    },
    /// Call a producer's gRPC method and read the messages it streams back,
    /// one frame each (the protobuf codec decodes them). A call that ends is
    /// made again, with backoff.
    GrpcClient {
        /// `http://host:port` or `https://host:port`.
        url: String,
        /// `package.Service/Method`, from the codec's `.proto` files. Its
        /// response type is the codec's message.
        method: String,
        /// The request message as JSON (field names as in the `.proto`);
        /// unset: an empty request.
        #[serde(default, skip_serializing_if = "Value::is_null")]
        request: Value,
        /// gRPC metadata sent with the call (lowercase names), e.g.
        /// `authorization: Bearer ${env:ACME_TOKEN}`.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        metadata: BTreeMap<String, String>,
        /// TLS for `https://`: a private CA, a client certificate, …
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tls: Option<ClientTls>,
        /// Largest message accepted, KiB.
        #[serde(default = "four_mib")]
        max_message_kib: u64,
        /// HTTP/2 keepalive pings, seconds (0: none), so a quiet stream
        /// through a NAT or firewall is not silently cut.
        #[serde(default = "twenty")]
        keepalive_secs: f64,
    },
    /// Accept producers' gRPC calls: every message they send is a frame
    /// (the protobuf codec decodes them). A malformed message fails the
    /// producer's call with INVALID_ARGUMENT, saying why.
    GrpcServer {
        bind: String,
        /// Methods producers may call, `package.Service/Method`. Unset:
        /// every method of the `.proto` files whose request type is the
        /// codec's message.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        methods: Vec<String>,
        /// Accept TLS only; with `client_ca_file`, only producers holding a
        /// certificate that CA signed (mutual TLS).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tls: Option<ServerTls>,
        /// A bearer token producers must send (`authorization: Bearer …`).
        /// Use `${env:NAME}` to keep it out of the source spec.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token: Option<String>,
        /// Largest message accepted, KiB.
        #[serde(default = "four_mib")]
        max_message_kib: u64,
        /// Producers connected at once; more are refused.
        #[serde(default = "sixty_four")]
        max_connections: usize,
        #[serde(default = "twenty")]
        keepalive_secs: f64,
    },
    /// Replay recorded data: a file, or every file in a directory (by name,
    /// optionally only those with `extension`), split by `framing`. The file
    /// name is available to mappings as `_frame.file`.
    File {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        extension: Option<String>,
        #[serde(default = "message")]
        framing: Framing,
        /// Pace the replay (frames per second); unset: as fast as the pipeline takes them.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frames_per_second: Option<f64>,
        /// Start again from the first file after the last.
        #[serde(default)]
        repeat: bool,
    },
}

fn message() -> Framing {
    Framing::Message
}

fn lines() -> Framing {
    Framing::Lines {
        max_len: crate::frame::DEFAULT_MAX_FRAME,
    }
}
fn five() -> f64 {
    5.0
}
fn four_mib() -> u64 {
    4096
}
fn sixty_four() -> usize {
    64
}
fn twenty() -> f64 {
    20.0
}
fn get() -> String {
    "GET".into()
}
fn thirty() -> f64 {
    30.0
}
fn yes() -> bool {
    true
}

/// TLS settings on a URL that will not use them are a mistake.
fn tls_url(url: &str, scheme: &str) -> Result<(), String> {
    if url.starts_with(scheme) || url.starts_with("${env:") {
        Ok(())
    } else {
        Err(format!("tls settings need a {scheme} url, got {url:?}"))
    }
}

/// Largest MQTT message accepted, matching the frame ceiling.
const MQTT_MAX_PACKET: usize = crate::frame::DEFAULT_MAX_FRAME;

impl TransportConfig {
    pub fn kind(&self) -> &'static str {
        match self {
            TransportConfig::TcpClient { .. } => "tcp_client",
            TransportConfig::TcpServer { .. } => "tcp_server",
            TransportConfig::Udp { .. } => "udp",
            TransportConfig::HttpPoll { .. } => "http_poll",
            TransportConfig::Websocket { .. } => "websocket",
            TransportConfig::Mqtt { .. } => "mqtt",
            TransportConfig::File { .. } => "file",
            TransportConfig::GrpcClient { .. } => "grpc_client",
            TransportConfig::GrpcServer { .. } => "grpc_server",
        }
    }

    /// Settings that cannot be caught by the type alone.
    pub fn check(&self) -> Result<(), String> {
        match self {
            TransportConfig::TcpClient { tls: Some(t), .. } => t.check()?,
            TransportConfig::TcpServer { tls: Some(t), .. } => t.check()?,
            TransportConfig::HttpPoll {
                url, tls: Some(t), ..
            } => {
                tls_url(url, "https://")?;
                t.check()?;
            }
            TransportConfig::Websocket {
                url, tls: Some(t), ..
            } => {
                tls_url(url, "wss://")?;
                t.check()?;
            }
            TransportConfig::Mqtt {
                url,
                ca_file,
                tls: Some(t),
                ..
            } => {
                tls_url(url, "mqtts://")?;
                if ca_file.is_some() && t.ca_file.is_some() {
                    return Err("set the MQTT CA once: tls.ca_file (or the older ca_file)".into());
                }
                t.check()?;
            }
            TransportConfig::GrpcClient {
                url, tls: Some(t), ..
            } => {
                tls_url(url, "https://")?;
                t.check()?;
            }
            TransportConfig::GrpcServer { tls: Some(t), .. } => t.check()?,
            TransportConfig::Udp {
                bind,
                multicast_group: Some(group),
                multicast_interface,
            } => {
                check_multicast(*group, multicast_interface.as_ref())?;
                // `${env:…}` binds are checked when the source starts.
                if let Ok(addr) = bind.trim().parse::<SocketAddr>()
                    && addr.is_ipv4() != group.is_ipv4()
                {
                    return Err(format!(
                        "multicast group {group} and bind {addr} are different IP versions \
                         (bind 0.0.0.0:port for IPv4, [::]:port for IPv6)"
                    ));
                }
            }
            _ => {}
        }
        match self {
            TransportConfig::GrpcClient {
                url,
                metadata,
                max_message_kib,
                keepalive_secs,
                ..
            } => {
                if !(url.starts_with("http://")
                    || url.starts_with("https://")
                    || url.starts_with("${env:"))
                {
                    return Err(format!(
                        "gRPC url must start with http:// or https://, got {url:?}"
                    ));
                }
                for k in metadata.keys() {
                    let ok = !k.is_empty()
                        && k.bytes().all(|b| {
                            b.is_ascii_lowercase() || b.is_ascii_digit() || b"-_.".contains(&b)
                        })
                        && !k.starts_with("grpc-")
                        && !matches!(k.as_str(), "content-type" | "te" | "user-agent");
                    if !ok {
                        return Err(format!(
                            "gRPC metadata name {k:?}: lowercase letters, digits, - _ . \
                             (not grpc-*, content-type, te or user-agent)"
                        ));
                    }
                }
                grpc_limits(*max_message_kib, *keepalive_secs)?;
            }
            TransportConfig::GrpcServer {
                max_message_kib,
                max_connections,
                keepalive_secs,
                ..
            } => {
                grpc_limits(*max_message_kib, *keepalive_secs)?;
                if !(1..=10_000).contains(max_connections) {
                    return Err("max_connections: 1 to 10000".into());
                }
            }
            _ => {}
        }
        if let TransportConfig::File {
            path,
            frames_per_second,
            ..
        } = self
        {
            if path.trim().is_empty() {
                return Err("a file transport needs a path".into());
            }
            if frames_per_second.is_some_and(|f| !(f.is_finite() && f > 0.0)) {
                return Err("frames_per_second must be a positive number".into());
            }
        }
        if let TransportConfig::Mqtt {
            url,
            topics,
            qos,
            client_id,
            clean_session,
            ..
        } = self
        {
            if !(url.starts_with("mqtt://")
                || url.starts_with("mqtts://")
                || url.starts_with("${env:"))
            {
                return Err(format!(
                    "MQTT url must start with mqtt:// or mqtts://, got {url:?}"
                ));
            }
            if topics.is_empty() || topics.iter().any(|t| t.trim().is_empty()) {
                return Err("MQTT needs at least one topic filter".into());
            }
            if *qos > 1 {
                return Err(format!("MQTT qos must be 0 or 1, got {qos}"));
            }
            if !clean_session && client_id.as_deref().is_none_or(|c| c.trim().is_empty()) {
                return Err(
                    "a persistent MQTT session (clean_session false) needs a client_id".into(),
                );
            }
        }
        Ok(())
    }

    /// Where it connects or listens, with secrets left unresolved.
    pub fn endpoint(&self) -> String {
        match self {
            TransportConfig::TcpClient { host, port, .. } => format!("{host}:{port}"),
            TransportConfig::TcpServer { bind, .. } | TransportConfig::Udp { bind, .. } => {
                bind.clone()
            }
            TransportConfig::HttpPoll { url, .. }
            | TransportConfig::Websocket { url, .. }
            | TransportConfig::Mqtt { url, .. } => url.clone(),
            TransportConfig::File { path, .. } => path.clone(),
            TransportConfig::GrpcClient { url, method, .. } => {
                format!(
                    "{}/{}",
                    url.trim_end_matches('/'),
                    method.trim_start_matches('/')
                )
            }
            TransportConfig::GrpcServer { bind, .. } => bind.clone(),
        }
    }
}

fn grpc_limits(max_message_kib: u64, keepalive_secs: f64) -> Result<(), String> {
    if !(1..=65_536).contains(&max_message_kib) {
        return Err("max_message_kib: 1 to 65536 (64 MiB)".into());
    }
    if !(keepalive_secs == 0.0 || (1.0..=3600.0).contains(&keepalive_secs)) {
        return Err("keepalive_secs: 0 (none) or 1 to 3600".into());
    }
    Ok(())
}

/// The protobuf schema a gRPC transport works with: the codec's compiled
/// `.proto` files and the message each frame holds.
#[derive(Clone)]
pub struct ProtoContext {
    pub set: Arc<crate::proto::ProtoSet>,
    pub message: String,
}

impl ProtoContext {
    /// From a source's codec (none unless it is protobuf).
    pub fn of(codec: &crate::codec::CodecConfig) -> Result<Option<Self>, String> {
        match codec {
            crate::codec::CodecConfig::Protobuf { message, .. } => Ok(Some(Self {
                set: codec
                    .proto()
                    .map_err(|e| e.to_string())?
                    .expect("a protobuf codec compiles"),
                message: message.clone(),
            })),
            _ => Ok(None),
        }
    }

    /// The methods a gRPC server accepts: those listed, or every method
    /// whose request is the codec's message. Each must send that message.
    pub fn server_methods(&self, listed: &[String]) -> Result<Vec<crate::proto::Method>, String> {
        let wanted = self.message.trim_start_matches('.');
        let methods = if listed.is_empty() {
            self.set
                .methods()
                .into_iter()
                .filter(|m| m.input == wanted)
                .collect::<Vec<_>>()
        } else {
            listed
                .iter()
                .map(|m| self.set.method(m).map_err(|e| e.to_string()))
                .collect::<Result<Vec<_>, _>>()?
        };
        if methods.is_empty() {
            return Err(format!(
                "no method in the .proto files takes {wanted}: list the methods producers call"
            ));
        }
        for m in &methods {
            if m.input != wanted {
                return Err(format!(
                    "{} takes {}, but the codec decodes {wanted}",
                    m.name, m.input
                ));
            }
        }
        Ok(methods)
    }

    /// Check a gRPC client's method and request against the schema; the
    /// request, encoded.
    pub fn client_request(&self, method: &str, request: &Value) -> Result<Vec<u8>, String> {
        let m = self.set.method(method).map_err(|e| e.to_string())?;
        let wanted = self.message.trim_start_matches('.');
        if m.output != wanted {
            return Err(format!(
                "{} answers {}, but the codec decodes {wanted}",
                m.name, m.output
            ));
        }
        let input = self.set.message(&m.input).map_err(|e| e.to_string())?;
        let empty = serde_json::json!({});
        self.set
            .encode(&input, if request.is_null() { &empty } else { request })
            .map_err(|e| format!("request: {e}"))
    }
}

/// Live connection state, shared with the control plane via the worker.
#[derive(Debug, Default, Clone, Serialize)]
pub struct LinkStatus {
    pub connected: bool,
    pub connects: u64,
    pub errors: u64,
    /// Bytes read from a stream transport (TCP, TLS, serial, files), framed or not.
    pub bytes_in: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_frame_at: Option<chrono::DateTime<Utc>>,
}

pub type SharedStatus = Arc<Mutex<LinkStatus>>;

fn set(status: &SharedStatus, f: impl FnOnce(&mut LinkStatus)) {
    if let Ok(mut s) = status.lock() {
        f(&mut s);
    }
}

/// Replace `${env:NAME}` references with environment values.
pub fn resolve_env(s: &str) -> anyhow::Result<String> {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("${env:") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 6..];
        let end = after.find('}').context("unterminated ${env:...}")?;
        let name = &after[..end];
        let value = std::env::var(name)
            .with_context(|| format!("environment variable {name} is not set"))?;
        out.push_str(&value);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

fn resolve_value(v: &Value) -> anyhow::Result<Value> {
    Ok(match v {
        Value::String(s) => Value::String(resolve_env(s)?),
        Value::Array(items) => {
            Value::Array(items.iter().map(resolve_value).collect::<Result<_, _>>()?)
        }
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| Ok((k.clone(), resolve_value(v)?)))
                .collect::<anyhow::Result<_>>()?,
        ),
        other => other.clone(),
    })
}

async fn emit(tx: &mpsc::Sender<Frame>, status: &SharedStatus, frame: Frame) -> anyhow::Result<()> {
    set(status, |s| s.last_frame_at = Some(frame.received_at));
    tx.send(frame)
        .await
        .map_err(|_| anyhow::anyhow!("pipeline closed"))
}

/// Run the transport until the connection ends. `Ok` means a clean end
/// (peer closed); either way the supervisor decides when to reconnect.
pub async fn run(
    config: &TransportConfig,
    proto: Option<&ProtoContext>,
    tx: mpsc::Sender<Frame>,
    status: SharedStatus,
) -> anyhow::Result<()> {
    let need_proto =
        || proto.ok_or_else(|| anyhow::anyhow!("a gRPC source decodes with the protobuf codec"));
    match config {
        TransportConfig::GrpcClient {
            url,
            method,
            request,
            metadata,
            tls,
            max_message_kib,
            keepalive_secs,
        } => {
            let proto = need_proto()?;
            let request = proto
                .client_request(method, request)
                .map_err(|e| anyhow::anyhow!(e))?;
            let url = resolve_env(url)?;
            let host = url
                .split("://")
                .nth(1)
                .and_then(|r| r.split(['/', ':']).next())
                .unwrap_or_default()
                .to_owned();
            let tls = match tls {
                Some(t) => {
                    let name = match t.server_name()? {
                        Some(n) => n,
                        None => crate::tls::server_name(&host)?,
                    };
                    Some((t.client_config()?, name))
                }
                None => None,
            };
            let metadata = metadata
                .iter()
                .map(|(k, v)| Ok((k.clone(), resolve_env(v)?)))
                .collect::<anyhow::Result<BTreeMap<_, _>>>()?;
            let path = format!("/{}", method.trim_start_matches('/'));
            let call = crate::grpc::Subscription {
                url: url.clone(),
                path: path.clone(),
                request,
                metadata,
                tls,
                max_message: (*max_message_kib as usize) << 10,
                keepalive: (*keepalive_secs > 0.0)
                    .then(|| Duration::from_secs_f64(*keepalive_secs)),
                connect_timeout: Duration::from_secs(10),
            };
            let origin = url.clone();
            let ended = crate::grpc::subscribe(
                &call,
                || connected(&status),
                |bytes| {
                    let mut f = Frame::new(bytes);
                    f.origin = Some(origin.clone());
                    f.meta.insert("method".into(), Value::String(path.clone()));
                    let (tx, status) = (tx.clone(), status.clone());
                    async move { emit(&tx, &status, f).await }
                },
            )
            .await?;
            tracing::info!(%url, method = %path, messages = ended.messages, "gRPC call ended");
            Ok(())
        }
        TransportConfig::GrpcServer {
            bind,
            methods,
            tls,
            token,
            max_message_kib,
            max_connections,
            keepalive_secs,
        } => {
            let proto = need_proto()?;
            let accepted = proto
                .server_methods(methods)
                .map_err(|e| anyhow::anyhow!(e))?
                .into_iter()
                .map(|m| {
                    let input = proto.set.message(&m.input)?;
                    Ok((format!("/{}", m.name), crate::grpc::Accepted { input }))
                })
                .collect::<Result<BTreeMap<_, _>, crate::proto::ProtoError>>()?;
            let token = token.as_deref().map(resolve_env).transpose()?;
            let acceptor = tls.as_ref().map(ServerTls::h2_acceptor).transpose()?;
            let service = Arc::new(crate::grpc::Service {
                schema: proto.set.clone(),
                methods: accepted,
                token,
                max_message: (*max_message_kib as usize) << 10,
                max_connections: *max_connections,
                keepalive: (*keepalive_secs > 0.0)
                    .then(|| Duration::from_secs_f64(*keepalive_secs)),
                // A producer in trouble shows in the source's status.
                on_problem: Some({
                    let status = status.clone();
                    Arc::new(move |what: String| {
                        set(&status, |s| {
                            s.errors += 1;
                            s.last_error = Some(what);
                        })
                    })
                }),
            });
            let listener = tokio::net::TcpListener::bind(resolve_env(bind)?.as_str())
                .await
                .with_context(|| format!("binding {bind}"))?;
            connected(&status);
            let counts = Arc::new(crate::grpc::ServeCounts::default());
            crate::grpc::serve(listener, acceptor, service, tx, counts).await
        }
        TransportConfig::TcpClient {
            host,
            port,
            framing,
            send_on_connect,
            tls,
        } => {
            let host = resolve_env(host)?;
            // Build the TLS settings first: a bad certificate file is a
            // configuration error, not a connection failure.
            let tls = match tls {
                Some(t) => {
                    let name = match t.server_name()? {
                        Some(n) => n,
                        None => crate::tls::server_name(&host)?,
                    };
                    let config = Arc::new(t.client_config()?);
                    Some((tokio_rustls::TlsConnector::from(config), name))
                }
                None => None,
            };
            let stream = tokio::net::TcpStream::connect((host.as_str(), *port))
                .await
                .with_context(|| format!("connecting to {host}:{port}"))?;
            let origin = format!("{host}:{port}");
            log_peer("tcp", &origin, stream.peer_addr());
            let line = send_on_connect.as_deref().map(resolve_env).transpose()?;
            match tls {
                Some((connector, name)) => {
                    let stream = connector
                        .connect(name, stream)
                        .await
                        .with_context(|| format!("TLS handshake with {origin}"))?;
                    tcp_client(stream, line, framing, &tx, &status, &origin).await
                }
                None => tcp_client(stream, line, framing, &tx, &status, &origin).await,
            }
        }
        TransportConfig::TcpServer { bind, framing, tls } => {
            let acceptor = tls.as_ref().map(ServerTls::acceptor).transpose()?;
            let listener = tokio::net::TcpListener::bind(resolve_env(bind)?.as_str())
                .await
                .with_context(|| format!("binding {bind}"))?;
            connected(&status);
            loop {
                let (stream, peer) = listener.accept().await?;
                let (tx, status, framing) = (tx.clone(), status.clone(), framing.clone());
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    let origin = peer.to_string();
                    let ended = match acceptor {
                        Some(acceptor) => match tls_accept(&acceptor, stream, peer).await {
                            Ok(stream) => {
                                read_stream(stream, &framing, &tx, &status, &origin).await
                            }
                            Err(e) => {
                                // Failed verification (or not TLS at all): drop the client.
                                tracing::warn!(%peer, error = %e, "TLS client rejected");
                                set(&status, |s| {
                                    s.errors += 1;
                                    s.last_error = Some(format!("{peer}: {e:#}"));
                                });
                                return;
                            }
                        },
                        None => read_stream(stream, &framing, &tx, &status, &origin).await,
                    };
                    if let Err(e) = ended {
                        tracing::debug!(%peer, error = %e, "tcp client ended");
                    }
                });
            }
        }
        TransportConfig::Udp {
            bind,
            multicast_group,
            multicast_interface,
        } => {
            let addr: SocketAddr = resolve_env(bind)?
                .parse()
                .context("udp bind must be ip:port")?;
            let socket = udp_socket(addr, *multicast_group, multicast_interface.as_ref())?;
            connected(&status);
            let mut buf = vec![0u8; 65_536];
            loop {
                let (n, peer) = socket.recv_from(&mut buf).await?;
                let mut frame = Frame::new(buf[..n].to_vec());
                frame.origin = Some(peer.to_string());
                emit(&tx, &status, frame).await?;
            }
        }
        TransportConfig::HttpPoll {
            url,
            interval_secs,
            method,
            headers,
            body,
            timeout_secs,
            tls,
        } => {
            let mut client = reqwest::Client::builder()
                .timeout(Duration::from_secs_f64(*timeout_secs))
                .user_agent(concat!("opentrack/", env!("CARGO_PKG_VERSION")));
            if let Some(t) = tls {
                client = client.tls_backend_preconfigured(t.client_config()?);
            }
            let client = client.build()?;
            let url = resolve_env(url)?;
            let method: reqwest::Method = method.parse().context("bad HTTP method")?;
            let mut hdrs = reqwest::header::HeaderMap::new();
            for (k, v) in headers {
                hdrs.insert(
                    reqwest::header::HeaderName::from_bytes(k.as_bytes())?,
                    reqwest::header::HeaderValue::from_str(&resolve_env(v)?)?,
                );
            }
            let body = body.as_deref().map(resolve_env).transpose()?;
            let interval = Duration::from_secs_f64(interval_secs.max(0.1));
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            // Extra wait while the server is rate limiting or overloaded.
            let mut backoff = Duration::ZERO;
            // Logged when the polled address changes, not on every poll.
            let mut last_peer = crate::netlog::LastPeer::default();
            let shown = redact(&url);
            loop {
                ticker.tick().await;
                if !backoff.is_zero() {
                    tokio::time::sleep(backoff).await;
                }
                let mut req = client.request(method.clone(), &url).headers(hdrs.clone());
                if let Some(b) = &body {
                    req = req.body(b.clone());
                }
                let res = async {
                    let resp = req.send().await?;
                    if let Some(peer) = resp.remote_addr()
                        && last_peer.changed(peer)
                    {
                        tracing::info!(component = "http_poll", url = %shown, peer_addr = %peer, "polling");
                    }
                    let code = resp.status();
                    if code == reqwest::StatusCode::TOO_MANY_REQUESTS
                        || code == reqwest::StatusCode::SERVICE_UNAVAILABLE
                    {
                        let retry_after = resp
                            .headers()
                            .get(reqwest::header::RETRY_AFTER)
                            .and_then(|v| v.to_str().ok())
                            .and_then(|v| v.trim().parse::<f64>().ok());
                        return Ok(Err((code, retry_after)));
                    }
                    let resp = resp.error_for_status()?;
                    anyhow::Ok(Ok(resp.bytes().await?))
                }
                .await;
                match res {
                    Ok(Ok(bytes)) => {
                        backoff = Duration::ZERO;
                        connected(&status);
                        let mut frame = Frame::new(bytes);
                        frame.origin = Some(url.clone());
                        emit(&tx, &status, frame).await?;
                    }
                    Ok(Err((code, retry_after))) => {
                        // Rate limited: honour Retry-After, else back off
                        // exponentially (to five minutes) so the limit can clear.
                        backoff = match retry_after {
                            Some(secs) => Duration::from_secs_f64(secs.clamp(0.0, 3600.0)),
                            None => (backoff * 2).max(interval).min(Duration::from_secs(300)),
                        };
                        tracing::warn!(url = %shown, %code, ?backoff, "poll rate limited; backing off");
                        set(&status, |s| {
                            s.connected = false;
                            s.errors += 1;
                            s.last_error =
                                Some(format!("{code}; next poll in {}s", backoff.as_secs()));
                        });
                    }
                    Err(e) => {
                        // A failed poll is not fatal; count it and poll again.
                        tracing::warn!(url = %shown, error = %e, "poll failed");
                        last_peer.reset();
                        set(&status, |s| {
                            s.connected = false;
                            s.errors += 1;
                            s.last_error = Some(e.to_string());
                        });
                    }
                }
            }
        }
        TransportConfig::Websocket {
            url,
            headers,
            subscribe,
            error_path,
            ping_secs,
            tls,
        } => {
            let url = resolve_env(url)?;
            let mut request = url.as_str().into_client_request()?;
            for (k, v) in headers {
                request.headers_mut().insert(
                    tokio_tungstenite::tungstenite::http::HeaderName::from_bytes(k.as_bytes())?,
                    resolve_env(v)?.parse()?,
                );
            }
            let connector = tls
                .as_ref()
                .map(|t| anyhow::Ok(Connector::Rustls(Arc::new(t.client_config()?))))
                .transpose()?;
            let (mut ws, _) =
                tokio_tungstenite::connect_async_tls_with_config(request, None, false, connector)
                    .await
                    .with_context(|| format!("connecting to {}", redact(&url)))?;
            let peer = match ws.get_ref() {
                tokio_tungstenite::MaybeTlsStream::Plain(s) => s.peer_addr(),
                tokio_tungstenite::MaybeTlsStream::Rustls(s) => s.get_ref().0.peer_addr(),
                _ => Err(std::io::ErrorKind::Unsupported.into()),
            };
            log_peer("websocket", &redact(&url), peer);
            if let Some(sub) = subscribe {
                let text = match resolve_value(sub)? {
                    Value::String(s) => s,
                    other => other.to_string(),
                };
                ws.send(Message::text(text)).await?;
            }
            connected(&status);
            let mut ping = tokio::time::interval(Duration::from_secs_f64(ping_secs.max(1.0)));
            ping.tick().await;
            loop {
                tokio::select! {
                    _ = ping.tick() => ws.send(Message::Ping(Default::default())).await?,
                    msg = ws.next() => {
                        let Some(msg) = msg else { return Ok(()) };
                        let bytes = match msg? {
                            Message::Text(t) => bytes::Bytes::from(t.as_str().to_owned()),
                            Message::Binary(b) => b,
                            Message::Close(_) => return Ok(()),
                            _ => continue,
                        };
                        if let Some(path) = error_path
                            && let Ok(v) = serde_json::from_slice::<Value>(&bytes)
                            && path.get(&v).is_some_and(|e| !e.is_null())
                        {
                            bail!("server error: {}", path.get(&v).map(Value::to_string).unwrap_or_default());
                        }
                        let mut frame = Frame::new(bytes);
                        frame.origin = Some(redact(&url));
                        emit(&tx, &status, frame).await?;
                    }
                }
            }
        }
        TransportConfig::Mqtt { .. } => run_mqtt(config, &tx, &status).await,
        TransportConfig::File {
            path,
            extension,
            framing,
            frames_per_second,
            repeat,
        } => {
            let files = replay_files(&resolve_env(path)?, extension.as_deref())?;
            connected(&status);
            let pause = frames_per_second.map(|f| Duration::from_secs_f64(1.0 / f));
            loop {
                for file in &files {
                    let f = file.clone();
                    let bytes = tokio::task::spawn_blocking(move || std::fs::read(&f))
                        .await?
                        .with_context(|| format!("reading {}", file.display()))?;
                    let name = file
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let mut framer = Framer::new(framing.clone())?;
                    let mut buf = BytesMut::from(&bytes[..]);
                    loop {
                        match framer.next(&mut buf) {
                            Ok(Some(bytes)) => {
                                let mut frame = Frame::new(bytes);
                                frame.origin = Some(file.display().to_string());
                                frame
                                    .meta
                                    .insert("file".into(), serde_json::Value::String(name.clone()));
                                emit(&tx, &status, frame).await?;
                                if let Some(p) = pause {
                                    tokio::time::sleep(p).await;
                                }
                            }
                            Ok(None) => break,
                            Err(e) => {
                                tracing::warn!(file = %file.display(), error = %e, "framing error; rest of file skipped");
                                set(&status, |s| {
                                    s.errors += 1;
                                    s.last_error = Some(format!("{}: {e}", file.display()));
                                });
                                break;
                            }
                        }
                    }
                    if !buf.is_empty() {
                        set(&status, |s| {
                            s.errors += 1;
                            s.last_error = Some(format!(
                                "{}: {} trailing bytes are not a whole frame",
                                file.display(),
                                buf.len()
                            ));
                        });
                    }
                }
                if !repeat {
                    // Done: stay "connected" so the supervisor does not replay it again.
                    std::future::pending::<()>().await;
                }
            }
        }
    }
}

/// Split `mqtt[s]://host[:port]` into (tls, host, port).
fn mqtt_endpoint(url: &str) -> anyhow::Result<(bool, String, u16)> {
    let (tls, rest) = if let Some(r) = url.strip_prefix("mqtts://") {
        (true, r)
    } else if let Some(r) = url.strip_prefix("mqtt://") {
        (false, r)
    } else {
        bail!("MQTT url must start with mqtt:// or mqtts://");
    };
    let rest = rest.trim_end_matches('/');
    let default_port = if tls { 8883 } else { 1883 };
    // `[v6]:port`, `[v6]`, `host:port` or `host`.
    let (host, port) = if let Some(v6) = rest.strip_prefix('[') {
        let (h, after) = v6.split_once(']').context("MQTT url: unclosed [")?;
        (h, after.strip_prefix(':'))
    } else {
        match rest.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (rest, None),
        }
    };
    let port = match port {
        Some(p) => p.parse().with_context(|| format!("MQTT port {p:?}"))?,
        None => default_port,
    };
    if host.is_empty() {
        bail!("MQTT url has no host");
    }
    Ok((tls, host.to_owned(), port))
}

async fn run_mqtt(
    config: &TransportConfig,
    tx: &mpsc::Sender<Frame>,
    status: &SharedStatus,
) -> anyhow::Result<()> {
    use rumqttc::{
        AsyncClient, Event, MqttOptions, Packet, QoS, SubscribeReasonCode, TlsConfiguration,
        Transport,
    };
    let TransportConfig::Mqtt {
        url,
        topics,
        qos,
        client_id,
        username,
        password,
        clean_session,
        keepalive_secs,
        ca_file,
        tls: tls_settings,
    } = config
    else {
        unreachable!("run_mqtt called with another transport");
    };
    config.check().map_err(anyhow::Error::msg)?;
    let url = resolve_env(url)?;
    let (tls, host, port) = mqtt_endpoint(&url)?;
    let id = match client_id {
        Some(c) if !c.trim().is_empty() => resolve_env(c)?,
        _ => format!(
            "opentrack-{:x}",
            Utc::now().timestamp_nanos_opt().unwrap_or_default() ^ i64::from(std::process::id())
        ),
    };
    let mut opts = MqttOptions::new(id, host.clone(), port);
    opts.set_keep_alive(Duration::from_secs_f64(keepalive_secs.max(5.0)))
        .set_clean_session(*clean_session)
        .set_max_packet_size(MQTT_MAX_PACKET, 64 * 1024);
    if let Some(user) = username {
        let pass = password.as_deref().map(resolve_env).transpose()?;
        opts.set_credentials(resolve_env(user)?, pass.unwrap_or_default());
    }
    if tls {
        // The top-level `ca_file` is the older spelling of `tls.ca_file`.
        let mut settings = tls_settings.clone();
        if let Some(ca) = ca_file {
            settings
                .get_or_insert_default()
                .ca_file
                .get_or_insert(ca.clone());
        }
        opts.set_transport(match settings {
            Some(t) => {
                Transport::tls_with_config(TlsConfiguration::Rustls(Arc::new(t.client_config()?)))
            }
            None => Transport::tls_with_default_config(),
        });
    }
    let qos = if *qos == 1 {
        QoS::AtLeastOnce
    } else {
        QoS::AtMostOnce
    };
    let origin = format!("{}{host}:{port}", if tls { "mqtts://" } else { "mqtt://" });
    let (client, mut events) = AsyncClient::new(opts, 64);
    let filters: Vec<String> = topics.iter().map(|t| t.trim().to_owned()).collect();
    loop {
        // Any connection error ends the run; the supervisor reconnects with
        // backoff (and re-subscribes, since each run starts afresh).
        let event = events
            .poll()
            .await
            .with_context(|| format!("MQTT {origin}"))?;
        match event {
            Event::Incoming(Packet::ConnAck(_)) => {
                // rumqttc keeps the socket to itself: log what the broker
                // host resolves to as the session opens.
                let peer_addr = crate::netlog::resolve(&host, port).await;
                tracing::info!(component = "mqtt", destination = %origin, %peer_addr, "connected (resolved address)");
                for f in &filters {
                    client.subscribe(f.clone(), qos).await?;
                }
            }
            Event::Incoming(Packet::SubAck(ack)) => {
                if ack
                    .return_codes
                    .iter()
                    .any(|c| matches!(c, SubscribeReasonCode::Failure))
                {
                    bail!(
                        "MQTT broker refused a subscription ({})",
                        filters.join(", ")
                    );
                }
                connected(status);
            }
            Event::Incoming(Packet::Publish(p)) => {
                let mut meta = serde_json::Map::new();
                meta.insert(
                    "topic_levels".into(),
                    Value::Array(
                        p.topic
                            .split('/')
                            .map(|l| Value::String(l.to_owned()))
                            .collect(),
                    ),
                );
                meta.insert("topic".into(), Value::String(p.topic));
                if p.retain {
                    meta.insert("retained".into(), Value::Bool(true));
                }
                let mut frame = Frame::new(p.payload).with_meta(meta);
                frame.origin = Some(origin.clone());
                emit(tx, status, frame).await?;
            }
            Event::Incoming(Packet::Disconnect) => return Ok(()),
            _ => {}
        }
    }
}

fn connected(status: &SharedStatus) {
    set(status, |s| {
        if !s.connected {
            s.connects += 1;
        }
        s.connected = true;
    });
}

/// Log an outbound connection's remote address (ASD STIG V-222470).
fn log_peer(component: &str, destination: &str, peer: std::io::Result<SocketAddr>) {
    match peer {
        Ok(peer) => tracing::info!(component, destination, peer_addr = %peer, "connected"),
        Err(e) => {
            tracing::info!(component, destination, error = %e, "connected; peer address unknown")
        }
    }
}

/// Hide query strings, which often carry keys, in logs.
fn redact(url: &str) -> String {
    match url.split_once('?') {
        Some((base, _)) => format!("{base}?…"),
        None => url.to_owned(),
    }
}

/// The files a file transport replays: `path` itself, or the files in it
/// (sorted by name, optionally by extension).
fn replay_files(path: &str, extension: Option<&str>) -> anyhow::Result<Vec<std::path::PathBuf>> {
    let p = std::path::Path::new(path);
    let mut files = if p.is_dir() {
        std::fs::read_dir(p)
            .with_context(|| format!("listing {path}"))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|f| f.is_file())
            .filter(|f| {
                extension.is_none_or(|x| {
                    f.extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case(x.trim_start_matches('.')))
                })
            })
            .collect::<Vec<_>>()
    } else if p.is_file() {
        vec![p.to_path_buf()]
    } else {
        anyhow::bail!("{path} is not a file or directory");
    };
    files.sort();
    if files.is_empty() {
        anyhow::bail!("no files to replay in {path}");
    }
    Ok(files)
}

/// A connected TCP client (plain or TLS): say hello, then read.
async fn tcp_client<S: AsyncRead + AsyncWrite + Unpin>(
    mut stream: S,
    send_on_connect: Option<String>,
    framing: &Framing,
    tx: &mpsc::Sender<Frame>,
    status: &SharedStatus,
    origin: &str,
) -> anyhow::Result<()> {
    connected(status);
    if let Some(line) = send_on_connect {
        stream.write_all(line.as_bytes()).await?;
        stream.flush().await?;
    }
    read_stream(stream, framing, tx, status, origin).await
}

/// The TLS handshake with a `tcp_server` client, bounded so a silent peer
/// cannot hold a task open. Logs who connected.
async fn tls_accept(
    acceptor: &tokio_rustls::TlsAcceptor,
    stream: tokio::net::TcpStream,
    peer: SocketAddr,
) -> anyhow::Result<tokio_rustls::server::TlsStream<tokio::net::TcpStream>> {
    let stream = tokio::time::timeout(TLS_HANDSHAKE, acceptor.accept(stream))
        .await
        .context("TLS handshake timed out")?
        .context("TLS handshake")?;
    match stream
        .get_ref()
        .1
        .peer_certificates()
        .and_then(|c| c.first())
    {
        Some(cert) => {
            let subject = crate::tls::subject(cert).unwrap_or_else(|| "(unreadable)".into());
            tracing::info!(%peer, %subject, "TLS client connected");
        }
        None => tracing::info!(%peer, "TLS client connected (no client certificate)"),
    }
    Ok(stream)
}

/// Longest a `tcp_server` client may take over its TLS handshake.
const TLS_HANDSHAKE: Duration = Duration::from_secs(10);

async fn read_stream(
    mut stream: impl AsyncRead + Unpin,
    framing: &Framing,
    tx: &mpsc::Sender<Frame>,
    status: &SharedStatus,
    origin: &str,
) -> anyhow::Result<()> {
    let mut framer = Framer::new(framing.clone())?;
    let mut buf = BytesMut::with_capacity(64 * 1024);
    loop {
        let n = stream.read_buf(&mut buf).await?;
        if n == 0 {
            return Ok(());
        }
        set(status, |s| s.bytes_in += n as u64);
        loop {
            match framer.next(&mut buf) {
                Ok(Some(bytes)) => {
                    let mut frame = Frame::new(bytes);
                    frame.origin = Some(origin.to_owned());
                    emit(tx, status, frame).await?;
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!(%origin, error = %e, "framing error; frame skipped");
                    set(status, |s| {
                        s.errors += 1;
                        s.last_error = Some(e.to_string());
                    });
                }
            }
        }
    }
}

fn udp_socket(
    addr: SocketAddr,
    group: Option<IpAddr>,
    interface: Option<&MulticastInterface>,
) -> anyhow::Result<tokio::net::UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let socket = Socket::new(Domain::for_address(addr), Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    socket.set_broadcast(true)?;
    socket.set_recv_buffer_size(4 * 1024 * 1024).ok();
    socket.set_nonblocking(true)?;
    socket.bind(&addr.into())?;
    match group {
        Some(IpAddr::V4(g)) => {
            let at = match interface {
                None => Ipv4Addr::UNSPECIFIED,
                Some(MulticastInterface::Address(a)) => *a,
                Some(other) => bail!("an IPv4 group needs an interface address, not {other}"),
            };
            socket
                .join_multicast_v4(&g, &at)
                .with_context(|| format!("joining multicast group {g}"))?;
        }
        Some(IpAddr::V6(g)) => {
            let index = interface.map(MulticastInterface::index).transpose()?;
            socket
                .join_multicast_v6(&g, index.unwrap_or(0))
                .with_context(|| format!("joining multicast group {g}"))?;
        }
        None => {}
    }
    Ok(tokio::net::UdpSocket::from_std(socket.into())?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[test]
    fn env_references_resolve_and_fail_loudly() {
        // set_var is unsafe in edition 2024, so use PATH, which is always set.
        let path = std::env::var("PATH").unwrap();
        assert_eq!(resolve_env("x${env:PATH}y").unwrap(), format!("x{path}y"));
        assert!(resolve_env("${env:OT_SURELY_UNSET_VAR}").is_err());
        assert!(resolve_env("${env:PATH").is_err());
        assert_eq!(resolve_env("plain").unwrap(), "plain");
    }

    #[test]
    fn configs_parse_with_defaults() {
        let t: TransportConfig = serde_json::from_value(serde_json::json!({
            "type": "http_poll", "url": "https://api.example/v2/mil"
        }))
        .unwrap();
        assert!(
            matches!(t, TransportConfig::HttpPoll { interval_secs, .. } if interval_secs == 5.0)
        );
        let t: TransportConfig = serde_json::from_value(serde_json::json!({
            "type": "tcp_server", "bind": "0.0.0.0:8087", "framing": { "type": "end_tag", "tag": "</event>" }
        }))
        .unwrap();
        assert_eq!(t.kind(), "tcp_server");
        assert!(
            serde_json::from_value::<TransportConfig>(
                serde_json::json!({"type": "carrier_pigeon"})
            )
            .is_err()
        );
        assert_eq!(redact("wss://x/y?key=secret"), "wss://x/y?…");
    }

    #[tokio::test]
    async fn tcp_server_frames_cot_from_a_client() {
        // Bind to an ephemeral port first to learn a free one.
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = probe.local_addr().unwrap();
        drop(probe);
        let config = TransportConfig::TcpServer {
            bind: addr.to_string(),
            framing: Framing::EndTag {
                tag: "</event>".into(),
                max_len: 1 << 20,
            },
            tls: None,
        };
        let (tx, mut rx) = mpsc::channel(8);
        let status = SharedStatus::default();
        let task = tokio::spawn(async move { run(&config, None, tx, status).await });
        let mut client = loop {
            match tokio::net::TcpStream::connect(addr).await {
                Ok(c) => break c,
                Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        };
        client
            .write_all(b"<event uid=\"a\"></event><event uid=\"b\"></ev")
            .await
            .unwrap();
        client.write_all(b"ent>").await.unwrap();
        let a = rx.recv().await.unwrap();
        let b = rx.recv().await.unwrap();
        assert_eq!(&a.bytes[..], b"<event uid=\"a\"></event>");
        assert_eq!(&b.bytes[..], b"<event uid=\"b\"></event>");
        task.abort();
    }

    #[tokio::test]
    async fn udp_datagrams_are_frames() {
        let probe = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = probe.local_addr().unwrap();
        drop(probe);
        let config = TransportConfig::Udp {
            bind: addr.to_string(),
            multicast_group: None,
            multicast_interface: None,
        };
        let (tx, mut rx) = mpsc::channel(8);
        let task =
            tokio::spawn(async move { run(&config, None, tx, SharedStatus::default()).await });
        let sender = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let frame = loop {
            sender.send_to(b"{\"id\":1}", addr).await.unwrap();
            if let Ok(Some(f)) = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
                break f;
            }
        };
        assert_eq!(&frame.bytes[..], b"{\"id\":1}");
        task.abort();
    }

    #[test]
    fn udp_multicast_settings() {
        let udp = |v: serde_json::Value| -> TransportConfig {
            let mut t = serde_json::json!({"type": "udp", "bind": "0.0.0.0:6969"});
            t.as_object_mut()
                .unwrap()
                .extend(v.as_object().unwrap().clone());
            serde_json::from_value(t).unwrap()
        };
        // An IPv4 source saved before IPv6 reads and writes back the same.
        let old = serde_json::json!({
            "type": "udp", "bind": "0.0.0.0:6969",
            "multicast_group": "239.2.3.1", "multicast_interface": "10.0.0.2"
        });
        let t: TransportConfig = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(
            t,
            TransportConfig::Udp {
                bind: "0.0.0.0:6969".into(),
                multicast_group: Some("239.2.3.1".parse().unwrap()),
                multicast_interface: Some(MulticastInterface::Address(Ipv4Addr::new(10, 0, 0, 2))),
            }
        );
        assert_eq!(serde_json::to_value(&t).unwrap(), old);
        assert_eq!(t.check(), Ok(()));

        // The interface: an IPv4 address, an index or a name.
        assert_eq!("7".parse(), Ok(MulticastInterface::Index(7)));
        assert_eq!(
            " eth0 ".parse(),
            Ok(MulticastInterface::Name("eth0".into()))
        );
        assert_eq!(
            "br-8e120e2d3abd".parse(),
            Ok(MulticastInterface::Name("br-8e120e2d3abd".into()))
        );
        for bad in ["", "fe80::1", "eth 0", "a/b", "sixteen-letters!"] {
            assert!(bad.parse::<MulticastInterface>().is_err(), "{bad:?}");
        }
        assert!(
            serde_json::from_value::<TransportConfig>(serde_json::json!({
                "type": "udp", "bind": "[::]:6969", "multicast_interface": "fe80::1"
            }))
            .is_err()
        );

        let v6 = udp(serde_json::json!({"bind": "[::]:6969", "multicast_group": "ff15::6969"}));
        assert_eq!(v6.check(), Ok(()));
        for iface in ["eth0", "3", "0"] {
            let t = udp(serde_json::json!({
                "bind": "[::]:6969", "multicast_group": "ff15::6969", "multicast_interface": iface
            }));
            assert_eq!(t.check(), Ok(()), "{iface}");
            assert_eq!(
                serde_json::to_value(&t).unwrap()["multicast_interface"],
                iface
            );
        }
        // A bind from the environment is checked when the source starts.
        assert_eq!(
            udp(serde_json::json!({"bind": "${env:BIND}", "multicast_group": "ff15::6969"}))
                .check(),
            Ok(())
        );
        // An interface alone was accepted (and ignored) before; still is.
        assert_eq!(
            udp(serde_json::json!({"multicast_interface": "10.0.0.2"})).check(),
            Ok(())
        );

        for (bad, why) in [
            (
                serde_json::json!({"multicast_group": "10.0.0.1"}),
                "not multicast",
            ),
            (
                serde_json::json!({"bind": "[::]:6969", "multicast_group": "2001:db8::1"}),
                "not multicast",
            ),
            (
                serde_json::json!({"multicast_group": "ff15::6969"}),
                "IPv4 bind, IPv6 group",
            ),
            (
                serde_json::json!({"bind": "[::]:6969", "multicast_group": "239.2.3.1"}),
                "IPv6 bind, IPv4 group",
            ),
            (
                serde_json::json!({"multicast_group": "239.2.3.1", "multicast_interface": "eth0"}),
                "IPv4 group by name",
            ),
            (
                serde_json::json!({"bind": "[::]:6969", "multicast_group": "ff15::6969", "multicast_interface": "10.0.0.2"}),
                "IPv6 group by IPv4 address",
            ),
        ] {
            assert!(udp(bad).check().is_err(), "{why}");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn interface_names_are_looked_up_when_the_source_starts() {
        assert_eq!(MulticastInterface::Name("lo".into()).index().unwrap(), 1);
        assert_eq!(MulticastInterface::Index(4).index().unwrap(), 4);
        let missing = MulticastInterface::Name("no-such-if0".into());
        assert!(missing.index().is_err());
        let group = Some("ff02::6969:98".parse().unwrap());
        let err = udp_socket("[::]:0".parse().unwrap(), group, Some(&missing)).unwrap_err();
        assert!(format!("{err:#}").contains("no-such-if0"), "{err:#}");
    }

    #[tokio::test]
    async fn udp_joins_an_ipv6_multicast_group() {
        // Link-local scope on the system's default interface. Hosts without
        // IPv6 multicast (no IPv6, no route, no loopback of multicast)
        // skip.
        let group: Ipv6Addr = "ff02::6969:97".parse().unwrap();
        let Ok(probe) = std::net::UdpSocket::bind("[::]:0") else {
            eprintln!("skipped: no IPv6 here");
            return;
        };
        let port = probe.local_addr().unwrap().port();
        drop(probe);
        let config = TransportConfig::Udp {
            bind: format!("[::]:{port}"),
            multicast_group: Some(IpAddr::V6(group)),
            multicast_interface: None,
        };
        assert_eq!(config.check(), Ok(()));
        let (tx, mut rx) = mpsc::channel(8);
        let task =
            tokio::spawn(async move { run(&config, None, tx, SharedStatus::default()).await });
        let sender = std::net::UdpSocket::bind("[::]:0").unwrap();
        let dest = SocketAddr::from((group, port));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        let frame = loop {
            if task.is_finished() {
                let e = task.await.unwrap().unwrap_err();
                eprintln!("skipped: cannot join an IPv6 group here ({e:#})");
                return;
            }
            if let Err(e) = sender.send_to(b"{\"id\":6}", dest) {
                eprintln!("skipped: no IPv6 multicast route here ({e})");
                task.abort();
                return;
            }
            if let Ok(Some(f)) = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
                break f;
            }
            if tokio::time::Instant::now() > deadline {
                eprintln!("skipped: IPv6 multicast is not looped back here");
                task.abort();
                return;
            }
        };
        assert_eq!(&frame.bytes[..], b"{\"id\":6}");
        task.abort();
    }

    #[test]
    fn mqtt_config_urls_and_checks() {
        let t: TransportConfig = serde_json::from_value(serde_json::json!({
            "type": "mqtt", "url": "mqtt://broker.local", "topics": ["ais/+/pos"]
        }))
        .unwrap();
        assert_eq!(t.kind(), "mqtt");
        assert!(matches!(
            &t,
            TransportConfig::Mqtt { qos: 0, clean_session: true, keepalive_secs, .. } if *keepalive_secs == 30.0
        ));
        assert_eq!(t.check(), Ok(()));

        assert_eq!(
            mqtt_endpoint("mqtt://broker.local").unwrap(),
            (false, "broker.local".into(), 1883)
        );
        assert_eq!(
            mqtt_endpoint("mqtts://broker.local:9883/").unwrap(),
            (true, "broker.local".into(), 9883)
        );
        assert_eq!(
            mqtt_endpoint("mqtts://[::1]").unwrap(),
            (true, "::1".into(), 8883)
        );
        assert_eq!(
            mqtt_endpoint("mqtt://[::1]:1884").unwrap(),
            (false, "::1".into(), 1884)
        );
        assert!(mqtt_endpoint("tcp://x").is_err());
        assert!(mqtt_endpoint("mqtt://x:notaport").is_err());
        assert!(mqtt_endpoint("mqtt://:1883").is_err());

        let bad = |patch: serde_json::Value| {
            let mut v = serde_json::json!({
                "type": "mqtt", "url": "mqtt://b", "topics": ["t"]
            });
            v.as_object_mut()
                .unwrap()
                .extend(patch.as_object().unwrap().clone());
            serde_json::from_value::<TransportConfig>(v)
                .unwrap()
                .check()
                .unwrap_err()
        };
        assert!(bad(serde_json::json!({"topics": []})).contains("topic"));
        assert!(bad(serde_json::json!({"qos": 2})).contains("qos"));
        assert!(bad(serde_json::json!({"clean_session": false})).contains("client_id"));
        assert!(bad(serde_json::json!({"url": "http://b"})).contains("mqtt://"));
    }

    /// Against a real broker: set `OT_TEST_MQTT_URL` (e.g. `mqtt://127.0.0.1:11883`).
    #[tokio::test]
    async fn mqtt_messages_are_frames_with_their_topic() {
        let Ok(url) = std::env::var("OT_TEST_MQTT_URL") else {
            eprintln!("skipped: OT_TEST_MQTT_URL not set");
            return;
        };
        let run_id = format!("ottest{}", Utc::now().timestamp_micros());
        let config = TransportConfig::Mqtt {
            url: url.clone(),
            topics: vec![format!("{run_id}/+/pos")],
            qos: 1,
            client_id: None,
            username: None,
            password: None,
            clean_session: true,
            keepalive_secs: 30.0,
            ca_file: None,
            tls: None,
        };
        let (tx, mut rx) = mpsc::channel(8);
        let status = SharedStatus::default();
        let task_status = status.clone();
        let task = tokio::spawn(async move { run(&config, None, tx, task_status).await });
        for _ in 0..100 {
            if status.lock().unwrap().connected {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(status.lock().unwrap().connected, "never subscribed");

        let (_, host, port) = mqtt_endpoint(&url).unwrap();
        let (client, mut events) = rumqttc::AsyncClient::new(
            rumqttc::MqttOptions::new(format!("{run_id}-pub"), host, port),
            16,
        );
        let publisher = tokio::spawn(async move { while events.poll().await.is_ok() {} });
        client
            .publish(
                format!("{run_id}/other/vel"),
                rumqttc::QoS::AtLeastOnce,
                false,
                "no",
            )
            .await
            .unwrap();
        client
            .publish(
                format!("{run_id}/366123456/pos"),
                rumqttc::QoS::AtLeastOnce,
                false,
                r#"{"lat":32.7}"#,
            )
            .await
            .unwrap();
        let frame = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("no frame")
            .unwrap();
        assert_eq!(&frame.bytes[..], br#"{"lat":32.7}"#);
        assert_eq!(frame.meta["topic"], format!("{run_id}/366123456/pos"));
        assert_eq!(frame.meta["topic_levels"][1], "366123456");
        // The non-matching topic never arrived.
        assert!(rx.try_recv().is_err());
        task.abort();
        publisher.abort();
    }

    /// A gRPC server source with mutual TLS: only producers holding a
    /// certificate from the trusted CA get in, and the refusals show in the
    /// source's status.
    #[tokio::test]
    async fn a_grpc_server_with_mutual_tls_admits_only_trusted_producers() {
        let pki = Pki::new();
        let files = crate::proto::tests::files();
        let codec = crate::codec::CodecConfig::Protobuf {
            files: files.clone(),
            message: "acme.tracks.v1.TrackBatch".into(),
            records: None,
            context: Vec::new(),
        };
        let proto = ProtoContext::of(&codec).unwrap();
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = probe.local_addr().unwrap();
        drop(probe);
        let config = TransportConfig::GrpcServer {
            bind: addr.to_string(),
            methods: Vec::new(),
            tls: Some(pki.server(true)),
            token: None,
            max_message_kib: 64,
            max_connections: 8,
            keepalive_secs: 0.0,
        };
        let (tx, mut rx) = mpsc::channel(8);
        let status = SharedStatus::default();
        let task_status = status.clone();
        let task = tokio::spawn(async move {
            let _ = run(&config, proto.as_ref(), tx, task_status).await;
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        let set = crate::proto::ProtoSet::compile(&files).unwrap();
        let batch = set.message("acme.tracks.v1.TrackBatch").unwrap();
        let body = set
            .encode(&batch, &serde_json::json!({"tracks": [{"track_id": "M1"}]}))
            .unwrap();
        let push = |cert: Option<&str>| {
            let t = pki.client(cert);
            crate::grpc::Subscription {
                url: format!("https://localhost:{}", addr.port()),
                path: "/acme.tracks.v1.TrackFeed/Push".into(),
                request: body.clone(),
                metadata: BTreeMap::new(),
                tls: Some((
                    t.client_config().unwrap(),
                    crate::tls::server_name("localhost").unwrap(),
                )),
                max_message: 1 << 16,
                keepalive: None,
                connect_timeout: Duration::from_secs(5),
            }
        };
        let ok = crate::grpc::subscribe(&push(Some("client-a")), || {}, |_| async { Ok(()) }).await;
        assert!(ok.is_ok(), "{ok:?}");
        let f = rx.recv().await.unwrap();
        assert!(
            f.meta["peer_subject"]
                .as_str()
                .unwrap()
                .contains("client-a"),
            "{:?}",
            f.meta
        );
        for who in [None, Some("client-x")] {
            let refused = crate::grpc::subscribe(&push(who), || {}, |_| async { Ok(()) }).await;
            assert!(refused.is_err(), "{who:?} got in");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        let s = status.lock().unwrap().clone();
        assert!(s.errors >= 2, "{s:?}");
        assert!(s.last_error.unwrap().contains("TLS"));
        task.abort();
    }

    /// Test certificates: a CA, a server certificate for localhost and
    /// 127.0.0.1, a client certificate from the CA and one from a stranger.
    struct Pki {
        dir: tempfile::TempDir,
    }

    impl Pki {
        fn new() -> Pki {
            use rcgen::{
                BasicConstraints, CertificateParams, CertifiedIssuer, DnType,
                ExtendedKeyUsagePurpose, IsCa, KeyPair, KeyUsagePurpose,
            };
            let dir = tempfile::tempdir().unwrap();
            let write =
                |name: &str, pem: String| std::fs::write(dir.path().join(name), pem).unwrap();
            let ca = |cn: &str| {
                let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
                params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
                params.distinguished_name.push(DnType::CommonName, cn);
                params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
                CertifiedIssuer::self_signed(params, KeyPair::generate().unwrap()).unwrap()
            };
            let (ours, stranger) = (ca("OpenTrack test CA"), ca("Somebody else"));
            write("ca.pem", ours.pem());
            let leaf =
                |name: &str, sans: Vec<String>, usage, issuer: &CertifiedIssuer<'_, KeyPair>| {
                    let mut params = CertificateParams::new(sans).unwrap();
                    // Known serials, for the revocation lists below.
                    let serial: u64 = match name {
                        "client-a" => 0xa1,
                        "client-r" => 0xbad,
                        _ => 0x51,
                    };
                    params.serial_number = Some(serial.into());
                    params.distinguished_name.push(DnType::CommonName, name);
                    params
                        .distinguished_name
                        .push(DnType::OrganizationName, "OpenTrack test");
                    params.extended_key_usages = vec![usage];
                    let key = KeyPair::generate().unwrap();
                    let cert = params.signed_by(&key, issuer).unwrap();
                    write(&format!("{name}.pem"), cert.pem());
                    write(&format!("{name}.key"), key.serialize_pem());
                };
            let server = ExtendedKeyUsagePurpose::ServerAuth;
            let client = ExtendedKeyUsagePurpose::ClientAuth;
            leaf(
                "server",
                vec!["localhost".into(), "127.0.0.1".into()],
                server,
                &ours,
            );
            leaf("client-a", vec![], client.clone(), &ours);
            leaf("client-r", vec![], client.clone(), &ours);
            leaf("client-x", vec![], client, &stranger);
            // Revocation lists from our CA: a current one revoking client-r,
            // and one past its next update.
            let crl = |name: &str, next_year: i32| {
                use rcgen::{CertificateRevocationListParams, KeyIdMethod, RevokedCertParams};
                let list = CertificateRevocationListParams {
                    this_update: rcgen::date_time_ymd(2020, 1, 1),
                    next_update: rcgen::date_time_ymd(next_year, 1, 1),
                    crl_number: 1u64.into(),
                    issuing_distribution_point: None,
                    revoked_certs: vec![RevokedCertParams {
                        serial_number: 0xbadu64.into(),
                        revocation_time: rcgen::date_time_ymd(2020, 1, 1),
                        reason_code: None,
                        invalidity_date: None,
                    }],
                    key_identifier_method: KeyIdMethod::Sha256,
                }
                .signed_by(&ours)
                .unwrap();
                write(name, list.pem().unwrap());
            };
            crl("crl.pem", 2099);
            crl("crl-expired.pem", 2021);
            Pki { dir }
        }

        fn path(&self, name: &str) -> String {
            self.dir.path().join(name).display().to_string()
        }

        /// Client settings trusting our CA, optionally with a client certificate.
        fn client(&self, cert: Option<&str>) -> ClientTls {
            ClientTls {
                ca_file: Some(self.path("ca.pem")),
                cert_file: cert.map(|c| self.path(&format!("{c}.pem"))),
                key_file: cert.map(|c| self.path(&format!("{c}.key"))),
                ..Default::default()
            }
        }

        fn server(&self, mutual: bool) -> ServerTls {
            ServerTls {
                cert_file: self.path("server.pem"),
                key_file: self.path("server.key"),
                client_ca_file: mutual.then(|| self.path("ca.pem")),
                client_cert_optional: false,
                client_crl_files: Vec::new(),
            }
        }
    }

    /// Start a TLS `tcp_server` on a free port.
    async fn tls_server(
        tls: ServerTls,
    ) -> (
        SocketAddr,
        mpsc::Receiver<Frame>,
        SharedStatus,
        tokio::task::JoinHandle<()>,
    ) {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = probe.local_addr().unwrap();
        drop(probe);
        let config = TransportConfig::TcpServer {
            bind: addr.to_string(),
            framing: lines(),
            tls: Some(tls),
        };
        let (tx, rx) = mpsc::channel(8);
        let status = SharedStatus::default();
        let task_status = status.clone();
        let task = tokio::spawn(async move {
            let _ = run(&config, None, tx, task_status).await;
        });
        while tokio::net::TcpStream::connect(addr).await.is_err() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        (addr, rx, status, task)
    }

    /// Run a `tcp_client` that sends one line, until it ends or `wait` passes.
    async fn tls_client(
        addr: SocketAddr,
        host: &str,
        tls: ClientTls,
        wait: Duration,
    ) -> Option<anyhow::Result<()>> {
        let config = TransportConfig::TcpClient {
            host: host.into(),
            port: addr.port(),
            framing: lines(),
            send_on_connect: Some("hello\n".into()),
            tls: Some(tls),
        };
        let (tx, _rx) = mpsc::channel(8);
        tokio::time::timeout(wait, run(&config, None, tx, SharedStatus::default()))
            .await
            .ok()
    }

    #[tokio::test]
    async fn mutual_tls_server_accepts_only_clients_the_ca_signed() {
        let pki = Pki::new();
        let (addr, mut rx, status, task) = tls_server(pki.server(true)).await;
        let wait = Duration::from_millis(500);

        // A client certificate from the CA: its line arrives.
        let client = tokio::spawn(tls_client(
            addr,
            "localhost",
            pki.client(Some("client-a")),
            Duration::from_secs(5),
        ));
        let frame = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("no frame from the trusted client")
            .unwrap();
        assert_eq!(&frame.bytes[..], b"hello");
        client.abort();

        // No client certificate, or one from another CA: nothing arrives,
        // and the server counts the rejection.
        for cert in [None, Some("client-x")] {
            let errors = status.lock().unwrap().errors;
            let _ = tls_client(addr, "127.0.0.1", pki.client(cert), wait).await;
            assert!(
                tokio::time::timeout(wait, rx.recv()).await.is_err(),
                "a frame from an untrusted client ({cert:?})"
            );
            assert_eq!(status.lock().unwrap().errors, errors + 1, "{cert:?}");
            assert!(
                status
                    .lock()
                    .unwrap()
                    .last_error
                    .as_deref()
                    .unwrap()
                    .contains("TLS handshake")
            );
        }
        task.abort();
    }

    #[tokio::test]
    async fn a_revoked_client_certificate_is_refused() {
        let pki = Pki::new();
        let with_crl = |file: &str| ServerTls {
            client_crl_files: vec![pki.path(file)],
            ..pki.server(true)
        };
        let wait = Duration::from_millis(500);
        let (addr, mut rx, _status, task) = tls_server(with_crl("crl.pem")).await;
        // Not on the list: in.
        let client = tokio::spawn(tls_client(
            addr,
            "localhost",
            pki.client(Some("client-a")),
            Duration::from_secs(5),
        ));
        let frame = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("no frame from the good client")
            .unwrap();
        assert_eq!(&frame.bytes[..], b"hello");
        client.abort();
        // Revoked: refused.
        let _ = tls_client(addr, "localhost", pki.client(Some("client-r")), wait).await;
        assert!(
            tokio::time::timeout(wait, rx.recv()).await.is_err(),
            "a revoked client got in"
        );
        task.abort();

        // A list past its next update fails closed, even for a good client.
        let (addr, mut rx, _status, task) = tls_server(with_crl("crl-expired.pem")).await;
        let _ = tls_client(addr, "localhost", pki.client(Some("client-a")), wait).await;
        assert!(
            tokio::time::timeout(wait, rx.recv()).await.is_err(),
            "an expired list let one in"
        );
        task.abort();

        // A directory stands for the lists in it.
        let dir = tempfile::tempdir().unwrap();
        std::fs::copy(pki.path("crl.pem"), dir.path().join("ours.crl")).unwrap();
        let tls = ServerTls {
            client_crl_files: vec![dir.path().display().to_string()],
            ..pki.server(true)
        };
        let before = tls.crl_stamp();
        assert!(tls.acceptor().is_ok());
        std::fs::write(dir.path().join("more.crl"), b"").unwrap();
        assert_ne!(tls.crl_stamp(), before, "a new file changes the stamp");
    }

    #[tokio::test]
    async fn tls_client_trusts_its_ca_file_and_checks_the_name() {
        let pki = Pki::new();
        let (addr, mut rx, _status, task) = tls_server(pki.server(false)).await;

        // By IP address (in the certificate), then under a server_name override.
        let by_name = ClientTls {
            server_name: Some("localhost".into()),
            ..pki.client(None)
        };
        for tls in [pki.client(None), by_name] {
            let client = tokio::spawn(tls_client(addr, "127.0.0.1", tls, Duration::from_secs(5)));
            let frame = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .expect("no frame over TLS")
                .unwrap();
            assert_eq!(&frame.bytes[..], b"hello");
            client.abort();
        }

        // A name the certificate does not cover, and a CA that did not sign it.
        let wrong_name = ClientTls {
            server_name: Some("elsewhere.example".into()),
            ..pki.client(None)
        };
        let wrong_ca = ClientTls {
            ca_file: Some(pki.path("client-x.pem")),
            ..Default::default()
        };
        for tls in [wrong_name, wrong_ca] {
            let err = tls_client(addr, "127.0.0.1", tls, Duration::from_secs(5))
                .await
                .expect("handshake did not fail")
                .unwrap_err();
            assert!(format!("{err:#}").contains("TLS handshake"), "{err:#}");
        }

        // Development escape hatch: no verification at all.
        let insecure = ClientTls {
            insecure_skip_verify: true,
            ..Default::default()
        };
        let client = tokio::spawn(tls_client(
            addr,
            "127.0.0.1",
            insecure,
            Duration::from_secs(5),
        ));
        assert!(
            tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .unwrap()
                .is_some()
        );
        client.abort();
        task.abort();
    }

    /// A mutual-TLS listener on a free port that hands each verified
    /// connection to `serve`.
    async fn mutual_tls_listener<F, Fut>(pki: &Pki, serve: F) -> (u16, tokio::task::JoinHandle<()>)
    where
        F: Fn(tokio_rustls::server::TlsStream<tokio::net::TcpStream>) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let acceptor = pki.server(true).acceptor().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                if let Ok(tls) = acceptor.accept(stream).await {
                    tokio::spawn(serve(tls));
                }
            }
        });
        (port, task)
    }

    async fn first_frame(config: TransportConfig) -> Frame {
        let (tx, mut rx) = mpsc::channel(8);
        let task =
            tokio::spawn(async move { run(&config, None, tx, SharedStatus::default()).await });
        let frame = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("no frame")
            .unwrap();
        task.abort();
        frame
    }

    #[tokio::test]
    async fn https_and_wss_present_the_client_certificate() {
        let pki = Pki::new();
        let (port, server) = mutual_tls_listener(&pki, |mut tls| async move {
            // Read the request head, then answer.
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                match tls.read_u8().await {
                    Ok(b) => head.push(b),
                    Err(_) => return,
                }
            }
            let _ = tls
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok")
                .await;
            let _ = tls.shutdown().await;
        })
        .await;
        let frame = first_frame(TransportConfig::HttpPoll {
            url: format!("https://localhost:{port}/feed"),
            interval_secs: 0.1,
            method: "GET".into(),
            headers: BTreeMap::new(),
            body: None,
            timeout_secs: 5.0,
            tls: Some(pki.client(Some("client-a"))),
        })
        .await;
        assert_eq!(&frame.bytes[..], b"ok");
        server.abort();

        let (port, server) = mutual_tls_listener(&pki, |tls| async move {
            let Ok(mut ws) = tokio_tungstenite::accept_async(tls).await else {
                return;
            };
            let _ = ws.send(Message::text("{\"id\":1}")).await;
            // Hold the connection open until the client goes.
            while ws.next().await.is_some() {}
        })
        .await;
        let frame = first_frame(TransportConfig::Websocket {
            url: format!("wss://127.0.0.1:{port}/stream"),
            headers: BTreeMap::new(),
            subscribe: None,
            error_path: None,
            ping_secs: 20.0,
            tls: Some(ClientTls {
                server_name: Some("localhost".into()),
                ..pki.client(Some("client-a"))
            }),
        })
        .await;
        assert_eq!(&frame.bytes[..], b"{\"id\":1}");
        server.abort();
    }

    #[tokio::test]
    async fn mqtts_uses_the_old_ca_file_with_a_client_certificate() {
        async fn packet(s: &mut (impl AsyncRead + Unpin)) -> std::io::Result<(u8, Vec<u8>)> {
            let kind = s.read_u8().await?;
            let (mut len, mut shift) = (0usize, 0);
            loop {
                let b = s.read_u8().await?;
                len |= usize::from(b & 0x7f) << shift;
                shift += 7;
                if b & 0x80 == 0 {
                    break;
                }
            }
            let mut body = vec![0; len];
            s.read_exact(&mut body).await?;
            Ok((kind, body))
        }
        let pki = Pki::new();
        // Just enough of a broker: accept the session and the subscription,
        // then publish one message.
        let (port, server) = mutual_tls_listener(&pki, |mut tls| async move {
            while let Ok((kind, body)) = packet(&mut tls).await {
                let reply: Vec<u8> = match kind >> 4 {
                    1 => vec![0x20, 2, 0, 0],
                    8 => [
                        &[0x90, 3, body[0], body[1], 0][..],
                        b"\x30\x07\x00\x03a/bhi",
                    ]
                    .concat(),
                    12 => vec![0xd0, 0],
                    _ => continue,
                };
                if tls.write_all(&reply).await.is_err() {
                    return;
                }
            }
        })
        .await;
        let mut spec = serde_json::json!({
            "type": "mqtt", "url": format!("mqtts://localhost:{port}"), "topics": ["a/#"],
            "ca_file": pki.path("ca.pem"),
        });
        let client = pki.client(Some("client-a"));
        spec["tls"] =
            serde_json::json!({"cert_file": client.cert_file, "key_file": client.key_file});
        let frame = first_frame(serde_json::from_value(spec).unwrap()).await;
        assert_eq!(&frame.bytes[..], b"hi");
        assert_eq!(frame.meta["topic"], "a/b");
        server.abort();
    }

    #[test]
    fn tls_files_fail_clearly() {
        let pki = Pki::new();
        let err = ClientTls {
            ca_file: Some(pki.path("server.key")),
            ..Default::default()
        }
        .client_config()
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            format!(
                "tls.ca_file {}: no certificates found",
                pki.path("server.key")
            )
        );
        let err = ClientTls {
            cert_file: Some(pki.path("client-a.pem")),
            key_file: Some(pki.path("client-a.pem")),
            ..pki.client(None)
        }
        .client_config()
        .unwrap_err();
        assert!(err.to_string().ends_with("no private key found"), "{err}");
        let err = ServerTls {
            cert_file: pki.path("missing.pem"),
            ..pki.server(false)
        }
        .acceptor()
        .err()
        .unwrap();
        assert!(format!("{err:#}").starts_with("tls.cert_file "), "{err:#}");
        // Paths resolve environment references.
        assert!(
            ClientTls {
                ca_file: Some("${env:OT_SURELY_UNSET_VAR}/ca.pem".into()),
                ..Default::default()
            }
            .client_config()
            .is_err()
        );
        let subject = crate::tls::subject(
            &<rustls::pki_types::CertificateDer as rustls::pki_types::pem::PemObject>::from_pem_file(pki.path("client-a.pem")).unwrap(),
        );
        assert_eq!(subject.as_deref(), Some("CN=client-a, O=OpenTrack test"));
    }

    #[test]
    fn tls_settings_round_trip_and_check() {
        let spec = serde_json::json!({
            "type": "tcp_client", "host": "feed.local", "port": 8089,
            "tls": {
                "ca_file": "/etc/ot/ca.pem", "cert_file": "/etc/ot/me.pem",
                "key_file": "${env:OT_KEY_FILE}", "server_name": "feed.example"
            }
        });
        let t: TransportConfig = serde_json::from_value(spec.clone()).unwrap();
        assert_eq!(t.check(), Ok(()));
        let back = serde_json::to_value(&t).unwrap();
        assert_eq!(back["tls"], spec["tls"]);
        assert_eq!(serde_json::from_value::<TransportConfig>(back).unwrap(), t);

        let server: TransportConfig = serde_json::from_value(serde_json::json!({
            "type": "tcp_server", "bind": "0.0.0.0:8089",
            "tls": { "cert_file": "s.pem", "key_file": "s.key", "client_ca_file": "ca.pem" }
        }))
        .unwrap();
        assert_eq!(
            serde_json::from_value::<TransportConfig>(serde_json::to_value(&server).unwrap())
                .unwrap(),
            server
        );
        // Unknown TLS settings are refused, as elsewhere.
        assert!(
            serde_json::from_value::<TransportConfig>(serde_json::json!({
                "type": "websocket", "url": "wss://x", "tls": { "verify": false }
            }))
            .is_err()
        );

        let check = |v: serde_json::Value| {
            serde_json::from_value::<TransportConfig>(v)
                .unwrap()
                .check()
        };
        assert!(
            check(serde_json::json!({"type": "http_poll", "url": "https://x", "tls": {"key_file": "k.pem"}}))
                .unwrap_err()
                .contains("tls.key_file needs a tls.cert_file")
        );
        assert!(
            check(serde_json::json!({"type": "websocket", "url": "wss://x", "tls": {"cert_file": "c.pem"}}))
                .unwrap_err()
                .contains("tls.cert_file needs a tls.key_file")
        );
        assert!(
            check(serde_json::json!({"type": "websocket", "url": "ws://x", "tls": {}}))
                .unwrap_err()
                .contains("wss://")
        );
        assert!(
            check(serde_json::json!({"type": "tcp_server", "bind": "0.0.0.0:1", "tls": {"cert_file": "", "key_file": "k"}}))
                .unwrap_err()
                .contains("cert_file")
        );
    }

    #[test]
    fn mqtt_ca_file_still_parses() {
        let old = serde_json::json!({
            "type": "mqtt", "url": "mqtts://broker.local", "topics": ["t"], "ca_file": "/etc/ot/ca.pem"
        });
        let t: TransportConfig = serde_json::from_value(old.clone()).unwrap();
        assert!(matches!(
            &t,
            TransportConfig::Mqtt {
                ca_file: Some(_),
                tls: None,
                ..
            }
        ));
        assert_eq!(t.check(), Ok(()));
        assert_eq!(serde_json::to_value(&t).unwrap()["ca_file"], old["ca_file"]);

        // With a client certificate alongside; but only one CA.
        let mut both = old.clone();
        both["tls"] = serde_json::json!({"cert_file": "c.pem", "key_file": "c.key"});
        let t: TransportConfig = serde_json::from_value(both.clone()).unwrap();
        assert_eq!(t.check(), Ok(()));
        both["tls"]["ca_file"] = "other.pem".into();
        let t: TransportConfig = serde_json::from_value(both).unwrap();
        assert!(t.check().unwrap_err().contains("CA once"));
        let mut plain = old;
        plain["url"] = "mqtt://broker.local".into();
        plain["tls"] = serde_json::json!({});
        let t: TransportConfig = serde_json::from_value(plain).unwrap();
        assert!(t.check().unwrap_err().contains("mqtts://"));
    }
}
