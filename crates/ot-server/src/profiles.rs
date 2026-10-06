//! Tracker profiles: a sensor's tracker settings as a JSON file, so a
//! source's tracker stage starts from settings made for its sensor (a
//! GMTI radar, a marine radar, an air surveillance radar, a lidar) instead
//! of from scratch, and a tuned tracker can be saved and shared.
//!
//! Profiles come from two directories: those that ship with OpenTrack
//! (`OT_PROFILES_DIR`, read only) and those imported here
//! (`profiles/trackers` beside the database). A file is one profile:
//!
//! ```json
//! {"name": "marine-radar-x", "label": "…", "description": "…",
//!  "sensor": {"kind": "surface_radar", "band": "X", ...},
//!  "basis": "where the numbers come from", "tracker": {<a tracker stage>}}
//! ```

use std::path::{Path, PathBuf};

use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::api::actor;
use crate::config::Common;
use crate::control::{ApiError, AppState};

/// A tracker profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    /// Its file name: lowercase letters, digits, - and _.
    pub name: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
    /// What kind of sensor it is for: `kind` (gmti, maritime_mti,
    /// surface_radar, air_radar, lidar...) and anything else that helps
    /// pick it (platform, band, revisit).
    #[serde(default)]
    pub sensor: Value,
    /// Where the numbers come from (public figures, a benchmark run).
    #[serde(default)]
    pub basis: String,
    /// The tracker stage it sets, as a source's `pipeline.tracker`.
    pub tracker: Value,
}

impl Profile {
    pub fn validate(&self) -> Result<(), String> {
        let ok = !self.name.is_empty()
            && self.name.len() <= 64
            && self
                .name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
        if !ok {
            return Err(format!(
                "profile name {:?}: lowercase letters, digits, - and _ (at most 64)",
                self.name
            ));
        }
        if self.label.trim().is_empty() {
            return Err(format!("profile {}: give it a label", self.name));
        }
        let spec: ot_source::tracker::TrackerSpec = serde_json::from_value(self.tracker.clone())
            .map_err(|e| format!("profile {}: tracker: {e}", self.name))?;
        spec.validate()
            .map_err(|e| format!("profile {}: tracker: {e}", self.name))
    }
}

/// Where imported profiles go.
pub fn user_dir(common: &Common) -> PathBuf {
    common
        .sqlite
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .join("profiles/trackers")
}

/// A profile as listed: the profile, and whether it shipped with OpenTrack.
#[derive(Debug, Clone, Serialize)]
pub struct Listed {
    #[serde(flatten)]
    pub profile: Profile,
    pub builtin: bool,
}

fn read_dir(dir: &Path, builtin: bool, out: &mut Vec<Listed>, problems: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    paths.sort();
    for path in paths {
        let parsed = std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|s| serde_json::from_str::<Profile>(&s).map_err(|e| e.to_string()))
            .and_then(|p| {
                p.validate()?;
                if path.file_stem().and_then(|s| s.to_str()) != Some(p.name.as_str()) {
                    return Err(format!(
                        "its name is {:?}: the file must be {}.json",
                        p.name, p.name
                    ));
                }
                Ok(p)
            });
        match parsed {
            Ok(profile) if out.iter().any(|l| l.profile.name == profile.name) => {
                problems.push(format!(
                    "{}: a profile named {} exists already",
                    path.display(),
                    profile.name
                ));
            }
            Ok(profile) => out.push(Listed { profile, builtin }),
            Err(e) => problems.push(format!("{}: {e}", path.display())),
        }
    }
}

/// Every profile (shipped first), and the files that are not one.
pub fn list(common: &Common) -> (Vec<Listed>, Vec<String>) {
    let (mut out, mut problems) = (Vec::new(), Vec::new());
    read_dir(&common.profiles_dir, true, &mut out, &mut problems);
    read_dir(&user_dir(common), false, &mut out, &mut problems);
    for p in &problems {
        tracing::warn!(problem = %p, "tracker profile skipped");
    }
    (out, problems)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shipped_profile_is_valid() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../profiles/trackers");
        let (mut out, mut problems) = (Vec::new(), Vec::new());
        read_dir(&dir, true, &mut out, &mut problems);
        assert!(problems.is_empty(), "{problems:#?}");
        assert!(out.len() >= 9, "{}", out.len());
        let kinds: std::collections::BTreeSet<&str> = out
            .iter()
            .filter_map(|l| l.profile.sensor["kind"].as_str())
            .collect();
        // Not only MTI: radars and lidar too.
        for k in [
            "gmti",
            "maritime_mti",
            "surface_radar",
            "air_radar",
            "lidar",
        ] {
            assert!(kinds.contains(k), "no {k} profile");
        }
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/tracker-profiles", get(list_profiles).post(import))
        .route("/tracker-profiles/{name}", get(view).delete(delete))
}

async fn list_profiles(State(s): State<AppState>) -> Json<Value> {
    let (profiles, problems) = list(&s.common);
    Json(json!({ "profiles": profiles, "problems": problems }))
}

async fn view(
    State(s): State<AppState>,
    UrlPath(name): UrlPath<String>,
) -> Result<Json<Value>, ApiError> {
    list(&s.common)
        .0
        .into_iter()
        .find(|l| l.profile.name == name)
        .map(|l| Json(json!(l.profile)))
        .ok_or_else(|| ApiError::not_found(format!("tracker profile {name}")))
}

#[derive(Deserialize)]
struct ImportQuery {
    /// Replace an imported profile of the same name.
    #[serde(default)]
    replace: bool,
}

/// Import a profile (a file someone shared, or a tuned tracker saved from a
/// source) into the user directory.
async fn import(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<ImportQuery>,
    Json(profile): Json<Profile>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    profile.validate().map_err(ApiError::unprocessable)?;
    let existing = list(&s.common)
        .0
        .into_iter()
        .find(|l| l.profile.name == profile.name);
    match existing {
        Some(l) if l.builtin => {
            return Err(ApiError::conflict(format!(
                "{} ships with OpenTrack: save yours under another name",
                profile.name
            )));
        }
        Some(_) if !q.replace => {
            return Err(ApiError::conflict(format!(
                "tracker profile {} exists: replace it, or save under another name",
                profile.name
            )));
        }
        _ => {}
    }
    let dir = user_dir(&s.common);
    let path = dir.join(format!("{}.json", profile.name));
    std::fs::create_dir_all(&dir)
        .and_then(|_| {
            std::fs::write(
                &path,
                serde_json::to_string_pretty(&profile).unwrap_or_default() + "\n",
            )
        })
        .map_err(|e| ApiError::internal(format!("{}: {e}", path.display())))?;
    let actor = actor(&headers);
    let before = existing.map(|l| json!(l.profile));
    let after = json!(profile);
    let name = profile.name.clone();
    s.with_db(move |db| {
        db.record(&ot_store::Decision {
            before,
            after: Some(after),
            evidence: json!({ "tracker_profile": name }),
            ..ot_store::Decision::new(&actor, "import_tracker_profile")
        })
    })
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(json!(Listed {
            profile,
            builtin: false
        })),
    ))
}

async fn delete(
    State(s): State<AppState>,
    UrlPath(name): UrlPath<String>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let found = list(&s.common)
        .0
        .into_iter()
        .find(|l| l.profile.name == name)
        .ok_or_else(|| ApiError::not_found(format!("tracker profile {name}")))?;
    if found.builtin {
        return Err(ApiError::conflict(format!(
            "{name} ships with OpenTrack and cannot be deleted"
        )));
    }
    let path = user_dir(&s.common).join(format!("{name}.json"));
    std::fs::remove_file(&path)
        .map_err(|e| ApiError::internal(format!("{}: {e}", path.display())))?;
    let actor = actor(&headers);
    let before = json!(found.profile);
    s.with_db(move |db| {
        db.record(&ot_store::Decision {
            before: Some(before),
            evidence: json!({ "tracker_profile": name }),
            ..ot_store::Decision::new(&actor, "delete_tracker_profile")
        })
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
