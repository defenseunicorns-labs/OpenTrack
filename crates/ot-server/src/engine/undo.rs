//! Undo on the live picture: the store reverses the decision in the track
//! graph ([`ot_store::undo`]); here the engine takes its tracks, pairings,
//! groups and "do not pair" set back from the graph for what changed, and
//! publishes the result (a track brought back is published under its UID
//! again; one left with no source tracks is retired).

use ot_core::{Contributor, PairingType, SystemTrack, TrackState, Uid};
use serde_json::{Value, json};

use super::{DETECTIONS, Engine, latest_key, resolve};

impl Engine {
    pub(super) async fn undo(
        &mut self,
        id: i64,
        actor: &str,
        reason: &str,
    ) -> anyhow::Result<Value> {
        let (a, r) = (actor.to_owned(), reason.to_owned());
        let done = self.db(move |db| db.undo_decision(id, &a, &r)).await?;
        self.reports = self.db(|db| db.live_reports()).await?.into_iter().collect();
        if done.do_not_pair_changed {
            self.do_not_pair = self
                .db(|db| db.do_not_pairs())
                .await?
                .into_iter()
                .map(|(a, b)| if a < b { (a, b) } else { (b, a) })
                .collect();
        }
        for &uid in &done.tracks {
            let live = self.reports.values().any(|u| *u == uid);
            if live {
                self.rebuild(uid).await?;
            } else if self.tracks.contains_key(&uid) {
                let published = self.forget(uid);
                self.retire_in_redis(uid, published, "undone").await?;
            }
        }
        if done.groups_changed {
            let stored: Vec<SystemTrack> = self.groups.drain().map(|(_, s)| s.track).collect();
            self.load_groups(stored).await?;
        }
        self.refresh_groups().await?;
        tracing::info!(decision = id, op = %done.op, %actor, "undone");
        Ok(json!(done))
    }

    /// Take a track's source tracks and pairings from the graph, keeping
    /// what memory knows of each (and its detections), and publish it.
    async fn rebuild(&mut self, uid: Uid) -> anyhow::Result<()> {
        let links = self.db(move |db| db.reports_for(uid)).await?;
        let paired = self.db(move |db| db.paired_with(uid)).await?;
        let mut contributors = Vec::new();
        let mut first = None;
        for (key, attrs) in links {
            let Some((source, track_key)) = key.split_once('/') else {
                continue;
            };
            let obs = match self.latest.get(&key).cloned() {
                Some(o) => Some(o),
                None => self.redis.get_source_track(source, track_key).await?,
            };
            let Some(obs) = obs else {
                continue;
            };
            let pairing = match attrs["pairing"].as_str() {
                Some("manual") => PairingType::Manual,
                _ => PairingType::Auto,
            };
            contributors.push(Contributor {
                source_id: source.to_owned(),
                source_track_key: track_key.to_owned(),
                pairing,
                confidence: attrs["confidence"].as_f64().unwrap_or(1.0),
                last_report: obs.observed_at,
                existence: obs.provenance.confidence,
            });
            self.latest
                .insert(latest_key(uid, source, track_key), obs.clone());
            first.get_or_insert(obs);
        }
        let Some(first) = first else {
            // Its source tracks have no reports left to show it by.
            if self.tracks.contains_key(&uid) {
                let published = self.forget(uid);
                self.retire_in_redis(uid, published, "undone: nothing reports for it")
                    .await?;
            }
            return Ok(());
        };
        let confirm_after = self.settings.confirm_after;
        let t = self.tracks.entry(uid).or_insert_with(|| {
            let mut t = SystemTrack::from_first_observation(uid, first);
            t.observation_count = confirm_after;
            t.state = TrackState::Confirmed;
            t
        });
        let detections: Vec<Contributor> = t
            .contributors
            .iter()
            .filter(|c| c.source_track_key == DETECTIONS)
            .cloned()
            .collect();
        t.contributors = contributors;
        t.contributors.extend(detections);
        t.paired_with = paired;
        if let Some((view, provenance)) = self.best_of(uid)
            && let Some(t) = self.tracks.get_mut(&uid)
        {
            t.view = view;
            t.provenance = provenance;
            resolve(&self.attrs, t);
        }
        if let Some(t) = self.tracks.get(&uid) {
            self.grid
                .put(uid, t.view.position.latitude, t.view.position.longitude);
        }
        self.index_track(uid);
        self.misses.retain(|(u, _), _| *u != uid);
        self.candidates.retain(|(a, b), _| *a != uid && *b != uid);
        self.save(uid, true).await?;
        Ok(())
    }
}
