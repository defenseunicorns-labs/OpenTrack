//! The writer role: drains the Redis outbox into NATS.
//!
//! Each system track is published on its own subject (see [`ot_core::wire`]).
//! Writes are coalesced per track: at most one per minimum interval (default
//! 5 s), immediately when the producer marks an update urgent (identity or
//! classification changed). Outbox entries are acknowledged only once the
//! stream has acknowledged the message, so a crash re-delivers them; the
//! message id is the outbox entry id, so a re-delivered message is dropped by
//! JetStream as a duplicate.

use std::collections::HashMap;
use std::time::Duration;

use bytes::Bytes;
use ot_core::Uid;
use ot_core::wire::{self, Op, PublishContext};
use ot_nats::{Outgoing, TrackSink};
use ot_store::{OutboxOp, RedisStore};
use tokio::time::Instant;
use tracing::Instrument;

/// Consumer group every writer instance joins.
pub const GROUP: &str = "track-writer";

/// Metrics bucket the writer counts under.
const METRICS_SOURCE: &str = crate::metrics::WRITER;

/// Publications in flight at once (each waits for its acknowledgement).
const CONCURRENT_PUBLISHES: usize = 256;

#[derive(Debug, Clone)]
pub struct WriterSettings {
    pub consumer: String,
    /// Subject prefix; each track goes to `<prefix>.tms-<UID>`.
    pub tracks_subject: String,
    pub min_interval: Duration,
    /// Delay before retrying a write that failed transiently.
    pub retry_after: Duration,
    pub batch: usize,
}

/// Per-track write scheduling, kept free of I/O so it can be tested alone.
#[derive(Debug, Default)]
pub struct Schedule {
    min_interval: Duration,
    last_written: HashMap<Uid, Instant>,
    /// Tracks waiting to be written: when, and which outbox entries that
    /// write will acknowledge.
    waiting: HashMap<Uid, (Instant, Vec<String>)>,
}

impl Schedule {
    pub fn new(min_interval: Duration) -> Self {
        Self {
            min_interval,
            ..Default::default()
        }
    }

    /// Record an outbox entry for `uid`.
    pub fn offer(&mut self, uid: Uid, entry_id: String, urgent: bool, now: Instant) {
        let earliest = match (urgent, self.last_written.get(&uid)) {
            (false, Some(last)) => (*last + self.min_interval).max(now),
            _ => now,
        };
        let slot = self.waiting.entry(uid).or_insert((earliest, Vec::new()));
        slot.0 = slot.0.min(earliest);
        slot.1.push(entry_id);
    }

    /// Take every track whose write is due.
    pub fn take_due(&mut self, now: Instant) -> Vec<(Uid, Vec<String>)> {
        let due: Vec<Uid> = self
            .waiting
            .iter()
            .filter(|(_, (at, _))| *at <= now)
            .map(|(uid, _)| *uid)
            .collect();
        due.into_iter()
            .filter_map(|uid| self.waiting.remove(&uid).map(|(_, ids)| (uid, ids)))
            .collect()
    }

    pub fn written(&mut self, uid: Uid, now: Instant) {
        self.last_written.insert(uid, now);
    }

    /// Put a failed write back, to be retried at `at`.
    pub fn retry(&mut self, uid: Uid, ids: Vec<String>, at: Instant) {
        let slot = self.waiting.entry(uid).or_insert((at, Vec::new()));
        slot.0 = slot.0.min(at);
        slot.1.extend(ids);
    }

    pub fn next_due(&self) -> Option<Instant> {
        self.waiting.values().map(|(at, _)| *at).min()
    }

    pub fn waiting(&self) -> usize {
        self.waiting.len()
    }

    /// Forget write times older than the interval; they no longer delay anything.
    pub fn prune(&mut self, now: Instant) {
        let horizon = self.min_interval;
        self.last_written
            .retain(|_, at| now.saturating_duration_since(*at) < horizon);
    }
}

/// Outcome counters, for logs and tests.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Tally {
    pub written: u64,
    pub deleted: u64,
    pub rejected: u64,
    pub retried: u64,
    pub skipped: u64,
}

/// A delete waiting to be published: when, the outbox entries it settles,
/// and the reason recorded with it.
type PendingDelete = (Instant, Vec<String>, Option<String>);

pub struct Writer<S> {
    redis: RedisStore,
    sink: S,
    ctx: PublishContext,
    settings: WriterSettings,
    schedule: Schedule,
    deletes: HashMap<Uid, PendingDelete>,
    /// Deleted history points to publish: (when, outbox entry, message).
    history: Vec<(Instant, String, wire::HistoryDeleteMessage)>,
    /// Set after a failed publish: check the destination before the next one.
    needs_prepare: bool,
    pub tally: Tally,
}

impl<S: TrackSink> Writer<S> {
    pub fn new(redis: RedisStore, sink: S, ctx: PublishContext, settings: WriterSettings) -> Self {
        Self {
            schedule: Schedule::new(settings.min_interval),
            deletes: HashMap::new(),
            history: Vec::new(),
            needs_prepare: true,
            redis,
            sink,
            ctx,
            settings,
            tally: Tally::default(),
        }
    }

    /// Run until `shutdown` resolves.
    pub async fn run(
        mut self,
        shutdown: impl std::future::Future<Output = ()>,
    ) -> anyhow::Result<Tally> {
        self.redis.ensure_outbox_group(GROUP).await?;
        tracing::info!(consumer = %self.settings.consumer,
            subject = %format!("{}.>", self.settings.tracks_subject), "writer started");
        tokio::pin!(shutdown);
        let mut backoff = Duration::from_millis(500);
        // First, anything this consumer was handed before a restart.
        let mut pending = true;
        loop {
            tokio::select! {
                _ = &mut shutdown => break,
                res = self.pump(pending) => match res {
                    Ok(()) => {
                        pending = false;
                        backoff = Duration::from_millis(500);
                    }
                    Err(e) => {
                        // Redis hiccups must not kill the writer; unacknowledged
                        // entries are simply read again.
                        tracing::warn!(error = %e, ?backoff, "writer cycle failed; retrying");
                        tokio::time::sleep(backoff).await;
                        backoff = (backoff * 2).min(Duration::from_secs(10));
                    }
                },
            }
        }
        // Deferred entries stay unacknowledged and are re-delivered on restart.
        tracing::info!(tally = ?self.tally, waiting = self.waiting(), "writer stopped");
        Ok(self.tally)
    }

    /// One read-and-flush cycle. With `pending`, drain this consumer's
    /// unacknowledged entries instead of waiting for new ones.
    pub async fn pump(&mut self, pending: bool) -> anyhow::Result<()> {
        // Perishable contacts first: they go live, not through the outbox.
        for (subject, body) in self.redis.pop_contacts(1000).await? {
            if let Err(e) = self
                .sink
                .publish_live(subject, bytes::Bytes::from(body.to_string()))
                .await
            {
                tracing::debug!(error = %e, "contact not published");
            }
        }
        let now = Instant::now();
        let next_due = [
            self.schedule.next_due(),
            self.deletes.values().map(|(at, ..)| *at).min(),
        ]
        .into_iter()
        .flatten()
        .min();
        let block = next_due
            .map(|at| at.saturating_duration_since(now))
            .unwrap_or(Duration::from_secs(1))
            .clamp(Duration::from_millis(1), Duration::from_secs(1));
        let entries = self
            .redis
            .read_outbox(
                GROUP,
                &self.settings.consumer,
                self.settings.batch,
                block,
                pending,
            )
            .await?;
        let now = Instant::now();
        for entry in entries {
            match entry.op {
                OutboxOp::Publish { uid, urgent } => {
                    self.schedule.offer(uid, entry.id, urgent, now)
                }
                OutboxOp::Tombstone { uid, reason } => {
                    let slot = self.deletes.entry(uid).or_insert((now, Vec::new(), None));
                    slot.1.push(entry.id);
                    slot.2 = reason.or(slot.2.take());
                }
                OutboxOp::HistoryDelete {
                    uid,
                    point,
                    reason,
                    decision,
                } => {
                    let msg = wire::HistoryDeleteMessage {
                        schema: wire::TRACK_SCHEMA.into(),
                        op: Op::DeleteHistoryPoint,
                        track_id: uid.doc_id(),
                        uid,
                        observed_at: chrono::DateTime::from_timestamp_millis(point.t)
                            .unwrap_or_default(),
                        position: wire::HistoryPosition {
                            latitude: point.lat,
                            longitude: point.lon,
                        },
                        reason,
                        decision_id: decision,
                        deleted_at: chrono::Utc::now(),
                        publisher: self.ctx.clone(),
                    };
                    self.history.push((now, entry.id, msg));
                }
            }
        }
        self.flush()
            .instrument(tracing::info_span!("writer.flush"))
            .await
    }

    /// Messages still waiting (coalesced updates and deletes).
    pub fn waiting(&self) -> usize {
        self.schedule.waiting() + self.deletes.len() + self.history.len()
    }

    /// Publish one message. `Ok(true)` when stored, `Ok(false)` when the
    /// destination refused it for good; `Err` when a retry could help.
    async fn send(&mut self, msg: Outgoing) -> Result<bool, ot_nats::NatsError> {
        if self.needs_prepare {
            self.sink.prepare().await?;
            self.needs_prepare = false;
        }
        match self.sink.publish(msg).await {
            Ok(()) => Ok(true),
            Err(e) if e.is_transient() => {
                self.needs_prepare = true;
                Err(e)
            }
            Err(e) => {
                tracing::error!(%e, "message refused");
                Ok(false)
            }
        }
    }

    fn outgoing(&self, uid: Uid, op: Op, msg_id: &str, body: Vec<u8>) -> Outgoing {
        Outgoing {
            subject: wire::subject(&self.settings.tracks_subject, uid),
            msg_id: format!("{}:{msg_id}", self.ctx.node_id),
            headers: vec![
                (wire::HEADER_OP, op.as_str().to_owned()),
                (wire::HEADER_SCHEMA, wire::TRACK_SCHEMA.to_owned()),
            ],
            body: Bytes::from(body),
        }
    }

    async fn flush(&mut self) -> anyhow::Result<()> {
        let now = Instant::now();
        let (mut written, mut deleted, mut failed) = (0, 0, 0);

        let due: Vec<Uid> = self
            .deletes
            .iter()
            .filter(|(_, (at, ..))| *at <= now)
            .map(|(uid, _)| *uid)
            .collect();
        for uid in due {
            let Some((_, ids, reason)) = self.deletes.remove(&uid) else {
                continue;
            };
            let body = wire::delete_message(uid, reason.clone(), &self.ctx, chrono::Utc::now());
            let msg = self.outgoing(
                uid,
                Op::Delete,
                ids.last().expect("non-empty"),
                serde_json::to_vec(&body)?,
            );
            match self.send(msg).await {
                Ok(stored) => {
                    if stored {
                        self.tally.deleted += 1;
                        deleted += 1;
                    } else {
                        self.tally.rejected += 1;
                        failed += 1;
                    }
                    self.redis.ack_outbox(GROUP, &ids).await?;
                }
                Err(e) => {
                    tracing::warn!(%e, %uid, "delete failed, will retry");
                    self.tally.retried += 1;
                    failed += 1;
                    self.deletes
                        .insert(uid, (now + self.settings.retry_after, ids, reason));
                }
            }
        }

        for (at, id, body) in std::mem::take(&mut self.history) {
            if at > now {
                self.history.push((at, id, body));
                continue;
            }
            let t = body.observed_at.timestamp_millis();
            let mut msg = self.outgoing(
                body.uid,
                Op::DeleteHistoryPoint,
                &id,
                serde_json::to_vec(&body)?,
            );
            msg.subject = wire::history_subject(&self.settings.tracks_subject, body.uid, t);
            match self.send(msg).await {
                Ok(stored) => {
                    if !stored {
                        self.tally.rejected += 1;
                        failed += 1;
                    }
                    self.redis
                        .ack_outbox(GROUP, std::slice::from_ref(&id))
                        .await?;
                }
                Err(e) => {
                    tracing::warn!(%e, uid = %body.uid, "history point delete failed, will retry");
                    self.tally.retried += 1;
                    failed += 1;
                    self.history
                        .push((now + self.settings.retry_after, id, body));
                }
            }
        }

        // Every due track read at once, published concurrently (each waits
        // for the stream's acknowledgement), acknowledged at once.
        let due = self.schedule.take_due(now);
        let uids: Vec<Uid> = due.iter().map(|(u, _)| *u).collect();
        let tracks = self.redis.get_system_tracks(&uids).await?;
        let mut acks: Vec<String> = Vec::new();
        let mut sends: Vec<(Uid, Vec<String>, Outgoing)> = Vec::new();
        for ((uid, ids), track) in due.into_iter().zip(tracks) {
            let Some(track) = track else {
                // Retired since it was queued; its delete is in the outbox.
                self.tally.skipped += 1;
                acks.extend(ids);
                continue;
            };
            let body = wire::to_message(&track, &self.ctx, chrono::Utc::now());
            let msg = self.outgoing(
                uid,
                Op::Upsert,
                ids.last().expect("non-empty"),
                serde_json::to_vec(&body)?,
            );
            sends.push((uid, ids, msg));
        }
        for chunk in sends.chunks(CONCURRENT_PUBLISHES) {
            if self.needs_prepare {
                if let Err(e) = self.sink.prepare().await {
                    tracing::warn!(%e, "stream not ready, will retry");
                    for (uid, ids, _) in chunk {
                        self.tally.retried += 1;
                        failed += 1;
                        self.schedule
                            .retry(*uid, ids.clone(), now + self.settings.retry_after);
                    }
                    continue;
                }
                self.needs_prepare = false;
            }
            let results = futures_util::future::join_all(
                chunk
                    .iter()
                    .map(|(_, _, msg)| self.sink.publish(msg.clone())),
            )
            .await;
            for ((uid, ids, _), result) in chunk.iter().zip(results) {
                match result {
                    Ok(()) => {
                        self.schedule.written(*uid, now);
                        self.tally.written += 1;
                        written += 1;
                        acks.extend(ids.iter().cloned());
                    }
                    Err(e) if e.is_transient() => {
                        tracing::warn!(%e, %uid, "write failed, will retry");
                        self.needs_prepare = true;
                        self.tally.retried += 1;
                        failed += 1;
                        self.schedule
                            .retry(*uid, ids.clone(), now + self.settings.retry_after);
                    }
                    Err(e) => {
                        tracing::error!(%e, "message refused");
                        self.tally.rejected += 1;
                        failed += 1;
                        acks.extend(ids.iter().cloned());
                    }
                }
            }
        }
        self.redis.ack_outbox(GROUP, &acks).await?;

        for (metric, n) in [
            ("written", written),
            ("deleted", deleted),
            ("write_error", failed),
        ] {
            if n > 0 {
                self.redis.incr_metric(METRICS_SOURCE, metric, n).await?;
            }
        }
        self.schedule.prune(now);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uid(n: u64) -> Uid {
        Uid::new(ot_core::SiteCode::new("OTK").unwrap(), n).unwrap()
    }

    #[test]
    fn first_write_is_immediate_then_rate_limited() {
        let t0 = Instant::now();
        let mut s = Schedule::new(Duration::from_secs(5));
        s.offer(uid(1), "1-0".into(), false, t0);
        assert_eq!(s.take_due(t0), vec![(uid(1), vec!["1-0".to_string()])]);
        s.written(uid(1), t0);

        // Two more updates within the interval coalesce into one write at t0+5s.
        s.offer(uid(1), "2-0".into(), false, t0 + Duration::from_secs(1));
        s.offer(uid(1), "3-0".into(), false, t0 + Duration::from_secs(2));
        assert!(s.take_due(t0 + Duration::from_secs(4)).is_empty());
        assert_eq!(s.next_due(), Some(t0 + Duration::from_secs(5)));
        assert_eq!(
            s.take_due(t0 + Duration::from_secs(5)),
            vec![(uid(1), vec!["2-0".to_string(), "3-0".to_string()])]
        );
    }

    #[test]
    fn urgent_updates_skip_the_interval() {
        let t0 = Instant::now();
        let mut s = Schedule::new(Duration::from_secs(5));
        s.written(uid(1), t0);
        s.offer(uid(1), "1-0".into(), false, t0 + Duration::from_secs(1));
        s.offer(uid(1), "2-0".into(), true, t0 + Duration::from_secs(2));
        let due = s.take_due(t0 + Duration::from_secs(2));
        assert_eq!(
            due,
            vec![(uid(1), vec!["1-0".to_string(), "2-0".to_string()])]
        );
    }

    #[test]
    fn retries_keep_their_entries_and_prune_forgets_old_writes() {
        let t0 = Instant::now();
        let mut s = Schedule::new(Duration::from_secs(5));
        s.retry(uid(2), vec!["9-0".into()], t0 + Duration::from_secs(5));
        assert!(s.take_due(t0).is_empty());
        assert_eq!(s.take_due(t0 + Duration::from_secs(5)).len(), 1);

        s.written(uid(3), t0);
        s.prune(t0 + Duration::from_secs(6));
        // uid(3) is no longer rate-limited.
        s.offer(uid(3), "1-0".into(), false, t0 + Duration::from_secs(6));
        assert_eq!(s.take_due(t0 + Duration::from_secs(6)).len(), 1);
    }

    /// End to end against a real Redis (set `OT_TEST_REDIS_URL`), with a fake
    /// sink. Runs in its own key namespace and cleans up after itself.
    mod redis_e2e {
        use super::*;
        use ot_core::SystemTrack;
        use ot_nats::NatsError;
        use std::sync::{Arc, Mutex};

        #[derive(Clone, Default)]
        struct FakeSink {
            sent: Arc<Mutex<Vec<Outgoing>>>,
            /// Fail this many publishes (transiently) before accepting.
            fail_next: Arc<Mutex<u32>>,
            prepared: Arc<Mutex<u32>>,
        }

        impl TrackSink for FakeSink {
            async fn prepare(&self) -> Result<(), NatsError> {
                *self.prepared.lock().unwrap() += 1;
                Ok(())
            }
            async fn publish(&self, msg: Outgoing) -> Result<(), NatsError> {
                let mut fail = self.fail_next.lock().unwrap();
                if *fail > 0 {
                    *fail -= 1;
                    return Err(NatsError::Publish {
                        subject: msg.subject,
                        message: "down".into(),
                        transient: true,
                    });
                }
                self.sent.lock().unwrap().push(msg);
                Ok(())
            }
        }

        impl FakeSink {
            fn bodies(&self) -> Vec<(String, String, serde_json::Value)> {
                self.sent
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|m| {
                        let op = m
                            .headers
                            .iter()
                            .find(|(k, _)| *k == wire::HEADER_OP)
                            .unwrap()
                            .1
                            .clone();
                        (
                            m.subject.clone(),
                            op,
                            serde_json::from_slice(&m.body).unwrap(),
                        )
                    })
                    .collect()
            }
        }

        async fn store() -> Option<RedisStore> {
            let url = std::env::var("OT_TEST_REDIS_URL").ok()?;
            let ns = format!(
                "ot-test-{}-{}",
                std::process::id(),
                chrono::Utc::now().timestamp_micros()
            );
            Some(
                RedisStore::connect(&url, ot_store::Keys::new(ns))
                    .await
                    .unwrap(),
            )
        }

        fn track(n: u64, lat: f64) -> SystemTrack {
            let now = chrono::Utc::now();
            let obs: ot_core::Observation = serde_json::from_value(serde_json::json!({
                "schema_version": 1, "source_id": "t", "source_track_key": format!("k{n}"),
                "observed_at": now, "received_at": now,
                "position": { "latitude": lat, "longitude": -117.0 }
            }))
            .unwrap();
            SystemTrack::from_first_observation(uid(n), obs)
        }

        #[tokio::test]
        async fn outbox_to_sink_coalesces_retries_and_deletes() {
            let Some(redis) = store().await else {
                eprintln!("skipped: OT_TEST_REDIS_URL not set");
                return;
            };
            let sink = FakeSink::default();
            let ctx = PublishContext {
                node_id: "opentrack-OTK".into(),
                version: "test".into(),
                correlation: "correlation-test".into(),
            };
            let settings = WriterSettings {
                consumer: "c1".into(),
                tracks_subject: "tracks".into(),
                min_interval: Duration::from_secs(60),
                retry_after: Duration::from_millis(10),
                batch: 100,
            };
            let mut w = Writer::new(redis.clone(), sink.clone(), ctx, settings);
            redis.ensure_outbox_group(GROUP).await.unwrap();

            // First update writes at once; the next two coalesce behind the interval.
            redis
                .put_system_track(&track(1, 32.0), false)
                .await
                .unwrap();
            w.pump(false).await.unwrap();
            redis
                .put_system_track(&track(1, 32.1), false)
                .await
                .unwrap();
            redis
                .put_system_track(&track(1, 32.2), false)
                .await
                .unwrap();
            w.pump(false).await.unwrap();
            assert_eq!(sink.sent.lock().unwrap().len(), 1);
            assert_eq!(*sink.prepared.lock().unwrap(), 1);

            // An urgent update goes straight out with the latest state, after
            // one transient failure (which re-checks the destination).
            *sink.fail_next.lock().unwrap() = 1;
            redis.put_system_track(&track(1, 32.3), true).await.unwrap();
            w.pump(false).await.unwrap();
            assert_eq!(w.tally.retried, 1);
            tokio::time::sleep(Duration::from_millis(20)).await;
            w.pump(false).await.unwrap();
            assert_eq!(*sink.prepared.lock().unwrap(), 2);
            {
                let bodies = sink.bodies();
                assert_eq!(bodies.len(), 2);
                let (subject, op, doc) = &bodies[1];
                assert_eq!(subject, "tracks.tms-OTK000000001");
                assert_eq!(op, "upsert");
                assert_eq!(doc["lat"], 32.3);
                assert_eq!(doc["schema"], "opentrack.track.v2");
                let sent = sink.sent.lock().unwrap();
                assert_ne!(sent[0].msg_id, sent[1].msg_id);
                assert!(sent[1].msg_id.starts_with("opentrack-OTK:"));
            }

            // Retiring publishes a delete; nothing is left unacknowledged.
            redis
                .retire_system_track(uid(1), "no report for 6h")
                .await
                .unwrap();
            w.pump(false).await.unwrap();
            let bodies = sink.bodies();
            let (subject, op, doc) = bodies.last().unwrap();
            assert_eq!(subject, "tracks.tms-OTK000000001");
            assert_eq!(op, "delete");
            assert_eq!(doc["reason"], "no report for 6h");
            assert!(
                redis
                    .read_outbox(GROUP, "c1", 100, Duration::from_millis(1), true)
                    .await
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(w.tally.written, 2);
            assert_eq!(w.tally.deleted, 1);
            assert_eq!(w.waiting(), 0);

            // A deleted history point goes out on a subject of its own.
            let t = track(2, 33.0);
            redis
                .write_batch(
                    &[],
                    Duration::ZERO,
                    &[(&t, ot_store::TrackWrite::Quiet, true)],
                    Some(Duration::from_secs(3600)),
                )
                .await
                .unwrap();
            let at = t.view.observed_at.timestamp_millis();
            let point = redis
                .delete_history_point(uid(2), at, "a GPS jump", 42)
                .await
                .unwrap();
            assert_eq!(point.map(|p| p.lat), Some(33.0));
            w.pump(false).await.unwrap();
            let bodies = sink.bodies();
            let (subject, op, doc) = bodies.last().unwrap();
            assert_eq!(subject, &format!("tracks.history.tms-OTK000000002.{at}"));
            assert_eq!(op, "delete_history_point");
            assert_eq!(
                (doc["reason"].as_str(), doc["decision_id"].as_i64()),
                (Some("a GPS jump"), Some(42))
            );
            assert_eq!(doc["position"]["latitude"], 33.0);
            assert_eq!(w.waiting(), 0);

            redis.purge_namespace().await.unwrap();
        }
    }
}
