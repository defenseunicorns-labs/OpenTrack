//! Track management a track manager does by hand (OTH-GOLD's track
//! management sets, beyond correlation's merge and split): pair tracks
//! (PAIR: the same object, kept as separate tracks), delete tracks (DEL),
//! and form groups — a battle group, a flight of bombers, a convoy — that
//! are published as tracks of their own at the centre of their live members.
//! A track manager's merge (MRG) is correlation's merge with `hold`.

use std::collections::HashSet;

use chrono::Utc;
use ot_core::{Observation, SystemTrack, TrackKind, TrackState, Uid};
use ot_store::{Decision, GroupSpec};
use serde_json::{Value, json};

use super::Engine;

/// Commands this module answers.
pub(super) const OPS: &[&str] = &[
    "pair",
    "unpair",
    "delete",
    "group_create",
    "group_update",
    "group_members",
    "group_dissolve",
];

/// A live group: what it is, and its published track.
pub(super) struct GroupState {
    pub spec: GroupSpec,
    pub track: SystemTrack,
}

/// Great-circle distance in metres.
fn distance_m(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (la1, lo1, la2, lo2) = (
        a.0.to_radians(),
        a.1.to_radians(),
        b.0.to_radians(),
        b.1.to_radians(),
    );
    let h = ((la2 - la1) / 2.0).sin().powi(2)
        + la1.cos() * la2.cos() * ((lo2 - lo1) / 2.0).sin().powi(2);
    2.0 * 6_371_008.8 * h.sqrt().asin()
}

/// The group's view from its live members: the centre of their positions
/// (on the sphere, so a group across the antimeridian stays together), their
/// mean velocity, and an uncertainty that covers every member. Without live
/// members the last view stays and the group is `lost`.
fn group_view(
    spec: &GroupSpec,
    members: &[&SystemTrack],
    previous: Option<&SystemTrack>,
) -> (Observation, TrackState) {
    let now = Utc::now();
    let (lat, lon, observed_at) = if members.is_empty() {
        let p = previous.map(|t| {
            (
                t.view.position.latitude,
                t.view.position.longitude,
                t.view.observed_at,
            )
        });
        p.unwrap_or((0.0, 0.0, now))
    } else {
        let (mut x, mut y, mut z) = (0.0, 0.0, 0.0);
        for t in members {
            let (la, lo) = (
                t.view.position.latitude.to_radians(),
                t.view.position.longitude.to_radians(),
            );
            x += la.cos() * lo.cos();
            y += la.cos() * lo.sin();
            z += la.sin();
        }
        let lat = z.atan2((x * x + y * y).sqrt()).to_degrees();
        let lon = y.atan2(x).to_degrees();
        let at = members
            .iter()
            .map(|t| t.view.observed_at)
            .max()
            .unwrap_or(now);
        (lat, lon, at)
    };
    let spread = members
        .iter()
        .map(|t| {
            distance_m(
                (lat, lon),
                (t.view.position.latitude, t.view.position.longitude),
            )
        })
        .fold(0.0_f64, f64::max);
    let moving: Vec<(f64, f64)> = members
        .iter()
        .filter_map(|t| {
            let (c, s) = (t.view.kinematics.course_deg?, t.view.kinematics.speed_mps?);
            let r = c.to_radians();
            Some((s * r.cos(), s * r.sin()))
        })
        .collect();
    let (course, speed) = if moving.is_empty() {
        (None, None)
    } else {
        let n = moving.len() as f64;
        let (vn, ve) = moving
            .iter()
            .fold((0.0, 0.0), |(a, b), (n2, e2)| (a + n2, b + e2));
        let (vn, ve) = (vn / n, ve / n);
        (
            Some(ve.atan2(vn).to_degrees().rem_euclid(360.0)),
            Some((vn * vn + ve * ve).sqrt()),
        )
    };
    let domain = spec.domain.or_else(|| {
        // The members' most common domain.
        let mut counts: Vec<(ot_core::Domain, usize)> = Vec::new();
        for t in members {
            if let Some(d) = t.view.classification.effective_domain() {
                match counts.iter_mut().find(|(x, _)| *x == d) {
                    Some((_, n)) => *n += 1,
                    None => counts.push((d, 1)),
                }
            }
        }
        counts.into_iter().max_by_key(|(_, n)| *n).map(|(d, _)| d)
    });
    let class = spec
        .extra
        .get("class")
        .and_then(Value::as_str)
        .unwrap_or("GROUP");
    let obs: Observation = serde_json::from_value(json!({
        "schema_version": 1,
        "source_id": "opentrack",
        "source_track_key": "group",
        "name": spec.name,
        "observed_at": observed_at,
        "received_at": now,
        "position": { "latitude": lat, "longitude": lon },
        "uncertainty": { "circular_error_m": spread.max(50.0) },
        "kinematics": { "course_deg": course, "speed_mps": speed },
        "classification": { "sidc": spec.sidc, "domain": domain, "affiliation": spec.affiliation },
        "platform": { "class": class },
        "track_type": "tactical",
    }))
    .expect("a group view is a valid observation");
    let state = if members.is_empty() {
        TrackState::Lost
    } else {
        TrackState::Confirmed
    };
    (obs, state)
}

fn uids(v: &Value, key: &str) -> anyhow::Result<Vec<Uid>> {
    v[key]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("{key} is required (a list of track ids)"))?
        .iter()
        .map(|x| {
            let s = x
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("{key}: track ids are text"))?;
            s.trim_start_matches("tms-")
                .parse::<Uid>()
                .map_err(|e| anyhow::anyhow!("{key}: {s}: {e}"))
        })
        .collect()
}

fn uid(v: &Value, key: &str) -> anyhow::Result<Uid> {
    let s = v[key]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("{key} is required"))?;
    s.trim_start_matches("tms-")
        .parse()
        .map_err(|e| anyhow::anyhow!("{key}: {e}"))
}

impl Engine {
    /// Load the live groups (SQLite holds them; Redis their last published
    /// track), tell their members, and retire any stored group since dissolved.
    pub(super) async fn load_groups(&mut self, stored: Vec<SystemTrack>) -> anyhow::Result<()> {
        let c = self.common.clone();
        let groups = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
            Ok(c.open_db()?.groups()?)
        })
        .await??;
        let live: HashSet<Uid> = groups.iter().map(|g| g.uid).collect();
        for t in stored.iter().filter(|t| !live.contains(&t.uid)) {
            self.retire_in_redis(t.uid, t.is_published(), "group dissolved")
                .await?;
        }
        let mut previous: std::collections::HashMap<Uid, SystemTrack> =
            stored.into_iter().map(|t| (t.uid, t)).collect();
        for g in groups {
            let track = previous
                .remove(&g.uid)
                .unwrap_or_else(|| self.new_group_track(g.uid, &g.spec));
            let mut state = GroupState {
                spec: g.spec,
                track,
            };
            state.track.members = g.members;
            self.groups.insert(g.uid, state);
            self.dirty.insert(g.uid);
        }
        self.sync_memberships();
        tracing::info!(groups = self.groups.len(), "groups loaded");
        Ok(())
    }

    fn new_group_track(&self, uid: Uid, spec: &GroupSpec) -> SystemTrack {
        let (view, _) = group_view(spec, &[], None);
        let mut t = SystemTrack::from_first_observation(uid, view);
        t.kind = TrackKind::Group;
        t.contributors.clear();
        t.observation_count = 0;
        t.published = Some(true);
        t
    }

    /// Make every track's `groups` match the groups' members.
    fn sync_memberships(&mut self) {
        let mut of: std::collections::HashMap<Uid, Vec<String>> = std::collections::HashMap::new();
        for (g, s) in &self.groups {
            for m in &s.track.members {
                of.entry(*m).or_default().push(g.doc_id());
            }
        }
        for t in self.tracks.values_mut() {
            let mut want = of.remove(&t.uid).unwrap_or_default();
            want.sort();
            let mut have = t.groups.clone();
            have.sort();
            if want != have {
                t.groups = want;
                self.dirty.insert(t.uid);
            }
        }
    }

    /// A track leaves (retired, or merged into `into`): take it out of every
    /// pairing and group, or hand its place to `into`.
    pub(super) fn detach(&mut self, uid: Uid, into: Option<Uid>) {
        for t in self.tracks.values_mut() {
            if t.paired_with.contains(&uid) {
                t.paired_with.retain(|p| *p != uid);
                if let Some(i) = into
                    && i != t.uid
                    && !t.paired_with.contains(&i)
                {
                    t.paired_with.push(i);
                }
                self.dirty.insert(t.uid);
            }
        }
        for (g, s) in self.groups.iter_mut() {
            if s.track.members.contains(&uid) {
                s.track.members.retain(|m| *m != uid);
                if let Some(i) = into
                    && !s.track.members.contains(&i)
                {
                    s.track.members.push(i);
                }
                self.dirty.insert(*g);
            }
        }
    }

    /// Recompute every group from its members, and save what track
    /// management changed.
    pub(super) async fn refresh_groups(&mut self) -> anyhow::Result<()> {
        let uids: Vec<Uid> = self.groups.keys().copied().collect();
        for g in uids {
            let s = &self.groups[&g];
            let members: Vec<&SystemTrack> = s
                .track
                .members
                .iter()
                .filter_map(|m| self.tracks.get(m))
                .collect();
            let (mut view, state) = group_view(&s.spec, &members, Some(&s.track));
            // A group is marked with the highest classification of its
            // members (without live members, as it was).
            view.security = if members.is_empty() {
                s.track.view.security.clone()
            } else {
                self.settings
                    .correlation
                    .labels
                    .combine(members.iter().filter_map(|t| t.view.security.as_ref()))
            };
            let moved = view.position != s.track.view.position
                || view.kinematics != s.track.view.kinematics
                || state != s.track.state;
            let s = self.groups.get_mut(&g).expect("listed above");
            let changed = moved
                || view.name != s.track.view.name
                || view.classification != s.track.view.classification
                || view.security != s.track.view.security;
            s.track.view = view;
            s.track.state = state;
            s.track.last_seen = s.track.view.observed_at;
            if changed || self.dirty.contains(&g) {
                self.dirty.insert(g);
            }
        }
        let dirty: Vec<Uid> = self.dirty.drain().collect();
        for uid in dirty {
            if let Some(s) = self.groups.get_mut(&uid) {
                s.track.published = Some(true);
                self.redis.put_system_track(&s.track, false).await?;
            } else if self.tracks.contains_key(&uid) {
                self.save(uid, false).await?;
            }
        }
        Ok(())
    }

    fn live(&self, uids: &[Uid]) -> anyhow::Result<()> {
        for u in uids {
            if !self.tracks.contains_key(u) {
                anyhow::bail!("{} is not a live track", u.doc_id());
            }
        }
        Ok(())
    }

    pub(super) async fn db<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut ot_store::Db) -> ot_store::sqlite::Result<T> + Send + 'static,
    ) -> anyhow::Result<T> {
        let c = self.common.clone();
        let ip = crate::auth::access::current_ip().filter(|ip| !ip.is_empty());
        tokio::task::spawn_blocking(move || -> anyhow::Result<T> {
            let mut db = c.open_db()?;
            Ok(ot_store::audit::with_client(ip, || f(&mut db))?)
        })
        .await?
    }

    fn group_spec(cmd: &Value) -> anyhow::Result<GroupSpec> {
        serde_json::from_value(cmd["spec"].clone()).map_err(|e| anyhow::anyhow!("spec: {e}"))
    }

    pub(super) async fn manage(
        &mut self,
        op: &str,
        cmd: &Value,
        actor: &str,
    ) -> anyhow::Result<Value> {
        let reason = cmd["reason"].as_str().map(str::to_owned);
        let decision = |op: &str, default: &str| {
            Decision::new(actor, op).reason(reason.clone().unwrap_or_else(|| default.to_owned()))
        };
        match op {
            "pair" => {
                let tracks = uids(cmd, "tracks")?;
                self.live(&tracks)?;
                let d = decision("pair_tracks", "the same object, paired by a track manager");
                let t2 = tracks.clone();
                self.db(move |db| db.pair_system_tracks(&t2, d)).await?;
                for u in &tracks {
                    let t = self.tracks.get_mut(u).expect("live");
                    for o in &tracks {
                        if o != u && !t.paired_with.contains(o) {
                            t.paired_with.push(*o);
                        }
                    }
                    self.dirty.insert(*u);
                }
                self.refresh_groups().await?;
                Ok(json!({ "paired": tracks.iter().map(|u| u.doc_id()).collect::<Vec<_>>() }))
            }
            "unpair" => {
                let (a, b) = (uid(cmd, "a")?, uid(cmd, "b")?);
                let d = decision("unpair_tracks", "unpaired by a track manager");
                self.db(move |db| db.unpair_system_tracks(a, b, d)).await?;
                for (x, y) in [(a, b), (b, a)] {
                    if let Some(t) = self.tracks.get_mut(&x) {
                        t.paired_with.retain(|p| *p != y);
                        self.dirty.insert(x);
                    }
                }
                self.refresh_groups().await?;
                Ok(json!({}))
            }
            "delete" => {
                let tracks = uids(cmd, "tracks")?;
                let mut deleted = Vec::new();
                for u in tracks {
                    if self.groups.contains_key(&u) {
                        self.dissolve(u, decision("dissolve_group", "deleted by a track manager"))
                            .await?;
                    } else {
                        self.live(&[u])?;
                        let d = decision("delete_track", "deleted by a track manager");
                        self.db(move |db| db.retire_system_track(u, d)).await?;
                        let published = self.forget(u);
                        self.retire_in_redis(u, published, "deleted by a track manager")
                            .await?;
                    }
                    deleted.push(u.doc_id());
                }
                self.refresh_groups().await?;
                Ok(json!({ "deleted": deleted }))
            }
            "group_create" => {
                let spec = Self::group_spec(cmd)?;
                let members = uids(cmd, "members")?;
                self.live(&members)?;
                let (site, s2, m2) = (self.common.site, spec.clone(), members.clone());
                let d = decision("create_group", "formed by a track manager");
                // Another node's group keeps the UID that node gave it.
                let given = if self.remote {
                    Some(uid(cmd, "group")?)
                } else {
                    None
                };
                let (g, _) = self
                    .db(move |db| match given {
                        Some(g) => db.create_group_as(g, &s2, &m2, d),
                        None => db.create_group(site, &s2, &m2, d),
                    })
                    .await?;
                let mut track = self.new_group_track(g, &spec);
                track.members = members;
                self.groups.insert(g, GroupState { spec, track });
                self.sync_memberships();
                self.dirty.insert(g);
                self.refresh_groups().await?;
                Ok(json!({ "group": g.doc_id() }))
            }
            "group_update" => {
                let g = uid(cmd, "group")?;
                if !self.groups.contains_key(&g) {
                    anyhow::bail!("{} is not a group", g.doc_id());
                }
                let spec = Self::group_spec(cmd)?;
                let s2 = spec.clone();
                let d = decision("update_group", "changed by a track manager");
                self.db(move |db| db.update_group(g, &s2, d)).await?;
                self.groups.get_mut(&g).expect("checked").spec = spec;
                self.dirty.insert(g);
                self.refresh_groups().await?;
                Ok(json!({ "group": g.doc_id() }))
            }
            "group_members" => {
                let g = uid(cmd, "group")?;
                if !self.groups.contains_key(&g) {
                    anyhow::bail!("{} is not a group", g.doc_id());
                }
                let add = if cmd["add"].is_null() {
                    Vec::new()
                } else {
                    uids(cmd, "add")?
                };
                let remove = if cmd["remove"].is_null() {
                    Vec::new()
                } else {
                    uids(cmd, "remove")?
                };
                self.live(&add)?;
                let (a2, r2) = (add.clone(), remove.clone());
                let d = decision("group_members", "changed by a track manager");
                self.db(move |db| db.change_group_members(g, &a2, &r2, d))
                    .await?;
                let s = self.groups.get_mut(&g).expect("checked");
                s.track.members.retain(|m| !remove.contains(m));
                for a in add {
                    if !s.track.members.contains(&a) {
                        s.track.members.push(a);
                    }
                }
                self.sync_memberships();
                self.dirty.insert(g);
                self.refresh_groups().await?;
                Ok(json!({ "group": g.doc_id() }))
            }
            "group_dissolve" => {
                let g = uid(cmd, "group")?;
                self.dissolve(
                    g,
                    decision("dissolve_group", "dissolved by a track manager"),
                )
                .await?;
                self.refresh_groups().await?;
                Ok(json!({}))
            }
            other => anyhow::bail!("unknown command {other:?}"),
        }
    }

    async fn dissolve(&mut self, g: Uid, d: Decision) -> anyhow::Result<()> {
        if !self.groups.contains_key(&g) {
            anyhow::bail!("{} is not a group", g.doc_id());
        }
        self.db(move |db| db.dissolve_group(g, d)).await?;
        let s = self.groups.remove(&g).expect("checked");
        self.sync_memberships();
        self.retire_in_redis(g, s.track.is_published(), "group dissolved")
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(uid: &str, lat: f64, lon: f64, course: f64, speed: f64) -> SystemTrack {
        let obs: Observation = serde_json::from_value(json!({
            "schema_version": 1, "source_id": "ais", "source_track_key": uid,
            "observed_at": "2026-09-26T00:00:00Z", "received_at": "2026-09-26T00:00:00Z",
            "position": {"latitude": lat, "longitude": lon},
            "kinematics": {"course_deg": course, "speed_mps": speed},
            "classification": {"domain": "surface"}
        }))
        .unwrap();
        SystemTrack::from_first_observation(uid.parse().unwrap(), obs)
    }

    #[test]
    fn a_group_sits_at_its_members_centre_even_across_the_antimeridian() {
        let spec: GroupSpec = serde_json::from_value(json!({
            "name": "CSG 12", "sidc": "SHSPGG----", "affiliation": "hostile"
        }))
        .unwrap();
        let a = member("TST000000001", 10.0, 179.9, 90.0, 10.0);
        let b = member("TST000000002", 10.0, -179.9, 90.0, 10.0);
        let (v, state) = group_view(&spec, &[&a, &b], None);
        assert_eq!(state, TrackState::Confirmed);
        assert!(
            v.position.longitude.abs() > 179.9,
            "{}",
            v.position.longitude
        );
        assert!((v.kinematics.course_deg.unwrap() - 90.0).abs() < 1e-6);
        assert!((v.kinematics.speed_mps.unwrap() - 10.0).abs() < 1e-6);
        let cep = v.uncertainty.unwrap().circular_error_m.unwrap();
        assert!(cep > 10_000.0 && cep < 12_000.0, "{cep}");
        assert_eq!(v.classification.domain, Some(ot_core::Domain::Surface));
        assert_eq!(v.classification.sidc.as_deref(), Some("SHSPGG----"));
        assert_eq!(v.name.as_deref(), Some("CSG 12"));

        // No live members: the last view stays, lost.
        let prev = SystemTrack::from_first_observation("TST000000009".parse().unwrap(), v.clone());
        let (still, state) = group_view(&spec, &[], Some(&prev));
        assert_eq!(state, TrackState::Lost);
        assert_eq!(still.position, v.position);
    }
}
