//! Hot state in Redis: observation streams, live track state and the outbox
//! the track writer consumes.

use std::time::Duration;

use chrono::Utc;
use ot_core::{Observation, SystemTrack, Uid};
use redis::aio::ConnectionManager;
use redis::streams::StreamReadReply;

use crate::keys::Keys;
use crate::sqlite::{Result, StoreError};

/// The error of an observation stream entry trimmed (by the stream's length
/// cap) before its consumer acknowledged it: nothing to read, only to ack.
pub const TRIMMED: &str = "trimmed before it was read";

/// Marks an outbox entry id from the control stream (deletes), which is
/// never trimmed, unlike the publication stream.
const CONTROL_TAG: &str = "c:";

/// Work item for the track writer.
#[derive(Debug, Clone, PartialEq)]
pub enum OutboxOp {
    /// Publish the current state of a system track (read from `sys:<uid>`
    /// at publish time, so bursts of updates coalesce naturally). `urgent`
    /// bypasses the writer's minimum interval, for identity and
    /// classification changes.
    Publish { uid: Uid, urgent: bool },
    /// Publish a delete for a retired (dropped, merged-away or deleted) track.
    Tombstone { uid: Uid, reason: Option<String> },
    /// Publish that a point of a track's history was deleted.
    HistoryDelete {
        uid: Uid,
        point: HistoryPoint,
        reason: Option<String>,
        decision: Option<i64>,
    },
}

/// A system track's position at one time, as published.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct HistoryPoint {
    /// Observed at (Unix ms).
    pub t: i64,
    pub lat: f64,
    pub lon: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alt: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub course: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<f64>,
}

impl HistoryPoint {
    /// Stored compactly (`t,lat,lon,alt,course,speed`, blanks for none,
    /// positions to 1e-6°): a point costs about half its JSON in Redis.
    fn encode(&self) -> String {
        let opt =
            |v: Option<f64>, digits: usize| v.map_or(String::new(), |v| format!("{v:.digits$}"));
        format!(
            "{},{:.6},{:.6},{},{},{}",
            self.t,
            self.lat,
            self.lon,
            opt(self.alt, 1),
            opt(self.course, 1),
            opt(self.speed, 2)
        )
    }

    fn decode(s: &str) -> Option<Self> {
        let mut f = s.split(',');
        let mut next = || f.next();
        let opt = |v: Option<&str>| v.filter(|x| !x.is_empty()).and_then(|x| x.parse().ok());
        Some(Self {
            t: next()?.parse().ok()?,
            lat: next()?.parse().ok()?,
            lon: next()?.parse().ok()?,
            alt: opt(next()),
            course: opt(next()),
            speed: opt(next()),
        })
    }

    pub fn of(track: &SystemTrack) -> Self {
        let v = &track.view;
        Self {
            t: v.observed_at.timestamp_millis(),
            lat: v.position.latitude,
            lon: v.position.longitude,
            alt: v.position.altitude_hae_m,
            course: v.kinematics.course_deg,
            speed: v.kinematics.speed_mps,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct OutboxEntry {
    pub id: String,
    pub op: OutboxOp,
}

/// Backlog of a stream consumer group.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct GroupBacklog {
    /// Delivered, not yet acknowledged.
    pub pending: u64,
    /// Not yet delivered.
    pub lag: u64,
}

/// What [`RedisStore::write_batch`] does with a system track besides storing it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TrackWrite<'a> {
    /// Stored only (not authoritative enough to leave OpenTrack).
    Quiet,
    /// Published.
    Publish { urgent: bool },
    /// Withdrawn downstream (consumers delete it), with the reason.
    Withdraw { reason: &'a str },
}

#[derive(Clone)]
pub struct RedisStore {
    conn: ConnectionManager,
    keys: Keys,
    /// How much of each observation stream to keep.
    pub obs_window: Duration,
    /// Approximate cap on the outbox stream.
    pub outbox_maxlen: usize,
}

/// TLS to Redis beyond a `rediss://` URL's defaults (system roots).
#[derive(Debug, Clone, Default)]
pub struct RedisTls {
    /// PEM of the CA that signed the server's certificate.
    pub ca: Option<Vec<u8>>,
    /// PEM client certificate and key, for mutual TLS.
    pub client: Option<(Vec<u8>, Vec<u8>)>,
}

/// The server a Redis client dials: host and port, or a Unix socket path.
#[derive(Debug, Clone, PartialEq)]
enum RedisDestination {
    Tcp(String, u16),
    Unix(String),
}

fn redis_destination(addr: &redis::ConnectionAddr) -> RedisDestination {
    match addr {
        redis::ConnectionAddr::Tcp(host, port)
        | redis::ConnectionAddr::TcpTls { host, port, .. } => {
            RedisDestination::Tcp(host.clone(), *port)
        }
        redis::ConnectionAddr::Unix(path) => RedisDestination::Unix(path.display().to_string()),
        other => RedisDestination::Unix(other.to_string()),
    }
}

/// Log where Redis is (ASD STIG V-222470). The connection manager keeps its
/// socket to itself, so this is what the configured host resolves to now.
async fn log_redis_destination(dest: &RedisDestination, message: &str) {
    match dest {
        RedisDestination::Tcp(host, port) => {
            let peer_addr = match tokio::net::lookup_host((host.as_str(), *port)).await {
                Ok(addrs) => ot_core::netlog::addr_list(addrs),
                Err(e) => format!("unresolved ({e})"),
            };
            tracing::info!(component = "redis", server = %format!("{host}:{port}"), %peer_addr, "{message}");
        }
        RedisDestination::Unix(path) => {
            tracing::info!(component = "redis", server = %path, peer_addr = %path, "{message}");
        }
    }
}

/// How often the reconnect watcher pings Redis.
const WATCH_EVERY: Duration = Duration::from_secs(30);

/// The connection manager reconnects on its own and says nothing, so ping
/// it now and then and log the destination again when it comes back.
fn watch_reconnects(conn: ConnectionManager, dest: RedisDestination) {
    tokio::spawn(async move {
        let mut up = true;
        let mut tick = tokio::time::interval(WATCH_EVERY);
        tick.tick().await;
        loop {
            tick.tick().await;
            let ok = redis::cmd("PING")
                .query_async::<String>(&mut conn.clone())
                .await
                .is_ok();
            match (up, ok) {
                (true, false) => {
                    tracing::warn!(component = "redis", server = ?dest, "Redis unreachable");
                }
                (false, true) => {
                    log_redis_destination(&dest, "Redis reconnected (resolved address)").await
                }
                _ => {}
            }
            up = ok;
        }
    });
}

impl RedisStore {
    pub async fn connect(url: &str, keys: Keys) -> Result<Self> {
        Self::connect_tls(url, keys, None).await
    }

    /// As [`connect`](Self::connect); with `tls`, the URL must be `rediss://`.
    pub async fn connect_tls(url: &str, keys: Keys, tls: Option<RedisTls>) -> Result<Self> {
        let client = match tls {
            Some(t) => redis::Client::build_with_tls(
                url,
                redis::TlsCertificates {
                    client_tls: t
                        .client
                        .map(|(client_cert, client_key)| redis::ClientTlsConfig {
                            client_cert,
                            client_key,
                        }),
                    root_cert: t.ca,
                },
            )?,
            None => redis::Client::open(url)?,
        };
        // The default response timeout is shorter than the writer's blocking
        // XREADGROUP, which would then fail every idle poll.
        let config = redis::aio::ConnectionManagerConfig::new()
            .set_response_timeout(Some(Duration::from_secs(15)));
        let destination = redis_destination(client.get_connection_info().addr());
        let conn = ConnectionManager::new_with_config(client, config).await?;
        log_redis_destination(&destination, "Redis connected (resolved address)").await;
        watch_reconnects(conn.clone(), destination);
        Ok(Self {
            conn,
            keys,
            obs_window: Duration::from_secs(600),
            outbox_maxlen: 100_000,
        })
    }

    pub fn keys(&self) -> &Keys {
        &self.keys
    }

    pub async fn ping(&self) -> Result<()> {
        let _: String = redis::cmd("PING")
            .query_async(&mut self.conn.clone())
            .await?;
        Ok(())
    }

    /// Append a mapped observation to its source's stream, trimming entries
    /// older than the window.
    pub async fn append_observation(&self, obs: &Observation) -> Result<String> {
        let min_id = Utc::now().timestamp_millis() - self.obs_window.as_millis() as i64;
        Ok(redis::cmd("XADD")
            .arg(self.keys.obs_stream(&obs.source_id))
            .arg("MINID")
            .arg("~")
            .arg(min_id.max(0))
            .arg("*")
            .arg("obs")
            .arg(serde_json::to_string(obs)?)
            .query_async(&mut self.conn.clone())
            .await?)
    }

    /// Store a system track's current state and queue it for publishing, in
    /// one MULTI so the writer never sees a queued UID without its state.
    pub async fn put_system_track(&self, track: &SystemTrack, urgent: bool) -> Result<()> {
        let uid = track.uid.to_string();
        redis::pipe()
            .atomic()
            .set(self.keys.system_track(&uid), serde_json::to_string(track)?)
            .ignore()
            .cmd("XADD")
            .arg(self.keys.outbox())
            .arg("MAXLEN")
            .arg("~")
            .arg(self.outbox_maxlen)
            .arg("*")
            .arg("op")
            .arg("publish")
            .arg("uid")
            .arg(&uid)
            .arg("urgent")
            .arg(u8::from(urgent))
            .ignore()
            .query_async::<()>(&mut self.conn.clone())
            .await?;
        Ok(())
    }

    /// Store a system track's state without publishing it (a track not yet
    /// authoritative enough to leave OpenTrack).
    pub async fn put_system_track_quietly(&self, track: &SystemTrack) -> Result<()> {
        redis::cmd("SET")
            .arg(self.keys.system_track(&track.uid.to_string()))
            .arg(serde_json::to_string(track)?)
            .query_async::<()>(&mut self.conn.clone())
            .await?;
        Ok(())
    }

    /// Withdraw a published track: keep it (now unpublished) and tell
    /// consumers to delete it.
    pub async fn withdraw_system_track(&self, track: &SystemTrack, reason: &str) -> Result<()> {
        redis::pipe()
            .atomic()
            .cmd("SET")
            .arg(self.keys.system_track(&track.uid.to_string()))
            .arg(serde_json::to_string(track)?)
            .ignore()
            .cmd("XADD")
            .arg(self.keys.outbox_control())
            .arg("*")
            .arg("op")
            .arg("tombstone")
            .arg("uid")
            .arg(track.uid.to_string())
            .arg("reason")
            .arg(reason)
            .ignore()
            .query_async::<()>(&mut self.conn.clone())
            .await?;
        Ok(())
    }

    /// Many writes in one round trip, in order and atomically: source
    /// tracks' latest reports (with a TTL), then system tracks, each stored
    /// and, when published or withdrawn, queued for the writer. The engine
    /// uses it to write a whole batch of observations at once.
    pub async fn write_batch(
        &self,
        sources: &[&Observation],
        source_ttl: Duration,
        tracks: &[(&SystemTrack, TrackWrite<'_>, bool)],
        history: Option<Duration>,
    ) -> Result<()> {
        if sources.is_empty() && tracks.is_empty() {
            return Ok(());
        }
        let mut pipe = redis::pipe();
        pipe.atomic();
        for obs in sources {
            pipe.cmd("SET")
                .arg(
                    self.keys
                        .source_track(&obs.source_id, &obs.source_track_key),
                )
                .arg(serde_json::to_string(obs)?)
                .arg("EX")
                .arg(source_ttl.as_secs().max(1))
                .ignore();
        }
        for (track, write, record) in tracks {
            let uid = track.uid.to_string();
            pipe.set(self.keys.system_track(&uid), serde_json::to_string(track)?)
                .ignore();
            if let Some(keep) = history
                && *record
                && !matches!(write, TrackWrite::Withdraw { .. })
                && track.kind != ot_core::TrackKind::Group
            {
                self.add_history(&mut pipe, track, keep)?;
            }
            let queued = match write {
                TrackWrite::Quiet => None,
                TrackWrite::Publish { urgent } => {
                    Some(("publish", "urgent", u8::from(*urgent).to_string()))
                }
                TrackWrite::Withdraw { reason } => {
                    Some(("tombstone", "reason", (*reason).to_owned()))
                }
            };
            if let Some((op, key, value)) = queued {
                pipe.cmd("XADD");
                if op == "tombstone" {
                    pipe.arg(self.keys.outbox_control());
                } else {
                    pipe.arg(self.keys.outbox())
                        .arg("MAXLEN")
                        .arg("~")
                        .arg(self.outbox_maxlen);
                }
                pipe.arg("*")
                    .arg("op")
                    .arg(op)
                    .arg("uid")
                    .arg(&uid)
                    .arg(key)
                    .arg(value)
                    .ignore();
            }
        }
        pipe.query_async::<()>(&mut self.conn.clone()).await?;
        Ok(())
    }

    /// Record a track's position: one point per time (a later write at the
    /// same time replaces it), trimmed to `keep` before this point (so a
    /// replay of old data keeps its history too); the key expires `keep`
    /// after the track's last write.
    fn add_history(
        &self,
        pipe: &mut redis::Pipeline,
        track: &SystemTrack,
        keep: Duration,
    ) -> Result<()> {
        let key = self.keys.track_history(&track.uid.to_string());
        let p = HistoryPoint::of(track);
        let keep_ms = keep.as_millis().max(1) as i64;
        pipe.cmd("ZREMRANGEBYSCORE")
            .arg(&key)
            .arg(p.t)
            .arg(p.t)
            .ignore();
        pipe.cmd("ZADD").arg(&key).arg(p.t).arg(p.encode()).ignore();
        pipe.cmd("ZREMRANGEBYSCORE")
            .arg(&key)
            .arg("-inf")
            .arg(format!("({}", p.t - keep_ms))
            .ignore();
        pipe.cmd("PEXPIRE").arg(&key).arg(keep_ms).ignore();
        Ok(())
    }

    /// A track's history between two times (Unix ms), oldest first, at
    /// most `limit` points (the newest of them).
    pub async fn track_history(
        &self,
        uid: Uid,
        since_ms: Option<i64>,
        until_ms: Option<i64>,
        limit: usize,
    ) -> Result<Vec<HistoryPoint>> {
        let key = self.keys.track_history(&uid.to_string());
        let lo = since_ms.map_or("-inf".to_owned(), |t| t.to_string());
        let hi = until_ms.map_or("+inf".to_owned(), |t| t.to_string());
        let raw: Vec<String> = redis::cmd("ZREVRANGEBYSCORE")
            .arg(&key)
            .arg(&hi)
            .arg(&lo)
            .arg("LIMIT")
            .arg(0)
            .arg(limit)
            .query_async(&mut self.conn.clone())
            .await?;
        let mut out: Vec<HistoryPoint> =
            raw.iter().filter_map(|s| HistoryPoint::decode(s)).collect();
        out.reverse();
        Ok(out)
    }

    /// Delete a track's history point at `t_ms` and queue its publication;
    /// the point deleted, if there was one.
    pub async fn delete_history_point(
        &self,
        uid: Uid,
        t_ms: i64,
        reason: &str,
        decision: i64,
    ) -> Result<Option<HistoryPoint>> {
        let key = self.keys.track_history(&uid.to_string());
        let raw: Vec<String> = redis::cmd("ZRANGEBYSCORE")
            .arg(&key)
            .arg(t_ms)
            .arg(t_ms)
            .query_async(&mut self.conn.clone())
            .await?;
        let Some(point) = raw.first().and_then(|s| HistoryPoint::decode(s)) else {
            return Ok(None);
        };
        redis::pipe()
            .atomic()
            .cmd("ZREMRANGEBYSCORE")
            .arg(&key)
            .arg(t_ms)
            .arg(t_ms)
            .ignore()
            .cmd("XADD")
            .arg(self.keys.outbox_control())
            .arg("*")
            .arg("op")
            .arg("history_delete")
            .arg("uid")
            .arg(uid.to_string())
            .arg("point")
            .arg(serde_json::to_string(&point)?)
            .arg("reason")
            .arg(reason)
            .arg("decision")
            .arg(decision)
            .ignore()
            .query_async::<()>(&mut self.conn.clone())
            .await?;
        Ok(Some(point))
    }

    /// Queue a command for the engine.
    pub async fn push_command(&self, command: &serde_json::Value) -> Result<()> {
        redis::cmd("LPUSH")
            .arg(self.keys.commands())
            .arg(command.to_string())
            .query_async::<()>(&mut self.conn.clone())
            .await?;
        Ok(())
    }

    /// Queue sync messages for the link to send. The queue is capped: with
    /// no link running, the oldest go (the next heartbeat replaces them).
    pub async fn push_sync_out(&self, messages: &[Vec<u8>]) -> Result<()> {
        if messages.is_empty() {
            return Ok(());
        }
        let key = self.keys.sync_out();
        let mut pipe = redis::pipe();
        pipe.cmd("RPUSH").arg(&key);
        for m in messages {
            pipe.arg(m.as_slice());
        }
        pipe.ignore()
            .cmd("LTRIM")
            .arg(&key)
            .arg(-10_000)
            .arg(-1)
            .ignore();
        pipe.query_async::<()>(&mut self.conn.clone()).await?;
        Ok(())
    }

    /// Record how one part of sharing (`link`, `engine`) is going.
    pub async fn put_sync_status(&self, part: &str, status: &serde_json::Value) -> Result<()> {
        redis::cmd("HSET")
            .arg(self.keys.sync_status())
            .arg(part)
            .arg(status.to_string())
            .query_async::<()>(&mut self.conn.clone())
            .await?;
        Ok(())
    }

    /// Every part's last status.
    pub async fn sync_status(&self) -> Result<serde_json::Map<String, serde_json::Value>> {
        let raw: std::collections::HashMap<String, String> = redis::cmd("HGETALL")
            .arg(self.keys.sync_status())
            .query_async(&mut self.conn.clone())
            .await?;
        Ok(raw
            .into_iter()
            .filter_map(|(k, v)| Some((k, serde_json::from_str(&v).ok()?)))
            .collect())
    }

    /// Record how this process's OpenTelemetry export is going.
    pub async fn put_telemetry_status(
        &self,
        process: &str,
        status: &serde_json::Value,
    ) -> Result<()> {
        redis::cmd("HSET")
            .arg(self.keys.telemetry_status())
            .arg(process)
            .arg(status.to_string())
            .query_async::<()>(&mut self.conn.clone())
            .await?;
        Ok(())
    }

    /// Every process's last OpenTelemetry export status.
    pub async fn telemetry_status(&self) -> Result<serde_json::Map<String, serde_json::Value>> {
        let raw: std::collections::HashMap<String, String> = redis::cmd("HGETALL")
            .arg(self.keys.telemetry_status())
            .query_async(&mut self.conn.clone())
            .await?;
        Ok(raw
            .into_iter()
            .filter_map(|(k, v)| Some((k, serde_json::from_str(&v).ok()?)))
            .collect())
    }

    /// Forget processes' export status (processes long gone).
    pub async fn forget_telemetry_status(&self, processes: &[String]) -> Result<()> {
        if processes.is_empty() {
            return Ok(());
        }
        redis::cmd("HDEL")
            .arg(self.keys.telemetry_status())
            .arg(processes)
            .query_async::<()>(&mut self.conn.clone())
            .await?;
        Ok(())
    }

    /// Record how the TAK outputs are going, for `ttl`.
    pub async fn put_cot_status(&self, status: &serde_json::Value, ttl: Duration) -> Result<()> {
        redis::cmd("SET")
            .arg(self.keys.cot_status())
            .arg(status.to_string())
            .arg("PX")
            .arg(ttl.as_millis().max(1) as u64)
            .query_async::<()>(&mut self.conn.clone())
            .await?;
        Ok(())
    }

    /// The TAK outputs' last status, if the `cot` role is writing it.
    pub async fn cot_status(&self) -> Result<Option<serde_json::Value>> {
        let raw: Option<String> = redis::cmd("GET")
            .arg(self.keys.cot_status())
            .query_async(&mut self.conn.clone())
            .await?;
        Ok(raw.and_then(|s| serde_json::from_str(&s).ok()))
    }

    /// Queue a contact for the writer to publish live (capped: they perish).
    pub async fn push_contact(&self, subject: &str, body: &serde_json::Value) -> Result<()> {
        let key = self.keys.contacts_out();
        redis::pipe()
            .cmd("RPUSH")
            .arg(&key)
            .arg(serde_json::json!({"subject": subject, "body": body}).to_string())
            .ignore()
            .cmd("LTRIM")
            .arg(&key)
            .arg(-2000)
            .arg(-1)
            .ignore()
            .query_async::<()>(&mut self.conn.clone())
            .await?;
        Ok(())
    }

    /// Take up to `n` queued contacts: (subject, body).
    pub async fn pop_contacts(&self, n: usize) -> Result<Vec<(String, serde_json::Value)>> {
        let raw: Option<Vec<String>> = redis::cmd("LPOP")
            .arg(self.keys.contacts_out())
            .arg(n)
            .query_async(&mut self.conn.clone())
            .await?;
        Ok(raw
            .unwrap_or_default()
            .iter()
            .filter_map(|s| serde_json::from_str::<serde_json::Value>(s).ok())
            .filter_map(|v| Some((v["subject"].as_str()?.to_owned(), v["body"].clone())))
            .collect())
    }

    /// Take up to `n` queued sync messages, oldest first.
    pub async fn pop_sync_out(&self, n: usize) -> Result<Vec<Vec<u8>>> {
        let raw: Option<Vec<Vec<u8>>> = redis::cmd("LPOP")
            .arg(self.keys.sync_out())
            .arg(n)
            .query_async(&mut self.conn.clone())
            .await?;
        Ok(raw.unwrap_or_default())
    }

    /// Take up to `n` queued commands, oldest first.
    pub async fn pop_commands(&self, n: usize) -> Result<Vec<serde_json::Value>> {
        let raw: Option<Vec<String>> = redis::cmd("RPOP")
            .arg(self.keys.commands())
            .arg(n)
            .query_async(&mut self.conn.clone())
            .await?;
        Ok(raw
            .unwrap_or_default()
            .iter()
            .filter_map(|s| serde_json::from_str(s).ok())
            .collect())
    }

    /// The engine's answer to a command, kept for a minute.
    pub async fn put_command_result(&self, id: &str, result: &serde_json::Value) -> Result<()> {
        redis::cmd("SET")
            .arg(self.keys.command_result(id))
            .arg(result.to_string())
            .arg("EX")
            .arg(60)
            .query_async::<()>(&mut self.conn.clone())
            .await?;
        Ok(())
    }

    pub async fn command_result(&self, id: &str) -> Result<Option<serde_json::Value>> {
        let raw: Option<String> = redis::cmd("GET")
            .arg(self.keys.command_result(id))
            .query_async(&mut self.conn.clone())
            .await?;
        Ok(raw.and_then(|s| serde_json::from_str(&s).ok()))
    }

    /// Remove a system track that was never published (no tombstone needed).
    pub async fn forget_system_track(&self, uid: Uid) -> Result<()> {
        redis::cmd("DEL")
            .arg(self.keys.system_track(&uid.to_string()))
            .query_async::<()>(&mut self.conn.clone())
            .await?;
        Ok(())
    }

    pub async fn get_system_track(&self, uid: Uid) -> Result<Option<SystemTrack>> {
        let raw: Option<String> = redis::cmd("GET")
            .arg(self.keys.system_track(&uid.to_string()))
            .query_async(&mut self.conn.clone())
            .await?;
        Ok(raw.map(|s| serde_json::from_str(&s)).transpose()?)
    }

    /// Several system tracks in one round trip, in order (`None` for one
    /// that is gone).
    pub async fn get_system_tracks(&self, uids: &[Uid]) -> Result<Vec<Option<SystemTrack>>> {
        if uids.is_empty() {
            return Ok(Vec::new());
        }
        let keys: Vec<String> = uids
            .iter()
            .map(|u| self.keys.system_track(&u.to_string()))
            .collect();
        let raw: Vec<Option<String>> = redis::cmd("MGET")
            .arg(&keys)
            .query_async(&mut self.conn.clone())
            .await?;
        raw.into_iter()
            .map(|r| Ok(r.map(|s| serde_json::from_str(&s)).transpose()?))
            .collect()
    }

    /// Remove a system track's live state and queue its tombstone.
    pub async fn retire_system_track(&self, uid: Uid, reason: &str) -> Result<()> {
        redis::pipe()
            .atomic()
            .del(self.keys.system_track(&uid.to_string()))
            .ignore()
            .cmd("XADD")
            .arg(self.keys.outbox_control())
            .arg("*")
            .arg("op")
            .arg("tombstone")
            .arg("uid")
            .arg(uid.to_string())
            .arg("reason")
            .arg(reason)
            .ignore()
            .query_async::<()>(&mut self.conn.clone())
            .await?;
        Ok(())
    }

    /// Create a consumer group on the outbox if it does not exist yet.
    pub async fn ensure_outbox_group(&self, group: &str) -> Result<()> {
        self.ensure_outbox_group_from(group, "0").await
    }

    /// The same, a new group starting at `from`: `0` for everything the
    /// outbox holds, `$` for only what is added from now on (a consumer that
    /// reads the current picture itself).
    pub async fn ensure_outbox_group_from(&self, group: &str, from: &str) -> Result<()> {
        for stream in [self.keys.outbox(), self.keys.outbox_control()] {
            let res: redis::RedisResult<String> = redis::cmd("XGROUP")
                .arg("CREATE")
                .arg(stream)
                .arg(group)
                .arg(from)
                .arg("MKSTREAM")
                .query_async(&mut self.conn.clone())
                .await;
            match res {
                Ok(_) => {}
                Err(e) if e.code() == Some("BUSYGROUP") => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }

    /// Read outbox entries for `consumer`. With `pending`, re-read this
    /// consumer's delivered-but-unacknowledged entries (after a restart);
    /// otherwise wait up to `block` for new ones.
    pub async fn read_outbox(
        &self,
        group: &str,
        consumer: &str,
        count: usize,
        block: Duration,
        pending: bool,
    ) -> Result<Vec<OutboxEntry>> {
        let mut cmd = redis::cmd("XREADGROUP");
        cmd.arg("GROUP")
            .arg(group)
            .arg(consumer)
            .arg("COUNT")
            .arg(count);
        // A zero block returns at once (BLOCK 0 would wait forever).
        if !pending && !block.is_zero() {
            cmd.arg("BLOCK").arg(block.as_millis() as u64);
        }
        // Deletes (never trimmed) first, then publications.
        let from = if pending { "0" } else { ">" };
        let control = self.keys.outbox_control();
        cmd.arg("STREAMS")
            .arg(&control)
            .arg(self.keys.outbox())
            .arg(from)
            .arg(from);
        let reply: Option<StreamReadReply> = cmd.query_async(&mut self.conn.clone()).await?;
        let mut out = Vec::new();
        for key in reply.map(|r| r.keys).unwrap_or_default() {
            let tag = if key.key == control { CONTROL_TAG } else { "" };
            for entry in key.ids {
                let field = |name: &str| entry.get::<String>(name);
                let op =
                    match field("op").as_deref() {
                        Some("publish") => {
                            field("uid")
                                .and_then(|u| u.parse().ok())
                                .map(|uid| OutboxOp::Publish {
                                    uid,
                                    urgent: field("urgent").as_deref() == Some("1"),
                                })
                        }
                        Some("tombstone") => field("uid").and_then(|u| u.parse().ok()).map(|uid| {
                            OutboxOp::Tombstone {
                                uid,
                                reason: field("reason").filter(|r| !r.is_empty()),
                            }
                        }),
                        Some("history_delete") => field("uid")
                            .and_then(|u| u.parse().ok())
                            .zip(field("point").and_then(|p| serde_json::from_str(&p).ok()))
                            .map(|(uid, point)| OutboxOp::HistoryDelete {
                                uid,
                                point,
                                reason: field("reason").filter(|r| !r.is_empty()),
                                decision: field("decision").and_then(|d| d.parse().ok()),
                            }),
                        _ => None,
                    };
                match op {
                    Some(op) => out.push(OutboxEntry {
                        id: format!("{tag}{}", entry.id),
                        op,
                    }),
                    None => {
                        // Unreadable entries are acknowledged so they cannot
                        // wedge the group, and logged for investigation.
                        tracing::warn!(id = %entry.id, "dropping malformed outbox entry");
                        self.ack_outbox(group, &[format!("{tag}{}", entry.id)])
                            .await?;
                    }
                }
            }
        }
        Ok(out)
    }

    /// Acknowledge outbox entries (ids as [`read_outbox`] gave them).
    pub async fn ack_outbox(&self, group: &str, ids: &[String]) -> Result<()> {
        let (control, publish): (Vec<&String>, Vec<&String>) =
            ids.iter().partition(|i| i.starts_with(CONTROL_TAG));
        let control: Vec<&str> = control.iter().map(|i| &i[CONTROL_TAG.len()..]).collect();
        for (stream, ids) in [
            (self.keys.outbox_control(), control),
            (
                self.keys.outbox(),
                publish.iter().map(|i| i.as_str()).collect(),
            ),
        ] {
            if ids.is_empty() {
                continue;
            }
            let _: i64 = redis::cmd("XACK")
                .arg(stream)
                .arg(group)
                .arg(ids)
                .query_async(&mut self.conn.clone())
                .await?;
        }
        Ok(())
    }

    /// Add `n` to a per-source, per-minute counter (kept 7 days).
    pub async fn incr_metric(&self, source: &str, field: &str, n: i64) -> Result<()> {
        if let Ok(n) = u64::try_from(n) {
            crate::otel::count(source, &[(field.to_owned(), n)]);
        }
        let minute = Utc::now().timestamp() / 60;
        let key = self.keys.metrics(source, minute);
        redis::pipe()
            .hincr(&key, field, n)
            .ignore()
            .expire(&key, 7 * 24 * 3600)
            .ignore()
            .query_async::<()>(&mut self.conn.clone())
            .await?;
        Ok(())
    }

    /// Append a batch of observations (one pipeline, trimmed to the window).
    pub async fn append_observations(&self, source: &str, batch: &[Observation]) -> Result<()> {
        if batch.is_empty() {
            return Ok(());
        }
        let min_id = (Utc::now().timestamp_millis() - self.obs_window.as_millis() as i64).max(0);
        let stream = self.keys.obs_stream(source);
        let mut pipe = redis::pipe();
        for obs in batch {
            pipe.cmd("XADD")
                .arg(&stream)
                .arg("MINID")
                .arg("~")
                .arg(min_id)
                .arg("*")
                .arg("obs")
                .arg(serde_json::to_string(obs)?)
                .ignore();
        }
        pipe.query_async::<()>(&mut self.conn.clone()).await?;
        Ok(())
    }

    /// Create `group` on each source's observation stream if missing.
    pub async fn ensure_obs_groups(&self, sources: &[String], group: &str) -> Result<()> {
        for source in sources {
            let res: redis::RedisResult<String> = redis::cmd("XGROUP")
                .arg("CREATE")
                .arg(self.keys.obs_stream(source))
                .arg(group)
                .arg("$")
                .arg("MKSTREAM")
                .query_async(&mut self.conn.clone())
                .await;
            match res {
                Ok(_) => {}
                Err(e) if e.code() == Some("BUSYGROUP") => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }

    /// Read observations from several sources' streams. Entries that fail to
    /// parse come back as `Err` with their id so they can be acknowledged.
    #[allow(clippy::type_complexity)]
    pub async fn read_observations(
        &self,
        sources: &[String],
        group: &str,
        consumer: &str,
        count: usize,
        block: Duration,
        pending: bool,
    ) -> Result<Vec<(String, String, std::result::Result<Observation, String>)>> {
        if sources.is_empty() {
            tokio::time::sleep(block).await;
            return Ok(Vec::new());
        }
        let mut cmd = redis::cmd("XREADGROUP");
        cmd.arg("GROUP")
            .arg(group)
            .arg(consumer)
            .arg("COUNT")
            .arg(count);
        // A zero block returns at once (BLOCK 0 would wait forever).
        if !pending && !block.is_zero() {
            cmd.arg("BLOCK").arg(block.as_millis() as u64);
        }
        cmd.arg("STREAMS");
        for source in sources {
            cmd.arg(self.keys.obs_stream(source));
        }
        for _ in sources {
            cmd.arg(if pending { "0" } else { ">" });
        }
        let reply: Option<StreamReadReply> = cmd.query_async(&mut self.conn.clone()).await?;
        let prefix = format!("{}:obs:", self.keys.namespace());
        let mut out = Vec::new();
        for key in reply.map(|r| r.keys).unwrap_or_default() {
            let source = key.key.strip_prefix(&prefix).unwrap_or(&key.key).to_owned();
            for entry in key.ids {
                // A pending entry the stream's length cap removed before it was
                // acknowledged comes back with no fields.
                if entry.map.is_empty() {
                    out.push((source.clone(), entry.id, Err(TRIMMED.to_owned())));
                    continue;
                }
                let obs = entry
                    .get::<String>("obs")
                    .ok_or_else(|| "missing obs field".to_owned())
                    .and_then(|s| serde_json::from_str(&s).map_err(|e| e.to_string()));
                out.push((source.clone(), entry.id, obs));
            }
        }
        Ok(out)
    }

    pub async fn ack_observations(&self, source: &str, group: &str, ids: &[String]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let _: i64 = redis::cmd("XACK")
            .arg(self.keys.obs_stream(source))
            .arg(group)
            .arg(ids)
            .query_async(&mut self.conn.clone())
            .await?;
        Ok(())
    }

    /// Current state of a source track, kept for `ttl`.
    pub async fn put_source_track(&self, obs: &Observation, ttl: Duration) -> Result<()> {
        let _: () = redis::cmd("SET")
            .arg(
                self.keys
                    .source_track(&obs.source_id, &obs.source_track_key),
            )
            .arg(serde_json::to_string(obs)?)
            .arg("EX")
            .arg(ttl.as_secs().max(1))
            .query_async(&mut self.conn.clone())
            .await?;
        Ok(())
    }

    /// A source track's latest observation, if it has reported within its TTL.
    pub async fn get_source_track(&self, source: &str, key: &str) -> Result<Option<Observation>> {
        let raw: Option<String> = redis::cmd("GET")
            .arg(self.keys.source_track(source, key))
            .query_async(&mut self.conn.clone())
            .await?;
        Ok(match raw {
            Some(s) => Some(serde_json::from_str(&s)?),
            None => None,
        })
    }

    /// Every live system track (for the engine's warm start and the API).
    pub async fn list_system_tracks(&self) -> Result<Vec<SystemTrack>> {
        let keys = self
            .scan(&format!("{}:sys:*", self.keys.namespace()))
            .await?;
        let mut out = Vec::with_capacity(keys.len());
        for chunk in keys.chunks(500) {
            let values: Vec<Option<String>> = redis::cmd("MGET")
                .arg(chunk)
                .query_async(&mut self.conn.clone())
                .await?;
            for v in values.into_iter().flatten() {
                match serde_json::from_str(&v) {
                    Ok(t) => out.push(t),
                    Err(e) => tracing::warn!(error = %e, "skipping unreadable system track"),
                }
            }
        }
        Ok(out)
    }

    /// Every UID with a system track key, readable or not.
    pub async fn live_system_uids(&self) -> Result<std::collections::HashSet<Uid>> {
        let prefix = self.keys.system_track("");
        Ok(self
            .scan(&format!("{prefix}*"))
            .await?
            .iter()
            .filter_map(|k| k.strip_prefix(&prefix)?.parse().ok())
            .collect())
    }

    /// The highest UID of `site` in Redis, per kind of key: live system
    /// tracks and track position history (which outlives the track).
    pub async fn uid_sightings(
        &self,
        site: ot_core::SiteCode,
    ) -> Result<Vec<crate::uid_counter::UidSighting>> {
        let mut out = Vec::new();
        for prefix in [self.keys.system_track(""), self.keys.track_history("")] {
            let keys = self.scan(&format!("{prefix}{}*", site.as_str())).await?;
            let best = keys
                .iter()
                .filter_map(|k| {
                    let uid: Uid = k.strip_prefix(&prefix)?.parse().ok()?;
                    (uid.site() == site).then_some((uid.sequence(), k))
                })
                .max();
            if let Some((seq, key)) = best {
                out.push(crate::uid_counter::UidSighting::new(
                    "redis",
                    key.clone(),
                    seq,
                ));
            }
        }
        Ok(out)
    }

    /// Persist one static-cache entry (JSON) with a TTL.
    pub async fn put_static(
        &self,
        source: &str,
        key: &str,
        json: &str,
        ttl: Duration,
    ) -> Result<()> {
        let _: () = redis::cmd("SET")
            .arg(self.keys.static_cache(source, key))
            .arg(json)
            .arg("EX")
            .arg(ttl.as_secs().max(1))
            .query_async(&mut self.conn.clone())
            .await?;
        Ok(())
    }

    /// Every cached static entry of a source, as (track key, JSON).
    pub async fn load_statics(&self, source: &str) -> Result<Vec<(String, String)>> {
        let keys = self.scan(&self.keys.static_cache_all(source)).await?;
        let prefix = self.keys.static_cache(source, "");
        let mut out = Vec::with_capacity(keys.len());
        for chunk in keys.chunks(500) {
            let values: Vec<Option<String>> = redis::cmd("MGET")
                .arg(chunk)
                .query_async(&mut self.conn.clone())
                .await?;
            for (k, v) in chunk.iter().zip(values) {
                if let Some(v) = v {
                    out.push((k.strip_prefix(&prefix).unwrap_or(k).to_owned(), v));
                }
            }
        }
        Ok(out)
    }

    /// Publish a source worker's status (JSON), expiring if the worker stops.
    pub async fn put_source_status(&self, source: &str, json: &str, ttl: Duration) -> Result<()> {
        let _: () = redis::cmd("SET")
            .arg(self.keys.source_status(source))
            .arg(json)
            .arg("EX")
            .arg(ttl.as_secs().max(1))
            .query_async(&mut self.conn.clone())
            .await?;
        Ok(())
    }

    pub async fn get_source_status(&self, source: &str) -> Result<Option<String>> {
        Ok(redis::cmd("GET")
            .arg(self.keys.source_status(source))
            .query_async(&mut self.conn.clone())
            .await?)
    }

    /// Add several counters to this minute's bucket for `source`.
    pub async fn incr_metrics(&self, source: &str, counts: &[(String, u64)]) -> Result<()> {
        if counts.is_empty() {
            return Ok(());
        }
        crate::otel::count(source, counts);
        let minute = Utc::now().timestamp() / 60;
        let key = self.keys.metrics(source, minute);
        let mut pipe = redis::pipe();
        for (field, n) in counts {
            pipe.hincr(&key, field, *n).ignore();
        }
        pipe.expire(&key, 7 * 24 * 3600).ignore();
        pipe.query_async::<()>(&mut self.conn.clone()).await?;
        Ok(())
    }

    /// Per-minute counters for `source` over the last `minutes` minutes,
    /// oldest first, as (unix minute, counters).
    pub async fn metrics_range(
        &self,
        source: &str,
        minutes: i64,
    ) -> Result<Vec<(i64, std::collections::HashMap<String, u64>)>> {
        let now = Utc::now().timestamp() / 60;
        let mut pipe = redis::pipe();
        let range: Vec<i64> = (now - minutes + 1..=now).collect();
        for m in &range {
            pipe.hgetall(self.keys.metrics(source, *m));
        }
        let maps: Vec<std::collections::HashMap<String, u64>> =
            pipe.query_async(&mut self.conn.clone()).await?;
        Ok(range.into_iter().zip(maps).collect())
    }

    /// Set gauges (last value wins) in this minute's bucket for `source`,
    /// beside the counters, so charts get history for sampled values too.
    pub async fn set_gauges(&self, source: &str, gauges: &[(&str, u64)]) -> Result<()> {
        if gauges.is_empty() {
            return Ok(());
        }
        crate::otel::gauge(source, gauges);
        let minute = Utc::now().timestamp() / 60;
        let key = self.keys.metrics(source, minute);
        let mut pipe = redis::pipe();
        for (field, v) in gauges {
            pipe.hset(&key, *field, *v).ignore();
        }
        pipe.expire(&key, 7 * 24 * 3600).ignore();
        pipe.query_async::<()>(&mut self.conn.clone()).await?;
        Ok(())
    }

    /// Backlog of one consumer group: entries delivered but not acknowledged
    /// (`pending`) and entries not yet delivered (`lag`). Zeros when the
    /// stream or group does not exist.
    pub async fn group_backlog(&self, stream: &str, group: &str) -> Result<GroupBacklog> {
        let res: redis::RedisResult<Vec<std::collections::HashMap<String, redis::Value>>> =
            redis::cmd("XINFO")
                .arg("GROUPS")
                .arg(stream)
                .query_async(&mut self.conn.clone())
                .await;
        let groups = match res {
            Ok(g) => g,
            // `ERR no such key`: nothing has been written to the stream yet.
            Err(e) if e.code() == Some("ERR") => return Ok(GroupBacklog::default()),
            Err(e) => return Err(e.into()),
        };
        let int = |v: Option<&redis::Value>| match v {
            Some(redis::Value::Int(n)) => (*n).max(0) as u64,
            _ => 0,
        };
        let name_is = |v: Option<&redis::Value>| match v {
            Some(redis::Value::BulkString(b)) => b.as_slice() == group.as_bytes(),
            Some(redis::Value::SimpleString(s)) => s == group,
            _ => false,
        };
        Ok(groups
            .iter()
            .find(|g| name_is(g.get("name")))
            .map(|g| GroupBacklog {
                pending: int(g.get("pending")),
                lag: int(g.get("lag")),
            })
            .unwrap_or_default())
    }

    /// Backlog of the writer on the outbox.
    pub async fn outbox_backlog(&self, group: &str) -> Result<GroupBacklog> {
        let a = self.group_backlog(&self.keys.outbox(), group).await?;
        let b = self
            .group_backlog(&self.keys.outbox_control(), group)
            .await?;
        Ok(GroupBacklog {
            pending: a.pending + b.pending,
            lag: a.lag + b.lag,
        })
    }

    /// Backlog of the engine on a source's observation stream.
    pub async fn obs_backlog(&self, source: &str, group: &str) -> Result<GroupBacklog> {
        self.group_backlog(&self.keys.obs_stream(source), group)
            .await
    }

    /// Bytes Redis uses (`INFO memory` `used_memory`).
    pub async fn used_memory(&self) -> Result<u64> {
        let info: String = redis::cmd("INFO")
            .arg("memory")
            .query_async(&mut self.conn.clone())
            .await?;
        Ok(info
            .lines()
            .find_map(|l| l.strip_prefix("used_memory:"))
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0))
    }

    async fn scan(&self, pattern: &str) -> Result<Vec<String>> {
        let mut conn = self.conn.clone();
        let mut cursor: u64 = 0;
        let mut out = Vec::new();
        loop {
            let (next, batch): (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(pattern)
                .arg("COUNT")
                .arg(1000)
                .query_async(&mut conn)
                .await?;
            out.extend(batch);
            if next == 0 {
                return Ok(out);
            }
            cursor = next;
        }
    }

    /// Delete every key in this store's namespace. For tests and resets only.
    pub async fn purge_namespace(&self) -> Result<u64> {
        let mut conn = self.conn.clone();
        let mut cursor: u64 = 0;
        let mut deleted = 0;
        loop {
            let (next, batch): (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(self.keys.all())
                .arg("COUNT")
                .arg(500)
                .query_async(&mut conn)
                .await?;
            if !batch.is_empty() {
                deleted += redis::cmd("DEL")
                    .arg(&batch)
                    .query_async::<u64>(&mut conn)
                    .await?;
            }
            if next == 0 {
                return Ok(deleted);
            }
            cursor = next;
        }
    }
}

impl From<redis::RedisError> for StoreError {
    fn from(e: redis::RedisError) -> Self {
        StoreError::Redis(e)
    }
}

#[cfg(test)]
mod history_tests {
    use super::*;

    #[test]
    fn history_points_round_trip_compactly() {
        let p = HistoryPoint {
            t: 1_790_000_000_123,
            lat: 32.712_345_678,
            lon: -117.162_345_678,
            alt: None,
            course: Some(123.44),
            speed: Some(7.2),
        };
        let s = p.encode();
        assert_eq!(s, "1790000000123,32.712346,-117.162346,,123.4,7.20");
        let back = HistoryPoint::decode(&s).unwrap();
        assert_eq!((back.t, back.alt, back.course), (p.t, None, Some(123.4)));
        assert!((back.lat - p.lat).abs() < 1e-6);
        assert!(HistoryPoint::decode("x").is_none());
    }
}
