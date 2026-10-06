//! The Settings tab over the API: instance settings (site name,
//! classification banner), track export, and purging the tracks. The
//! configuration export is `config_backup`'s.

use std::time::Duration;

use axum::Extension;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::api::{actor, sees_secrets};
use crate::auth::AuthUser;
use crate::control::{ApiError, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/settings", get(get_settings).put(put_settings))
        .route("/public/banner", get(banner))
        .route("/public/warning-banner", get(warning_banner))
        .route("/export/tracks", get(export_tracks))
        .route("/admin/purge", post(purge))
}

/// Settings an admin changes from the Settings tab.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AppSettings {
    /// Shown in the header and the browser title; the site code in track
    /// UIDs is fixed at deployment (`OT_SITE_CODE`).
    pub site_name: String,
    pub banner: BannerSettings,
    pub warning: WarningSettings,
    /// How long each system track's position history is kept, in hours
    /// (0: none). A track manager can delete a point of it.
    pub history_hours: Option<f64>,
    /// At most one history point per track this often, in seconds (0:
    /// every update). Memory grows with tracks × hours ÷ this.
    pub history_interval_secs: Option<f64>,
    /// Sharing the picture with other OpenTrack nodes.
    pub sync: SyncSettings,
    /// An XYZ raster tile URL template (`{z}`, `{x}`, `{y}`) for the maps'
    /// basemap, fetched through this server (`crate::basemap`); empty: the
    /// built-in country outlines. It may carry a key: only admins read it.
    pub basemap_tiles_url: String,
    /// Cursor-on-Target outputs to TAK (the `cot` role).
    pub tak: crate::cot::settings::TakSettings,
}

/// Sharing the picture with other nodes (see `docs/multi-node.md`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SyncSettings {
    /// Exchange tracks and decisions with the peers below (the `link` role).
    pub enabled: bool,
    /// Site codes of the nodes this one trusts; anything else is dropped.
    pub peers: Vec<String>,
    /// Apply other nodes' track management but accept none here (a drone
    /// with no operator, say).
    pub receive_only: bool,
    /// Share the output schema and correlation settings, so every node
    /// publishes the same attributes and pairs by the same rules.
    pub share_profile: bool,
    /// What this node may send to the others for its tracks, kbit/s (0: no
    /// cap): its share of the link. Past it, the most urgent reports go
    /// first and the rest wait.
    pub budget_kbps: f64,
}

impl Default for SyncSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            peers: Vec::new(),
            receive_only: false,
            share_profile: true,
            budget_kbps: 0.0,
        }
    }
}

/// Log a change to the shared profile for every node, when this node shares
/// it.
pub fn share_profile(
    db: &mut ot_store::Db,
    site: ot_core::SiteCode,
    actor: &str,
    command: &Value,
) -> ot_store::sqlite::Result<()> {
    let sync = sync_settings(&db.app_settings()?);
    if sync.share_profile {
        let now = chrono::Utc::now().timestamp_millis() as u64;
        db.sync_append_own(site, actor, "admin", command, now)?;
    }
    Ok(())
}

/// The sync settings the saved settings ask for.
pub fn sync_settings(saved: &Value) -> SyncSettings {
    serde_json::from_value(saved["sync"].clone()).unwrap_or_default()
}

/// History kept when the settings give no figure.
pub const DEFAULT_HISTORY_HOURS: f64 = 12.0;
pub const DEFAULT_HISTORY_INTERVAL_SECS: f64 = 10.0;

/// The history retention (hours) and point interval (seconds) the saved
/// settings ask for.
pub fn history_retention(saved: &Value) -> (f64, f64) {
    (
        saved["history_hours"]
            .as_f64()
            .unwrap_or(DEFAULT_HISTORY_HOURS),
        saved["history_interval_secs"]
            .as_f64()
            .unwrap_or(DEFAULT_HISTORY_INTERVAL_SECS),
    )
}

/// The warning users must accept after signing in, as OpenStare's warning
/// banner (a notice and consent to monitoring, say): declining signs them
/// out.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WarningSettings {
    pub enabled: bool,
    pub text: String,
}

/// The classification banner, as OpenStare configures its own: on or off,
/// the marking and its colours. Each OpenTrack sets its own, whatever the
/// classification of the OpenStare it feeds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BannerSettings {
    pub enabled: bool,
    pub text: String,
    /// CSS colours, `#rrggbb`.
    pub background: String,
    pub color: String,
    /// Saved by an earlier build (off / manual / follow OpenStare): read
    /// once as enabled or not, never written.
    #[serde(skip_serializing)]
    pub mode: Option<String>,
    #[serde(skip_serializing)]
    pub openstare_url: Option<String>,
}

impl Default for BannerSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            text: "UNCLASSIFIED".into(),
            background: "#006400".into(),
            color: "#ffffff".into(),
            mode: None,
            openstare_url: None,
        }
    }
}

/// The longest basemap tile URL template accepted.
pub const BASEMAP_URL_MAX: usize = 2048;

/// The tile URL for these tile coordinates.
pub fn fill_tile_url(template: &str, z: u32, x: u64, y: u64) -> String {
    template
        .replace("{z}", &z.to_string())
        .replace("{x}", &x.to_string())
        .replace("{y}", &y.to_string())
}

/// Check a basemap tile URL template: empty (off), or an absolute http(s)
/// URL with `{z}`, `{x}` and `{y}`, no other placeholder and no user or
/// password in it.
pub(crate) fn validate_tiles_url(u: &str) -> Result<(), String> {
    if u.is_empty() {
        return Ok(());
    }
    if u.len() > BASEMAP_URL_MAX {
        return Err(format!(
            "basemap tiles URL is at most {BASEMAP_URL_MAX} characters"
        ));
    }
    if u.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("basemap tiles URL: no spaces or control characters".into());
    }
    let mut rest = u;
    while let Some(i) = rest.find(['{', '}']) {
        let tail = &rest[i..];
        let close = tail.strip_prefix('{').and_then(|t| t.find('}'));
        let Some(end) = close else {
            return Err("basemap tiles URL: an unmatched { or }".into());
        };
        let name = &tail[1..=end];
        match name {
            "z" | "x" | "y" => {}
            "s" => {
                return Err(
                    "basemap tiles URL: {s} (a choice of subdomains) is not supported; give one host"
                        .into(),
                );
            }
            other => {
                return Err(format!(
                    "basemap tiles URL: {{{other}}} is not a placeholder OpenTrack fills; use {{z}}, {{x}} and {{y}}"
                ));
            }
        }
        rest = &tail[end + 2..];
    }
    for p in ["{z}", "{x}", "{y}"] {
        if !u.contains(p) {
            return Err(format!("basemap tiles URL: it needs {p}"));
        }
    }
    let url = reqwest::Url::parse(&fill_tile_url(u, 0, 0, 0))
        .map_err(|e| format!("basemap tiles URL: not an absolute URL ({e})"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("basemap tiles URL: http or https only".into());
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err("basemap tiles URL: give the tile server's host".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(
            "basemap tiles URL: no user or password in it (user:pass@); put a key in the query if the server takes one"
                .into(),
        );
    }
    Ok(())
}

fn hex_colour(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

impl AppSettings {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.site_name.chars().count() > 64 {
            return Err("site_name is at most 64 characters".into());
        }
        let b = &self.banner;
        if b.enabled && (b.text.trim().is_empty() || b.text.chars().count() > 128) {
            return Err("banner text is 1 to 128 characters".into());
        }
        if !hex_colour(&b.background) || !hex_colour(&b.color) {
            return Err("banner colours are #rrggbb".into());
        }
        if let Some(h) = self.history_hours
            && !(0.0..=720.0).contains(&h)
        {
            return Err("history_hours: 0 (none) to 720".into());
        }
        if let Some(s) = self.history_interval_secs
            && !(0.0..=3600.0).contains(&s)
        {
            return Err("history_interval_secs: 0 (every update) to 3600".into());
        }
        if !(0.0..=100_000.0).contains(&self.sync.budget_kbps) {
            return Err("sync budget_kbps: 0 (no cap) to 100000".into());
        }
        for p in &self.sync.peers {
            if p.parse::<ot_core::SiteCode>().is_err() {
                return Err(format!(
                    "sync peer {p:?} is not a site code (3 characters A-Z, 0-9)"
                ));
            }
        }
        self.tak.check()?;
        let w = &self.warning;
        if w.enabled && w.text.trim().is_empty() {
            return Err("warning: give the text users must accept".into());
        }
        if w.text.chars().count() > 20_000 {
            return Err("warning text is at most 20,000 characters".into());
        }
        validate_tiles_url(&self.basemap_tiles_url)
    }
}

/// Whether the notice-and-consent warning is on (and has text).
pub(crate) async fn warning_enabled(s: &AppState) -> Result<bool, ApiError> {
    let w = load(s).await?.warning;
    Ok(w.enabled && !w.text.trim().is_empty())
}

async fn load(s: &AppState) -> Result<AppSettings, ApiError> {
    let v = s.with_db(|db| db.app_settings()).await?;
    let mut settings: AppSettings = serde_json::from_value(v)
        .map_err(|e| ApiError::internal(format!("saved settings: {e}")))?;
    let b = &mut settings.banner;
    if let Some(mode) = b.mode.take() {
        b.enabled = mode != "off";
        b.openstare_url = None;
    }
    Ok(settings)
}

/// The settings, with `basemap_tiles` (whether the maps have tiles) for
/// everyone and the tile URL itself (it may carry a key) for admins only.
async fn get_settings(
    State(s): State<AppState>,
    u: Option<Extension<AuthUser>>,
) -> Result<Json<Value>, ApiError> {
    let mut settings = load(&s).await?;
    let tiles = !settings.basemap_tiles_url.is_empty();
    if !sees_secrets(u.as_ref()) {
        settings.basemap_tiles_url.clear();
    }
    Ok(Json(json!({
        "settings": settings,
        "site_code": s.common.site.to_string(),
        "node_id": s.common.node_id(),
        "basemap_tiles": tiles,
    })))
}

async fn put_settings(
    State(s): State<AppState>,
    u: Option<Extension<AuthUser>>,
    headers: HeaderMap,
    Json(mut settings): Json<AppSettings>,
) -> Result<Json<Value>, ApiError> {
    settings.basemap_tiles_url = settings.basemap_tiles_url.trim().to_owned();
    settings.validate().map_err(ApiError::unprocessable)?;
    if settings.sync.peers.contains(&s.common.site.to_string()) {
        return Err(ApiError::unprocessable(format!(
            "sync peers: {} is this node's own site code",
            s.common.site
        )));
    }
    let (v, who) = (
        serde_json::to_value(&settings).unwrap_or_default(),
        actor(&headers),
    );
    s.with_db(move |db| db.put_app_settings(&v, &who)).await?;
    s.auth.consent.setting_changed();
    get_settings(State(s), u).await
}

/// The banner to show, unauthenticated and shaped as OpenStare's
/// `/api/public/banner`: `{enabled, text, background, color}`.
async fn banner(State(s): State<AppState>) -> Result<Json<Value>, ApiError> {
    let b = load(&s).await?.banner;
    Ok(Json(json!({
        "enabled": b.enabled, "text": b.text, "background": b.background, "color": b.color,
    })))
}

/// The warning to accept after signing in, unauthenticated and shaped as
/// OpenStare's `/api/public/warning-banner`: `{enabled, text}`.
async fn warning_banner(State(s): State<AppState>) -> Result<Json<Value>, ApiError> {
    let w = load(&s).await?.warning;
    Ok(Json(json!({ "enabled": w.enabled, "text": w.text })))
}

#[derive(Deserialize)]
struct ExportQuery {
    #[serde(default = "geojson")]
    format: String,
}

fn geojson() -> String {
    "geojson".into()
}

fn download(
    bytes: Vec<u8>,
    content_type: &'static str,
    name: String,
) -> Result<Response, ApiError> {
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{name}\""))
            .map_err(|e| ApiError::internal(e.to_string()))?,
    );
    Ok((StatusCode::OK, headers, bytes).into_response())
}

fn stamp() -> String {
    chrono::Utc::now().format("%Y%m%d-%H%M").to_string()
}

/// Every live system track as published (the GOLD fields and attributes),
/// as GeoJSON or CSV, marked (see `crate::marking`): GeoJSON with a
/// top-level `security.marking` and each labelled feature's `marking`
/// beside its `security` label; CSV with the marking on its first line and
/// each track's in a `classification` column (empty when unlabelled).
async fn export_tracks(
    State(s): State<AppState>,
    Query(q): Query<ExportQuery>,
) -> Result<Response, ApiError> {
    let tracks = s.redis.list_system_tracks().await?;
    let (ctx, now) = (s.common.publish_context(), chrono::Utc::now());
    let messages: Vec<(ot_core::SystemTrack, ot_core::wire::TrackMessage)> = tracks
        .into_iter()
        .map(|t| {
            let m = ot_core::wire::to_message(&t, &ctx, now);
            (t, m)
        })
        .collect();
    // The file's marking: its highest track label, else the banner's.
    let marking = crate::marking::Marker::load(&s)
        .await?
        .file(messages.iter().map(|(t, _)| t.view.security.as_ref()));
    match q.format.as_str() {
        "geojson" => {
            let features: Vec<Value> = messages
                .iter()
                .map(|(t, m)| {
                    let mut props = serde_json::to_value(m).unwrap_or_default();
                    if let Some(p) = props.as_object_mut() {
                        p.remove("lat");
                        p.remove("lon");
                        if let Some(l) = &t.view.security {
                            p.insert("marking".into(), json!(l.marking()));
                        }
                        p.insert("state".into(), json!(t.state));
                        p.insert("confidence".into(), json!(t.confidence()));
                        p.insert(
                            "sources".into(),
                            json!(
                                t.contributors
                                    .iter()
                                    .map(|c| format!("{}/{}", c.source_id, c.source_track_key))
                                    .collect::<Vec<_>>()
                            ),
                        );
                    }
                    json!({"type": "Feature", "id": m.track_id,
                           "geometry": {"type": "Point", "coordinates": [m.lon, m.lat]},
                           "properties": props})
                })
                .collect();
            let body = serde_json::to_vec_pretty(
                &json!({"type": "FeatureCollection", "security": {"marking": marking},
                        "features": features}),
            )
            .map_err(|e| ApiError::internal(e.to_string()))?;
            download(
                body,
                "application/geo+json",
                format!("opentrack-tracks-{}.geojson", stamp()),
            )
        }
        "csv" => {
            let mut w = csv::WriterBuilder::new()
                .flexible(true)
                .from_writer(Vec::new());
            // The file's marking on a line of its own, above the header.
            w.write_record([&marking])
                .map_err(|e| ApiError::internal(e.to_string()))?;
            let header = [
                "classification",
                "track_id",
                "name",
                "class",
                "domain",
                "affiliation",
                "force_code",
                "track_type",
                "sidc",
                "time",
                "lat",
                "lon",
                "state",
                "confidence",
                "published",
                "sources",
                "attributes",
            ];
            w.write_record(header)
                .map_err(|e| ApiError::internal(e.to_string()))?;
            for (t, m) in &messages {
                let text = |v: Value| match v {
                    Value::String(s) => s,
                    other => other.to_string(),
                };
                w.write_record([
                    t.view
                        .security
                        .as_ref()
                        .map(ot_core::SecurityLabel::marking)
                        .unwrap_or_default(),
                    m.track_id.clone(),
                    m.name.clone(),
                    m.class.clone(),
                    m.domain.clone(),
                    m.affiliation.clone(),
                    m.force_code.to_string(),
                    text(json!(m.track_type)),
                    m.sidc.code.clone(),
                    m.time.to_rfc3339(),
                    m.lat.to_string(),
                    m.lon.to_string(),
                    text(json!(t.state)),
                    t.confidence().to_string(),
                    t.is_published().to_string(),
                    t.contributors
                        .iter()
                        .map(|c| format!("{}/{}", c.source_id, c.source_track_key))
                        .collect::<Vec<_>>()
                        .join(" "),
                    Value::Object(m.attributes.clone()).to_string(),
                ])
                .map_err(|e| ApiError::internal(e.to_string()))?;
            }
            let body = w
                .into_inner()
                .map_err(|e| ApiError::internal(e.to_string()))?;
            download(
                body,
                "text/csv; charset=utf-8",
                format!("opentrack-tracks-{}.csv", stamp()),
            )
        }
        other => Err(ApiError::bad_request(format!(
            "format {other:?}: use geojson or csv"
        ))),
    }
}

#[derive(Deserialize)]
struct PurgeBody {
    /// The site code, typed to confirm.
    confirm: String,
    /// Also delete the track graph (how every track was formed and paired).
    #[serde(default)]
    history: bool,
}

/// Retire every live track (published ones are deleted downstream too) and,
/// with `history`, delete the track graph. Configuration, the registry,
/// and the decision log stay.
async fn purge(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<PurgeBody>,
) -> Result<Json<Value>, ApiError> {
    if body.confirm.trim() != s.common.site.to_string() {
        return Err(ApiError::unprocessable(format!(
            "type the site code ({}) to confirm",
            s.common.site
        )));
    }
    crate::correlation_api::command_waiting(
        &s,
        &headers,
        json!({"op": "purge", "history": body.history}),
        Duration::from_secs(300),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basemap_tile_urls_are_checked() {
        for ok in [
            "",
            "https://tiles.example/{z}/{x}/{y}.png",
            "http://10.0.0.5:8080/tiles/{z}/{y}/{x}?key=abc",
            "https://{z}.tiles.example/{x}/{y}",
        ] {
            assert_eq!(validate_tiles_url(ok), Ok(()), "{ok}");
        }
        let long = format!("https://t.example/{{z}}/{{x}}/{{y}}?k={}", "a".repeat(2048));
        for (bad, why) in [
            ("tiles.example/{z}/{x}/{y}.png", "absolute"),
            ("/tiles/{z}/{x}/{y}.png", "absolute"),
            ("ftp://tiles.example/{z}/{x}/{y}", "http or https"),
            ("file:///{z}/{x}/{y}", "http or https"),
            ("https://tiles.example/{z}/{x}.png", "{y}"),
            ("https://tiles.example/{x}/{y}.png", "{z}"),
            ("https://{s}.tiles.example/{z}/{x}/{y}.png", "{s}"),
            ("https://tiles.example/{z}/{x}/{y}{r}.png", "{r}"),
            ("https://tiles.example/{z}/{x}/{y.png", "unmatched"),
            ("https://tiles.example/{z}/{x}/{y}}.png", "unmatched"),
            (
                "https://user:pass@tiles.example/{z}/{x}/{y}",
                "user or password",
            ),
            ("https://key@tiles.example/{z}/{x}/{y}", "user or password"),
            (" https://tiles.example/{z}/{x}/{y}", "spaces"),
            (long.as_str(), "2048"),
        ] {
            let e = validate_tiles_url(bad).expect_err(bad);
            assert!(e.contains(why), "{bad}: {e}");
        }
        // Checked as part of the settings, and an old document loads.
        let old: AppSettings = serde_json::from_value(json!({"site_name": "x"})).unwrap();
        assert_eq!(old.basemap_tiles_url, "");
        let bad = AppSettings {
            basemap_tiles_url: "https://t.example/{z}".into(),
            ..AppSettings::default()
        };
        assert!(bad.validate().is_err());
    }

    #[test]
    fn tile_coordinates_fill_the_template() {
        assert_eq!(
            fill_tile_url("https://t.example/{z}/{x}/{y}.png?z=1", 3, 5, 7),
            "https://t.example/3/5/7.png?z=1"
        );
    }
}
