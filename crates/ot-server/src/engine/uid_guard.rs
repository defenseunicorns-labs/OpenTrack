//! On start, before any UID is allocated: keep the track number counter
//! past every number of this site already in use (GitHub #16).
//!
//! The counter lives in SQLite, so a database restored from an older backup
//! would hand out numbers already published. The engine looks in the
//! database (track graph, decision log, multi-node log), in Redis (live
//! system tracks and their history) and in the NATS tracks stream (subjects
//! `<prefix>.tms-<UID>` and `<prefix>.history.tms-<UID>.<t>`), and moves the
//! counter past the highest it finds, recording a `uid_counter_advanced`
//! decision. It never moves the counter back.
//!
//! After Redis is lost, the stream still holds this node's last upsert for
//! tracks that are no longer live anywhere. The engine queues a delete
//! (through the writer, like any retired track) for each subject of this
//! site whose last message is an upsert this node published and whose UID
//! has no system track in Redis. Other sites' subjects, and subjects last
//! written by another publisher, are left alone.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use ot_core::{SiteCode, Uid};
use ot_store::UidSighting;
use serde_json::{Value, json};

use super::Engine;

/// At most this many subjects are read from the tracks stream.
const MAX_SUBJECTS: usize = 1_000_000;
/// At most this many stale subjects are looked at (and deleted) per start.
const MAX_STALE: usize = 20_000;
/// Last messages read at once when looking for stale tracks, and how long
/// that may take in all.
const CONCURRENCY: usize = 32;
const STALE_BUDGET: Duration = Duration::from_secs(60);

/// Why a stale track is deleted, as published in its delete.
pub(crate) const STALE_REASON: &str = "not live after a restart (live state was lost)";

/// A UID in a tracks-stream subject, and whether it is the track's own
/// subject (not a deleted history point's).
pub(crate) fn subject_uid(prefix: &str, subject: &str) -> Option<(Uid, bool)> {
    let rest = subject.strip_prefix(prefix)?.strip_prefix('.')?;
    if let Ok(uid) = Uid::from_doc_id(rest) {
        return Some((uid, true));
    }
    let point = rest.strip_prefix("history.")?;
    let (doc, _t) = point.rsplit_once('.')?;
    Some((Uid::from_doc_id(doc).ok()?, false))
}

/// What the tracks stream showed.
enum NatsView {
    /// Not asked to look (replays, benchmarks, most tests).
    NotChecked,
    Unreachable(String),
    NoStream,
    Seen {
        nats: Box<ot_nats::Nats>,
        /// This site's UIDs with a track subject, and the highest seen.
        tracks: Vec<Uid>,
        highest: Option<UidSighting>,
        subjects: usize,
        truncated: bool,
    },
}

impl NatsView {
    fn checked(&self) -> Value {
        match self {
            NatsView::NotChecked => json!("not checked"),
            NatsView::Unreachable(e) => json!(format!("unreachable: {e}")),
            NatsView::NoStream => json!("no tracks stream"),
            NatsView::Seen {
                subjects,
                truncated,
                ..
            } => json!({ "subjects": subjects, "truncated": truncated }),
        }
    }
}

async fn look_at_nats(
    settings: ot_nats::NatsSettings,
    site: SiteCode,
    budget: Duration,
) -> NatsView {
    let started = Instant::now();
    let stream = settings.stream.clone();
    let prefix = settings.tracks_subject.clone();
    let nats = match ot_nats::Nats::connect(settings).await {
        Ok(n) => n,
        Err(e) => return NatsView::Unreachable(e.to_string()),
    };
    // The client connects in the background: wait for it, within the budget.
    while nats.client().connection_state() != ot_nats::async_nats::connection::State::Connected {
        if started.elapsed() >= budget {
            return NatsView::Unreachable(format!("not connected within {budget:?}"));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let left = budget.saturating_sub(started.elapsed());
    let listed = tokio::time::timeout(
        left,
        nats.stream_subjects(&format!("{prefix}.>"), MAX_SUBJECTS),
    )
    .await;
    let (subjects, truncated) = match listed {
        Err(_) => return NatsView::Unreachable(format!("listing {stream} took over {budget:?}")),
        Ok(Err(e)) => return NatsView::Unreachable(e.to_string()),
        Ok(Ok(None)) => return NatsView::NoStream,
        Ok(Ok(Some(s))) => s,
    };
    let mut tracks = Vec::new();
    let mut highest: Option<UidSighting> = None;
    for subject in &subjects {
        let Some((uid, own)) = subject_uid(&prefix, subject) else {
            continue;
        };
        if uid.site() != site {
            continue;
        }
        if own {
            tracks.push(uid);
        }
        if highest.as_ref().is_none_or(|h| uid.sequence() > h.sequence) {
            highest = Some(UidSighting::new("nats", subject.clone(), uid.sequence()));
        }
    }
    NatsView::Seen {
        nats: Box::new(nats),
        tracks,
        highest,
        subjects: subjects.len(),
        truncated,
    }
}

impl Engine {
    /// Move the UID counter past every number of this site in use (see the
    /// module docs), then delete tracks left in NATS that are not live.
    pub(super) async fn guard_uid_counter(&mut self) -> anyhow::Result<()> {
        let site = self.common.site;
        let c = self.common.clone();
        let (next, mut sightings) = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
            let db = c.open_db()?;
            Ok((db.uid_next_sequence(site)?, db.uid_sightings(site)?))
        })
        .await??;
        sightings.extend(self.redis.uid_sightings(site).await?);
        let view = match self.settings.uid_check_nats {
            Some(budget) => look_at_nats(self.common.nats_settings(), site, budget).await,
            None => NatsView::NotChecked,
        };
        match &view {
            NatsView::Unreachable(e) => tracing::warn!(
                %site, error = %e,
                "could not check the NATS tracks stream for track numbers already published; \
                 relying on the database and Redis"
            ),
            NatsView::Seen {
                truncated: true, ..
            } => tracing::warn!(
                %site, limit = MAX_SUBJECTS,
                "the NATS tracks stream holds more subjects than were checked for track numbers"
            ),
            _ => {}
        }
        if let NatsView::Seen {
            highest: Some(h), ..
        } = &view
        {
            sightings.push(h.clone());
        }
        let checked = json!({
            "sqlite": "checked",
            "redis": "checked",
            "nats": view.checked(),
        });
        let c = self.common.clone();
        let found = sightings.clone();
        let advanced = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
            Ok(c.open_db()?.advance_uid_counter(site, &found, checked)?)
        })
        .await??;
        match advanced {
            Some(a) => {
                let found: Vec<String> = sightings
                    .iter()
                    .filter(|s| s.sequence >= a.before)
                    .map(|s| format!("{} {} ({})", s.store, s.location, s.sequence))
                    .collect();
                tracing::warn!(
                    %site, before = a.before, after = a.after, decision = a.decision_id,
                    found = ?found,
                    "track numbers at or past the UID counter are already in use \
                     (database restored from a backup?); counter advanced"
                );
            }
            None => tracing::info!(%site, next, "UID counter is past every track number in use"),
        }
        if let NatsView::Seen { nats, tracks, .. } = view
            && let Err(e) = self.delete_stale(&nats, tracks).await
        {
            tracing::warn!(error = %format!("{e:#}"), "deleting stale tracks in NATS failed");
        }
        Ok(())
    }

    /// Queue deletes for this site's tracks the stream still shows (its last
    /// message an upsert this node published) that have no live state here.
    async fn delete_stale(&self, nats: &ot_nats::Nats, tracks: Vec<Uid>) -> anyhow::Result<()> {
        let mut live: HashSet<Uid> = self.redis.live_system_uids().await?;
        live.extend(self.tracks.keys().copied());
        live.extend(self.groups.keys().copied());
        let mut stale: Vec<Uid> = tracks.into_iter().filter(|u| !live.contains(u)).collect();
        if stale.is_empty() {
            return Ok(());
        }
        stale.sort();
        if stale.len() > MAX_STALE {
            tracing::warn!(
                candidates = stale.len(),
                limit = MAX_STALE,
                "more stale-looking tracks in NATS than are checked on one start"
            );
            stale.truncate(MAX_STALE);
        }
        let prefix = nats.settings().tracks_subject.clone();
        let ours = format!("{}:", self.common.node_id());
        let looked = futures_util::stream::iter(stale)
            .map(|uid| {
                let subject = ot_core::wire::subject(&prefix, uid);
                async move {
                    let last = nats.last_message(&subject).await;
                    (uid, last)
                }
            })
            .buffer_unordered(CONCURRENCY)
            .filter_map(|(uid, last)| {
                let ours = ours.clone();
                async move {
                    let (headers, _) = last.ok()??;
                    let header = |name: &str| {
                        headers
                            .iter()
                            .find(|(k, _)| k.eq_ignore_ascii_case(name))
                            .map(|(_, v)| v.as_str())
                    };
                    let upsert = header(ot_core::wire::HEADER_OP) == Some("upsert");
                    let mine = header("Nats-Msg-Id").is_some_and(|id| id.starts_with(&ours));
                    (upsert && mine).then_some(uid)
                }
            })
            .collect::<Vec<Uid>>();
        let Ok(upserts) = tokio::time::timeout(STALE_BUDGET, looked).await else {
            tracing::warn!(
                budget = ?STALE_BUDGET,
                "looking for stale tracks in NATS took too long; none deleted"
            );
            return Ok(());
        };
        if upserts.is_empty() {
            return Ok(());
        }
        tracing::warn!(
            site = %self.common.site, count = upserts.len(),
            "publishing deletes for this site's tracks still shown in NATS but not live here \
             (live state lost?)"
        );
        // One decision for the lot, so the deletes are in the audit record.
        let c = self.common.clone();
        let listed: Vec<String> = upserts.iter().map(|u| u.to_string()).collect();
        let decision = ot_store::Decision::new("system", "stale_tracks_deleted")
            .reason(STALE_REASON)
            .evidence(json!({
                "site": self.common.site.as_str(),
                "count": listed.len(),
                "uids": listed,
            }));
        tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
            Ok(c.open_db()?.record(&decision)?)
        })
        .await??;
        for uid in upserts {
            self.redis.retire_system_track(uid, STALE_REASON).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Common;
    use crate::engine::EngineSettings;
    use crate::engine::tests::{obs, t0, test_common};
    use ot_core::{Domain, SystemTrack};
    use ot_store::{Decision, OutboxOp, RedisStore};

    fn uid(s: &str) -> Uid {
        s.parse().unwrap()
    }

    async fn put_live(redis: &RedisStore, u: &str) {
        let t = SystemTrack::from_first_observation(uid(u), obs(t0(), "A", Domain::Surface));
        redis.put_system_track(&t, false).await.unwrap();
    }

    fn counter(c: &Common) -> u64 {
        c.open_db().unwrap().uid_next_sequence(c.site).unwrap()
    }

    fn decisions(c: &Common) -> Vec<Value> {
        let db = c.open_db().unwrap();
        let ids: Vec<i64> = db
            .connection()
            .prepare("SELECT id FROM decisions WHERE op = 'uid_counter_advanced' ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        ids.into_iter()
            .map(|id| db.decision(id).unwrap().unwrap())
            .collect()
    }

    async fn start(c: &Common, settings: EngineSettings) -> Engine {
        Engine::new(c.clone(), settings).await.unwrap()
    }

    #[tokio::test]
    async fn advances_past_a_number_live_in_redis() {
        let Some((c, _dir)) = test_common("UGA") else {
            eprintln!("skipped: OT_TEST_REDIS_URL not set");
            return;
        };
        let redis = c.open_redis().await.unwrap();
        put_live(&redis, "UGA000000050").await;
        let e = start(&c, EngineSettings::default()).await;
        assert_eq!(counter(&c), 51);
        let d = decisions(&c);
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(
            d[0]["evidence"]["next_sequence"],
            json!({"before": 1, "after": 51})
        );
        let found = &d[0]["evidence"]["found"][0];
        assert_eq!(found["store"], "redis");
        assert_eq!(found["uid"], "UGA000000050");
        assert_eq!(d[0]["evidence"]["checked"]["nats"], "not checked");
        // The next track gets a new number.
        let (next, _) = c
            .open_db()
            .unwrap()
            .create_system_track(c.site, "ais", "1", Decision::new("engine", "create"))
            .unwrap();
        assert_eq!(next.to_string(), "UGA000000051");
        // A second start finds nothing new.
        drop(e);
        start(&c, EngineSettings::default()).await;
        assert_eq!(decisions(&c).len(), 1);
        redis.purge_namespace().await.unwrap();
    }

    #[tokio::test]
    async fn advances_past_a_number_in_the_track_graph() {
        let Some((c, _dir)) = test_common("UGB") else {
            eprintln!("skipped: OT_TEST_REDIS_URL not set");
            return;
        };
        {
            let mut db = c.open_db().unwrap();
            for key in ["1", "2", "3"] {
                db.create_system_track(c.site, "ais", key, Decision::new("engine", "create"))
                    .unwrap();
            }
            // The counter row as an older backup had it.
            db.connection()
                .execute("UPDATE uid_sequences SET next_sequence = 2", [])
                .unwrap();
        }
        start(&c, EngineSettings::default()).await;
        assert_eq!(counter(&c), 4);
        let d = decisions(&c);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0]["evidence"]["found"][0]["store"], "sqlite");
        c.open_redis()
            .await
            .unwrap()
            .purge_namespace()
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn never_moves_the_counter_back() {
        let Some((c, _dir)) = test_common("UGC") else {
            eprintln!("skipped: OT_TEST_REDIS_URL not set");
            return;
        };
        c.open_db()
            .unwrap()
            .connection()
            .execute(
                "INSERT INTO uid_sequences (site, next_sequence) VALUES ('UGC', 1000)",
                [],
            )
            .unwrap();
        let redis = c.open_redis().await.unwrap();
        put_live(&redis, "UGC000000005").await;
        put_live(&redis, "UGC000000999").await;
        start(&c, EngineSettings::default()).await;
        assert_eq!(counter(&c), 1000);
        assert!(decisions(&c).is_empty());
        redis.purge_namespace().await.unwrap();
    }

    #[tokio::test]
    async fn an_unreachable_nats_is_skipped() {
        let Some((c, _dir)) = test_common("UGD") else {
            eprintln!("skipped: OT_TEST_REDIS_URL not set");
            return;
        };
        let redis = c.open_redis().await.unwrap();
        put_live(&redis, "UGD000000007").await;
        let settings = EngineSettings {
            uid_check_nats: Some(Duration::from_millis(500)),
            ..Default::default()
        };
        let started = Instant::now();
        let e = start(&c, settings).await;
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(e.tracks.contains_key(&uid("UGD000000007")));
        assert_eq!(counter(&c), 8);
        let d = decisions(&c);
        let nats = d[0]["evidence"]["checked"]["nats"].as_str().unwrap();
        assert!(nats.starts_with("unreachable"), "{nats}");
        redis.purge_namespace().await.unwrap();
    }

    /// Against a real server: set `OT_TEST_NATS_URL` (and `OT_TEST_REDIS_URL`).
    #[tokio::test]
    async fn looks_in_the_stream_and_deletes_this_nodes_stale_tracks() {
        let (Some((mut c, _dir)), Ok(url)) =
            (test_common("UGE"), std::env::var("OT_TEST_NATS_URL"))
        else {
            eprintln!("skipped: OT_TEST_REDIS_URL or OT_TEST_NATS_URL not set");
            return;
        };
        let run = format!(
            "{}{}",
            std::process::id(),
            chrono::Utc::now().timestamp_micros()
        );
        c.nats.url = url;
        c.nats.stream = format!("OT_TEST_UID_{run}");
        c.nats.tracks_subject = format!("ottestuid{run}");
        let nats = c.connect_nats().await.unwrap();
        nats.ensure_stream().await.unwrap();
        let prefix = c.nats.tracks_subject.clone();
        let send = |doc: &str, msg_id: &str, op: &str| ot_nats::Outgoing {
            subject: format!("{prefix}.{doc}"),
            msg_id: msg_id.into(),
            headers: vec![(ot_core::wire::HEADER_OP, op.into())],
            body: bytes::Bytes::from_static(b"{}"),
        };
        use ot_nats::TrackSink;
        for m in [
            // Published by this node, not live any more: stale.
            send("tms-UGE000000090", "opentrack-UGE:1", "upsert"),
            // Live in Redis: left alone.
            send("tms-UGE000000010", "opentrack-UGE:2", "upsert"),
            // Already deleted: left alone.
            send("tms-UGE000000020", "opentrack-UGE:3", "delete"),
            // Last written by another publisher: left alone, but counted.
            send("tms-UGE000000091", "opentrack-BBB:1", "upsert"),
            // Another site's: neither counted nor deleted.
            send("tms-AAA000000999", "opentrack-AAA:1", "upsert"),
        ] {
            nats.publish(m).await.unwrap();
        }
        let redis = c.open_redis().await.unwrap();
        put_live(&redis, "UGE000000010").await;
        let settings = EngineSettings {
            uid_check_nats: Some(Duration::from_secs(10)),
            ..Default::default()
        };
        start(&c, settings).await;
        assert_eq!(counter(&c), 92);
        let d = decisions(&c);
        let found = d[0]["evidence"]["found"].as_array().unwrap();
        let in_nats = found.iter().find(|f| f["store"] == "nats").unwrap();
        assert_eq!(in_nats["location"], format!("{prefix}.tms-UGE000000091"));
        assert_eq!(in_nats["uid"], "UGE000000091");
        redis.ensure_outbox_group("t").await.unwrap();
        let deletes: Vec<Uid> = redis
            .read_outbox("t", "t", 100, Duration::ZERO, false)
            .await
            .unwrap()
            .into_iter()
            .filter_map(|e| match e.op {
                OutboxOp::Tombstone { uid, reason } => {
                    assert_eq!(reason.as_deref(), Some(STALE_REASON));
                    Some(uid)
                }
                _ => None,
            })
            .collect();
        assert_eq!(deletes, [uid("UGE000000090")]);
        let db = c.open_db().unwrap();
        let id: i64 = db
            .connection()
            .query_row(
                "SELECT id FROM decisions WHERE op = 'stale_tracks_deleted'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let d = db.decision(id).unwrap().unwrap();
        assert_eq!(d["evidence"]["uids"], json!(["UGE000000090"]));
        ot_nats::async_nats::jetstream::new(nats.client().clone())
            .delete_stream(&c.nats.stream)
            .await
            .unwrap();
        redis.purge_namespace().await.unwrap();
    }

    #[test]
    fn reads_uids_from_track_and_history_subjects() {
        let uid: Uid = "OTK000000042".parse().unwrap();
        assert_eq!(
            subject_uid("tracks", "tracks.tms-OTK000000042"),
            Some((uid, true))
        );
        assert_eq!(
            subject_uid("tracks", "tracks.history.tms-OTK000000042.1790000000123"),
            Some((uid, false))
        );
        assert_eq!(subject_uid("tracks", "tracks.tms-nope"), None);
        assert_eq!(subject_uid("tracks", "other.tms-OTK000000042"), None);
        assert_eq!(subject_uid("tracks", "tracksx.tms-OTK000000042"), None);
    }
}
