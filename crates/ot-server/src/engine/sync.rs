//! Track management between nodes (see `docs/multi-node.md`). A command a
//! person runs here that holds on every node (see [`ot_sync::replicated`])
//! is logged under a global id with this node's clock, and the local
//! decisions it made are tagged with that id. Another node's entry is kept,
//! judged against what was applied here (a later decision on the same
//! tracks wins), and applied once every track it names is here; until then
//! it waits.

use chrono::Utc;
use ot_core::{Observation, SystemTrack, Uid};
use ot_store::SyncStatus;
use ot_sync::{Entry, GlobalId, Verdict};
use serde_json::{Value, json};

use super::Engine;

/// Source ids of other nodes' reports: `peer:<site>`.
pub const PEER: &str = "peer:";

/// How long another node's entry waits for its tracks.
const PENDING_FOR_MS: i64 = 12 * 3600 * 1000;

/// A command's logged form, from what it was given and what it answered:
/// UIDs it minted or resolved are filled in, and who ran it is left to the
/// entry.
fn canonical(cmd: &Value, result: &Value) -> Option<Value> {
    let op = cmd["op"].as_str()?;
    if !ot_sync::replicated(op) || op == "undo" {
        return None;
    }
    let mut c = cmd.clone();
    let o = c.as_object_mut()?;
    for k in ["id", "actor", "role"] {
        o.remove(k);
    }
    match op {
        "merge" => {
            o.insert("into".into(), result["merged_into"].clone());
        }
        "delete" => {
            o.insert("tracks".into(), result["deleted"].clone());
        }
        "group_create" => {
            o.insert("group".into(), result["group"].clone());
        }
        _ => {}
    }
    Some(c)
}

fn uid_strings(v: &Value) -> Vec<String> {
    let one = |v: &Value| {
        v.as_str()?
            .trim_start_matches(ot_core::uid::DOC_ID_PREFIX)
            .parse::<Uid>()
            .ok()
            .map(|u| u.to_string())
    };
    match v {
        Value::Array(a) => a.iter().filter_map(one).collect(),
        other => one(other).into_iter().collect(),
    }
}

impl Engine {
    /// Run an operator command, logging it for every node when it is track
    /// management that replicates. Another node's entry arrives as
    /// `{"op": "sync_entry", "entry": ...}`.
    pub(super) async fn command(&mut self, cmd: &Value) -> anyhow::Result<Value> {
        let op = cmd["op"].as_str().unwrap_or_default();
        if let Some(answer) = self.link_command(cmd).await? {
            return Ok(answer);
        }
        if op == "sync_entry" {
            let entry: Entry = serde_json::from_value(cmd["entry"].clone())
                .map_err(|e| anyhow::anyhow!("entry: {e}"))?;
            let status = self.receive(entry).await?;
            return Ok(json!({ "status": status.as_str() }));
        }
        if !(ot_sync::replicated(op) || matches!(op, "accept" | "reject")) {
            return self.run_command(cmd).await;
        }
        if op.starts_with("profile_") {
            anyhow::bail!("{op} comes only from another node");
        }
        if self.receive_only {
            anyhow::bail!(
                "this node is receive-only: track management is done on another node (Settings)"
            );
        }
        let before = self.db(|db| db.max_decision_id()).await?;
        self.canonical = None;
        let result = self.run_command(cmd).await?;
        let logged = match self.canonical.take() {
            Some(c) => Some(c),
            None if op == "undo" => {
                // An undo holds everywhere when what it undid did.
                let undone = cmd["decision"].as_i64().unwrap_or_default();
                self.db(move |db| db.sync_of_decision(undone))
                    .await?
                    .map(|(id, part)| {
                        json!({ "op": "undo", "decision": id.to_string(), "part": part,
                                "reason": cmd["reason"] })
                    })
            }
            None => canonical(cmd, &result),
        };
        if let Some(command) = logged {
            let actor = cmd["actor"].as_str().unwrap_or("operator").to_owned();
            let role = cmd["role"].as_str().unwrap_or("track_manager").to_owned();
            let parts = if command["op"] == "delete" {
                uid_strings(&command["tracks"])
            } else {
                Vec::new()
            };
            let site = self.common.site;
            let now = Utc::now().timestamp_millis() as u64;
            let id = self
                .db(move |db| {
                    let entry = db.sync_append_own(site, &actor, &role, &command, now)?;
                    db.tag_decisions(before, &actor, entry.id, &parts)?;
                    Ok(entry.id)
                })
                .await?;
            tracing::debug!(%id, %op, "logged for every node");
        }
        Ok(result)
    }

    /// Another node's entry: keep it once, and apply it if it can be.
    async fn receive(&mut self, entry: Entry) -> anyhow::Result<SyncStatus> {
        if entry.id.site == self.common.site {
            // Our own, heard back.
            return Ok(SyncStatus::Applied);
        }
        let e = entry.clone();
        let fresh = self
            .db(move |db| db.sync_record(&e, SyncStatus::Pending, None))
            .await?;
        if !fresh {
            let id = entry.id;
            let status = self.db(move |db| db.sync_entry(id)).await?;
            return Ok(status.map_or(SyncStatus::Failed, |(_, s)| s));
        }
        self.apply_entry(&entry).await
    }

    /// Apply entries still waiting for their tracks, and give up on old ones.
    pub(super) async fn retry_pending(&mut self) -> anyhow::Result<()> {
        let cutoff = Utc::now().timestamp_millis() - PENDING_FOR_MS;
        let pending = self
            .db(move |db| {
                db.sync_expire_pending(cutoff)?;
                db.sync_pending()
            })
            .await?;
        for entry in pending {
            self.apply_entry(&entry).await?;
        }
        Ok(())
    }

    /// Another node's output schema or correlation settings, saved here as
    /// this node's own (when this node shares the profile).
    async fn apply_profile(&mut self, cmd: &Value) -> anyhow::Result<Value> {
        if !self.share_profile {
            return Ok(json!({ "skipped": "this node does not share the profile" }));
        }
        let actor = cmd["actor"].as_str().unwrap_or("operator").to_owned();
        match cmd["op"].as_str().unwrap_or_default() {
            "profile_correlation" => {
                let settings: crate::correlate::CorrelationSettings =
                    serde_json::from_value(cmd["settings"].clone())
                        .map_err(|e| anyhow::anyhow!("settings: {e}"))?;
                settings.validate().map_err(|e| anyhow::anyhow!(e))?;
                let value = serde_json::to_value(&settings)?;
                let d = ot_store::Decision::new(&actor, "correlation_settings")
                    .reason("correlation settings from another node")
                    .evidence(value.clone());
                self.db(move |db| db.save_correlation_settings(&value, d))
                    .await?;
                self.refresh_correlation().await?;
                Ok(json!({}))
            }
            "profile_schema" => {
                let fields: Vec<Value> = serde_json::from_value(cmd["fields"].clone())
                    .map_err(|e| anyhow::anyhow!("fields: {e}"))?;
                let notes = cmd["notes"].as_str().map(str::to_owned);
                let v = self
                    .db(move |db| db.publish_schema_as(&fields, notes.as_deref(), &actor))
                    .await?;
                self.refresh_attributes().await?;
                Ok(json!({ "version": v.version }))
            }
            other => anyhow::bail!("unknown profile change {other:?}"),
        }
    }

    /// Another node's report of its track: it reports for the system track
    /// of the same UID here, made for it if this node has none.
    pub(super) async fn adopt(
        &mut self,
        obs: &Observation,
        uid: Uid,
        counts: &mut super::EngineCounts,
    ) -> anyhow::Result<Uid> {
        let (source, key) = (obs.source_id.clone(), obs.source_track_key.clone());
        // A number this node merged away: the report is of the survivor.
        let uid = if self.tracks.contains_key(&uid) {
            uid
        } else {
            self.tracks
                .values()
                .find(|t| t.aliases.contains(&uid))
                .map_or(uid, |t| t.uid)
        };
        if self.tracks.contains_key(&uid) {
            let attrs = json!({ "pairing": "auto", "confidence": 1.0 });
            let d = super::engine_decision("pair")
                .reason(format!("{source} reports its track {}", uid.doc_id()));
            let (s2, k2) = (source.clone(), key.clone());
            self.db(move |db| db.pair_source_track(&s2, &k2, uid, &attrs, d))
                .await?;
            if let Some(t) = self.tracks.get_mut(&uid) {
                t.contributors.push(ot_core::Contributor {
                    source_id: source.clone(),
                    source_track_key: key.clone(),
                    pairing: ot_core::PairingType::Auto,
                    confidence: 1.0,
                    last_report: obs.observed_at,
                    existence: obs.provenance.confidence,
                });
            }
            counts.paired += 1;
        } else {
            let d = super::engine_decision("create_system_track")
                .reason(format!("{source} reports its track {}", uid.doc_id()));
            let (s2, k2) = (source.clone(), key.clone());
            self.db(move |db| db.adopt_system_track(uid, &s2, &k2, d))
                .await?;
            counts.created += 1;
        }
        self.reports.insert(format!("{source}/{key}"), uid);
        Ok(uid)
    }

    fn known(&self, uid: Uid) -> bool {
        self.tracks.contains_key(&uid) || self.groups.contains_key(&uid)
    }

    async fn set_status(
        &self,
        id: GlobalId,
        status: SyncStatus,
        detail: Option<String>,
    ) -> anyhow::Result<SyncStatus> {
        self.db(move |db| db.sync_set_status(id, status, detail.as_deref()))
            .await?;
        Ok(status)
    }

    /// Apply a kept entry of another node's here, recording where it stands.
    async fn apply_entry(&mut self, entry: &Entry) -> anyhow::Result<SyncStatus> {
        let id = entry.id;
        if let Some(by) = self.db(move |db| db.sync_undone_by(id)).await? {
            // Undone before it could be applied here: it never will be.
            self.set_status(by, SyncStatus::Applied, None).await?;
            return self
                .set_status(id, SyncStatus::Superseded, Some(format!("undone by {by}")))
                .await;
        }
        let hlc = entry.hlc;
        let later = self.db(move |db| db.sync_applied_after(hlc)).await?;
        if let Verdict::Superseded(by) = ot_sync::judge(entry, &later) {
            return self
                .set_status(
                    id,
                    SyncStatus::Superseded,
                    Some(format!("{by} decided later on the same tracks")),
                )
                .await;
        }
        let mut cmd = entry.command.clone();
        cmd["actor"] = json!(entry.actor);
        cmd["role"] = json!(entry.role);
        let op = entry.op().to_owned();
        let mut parts = Vec::new();
        match op.as_str() {
            "undo" => {
                let target: GlobalId = cmd["decision"]
                    .as_str()
                    .unwrap_or_default()
                    .parse()
                    .map_err(|e| anyhow::anyhow!("undo: {e}"))?;
                let part = cmd["part"].as_str().map(str::to_owned);
                let local = self
                    .db(move |db| db.decision_of_sync(target, part.as_deref()))
                    .await?;
                match local {
                    Some(d) => cmd["decision"] = json!(d),
                    None => {
                        let target_entry = self.db(move |db| db.sync_entry(target)).await?;
                        return match target_entry {
                            // Not here yet: when it comes, it is superseded.
                            None => Ok(SyncStatus::Pending),
                            Some((_, SyncStatus::Applied)) => {
                                self.set_status(
                                    id,
                                    SyncStatus::Failed,
                                    Some(format!("{target} made no decision here")),
                                )
                                .await
                            }
                            Some(_) => {
                                self.set_status(
                                    target,
                                    SyncStatus::Superseded,
                                    Some(format!("undone by {id}")),
                                )
                                .await?;
                                self.set_status(id, SyncStatus::Applied, None).await
                            }
                        };
                    }
                }
            }
            "delete" => {
                // Whatever of it is here; the rest never arrived or went already.
                let here: Vec<Uid> = ot_sync::targets(&cmd)
                    .into_iter()
                    .filter(|u| self.known(*u))
                    .collect();
                if here.is_empty() {
                    return Ok(SyncStatus::Pending);
                }
                cmd["tracks"] = json!(here.iter().map(|u| u.doc_id()).collect::<Vec<_>>());
                parts = here.iter().map(|u| u.to_string()).collect();
            }
            _ => {
                let mut names = ot_sync::targets(&cmd);
                if op == "group_create" {
                    for g in uid_strings(&cmd["group"]) {
                        let g: Uid = g.parse()?;
                        if self.groups.contains_key(&g) {
                            return self
                                .set_status(id, SyncStatus::Applied, Some("formed already".into()))
                                .await;
                        }
                        names.remove(&g);
                    }
                }
                if !names.iter().all(|u| self.known(*u)) {
                    return Ok(SyncStatus::Pending);
                }
            }
        }
        let actor = entry.actor.clone();
        let before = self.db(|db| db.max_decision_id()).await?;
        self.remote = true;
        let result = if op.starts_with("profile_") {
            self.apply_profile(&cmd).await
        } else {
            self.run_command(&cmd).await
        };
        self.remote = false;
        match result {
            Ok(_) => {
                self.db(move |db| db.tag_decisions(before, &actor, id, &parts))
                    .await?;
                tracing::info!(%id, %op, actor = %entry.actor, "applied another node's decision");
                self.set_status(id, SyncStatus::Applied, None).await
            }
            Err(e) => {
                tracing::warn!(%id, %op, error = %format!("{e:#}"), "another node's decision failed here");
                self.set_status(id, SyncStatus::Failed, Some(format!("{e:#}")))
                    .await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_is_logged_with_what_it_resolved() {
        let c = canonical(
            &json!({"op": "merge", "from": "tms-AAA000000001", "into": "tms-AAA000000002",
                    "actor": "bob", "id": "x"}),
            &json!({"merged_into": "tms-AAA000000002"}),
        )
        .unwrap();
        assert_eq!(
            c,
            json!({"op": "merge", "from": "tms-AAA000000001", "into": "tms-AAA000000002"})
        );
        let c = canonical(
            &json!({"op": "group_create", "members": ["tms-AAA000000001"], "spec": {"name": "G"}}),
            &json!({"group": "tms-AAA000000009"}),
        )
        .unwrap();
        assert_eq!(c["group"], "tms-AAA000000009");
        assert!(canonical(&json!({"op": "split"}), &json!({})).is_none());
        assert!(canonical(&json!({"op": "purge"}), &json!({})).is_none());
    }
}

// --- Tracks between nodes ----------------------------------------------------
//
// Each node reports what its own sources say about the tracks it holds
// reporting responsibility for (`ot_sync::r2`), as often as dead reckoning
// needs (`ot_sync::dr`); never what it heard from other nodes. Another
// node's report arrives as a source track of `peer:<site>`, keyed by its
// UID, and reports for the system track of that UID here.

/// What this node has shared about one track.
#[derive(Debug, Default)]
pub(super) struct Shared {
    pub(super) resp: ot_sync::r2::Responsibility,
    sent: Option<ot_sync::dr::Sent>,
    ids: Option<Vec<(String, String)>>,
    attrs: Option<Value>,
}

/// A report waiting to be sent, with what to restore if the budget defers
/// it (so it is due again).
#[derive(Debug)]
struct Queued {
    urgency: f64,
    report: ot_sync::wire::Report,
    prev_sent: Option<ot_sync::dr::Sent>,
    prev_ids: Option<Vec<(String, String)>>,
}

/// Sync messages waiting for the end of the batch.
#[derive(Debug, Default)]
pub(super) struct SyncOut {
    reports: Vec<Queued>,
    attrs: Vec<(Uid, Value)>,
    release: Vec<Uid>,
}

pub(super) fn is_peer(source: &str) -> bool {
    source.starts_with(PEER)
}

/// A view's position error as an ellipse (none: unknown).
fn error_ellipse(o: &Observation) -> Option<ot_core::schema::Ellipse> {
    let u = o.uncertainty.as_ref()?;
    u.ellipse.or_else(|| {
        u.position_covariance()
            .map(ot_core::schema::Ellipse::from_covariance)
    })
}

/// Track quality of a view, from its error rounded as the wire carries it.
fn quality_of(o: &Observation) -> u8 {
    error_ellipse(o).map_or(0, |e| ot_sync::quality(e.semi_major_m.round()))
}

impl Engine {
    /// The view of a track from this node's own sources only (none when
    /// only other nodes report it).
    pub(super) fn local_view(&self, uid: Uid) -> Option<Observation> {
        let t = self.tracks.get(&uid)?;
        let local = t
            .contributors
            .iter()
            .filter(|c| !is_peer(&c.source_id))
            .count();
        if local == 0 {
            None
        } else if local == t.contributors.len() {
            Some(t.view.clone())
        } else {
            self.best_of_where(uid, |c| !is_peer(&c.source_id))
                .map(|(v, _)| v)
        }
    }

    /// Whether this node's own sources make the track worth sharing: it is
    /// confirmed or lost (not tentative), and one of them may stand alone.
    fn locally_authoritative(&self, t: &SystemTrack) -> bool {
        t.kind.is_track()
            && t.state != ot_core::TrackState::Tentative
            && t.contributors.iter().any(|c| {
                !is_peer(&c.source_id) && self.alone.get(&c.source_id).copied().unwrap_or(true)
            })
    }

    fn sync_now_ms(&self) -> i64 {
        self.now().timestamp_millis()
    }

    /// Decide whether this node reports a track, and queue a report (and
    /// its attributes) when one is due.
    pub(super) fn sync_consider(&mut self, uid: Uid) {
        if !self.sync_on {
            return;
        }
        let Some(t) = self.tracks.get(&uid) else {
            return;
        };
        if !t.kind.is_track() {
            return;
        }
        let local = if self.locally_authoritative(t) {
            self.local_view(uid)
        } else {
            None
        };
        let (me, now) = (self.common.site, self.sync_now_ms());
        let mine = local.as_ref().map(quality_of);
        let (state, first_seen) = (t.state, t.first_seen.timestamp_millis());
        let attrs = json!({
            "name": t.view.name, "callsign": t.view.callsign,
            "classification": t.view.classification, "attributes": t.attributes,
        });
        let entry = self.shared.entry(uid).or_default();
        let was = entry.resp.reporting;
        let reporting = entry.resp.decide(me, mine, now);
        let reporter = if reporting {
            Some(me.to_string())
        } else {
            entry
                .resp
                .other
                .filter(|h| now - h.at_ms < ot_sync::r2::TAKE_OVER_MS)
                .map(|h| h.site.to_string())
        };
        if let Some(t) = self.tracks.get_mut(&uid)
            && t.reported_by != reporter
        {
            t.reported_by = reporter;
        }
        let entry = self.shared.entry(uid).or_default();
        if !reporting {
            if was {
                entry.sent = None;
                entry.ids = None;
                entry.attrs = None;
                if mine.is_none() {
                    // Its sources lost it: whoever else sees it takes over now.
                    self.sync_out.release.push(uid);
                }
            }
            return;
        }
        let v = local.expect("reporting needs a view");
        let sent = ot_sync::dr::Sent {
            time_ms: v.observed_at.timestamp_millis(),
            lat: v.position.latitude,
            lon: v.position.longitude,
            course_deg: v.kinematics.course_deg,
            speed_mps: v.kinematics.speed_mps,
            state,
        };
        let domain = v.classification.effective_domain();
        if entry.attrs.as_ref() != Some(&attrs) {
            self.sync_out.attrs.push((uid, attrs.clone()));
            entry.attrs = Some(attrs);
        }
        let error_m = error_ellipse(&v).map_or(f64::INFINITY, |e| e.semi_major_m);
        let Some(urgency) = ot_sync::dr::urgency(entry.sent.as_ref(), &sent, domain, error_m)
        else {
            return;
        };
        let (prev_sent, prev_ids) = (entry.sent.clone(), entry.ids.clone());
        // Every report carries the origin: a node that missed the first
        // must not pick a different survivor in a merge meanwhile.
        let origin_ms = Some(first_seen);
        let ids: Vec<(String, String)> = v
            .identifiers
            .iter()
            .map(|i| (i.scheme.clone(), i.value.clone()))
            .collect();
        let identifiers = (entry.ids.as_ref() != Some(&ids)).then(|| ids.clone());
        entry.ids = Some(ids);
        let e = error_ellipse(&v);
        let report = ot_sync::wire::Report {
            uid,
            time_ms: sent.time_ms,
            lat: sent.lat,
            lon: sent.lon,
            alt_m: v.position.altitude_hae_m,
            course_deg: sent.course_deg,
            speed_mps: sent.speed_mps,
            error: e.map_or(
                ot_sync::wire::Ellipse {
                    major_m: f64::INFINITY,
                    minor_m: f64::INFINITY,
                    orientation_deg: 0.0,
                },
                |e| ot_sync::wire::Ellipse {
                    major_m: e.semi_major_m,
                    minor_m: e.semi_minor_m,
                    orientation_deg: e.orientation_deg,
                },
            ),
            quality: mine.unwrap_or(0),
            domain,
            state,
            origin_ms,
            identifiers,
        };
        self.sync_out.reports.push(Queued {
            urgency,
            report,
            prev_sent,
            prev_ids,
        });
        entry.sent = Some(sent);
    }

    /// Another node's report was applied: note who reports the track.
    pub(super) fn sync_heard(&mut self, obs: &Observation) {
        let Some(site) = obs
            .source_id
            .strip_prefix(PEER)
            .and_then(|s| s.parse::<ot_core::SiteCode>().ok())
        else {
            return;
        };
        let key = format!("{}/{}", obs.source_id, obs.source_track_key);
        let Some(&uid) = self.reports.get(&key) else {
            return;
        };
        if obs.state == Some(ot_core::TrackState::Dropped) {
            let now = self.sync_now_ms();
            if let Some(s) = self.shared.get_mut(&uid) {
                s.resp.released(site, now);
            }
            return;
        }
        // Only a report under this track's own number makes its sender the
        // track's reporter.
        if obs.source_track_key != uid.to_string() {
            return;
        }
        let mine = self
            .tracks
            .get(&uid)
            .filter(|t| self.locally_authoritative(t))
            .and_then(|_| self.local_view(uid))
            .map(|v| quality_of(&v));
        let heard = ot_sync::r2::Heard {
            site,
            quality: quality_of(obs),
            at_ms: self.sync_now_ms(),
        };
        let me = self.common.site;
        self.shared
            .entry(uid)
            .or_default()
            .resp
            .heard(me, mine, heard);
    }

    /// A track retired here: peers stop holding it for this node.
    pub(super) fn sync_retired(&mut self, uid: Uid) {
        let Some(s) = self.shared.remove(&uid) else {
            return;
        };
        if let (true, Some(sent)) = (s.resp.reporting, s.sent) {
            let report = ot_sync::wire::Report {
                uid,
                time_ms: sent.time_ms,
                lat: sent.lat,
                lon: sent.lon,
                alt_m: None,
                course_deg: None,
                speed_mps: None,
                error: Default::default(),
                quality: 0,
                domain: None,
                state: ot_core::TrackState::Dropped,
                origin_ms: None,
                identifiers: None,
            };
            self.sync_out.reports.push(Queued {
                urgency: f64::INFINITY,
                report,
                prev_sent: None,
                prev_ids: None,
            });
        }
    }

    /// Heartbeats and take-overs: look at every track this node shares or
    /// has heard of.
    pub(super) async fn sync_tick(&mut self) -> anyhow::Result<()> {
        if !self.sync_on {
            return Ok(());
        }
        let uids: Vec<Uid> = self.shared.keys().copied().collect();
        for uid in uids {
            if self.tracks.contains_key(&uid) {
                self.sync_consider(uid);
            } else {
                self.shared.remove(&uid);
            }
        }
        let reporting = self.shared.values().filter(|s| s.resp.reporting).count();
        self.redis
            .put_sync_status(
                "engine",
                &json!({
                    "at": Utc::now(), "reporting": reporting, "shared": self.shared.len(),
                    "from_other_nodes": self.tracks.values()
                        .filter(|t| t.contributors.iter().all(|c| is_peer(&c.source_id)))
                        .count(),
                }),
            )
            .await?;
        self.sync_flush().await
    }

    /// Send what the batch queued.
    pub(super) async fn sync_flush(&mut self) -> anyhow::Result<()> {
        let out = std::mem::take(&mut self.sync_out);
        if out.reports.is_empty() && out.attrs.is_empty() && out.release.is_empty() {
            return Ok(());
        }
        let (site, hlc) = (
            self.common.site,
            ot_sync::Hlc::new(Utc::now().timestamp_millis() as u64, 0),
        );
        let (reports, attrs) = self.within_budget(out.reports, out.attrs);
        let mut msgs = ot_sync::wire::encode_reports(site, hlc, &reports);
        // As many UIDs as fit in a message once signed.
        let most = (ot_sync::wire::MAX_UNSIGNED - ot_sync::wire::HEADER - 2) / 7;
        for chunk in out.release.chunks(most) {
            msgs.push(
                ot_sync::wire::Message::new(
                    site,
                    hlc,
                    ot_sync::wire::Body::Release(chunk.to_vec()),
                )
                .encode(),
            );
        }
        msgs.extend(ot_sync::wire::encode_attrs(site, hlc, &attrs));
        self.redis.push_sync_out(&msgs).await?;
        Ok(())
    }

    /// What of the queue the sending budget allows, most urgent first: new
    /// tracks, state changes and retirements, then the largest drifts, then
    /// heartbeats, then attributes. What does not fit waits for the next
    /// flush (it is due again). No budget: everything.
    fn within_budget(
        &mut self,
        mut reports: Vec<Queued>,
        attrs: Vec<(Uid, Value)>,
    ) -> (Vec<ot_sync::wire::Report>, Vec<(Uid, Value)>) {
        if self.sync_budget_bps <= 0.0 {
            return (reports.into_iter().map(|q| q.report).collect(), attrs);
        }
        let now = std::time::Instant::now();
        let rate = self.sync_budget_bps;
        self.sync_allowance = (self.sync_allowance
            + now.duration_since(self.sync_topped).as_secs_f64() * rate)
            .min(2.0 * rate);
        self.sync_topped = now;
        reports.sort_by(|a, b| b.urgency.total_cmp(&a.urgency));
        let (mut sent, mut deferred) = (Vec::new(), 0usize);
        for q in reports {
            // A report and its share of a message's envelope and signature
            // (83 bytes over the ~28 reports a full message holds).
            let size = ot_sync::wire::REPORT_BASE as f64
                + 8.0
                + q.report.identifiers.as_ref().map_or(0.0, |ids| {
                    ids.iter()
                        .map(|(a, b)| 2 + a.len() + b.len())
                        .sum::<usize>() as f64
                        + 1.0
                })
                + 3.0;
            if q.urgency.is_infinite() || self.sync_allowance >= size {
                self.sync_allowance -= size;
                sent.push(q.report);
            } else {
                deferred += 1;
                if let Some(s) = self.shared.get_mut(&q.report.uid) {
                    s.sent = q.prev_sent;
                    s.ids = q.prev_ids;
                }
            }
        }
        let mut kept = Vec::new();
        for (uid, v) in attrs {
            let size = 9.0 + v.to_string().len() as f64;
            if self.sync_allowance >= size {
                self.sync_allowance -= size;
                kept.push((uid, v));
            } else if let Some(s) = self.shared.get_mut(&uid) {
                s.attrs = None;
            }
        }
        if deferred > 0 {
            tracing::debug!(deferred, "sync budget: reports wait");
        }
        (sent, kept)
    }

    /// The link heard a node that asked for everything this node reports.
    fn sync_snapshot(&mut self) {
        for s in self.shared.values_mut() {
            if s.resp.reporting {
                s.sent = None;
                s.ids = None;
                s.attrs = None;
            }
        }
    }

    /// Commands from the link (not logged, not replicated).
    pub(super) async fn link_command(&mut self, cmd: &Value) -> anyhow::Result<Option<Value>> {
        match cmd["op"].as_str().unwrap_or_default() {
            "sync_snapshot" => {
                self.sync_snapshot();
                self.sync_tick().await?;
                Ok(Some(json!({})))
            }
            "sync_release" => {
                let site: ot_core::SiteCode = cmd["site"].as_str().unwrap_or_default().parse()?;
                let now = self.sync_now_ms();
                for u in uid_strings(&cmd["uids"]) {
                    let key = format!("{PEER}{site}/{u}");
                    if let Some(uid) = self.reports.get(&key).copied()
                        && let Some(s) = self.shared.get_mut(&uid)
                    {
                        s.resp.released(site, now);
                    }
                }
                Ok(Some(json!({})))
            }
            _ => Ok(None),
        }
    }

    /// Stopping: give up every track this node reports.
    pub(super) async fn sync_leave(&mut self) -> anyhow::Result<()> {
        if !self.sync_on {
            return Ok(());
        }
        self.sync_out.release = self
            .shared
            .iter()
            .filter(|(_, s)| s.resp.reporting)
            .map(|(u, _)| *u)
            .collect();
        self.sync_flush().await?;
        // Gone: nothing more is shared until the node starts again.
        self.sync_on = false;
        self.shared.clear();
        Ok(())
    }
}
