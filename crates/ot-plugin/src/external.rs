//! External plugins: a program of its own serving the plugin interface over
//! a socket, for code that cannot run as WebAssembly (Python with numpy or
//! Stone Soup, a GPU, a licensed library). It runs where the operator
//! starts it, with whatever it needs; OpenTrack only connects.
//!
//! The protocol is JSON lines. Each request is `{"id", "method", "params"}`
//! and each reply `{"id", "result"}` or `{"id", "error"}`. Every decoder,
//! tracker or scorer OpenTrack opens is one connection: it starts with
//! `open` (`{"kind": "codec" | "tracker" | "scorer", "options"}`), and its
//! state lives with that connection on the plugin's side.
//!
//! Every connection first authenticates both ends ([`crate::handshake`]:
//! `hello`, then `verify`) with the secret the operator shared with the
//! plugin. Only a unix socket under OpenTrack's data directory may go
//! without one: filesystem permissions decide who can listen there.
//!
//! | method | params | result |
//! |---|---|---|
//! | `hello` | `protocol`, `challenge` | `proof`, `challenge` |
//! | `verify` | `proof` | null |
//! | `describe` | | the manifest |
//! | `open` | `kind`, `options` | null |
//! | `decode` | `frame` (base64), `received_at_ms` | records |
//! | `hints` | | hints or null |
//! | `push` | `plot` (observation), `received_at_ms` | null |
//! | `run` | `now_ms`, `force` | tracks (observations) |
//! | `score` | `report`, `candidates` | scores |

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use base64::Engine as _;
use chrono::{DateTime, Utc};
use ot_core::Observation;
use ot_source::plugin::{
    Manifest, Plugin, PluginDecoder, PluginScorer, PluginTracker, Score, ScoreCandidate,
    StreamHints,
};
use serde_json::{Value, json};

use crate::handshake::{self, NONCE_LEN, PROTOCOL, Role};
use crate::{External, Grants};

/// Time allowed to connect.
const CONNECT: Duration = Duration::from_secs(5);

/// The plugin failed the handshake (a wrong secret, a wrong proof of
/// OpenTrack's, or no answer): it is refused.
#[derive(Debug)]
pub struct AuthFailed(pub String);

impl std::fmt::Display for AuthFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "authentication failed: {}", self.0)
    }
}

impl std::error::Error for AuthFailed {}

/// The plugin has no secret, and is not on a unix socket under the data
/// directory: it is not connected to at all.
#[derive(Debug)]
pub struct NoSecret;

impl std::fmt::Display for NoSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "no secret: an external plugin must share a secret with OpenTrack (set one in \
             Settings → Plugins, and start the plugin with it); only a unix socket under the \
             data directory may go without",
        )
    }
}

impl std::error::Error for NoSecret {}

pub struct ExternalPlugin {
    ext: External,
    manifest: Manifest,
    timeout: Duration,
}

impl ExternalPlugin {
    /// Connect, authenticate, and read the plugin's manifest.
    pub fn connect(ext: &External, grants: &Grants) -> anyhow::Result<Self> {
        let timeout = Duration::from_millis(grants.call_timeout_ms.max(1));
        let mut c = open(ext, timeout)?;
        let manifest = c
            .call("describe", Value::Null)
            .map_err(anyhow::Error::msg)?;
        Ok(Self {
            ext: ext.clone(),
            manifest: serde_json::from_value(manifest).context("the plugin's manifest")?,
            timeout,
        })
    }

    fn session(&self, kind: &str, options: &Value) -> Result<Conn, String> {
        let mut c = open(&self.ext, self.timeout).map_err(|e| format!("{e:#}"))?;
        let options = if options.is_null() {
            json!({})
        } else {
            options.clone()
        };
        c.call("open", json!({ "kind": kind, "options": options }))?;
        Ok(c)
    }
}

/// The secret to authenticate with: None only for a unix socket under the
/// data directory.
fn secret_for(ext: &External) -> anyhow::Result<Option<String>> {
    match ext
        .secret
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(s) => {
            let s = ot_source::transport::resolve_env(s).context("the plugin's secret")?;
            handshake::check_secret(&s).map_err(anyhow::Error::msg)?;
            Ok(Some(s))
        }
        None if trusted_socket(&ext.address, ext.data_dir.as_deref()) => Ok(None),
        None => Err(NoSecret.into()),
    }
}

/// Whether an address is a unix socket under the data directory (both
/// resolved, so a link out of it does not count).
pub fn trusted_socket(address: &str, data_dir: Option<&Path>) -> bool {
    let (Some(path), Some(dir)) = (address.strip_prefix("unix:"), data_dir) else {
        return false;
    };
    match (std::fs::canonicalize(path), std::fs::canonicalize(dir)) {
        (Ok(p), Ok(d)) => p.starts_with(d),
        _ => false,
    }
}

/// A connection that has passed the handshake (or needs none).
fn open(ext: &External, timeout: Duration) -> anyhow::Result<Conn> {
    let secret = secret_for(ext)?;
    let mut c = Conn::open(&ext.address, timeout)?;
    if let Some(secret) = secret {
        c.authenticate(&secret).inspect_err(|e| {
            tracing::error!(address = %ext.address, error = %e, "external plugin refused");
        })?;
    }
    Ok(c)
}

struct Conn {
    reader: BufReader<Box<dyn Read + Send>>,
    writer: Box<dyn Write + Send>,
    next: u64,
    broken: Option<String>,
}

impl Conn {
    fn open(address: &str, timeout: Duration) -> anyhow::Result<Self> {
        let (reader, writer): (Box<dyn Read + Send>, Box<dyn Write + Send>) = if let Some(path) =
            address.strip_prefix("unix:")
        {
            let s = UnixStream::connect(path).with_context(|| format!("connect {address}"))?;
            s.set_read_timeout(Some(timeout))?;
            tracing::info!(
                component = "plugin",
                address,
                peer_addr = path,
                "external plugin connected"
            );
            (Box::new(s.try_clone()?), Box::new(s))
        } else {
            let hostport = address.strip_prefix("tcp://").unwrap_or(address);
            let addr = hostport
                .to_socket_addrs()
                .with_context(|| format!("resolve {address}"))?
                .next()
                .with_context(|| format!("resolve {address}"))?;
            let s = TcpStream::connect_timeout(&addr, CONNECT)
                .with_context(|| format!("connect {address}"))?;
            s.set_read_timeout(Some(timeout))?;
            s.set_nodelay(true)?;
            match s.peer_addr() {
                Ok(peer) => {
                    tracing::info!(component = "plugin", address, peer_addr = %peer, "external plugin connected")
                }
                Err(e) => {
                    tracing::info!(component = "plugin", address, error = %e, "external plugin connected; peer address unknown")
                }
            }
            (Box::new(s.try_clone()?), Box::new(s))
        };
        Ok(Self {
            reader: BufReader::new(reader),
            writer,
            next: 1,
            broken: None,
        })
    }

    /// Prove each end to the other ([`crate::handshake`]).
    fn authenticate(&mut self, secret: &str) -> Result<(), AuthFailed> {
        let b64 = &base64::engine::general_purpose::STANDARD;
        let ours = handshake::nonce();
        let reply = self
            .call(
                "hello",
                json!({ "protocol": PROTOCOL, "challenge": b64.encode(ours) }),
            )
            .map_err(|e| {
                AuthFailed(format!(
                    "no answer to hello ({e}): is the plugin built with an SDK that \
                     authenticates, and started with its secret?"
                ))
            })?;
        let field = |k: &str| {
            reply
                .get(k)
                .and_then(Value::as_str)
                .and_then(|s| b64.decode(s).ok())
        };
        let (Some(tag), Some(theirs)) = (field("proof"), field("challenge")) else {
            return Err(AuthFailed(
                "the answer to hello has no proof and challenge".into(),
            ));
        };
        if theirs.len() != NONCE_LEN || theirs == ours {
            return Err(AuthFailed(format!(
                "the plugin's challenge must be {NONCE_LEN} fresh random bytes"
            )));
        }
        if !handshake::verify(secret, Role::Plugin, &ours, &theirs, &tag) {
            return Err(AuthFailed(
                "the plugin's proof is wrong: it does not have this secret".into(),
            ));
        }
        let mine = handshake::proof(secret, Role::OpenTrack, &ours, &theirs);
        self.call("verify", json!({ "proof": b64.encode(mine) }))
            .map_err(|e| {
                AuthFailed(format!(
                    "the plugin did not accept OpenTrack's proof ({e}): it has another secret"
                ))
            })?;
        Ok(())
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        if let Some(e) = &self.broken {
            return Err(format!("stopped after an earlier failure: {e}"));
        }
        let r = self.exchange(method, params);
        if let Err(e) = &r
            && !e.starts_with("plugin: ")
        {
            self.broken = Some(e.clone());
        }
        r
    }

    fn exchange(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next;
        self.next += 1;
        let mut line = json!({ "id": id, "method": method, "params": params }).to_string();
        line.push('\n');
        self.writer
            .write_all(line.as_bytes())
            .and_then(|_| self.writer.flush())
            .map_err(|e| format!("send {method}: {e}"))?;
        let mut reply = String::new();
        let n = self
            .reader
            .read_line(&mut reply)
            .map_err(|e| format!("{method}: no reply ({e})"))?;
        if n == 0 {
            return Err(format!("{method}: the plugin closed the connection"));
        }
        let mut v: Value = serde_json::from_str(&reply)
            .map_err(|e| format!("{method}: a reply is not JSON: {e}"))?;
        if v["id"] != json!(id) {
            return Err(format!("{method}: reply to {} instead of {id}", v["id"]));
        }
        if let Some(e) = v.get("error").filter(|e| !e.is_null()) {
            return Err(format!(
                "plugin: {}",
                e.as_str().map_or_else(|| e.to_string(), str::to_owned)
            ));
        }
        Ok(v["result"].take())
    }
}

impl Plugin for ExternalPlugin {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn decoder(&self, options: &Value) -> Result<Box<dyn PluginDecoder>, String> {
        Ok(Box::new(Decoder {
            conn: self.session("codec", options)?,
            hints: None,
        }))
    }

    fn tracker(&self, options: &Value) -> Result<Box<dyn PluginTracker>, String> {
        Ok(Box::new(Session(self.session("tracker", options)?)))
    }

    fn scorer(&self, options: &Value) -> Result<Box<dyn PluginScorer>, String> {
        Ok(Box::new(Session(self.session("scorer", options)?)))
    }
}

struct Decoder {
    conn: Conn,
    hints: Option<StreamHints>,
}

impl PluginDecoder for Decoder {
    fn decode(&mut self, bytes: &[u8], received_at: DateTime<Utc>) -> Result<Vec<Value>, String> {
        let frame = base64::engine::general_purpose::STANDARD.encode(bytes);
        let records = self.conn.call(
            "decode",
            json!({ "frame": frame, "received_at_ms": received_at.timestamp_millis() }),
        )?;
        if let Ok(h) = self.conn.call("hints", Value::Null) {
            self.hints = StreamHints::from_json(&h);
        }
        match records {
            Value::Array(r) => Ok(r),
            Value::Null => Ok(Vec::new()),
            other => Err(format!("decode returned {other}, not a list")),
        }
    }

    fn hints(&self) -> Option<StreamHints> {
        self.hints.clone()
    }
}

struct Session(Conn);

impl PluginTracker for Session {
    fn push(&mut self, plot: Observation, received_at: DateTime<Utc>) {
        let _ = self.0.call(
            "push",
            json!({ "plot": plot, "received_at_ms": received_at.timestamp_millis() }),
        );
    }

    fn run(&mut self, now: DateTime<Utc>, force: bool) -> Result<Vec<Observation>, String> {
        let tracks = self.0.call(
            "run",
            json!({ "now_ms": now.timestamp_millis(), "force": force }),
        )?;
        match tracks {
            Value::Null => Ok(Vec::new()),
            v => {
                serde_json::from_value(v).map_err(|e| format!("a track is not an observation: {e}"))
            }
        }
    }
}

impl PluginScorer for Session {
    fn score(
        &mut self,
        report: &Observation,
        candidates: &[ScoreCandidate],
    ) -> Result<Vec<Score>, String> {
        let scores = self.0.call(
            "score",
            json!({ "report": report, "candidates": candidates }),
        )?;
        serde_json::from_value(scores).map_err(|e| format!("scores: {e}"))
    }
}
