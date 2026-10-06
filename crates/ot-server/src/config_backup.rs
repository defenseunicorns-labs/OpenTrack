//! The node's whole configuration as one file (`opentrack-config`, version
//! 2; see `ot_store::backup` and docs/guides/admin.md), and restoring it
//! into an empty node: `GET /api/v1/export/config`, `GET|POST
//! /api/v1/import/config` (admins), and `opentrack config export|import`.
//!
//! An import only rebuilds a node that has no configuration yet (see
//! [`present`]); it checks every section first, as the API checks each when
//! it is saved, and writes all of it in one transaction or none of it.

use std::path::{Path, PathBuf};

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use ot_source::schema::{ExtensionField, ExtensionSchema};
use ot_source::source::SourceSpec;
use ot_store::backup::{self, ConfigFile};
use serde_json::{Value, json};

use crate::api::actor;
use crate::config::Common;
use crate::control::{ApiError, AppState};

/// What the file says about itself.
pub const NOTICE: &str = "This file holds this node's secrets: source credentials, password hashes, \
    SAML and OpenStare settings and plugin components. Keep it as you would the node's database, \
    and delete copies you no longer need. API tokens listed in it work again only on a node with \
    the same session signing key, which is never exported.";

/// Largest file an import accepts (plugin components are in it).
const MAX_IMPORT_BYTES: usize = 512 * 1024 * 1024;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/export/config", get(export_route))
        .route(
            "/import/config",
            get(status_route)
                .post(import_route)
                .layer(DefaultBodyLimit::max(MAX_IMPORT_BYTES)),
        )
}

/// The directory beside the database (the session key, the first admin's
/// password file, imported tracker profiles).
fn data_dir(common: &Common) -> PathBuf {
    common
        .sqlite
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .to_path_buf()
}

/// The tracker profile files imported on this node.
fn profile_files(common: &Common) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(crate::profiles::user_dir(common)) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    out.sort();
    out
}

/// The whole configuration, recorded as an `export_config` decision (in
/// the audit record) by `actor`.
pub fn export(
    db: &mut ot_store::Db,
    common: &Common,
    actor: &str,
) -> ot_store::sqlite::Result<ConfigFile> {
    let mut f = db.config_snapshot()?;
    // Sign-in settings as they are read, without any saved before the
    // account policy was fixed.
    if !is_empty(&f.auth_settings)
        && let Ok(a) = serde_json::from_value::<crate::auth::AuthSettings>(f.auth_settings.clone())
    {
        f.auth_settings = json!(a);
    }
    f.opentrack = env!("CARGO_PKG_VERSION").into();
    f.site_code = common.site.to_string();
    f.exported_by = actor.into();
    f.notice = NOTICE.into();
    f.marking = marking(&f);
    f.tracker_profiles = crate::profiles::list(common)
        .0
        .into_iter()
        .filter(|l| !l.builtin)
        .map(|l| json!(l.profile))
        .collect();
    db.record(
        &ot_store::Decision::new(actor, "export_config")
            .reason("full configuration exported (secrets and password hashes included)")
            .evidence(json!({ "counts": f.counts() })),
    )?;
    Ok(f)
}

/// The file's marking (see `crate::marking`): its sources' labels, the
/// highest of them, with the banner's text standing for an unlabelled
/// source; the banner's text alone when no source has a label.
fn marking(f: &ConfigFile) -> String {
    let labels: Vec<Option<ot_core::SecurityLabel>> = f
        .sources
        .iter()
        .map(|s| {
            s.spec
                .get("security")
                .cloned()
                .and_then(|l| serde_json::from_value(l).ok())
        })
        .collect();
    crate::marking::Marker::from_saved(&f.app_settings, f.correlation_settings.as_ref())
        .file(labels.iter().map(Option::as_ref))
}

/// What configuration this node holds, one line each; empty when it may
/// import one. See `ot_store::backup` for the database's part; imported
/// tracker profiles count too.
pub fn present(db: &ot_store::Db, common: &Common) -> ot_store::sqlite::Result<Vec<String>> {
    let mut out = db.config_present()?;
    let n = profile_files(common).len();
    if n > 0 {
        out.push(format!("{n} imported tracker profile(s)"));
    }
    Ok(out)
}

/// Why an import did not happen.
#[derive(Debug)]
pub enum ImportError {
    /// The file is not one this build reads, or a section is invalid:
    /// every problem found.
    Invalid(Vec<String>),
    /// The node has configuration already.
    NotEmpty(Vec<String>),
    Failed(String),
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(p) => write!(f, "the file was not imported: {}", p.join("; ")),
            Self::NotEmpty(p) => write!(
                f,
                "this node is not empty, so nothing was imported (an import only rebuilds a node with no configuration): it has {}",
                p.join(", ")
            ),
            Self::Failed(m) => write!(f, "the import failed and wrote nothing: {m}"),
        }
    }
}

/// What an import did.
#[derive(Debug, Clone)]
pub struct Imported {
    pub decision_id: i64,
    pub counts: Value,
    /// Things the admin should know (another site code, API tokens).
    pub notes: Vec<String>,
    /// The sign-in settings now in force.
    pub auth_settings: crate::auth::AuthSettings,
}

/// Read a file: the right format and version, then every section checked.
pub fn parse(common: &Common, bytes: &[u8]) -> Result<ConfigFile, Vec<String>> {
    let v: Value =
        serde_json::from_slice(bytes).map_err(|e| vec![format!("not a JSON document: {e}")])?;
    match v.get("format").and_then(Value::as_str) {
        Some(backup::FORMAT) => {}
        None if v.get("sources").is_some() && v.get("opentrack").is_some() => {
            return Err(vec![
                "a version 1 export (OpenTrack 0.4.0 and earlier): it has no accounts, registry, \
                 plugins or sign-in settings, so it cannot rebuild a node; export again with this build"
                    .into(),
            ]);
        }
        _ => {
            return Err(vec![format!(
                "not an OpenTrack configuration file (its \"format\" is not {:?})",
                backup::FORMAT
            )]);
        }
    }
    match v.get("version").and_then(Value::as_u64) {
        Some(n) if n == u64::from(backup::VERSION) => {}
        other => {
            return Err(vec![format!(
                "version {}: this build reads version {} of {}",
                other.map_or("(none)".into(), |n| n.to_string()),
                backup::VERSION,
                backup::FORMAT
            )]);
        }
    }
    let mut f: ConfigFile =
        serde_json::from_value(v).map_err(|e| vec![format!("malformed: {e}")])?;
    let problems = check(common, &mut f);
    if problems.is_empty() {
        Ok(f)
    } else {
        Err(problems)
    }
}

/// Check every section as the API checks it when saved, normalising what
/// the API would (a source's spec, the correlation settings). Returns every
/// problem found.
fn check(common: &Common, f: &mut ConfigFile) -> Vec<String> {
    let mut p = Vec::new();

    // Output schema versions, then the published ones sources map to.
    let mut schemas = std::collections::BTreeMap::new();
    let mut seen = Seen::new();
    let newest = f
        .schema_versions
        .iter()
        .map(|v| v.version)
        .max()
        .unwrap_or(0);
    let drafts = f
        .schema_versions
        .iter()
        .filter(|v| v.status == "draft")
        .count();
    if drafts > 1 {
        p.push("schema_versions: at most one draft".into());
    }
    for v in &f.schema_versions {
        let at = format!("schema version {}", v.version);
        p.extend(unique(&mut seen, "schema version", v.version.to_string()));
        if v.version == 0 {
            p.push(format!("{at}: versions start at 1"));
        }
        if v.version == 1 && !v.fields.is_empty() {
            p.push(format!(
                "{at}: version 1 is the built-in core schema and has no fields"
            ));
        }
        match v.status.as_str() {
            "published" => {}
            "draft" if v.version == newest => {}
            "draft" => p.push(format!("{at}: the draft must be the newest version")),
            other => p.push(format!("{at}: status {other:?} (draft or published)")),
        }
        let fields: Result<Vec<ExtensionField>, _> = v
            .fields
            .iter()
            .cloned()
            .map(serde_json::from_value)
            .collect();
        match fields {
            Ok(fields) => {
                let schema = ExtensionSchema {
                    version: v.version,
                    fields,
                };
                if let Err(e) = schema.validate() {
                    p.push(format!("{at}: {e}"));
                } else if v.status == "published" {
                    schemas.insert(v.version, schema);
                }
            }
            Err(e) => p.push(format!("{at}: invalid field definition: {e}")),
        }
    }
    schemas.entry(1).or_insert(ExtensionSchema {
        version: 1,
        fields: Vec::new(),
    });

    // Sources, against those schemas.
    let mut seen = Seen::new();
    for s in &mut f.sources {
        let at = format!("source {}", s.id);
        p.extend(unique(&mut seen, "source", s.id.clone()));
        let spec: SourceSpec = match serde_json::from_value(s.spec.clone()) {
            Ok(spec) => spec,
            Err(e) => {
                p.push(format!("{at}: invalid source spec: {e}"));
                continue;
            }
        };
        if spec.id != s.id {
            p.push(format!("{at}: its spec's id is {:?}", spec.id));
            continue;
        }
        if let Err(e) = crate::probe::file_allowed(common, &spec.transport) {
            p.push(format!("{at}: {e}"));
            continue;
        }
        let schema = schemas.get(&spec.pipeline.mapping.schema_version);
        if let Err(e) = spec.validate_against(schema) {
            p.push(format!("{at}: {e}"));
            continue;
        }
        if let Some(sub) = &s.raw_subject
            && let Err(e) = crate::api::check_raw_subject(sub, &common.nats.tracks_subject)
        {
            p.push(format!("{at}: raw output: {e}"));
        }
        s.name = spec.name.clone();
        s.transport = spec.transport.kind().into();
        s.codec = spec.pipeline.codec.name().into();
        s.priority = spec.priority;
        match serde_json::to_value(&spec) {
            Ok(v) => s.spec = v,
            Err(e) => p.push(format!("{at}: {e}")),
        }
    }

    // Settings.
    if let Some(c) = &f.correlation_settings {
        match serde_json::from_value::<crate::correlate::CorrelationSettings>(c.clone()) {
            Ok(c) => match c.validate() {
                Ok(()) => {
                    f.correlation_settings = serde_json::to_value(&c).ok();
                }
                Err(e) => p.push(format!("correlation_settings: {e}")),
            },
            Err(e) => p.push(format!("correlation_settings: {e}")),
        }
    }
    if !is_empty(&f.app_settings) {
        match serde_json::from_value::<crate::settings_api::AppSettings>(f.app_settings.clone()) {
            Ok(a) => {
                if let Err(e) = a.validate() {
                    p.push(format!("app_settings: {e}"));
                }
                if a.sync.peers.contains(&common.site.to_string()) {
                    p.push(format!(
                        "app_settings: sync peers list {}, this node's own site code",
                        common.site
                    ));
                }
            }
            Err(e) => p.push(format!("app_settings: {e}")),
        }
    }
    if !is_empty(&f.auth_settings) {
        match serde_json::from_value::<crate::auth::AuthSettings>(f.auth_settings.clone()) {
            Ok(a) => match a.validate() {
                // Written as the API saves them: settings retired since
                // (the account policy, now fixed) are dropped.
                Ok(()) => f.auth_settings = json!(a),
                Err(e) => p.push(format!("auth_settings: {e}")),
            },
            Err(e) => p.push(format!("auth_settings: {e}")),
        }
    }

    // Accounts and their tokens.
    let (mut ids, mut emails) = (Seen::new(), Seen::new());
    for a in &f.accounts {
        let at = format!("account {}", a.email);
        p.extend(unique(&mut ids, "account id", a.id.clone()));
        p.extend(unique(
            &mut emails,
            "account",
            a.email.trim().to_lowercase(),
        ));
        if a.id.trim().is_empty() || a.email.trim().is_empty() {
            p.push(format!("{at}: an account needs an id and an email"));
        }
        if crate::auth::Role::parse(&a.role).is_none() {
            p.push(format!(
                "{at}: role {:?} (viewer, track_manager or admin)",
                a.role
            ));
        }
        if !matches!(a.origin.as_str(), "local" | "saml") {
            p.push(format!("{at}: origin {:?} (local or saml)", a.origin));
        }
    }
    if !f.accounts.is_empty() && !f.accounts.iter().any(|a| a.active && a.role == "admin") {
        p.push("accounts: none is an active admin, so no one could administer the node".into());
    }
    let mut seen = Seen::new();
    for t in &f.api_tokens {
        p.extend(unique(&mut seen, "API token", t.jti.clone()));
        if !f.accounts.iter().any(|a| a.id == t.user_id) {
            p.push(format!(
                "API token {}: its account {} is not in the file",
                t.jti, t.user_id
            ));
        }
    }

    // The registry.
    let (mut seen, mut idents) = (Seen::new(), Seen::new());
    for e in &mut f.registry.entities {
        let at = format!("entity {}", e.id);
        if e.id.trim().is_empty() {
            p.push("registry: an entity has no id".into());
            continue;
        }
        p.extend(unique(&mut seen, "entity", e.id.clone()));
        if let Err(err) = e.validate() {
            p.push(format!("{at}: {err}"));
            continue;
        }
        for i in &e.identifiers {
            p.extend(unique(&mut idents, "identifier", i.label()));
        }
    }

    // Plugins.
    let mut seen = Seen::new();
    for pl in &f.plugins {
        let at = format!("plugin {}", pl.name);
        p.extend(unique(&mut seen, "plugin", pl.name.clone()));
        if ot_source::plugin::is_builtin(&pl.name) {
            p.push(format!("{at}: a built-in plugin has that name"));
        }
        match serde_json::from_value::<ot_source::plugin::Manifest>(pl.manifest.clone()) {
            Ok(m) if m.name != pl.name => {
                p.push(format!("{at}: its manifest names it {:?}", m.name));
            }
            Ok(_) => {}
            Err(e) => p.push(format!("{at}: manifest: {e}")),
        }
        match serde_json::from_value::<ot_plugin::Grants>(pl.grants.clone()) {
            Ok(g) => {
                if let Err(e) = g.validate() {
                    p.push(format!("{at}: grants: {e}"));
                }
            }
            Err(e) => p.push(format!("{at}: grants: {e}")),
        }
        match (pl.runtime.as_str(), &pl.wasm, &pl.address) {
            ("wasm", Some(bytes), None) => {
                let sha = ot_plugin::sha256_hex(bytes);
                if pl.sha256.as_deref() != Some(sha.as_str()) {
                    p.push(format!(
                        "{at}: the component's SHA-256 is {sha}, not the {:?} recorded",
                        pl.sha256.as_deref().unwrap_or("(none)")
                    ));
                }
            }
            ("external", None, Some(a)) if !a.trim().is_empty() => {}
            ("wasm", ..) => p.push(format!("{at}: a WebAssembly plugin needs its component")),
            ("external", ..) => p.push(format!("{at}: an external plugin needs its address")),
            (other, ..) => p.push(format!("{at}: runtime {other:?} (wasm or external)")),
        }
    }

    // Tracker profiles, beside the shipped ones.
    let shipped: std::collections::BTreeSet<String> = crate::profiles::list(common)
        .0
        .into_iter()
        .filter(|l| l.builtin)
        .map(|l| l.profile.name)
        .collect();
    let mut seen = Seen::new();
    for v in &f.tracker_profiles {
        match serde_json::from_value::<crate::profiles::Profile>(v.clone()) {
            Ok(pr) => {
                let at = format!("tracker profile {}", pr.name);
                if let Err(e) = pr.validate() {
                    p.push(e);
                } else if shipped.contains(&pr.name) {
                    p.push(format!("{at}: one ships with this build under that name"));
                }
                p.extend(unique(&mut seen, "tracker profile", pr.name));
            }
            Err(e) => p.push(format!("tracker profile: {e}")),
        }
    }

    // Track numbers.
    let mut seen = Seen::new();
    for u in &f.uid_sequences {
        p.extend(unique(&mut seen, "uid_sequences site", u.site.clone()));
        if u.site.parse::<ot_core::SiteCode>().is_err() {
            p.push(format!("uid_sequences: {:?} is not a site code", u.site));
        }
        if u.next_sequence < 1 {
            p.push(format!(
                "uid_sequences {}: next_sequence starts at 1",
                u.site
            ));
        }
    }
    p
}

type Seen = std::collections::BTreeSet<String>;

/// A problem when `key` was seen before.
fn unique(seen: &mut Seen, what: &str, key: String) -> Option<String> {
    (!seen.insert(key.clone())).then(|| format!("{what} {key:?} is listed twice"))
}

fn is_empty(v: &Value) -> bool {
    v.is_null() || v.as_object().is_some_and(|m| m.is_empty())
}

/// Import a file into this node, which must be empty: checked, then
/// written in one transaction (tracker profiles as files, removed again if
/// the transaction fails), recorded as an `import_config` decision by
/// `actor`. The imported accounts replace the node's first admin, whose
/// password file (`initial-admin.txt`) is deleted; a file with no accounts
/// (from a node with sign-in off) leaves the first admin in place.
pub fn import(
    db: &mut ot_store::Db,
    common: &Common,
    bytes: &[u8],
    actor: &str,
) -> Result<Imported, ImportError> {
    let there = present(db, common).map_err(|e| ImportError::Failed(e.to_string()))?;
    if !there.is_empty() {
        return Err(ImportError::NotEmpty(there));
    }
    let f = parse(common, bytes).map_err(ImportError::Invalid)?;
    let auth_settings: crate::auth::AuthSettings = if is_empty(&f.auth_settings) {
        Default::default()
    } else {
        serde_json::from_value(f.auth_settings.clone())
            .map_err(|e| ImportError::Invalid(vec![format!("auth_settings: {e}")]))?
    };
    let dir = crate::profiles::user_dir(common);
    let written = std::cell::RefCell::new(Vec::<PathBuf>::new());
    let write_profiles = || -> ot_store::sqlite::Result<()> {
        if f.tracker_profiles.is_empty() {
            return Ok(());
        }
        let fail = |e: std::io::Error, p: &Path| {
            ot_store::StoreError::Conflict(format!("tracker profiles: {}: {e}", p.display()))
        };
        std::fs::create_dir_all(&dir).map_err(|e| fail(e, &dir))?;
        for v in &f.tracker_profiles {
            let name = v["name"].as_str().unwrap_or_default();
            let path = dir.join(format!("{name}.json"));
            let text = serde_json::to_string_pretty(v).unwrap_or_default() + "\n";
            std::fs::write(&path, text).map_err(|e| fail(e, &path))?;
            written.borrow_mut().push(path);
        }
        Ok(())
    };
    let decision_id = match db.restore_config(&f, actor, write_profiles) {
        Ok(id) => id,
        Err(e) => {
            for path in written.borrow().iter() {
                let _ = std::fs::remove_file(path);
            }
            return Err(ImportError::Failed(e.to_string()));
        }
    };
    let first_admin = data_dir(common).join("initial-admin.txt");
    if !f.accounts.is_empty() && first_admin.is_file() {
        let _ = std::fs::remove_file(&first_admin);
    }
    let mut notes = Vec::new();
    if f.site_code != common.site.to_string() {
        notes.push(format!(
            "the file came from site {}; this node is {} (its track numbers stay its own)",
            f.site_code, common.site
        ));
    }
    if !f.api_tokens.is_empty() {
        notes.push(
            "API tokens work here only if this node signs with the same session key \
             (OT_SESSION_SECRET or session.key) as the one they came from"
                .into(),
        );
    }
    if f.accounts.is_empty() {
        notes.push(
            "the file has no accounts (its node ran with sign-in off): this node keeps its own"
                .into(),
        );
    } else {
        notes.push(
            "sign in with the imported accounts: the first admin account was replaced".into(),
        );
    }
    Ok(Imported {
        decision_id,
        counts: f.counts(),
        notes,
        auth_settings,
    })
}

/// The file, as a download no cache keeps.
async fn export_route(State(s): State<AppState>, headers: HeaderMap) -> Result<Response, ApiError> {
    let (actor, common) = (actor(&headers), s.common.clone());
    let f = s.with_db(move |db| export(db, &common, &actor)).await?;
    let body = serde_json::to_vec_pretty(&f).map_err(|e| ApiError::internal(e.to_string()))?;
    let name = format!(
        "opentrack-config-{}-{}.json",
        s.common.site,
        chrono::Utc::now().format("%Y%m%d-%H%M")
    );
    let mut h = HeaderMap::new();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    h.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{name}\""))
            .map_err(|e| ApiError::internal(e.to_string()))?,
    );
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, private"),
    );
    h.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    Ok((StatusCode::OK, h, body).into_response())
}

/// Whether this node may import a configuration, and if not, what it has.
async fn status_route(State(s): State<AppState>) -> Result<Json<Value>, ApiError> {
    let common = s.common.clone();
    let there = s.with_db(move |db| present(db, &common)).await?;
    Ok(Json(json!({
        "empty": there.is_empty(),
        "present": there,
        "format": backup::FORMAT,
        "version": backup::VERSION,
    })))
}

async fn import_route(
    State(s): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let (actor, common) = (actor(&headers), s.common.clone());
    let db = s.db.clone();
    let result = tokio::task::spawn_blocking(move || {
        let mut db = db
            .lock()
            .map_err(|_| ImportError::Failed("database lock poisoned".into()))?;
        import(&mut db, &common, &body, &actor)
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?;
    let done = match result {
        Ok(done) => done,
        Err(e) => {
            let (status, extra) = match &e {
                ImportError::Invalid(p) => {
                    (StatusCode::UNPROCESSABLE_ENTITY, json!({ "problems": p }))
                }
                ImportError::NotEmpty(p) => (StatusCode::CONFLICT, json!({ "present": p })),
                ImportError::Failed(_) => (StatusCode::INTERNAL_SERVER_ERROR, json!({})),
            };
            let mut body = json!({ "error": e.to_string() });
            if let (Some(b), Some(x)) = (body.as_object_mut(), extra.as_object()) {
                b.extend(x.clone());
            }
            return Ok((status, Json(body)).into_response());
        }
    };
    s.auth.set_settings(done.auth_settings.clone());
    if let Err(e) = crate::plugins::sync(&s.common).await {
        tracing::warn!(error = %format!("{e:#}"), "plugins not loaded after the import");
    }
    tracing::warn!(decision = done.decision_id, "configuration imported");
    // The caller's account was the first admin, which the import replaced.
    let mut h = HeaderMap::new();
    h.insert(header::SET_COOKIE, s.auth.clear_cookie());
    Ok((
        StatusCode::OK,
        h,
        Json(json!({
            "imported": true,
            "decision": done.decision_id,
            "counts": done.counts,
            "notes": done.notes,
        })),
    )
        .into_response())
}

/// `opentrack config`: the configuration export and import without the UI
/// (an empty node's only account is its first admin, if it has one).
#[derive(Debug, clap::Subcommand)]
pub enum ConfigCommand {
    /// Write the whole configuration (secrets and password hashes
    /// included) to standard output or a file.
    Export {
        /// Write here (readable by this user only) instead of standard output.
        #[arg(long, short)]
        out: Option<PathBuf>,
    },
    /// Rebuild this node from an export. Only a node with no configuration
    /// yet takes one; the imported accounts replace its first admin. Restart
    /// a running server afterwards so it picks up the sign-in settings.
    Import {
        /// The export (`-`: standard input).
        file: PathBuf,
    },
    /// Whether this node may import a configuration, and if not, what it has.
    Status,
}

const CLI_ACTOR: &str = "cli";

pub fn run(common: &Common, cmd: ConfigCommand) -> anyhow::Result<()> {
    let mut db = common.open_new_db()?;
    match cmd {
        ConfigCommand::Export { out } => {
            let f = export(&mut db, common, CLI_ACTOR)?;
            let text = serde_json::to_string_pretty(&f)? + "\n";
            match out {
                Some(path) => {
                    crate::auth::write_private(&path, &text)?;
                    eprintln!(
                        "wrote {} (it holds secrets and password hashes: keep it safe)",
                        path.display()
                    );
                }
                None => {
                    use std::io::Write;
                    std::io::stdout().lock().write_all(text.as_bytes())?;
                }
            }
        }
        ConfigCommand::Import { file } => {
            let bytes = if file.as_os_str() == "-" {
                let mut b = Vec::new();
                std::io::Read::read_to_end(&mut std::io::stdin().lock(), &mut b)?;
                b
            } else {
                std::fs::read(&file).map_err(|e| anyhow::anyhow!("{}: {e}", file.display()))?
            };
            match import(&mut db, common, &bytes, CLI_ACTOR) {
                Ok(done) => {
                    println!("imported (decision {}): {}", done.decision_id, done.counts);
                    for n in &done.notes {
                        println!("note: {n}");
                    }
                }
                Err(ImportError::Invalid(problems)) => {
                    for p in &problems {
                        eprintln!("problem: {p}");
                    }
                    anyhow::bail!("the file was not imported ({} problem(s))", problems.len());
                }
                Err(e) => anyhow::bail!("{e}"),
            }
        }
        ConfigCommand::Status => {
            let there = present(&db, common)?;
            if there.is_empty() {
                println!("empty: this node can import a configuration");
            } else {
                println!("not empty: it has {}", there.join(", "));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ot_store::{Db, Decision, PluginWrite, SourceWrite};

    const AIS: &str = include_str!("../../../docs/examples/aisstream.json");
    const SCHEMA: &str = include_str!("../../../docs/examples/schema.json");
    const PASSWORD: &str = "correct horse battery";

    pub(crate) fn common(dir: &Path) -> Common {
        Common {
            sqlite: dir.join("ot.db"),
            redis: "redis://127.0.0.1:9".into(),
            redis_ca: None,
            redis_cert: None,
            redis_key: None,
            redis_namespace: "unused".into(),
            site: ot_core::SiteCode::new("TST").unwrap(),
            nats: crate::config::NatsArgs {
                url: "nats://127.0.0.1:9".into(),
                creds: None,
                token: None,
                user: None,
                password: None,
                ca: None,
                cert: None,
                key: None,
                stream: "TRACKS".into(),
                tracks_subject: "tracks".into(),
                max_age_hours: 24.0,
            },
            profiles_dir: Path::new(env!("CARGO_MANIFEST_DIR")).join("../../profiles/trackers"),
            obs_window_secs: 600,
            shared_db: Default::default(),
        }
    }

    /// A node with something in every section.
    fn configured(c: &Common) -> Db {
        let mut db = c.open_new_db().unwrap();
        let schema: Value = serde_json::from_str(SCHEMA).unwrap();
        let fields: Vec<Value> = serde_json::from_value(schema["fields"].clone()).unwrap();
        db.put_schema_draft(&fields, Some("examples"), "op:test")
            .unwrap();
        db.publish_schema_draft("op:test").unwrap();
        let mut spec: Value = serde_json::from_str(AIS).unwrap();
        spec["transport"]["subscribe"]["APIKey"] = json!("sk-live-secret-123");
        db.put_source(
            &SourceWrite {
                id: "aisstream",
                name: "AIS",
                transport: "websocket",
                codec: "json",
                priority: 100,
                spec: &spec,
            },
            "op:test",
        )
        .unwrap();
        db.set_source_enabled("aisstream", true, "op:test").unwrap();
        db.put_app_settings(&json!({"site_name": "Garden Island"}), "op:test")
            .unwrap();
        // Saved before the account policy was fixed: those settings are
        // dropped on the way out and ignored on the way in.
        db.put_auth_settings(&json!({"session_hours": 8.0, "lockout": {"max_failures": 5, "window_minutes": 15.0, "lock_minutes": 30.0}, "inactivity": {"disable_after_days": 0.0, "exempt": ["ann@example.org"]}}))
            .unwrap();
        // Saved as the API saves them: every setting, defaults filled in.
        let correlation: crate::correlate::CorrelationSettings =
            serde_json::from_value(json!({"mode": "suggest"})).unwrap();
        db.save_correlation_settings(
            &serde_json::to_value(correlation).unwrap(),
            Decision::new("op:test", "correlation_settings"),
        )
        .unwrap();
        let hash = crate::auth::hash_password(PASSWORD).unwrap();
        for (id, email, role) in [
            ("u1", "ann@example.org", "admin"),
            ("u2", "bob@example.org", "viewer"),
        ] {
            db.create_user(&ot_store::NewUser {
                id,
                email,
                name: "",
                role,
                password_hash: Some(&hash),
                origin: "local",
            })
            .unwrap();
        }
        db.set_must_change_password("u2", true).unwrap();
        db.add_api_token("jti-1", "feed", "u1", "ann@example.org", 4_000_000_000_000)
            .unwrap();
        db.create_session(&ot_store::auth::NewSession {
            id: "session-jti-1",
            user_id: "u1",
            expires_at_ms: 4_000_000_000_000,
            ip: Some("10.0.0.9"),
            user_agent: Some("test"),
            prev_login_at_ms: None,
            failed_before: 0,
        })
        .unwrap();
        db.save_entity(
            &ot_store::Entity {
                name: Some("TED STEVENS".into()),
                identifiers: vec![ot_store::RegistryIdentifier {
                    scheme: "mmsi".into(),
                    value: "338000001".into(),
                    expected_name: None,
                    source: None,
                }],
                ..Default::default()
            },
            "op:test",
        )
        .unwrap();
        let wasm = b"\0asm\x0d\0\x01\0 not really".to_vec();
        let sha = ot_plugin::sha256_hex(&wasm);
        db.put_plugin(
            &PluginWrite {
                name: "demo",
                version: "1.2",
                manifest: &json!({"name": "demo", "version": "1.2", "kinds": ["codec"]}),
                wasm: Some((&wasm, &sha)),
                address: None,
                grants: &json!({}),
                secret: None,
            },
            "op:test",
        )
        .unwrap();
        for k in ["a", "b", "c", "d", "e"] {
            db.create_system_track(c.site, "aisstream", k, Decision::new("engine", "create"))
                .unwrap();
        }
        // A tracker profile imported here, and the session key beside the db.
        let shipped: Value = serde_json::from_str(
            &std::fs::read_to_string(c.profiles_dir.join("astor-dmti.json")).unwrap(),
        )
        .unwrap();
        let mut mine = shipped.clone();
        mine["name"] = json!("my-radar");
        mine["label"] = json!("My radar");
        let dir = crate::profiles::user_dir(c);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("my-radar.json"), mine.to_string()).unwrap();
        crate::auth::load_secret(c, None).unwrap();
        db
    }

    /// A node as first started: its first admin only.
    fn fresh(c: &Common) -> Db {
        let mut db = c.open_new_db().unwrap();
        crate::auth::bootstrap_admin(c, &mut db, None, None).unwrap();
        assert!(data_dir(c).join("initial-admin.txt").is_file());
        db
    }

    fn exported(c: &Common, db: &mut Db) -> Vec<u8> {
        serde_json::to_vec_pretty(&export(db, c, "ann@example.org").unwrap()).unwrap()
    }

    #[test]
    fn the_export_is_marked_by_its_sources_labels_else_the_banner() {
        crate::fips::init().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let c = common(dir.path());
        let mut db = configured(&c);
        let mut f = export(&mut db, &c, "ann@example.org").unwrap();
        // No labels, no banner saved: the default banner text.
        assert_eq!(f.marking, "UNCLASSIFIED");
        f.app_settings = json!({"banner": {"text": "CONFIDENTIAL"}});
        assert_eq!(marking(&f), "CONFIDENTIAL");
        f.sources[0].spec["security"] = json!({"classification": "SECRET", "sharing": "USA, GBR"});
        assert_eq!(marking(&f), "(S//REL TO USA, GBR)");
        let mut second = f.sources[0].clone();
        second.spec.as_object_mut().unwrap().remove("security");
        f.sources.push(second);
        // An unlabelled source counts as the banner's classification.
        f.app_settings = json!({"banner": {"text": "TOP SECRET"}});
        f.correlation_settings =
            Some(json!({"labels": {"classification_order": ["U", "S", "TS"]}}));
        assert_eq!(marking(&f), "(TS//REL TO USA, GBR)");
    }

    #[test]
    fn a_node_rebuilds_from_its_export() {
        crate::fips::init().unwrap();
        let (a_dir, b_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (a, b) = (common(a_dir.path()), common(b_dir.path()));
        let mut src = configured(&a);
        let bytes = exported(&a, &mut src);
        let text = String::from_utf8(bytes.clone()).unwrap();
        // Secrets are in; the session key, sessions and the audit record not.
        assert!(text.contains("sk-live-secret-123"));
        let key = std::fs::read_to_string(a_dir.path().join("session.key")).unwrap();
        assert!(!text.contains(key.trim()), "the session key");
        assert!(!text.contains("session-jti-1"), "sessions");
        let doc: Value = serde_json::from_slice(&bytes).unwrap();
        let keys: Vec<&str> = doc
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        for k in ["sessions", "audit", "decisions", "session_key", "tracks"] {
            assert!(!keys.contains(&k), "{k} in {keys:?}");
        }
        assert_eq!(
            (doc["format"].as_str(), doc["version"].as_u64()),
            (Some("opentrack-config"), Some(2))
        );
        // The export is in the audit record, with who did it.
        let row = src
            .audit_rows(&ot_store::AuditFilter {
                ops: vec!["export_config".into()],
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(row.len(), 1);
        assert_eq!(row[0].actor, "ann@example.org");
        // Retired sign-in settings are not exported...
        assert_eq!(
            doc["auth_settings"]["inactivity"],
            json!({"exempt": ["ann@example.org"]})
        );
        assert!(doc["auth_settings"].get("lockout").is_none());
        // ...and a file from before, which has them, still imports.
        let mut old = doc.clone();
        old["auth_settings"]["session_hours"] = json!(8.0);
        old["auth_settings"]["password"] = json!({"min_length": 8});
        old["auth_settings"]["audit"] = json!({"retention_days": 30.0});
        let old = serde_json::to_vec(&old).unwrap();

        let mut dst = fresh(&b);
        assert!(
            present(&dst, &b).unwrap().is_empty(),
            "the first admin does not count"
        );
        let done = import(&mut dst, &b, &old, "cli").unwrap();
        assert_eq!(done.counts["accounts"], 2);
        assert!(done.auth_settings.inactivity.exempts("ann@example.org"));
        assert!(!data_dir(&b).join("initial-admin.txt").exists());

        // Everything came across.
        let s = dst.get_source("aisstream").unwrap().unwrap();
        assert!(s.enabled);
        assert_eq!(
            s.spec["transport"]["subscribe"]["APIKey"],
            "sk-live-secret-123"
        );
        assert_eq!(dst.latest_published_schema().unwrap(), 2);
        assert_eq!(dst.app_settings().unwrap()["site_name"], "Garden Island");
        assert_eq!(
            dst.correlation_settings().unwrap().unwrap()["mode"],
            "suggest"
        );
        let auth = dst.auth_settings().unwrap();
        assert_eq!(auth["inactivity"], json!({"exempt": ["ann@example.org"]}));
        assert!(auth.get("session_hours").is_none() && auth.get("lockout").is_none());
        assert!(
            dst.user_by_email("admin@opentrack.local")
                .unwrap()
                .is_none()
        );
        let (ann, hash) = dst.password_hash("ann@example.org").unwrap().unwrap();
        assert_eq!(ann.role, "admin");
        assert!(crate::auth::verify_password(PASSWORD, &hash.unwrap()));
        assert!(
            dst.user_by_email("bob@example.org")
                .unwrap()
                .unwrap()
                .must_change_password
        );
        assert_eq!(dst.recent_password_hashes("u1", 5).unwrap().len(), 1);
        let tokens = dst.api_tokens().unwrap();
        assert_eq!((tokens.len(), tokens[0].jti.as_str()), (1, "jti-1"));
        assert!(
            dst.sessions(None, true, 10).unwrap().is_empty(),
            "no sessions"
        );
        let ted = dst.registry_resolve("mmsi", "338000001").unwrap().unwrap();
        assert_eq!(ted.name.as_deref(), Some("TED STEVENS"));
        let p = dst.get_plugin("demo").unwrap().unwrap();
        assert_eq!(p.version, "1.2");
        assert_eq!(
            dst.plugin_wasm("demo").unwrap(),
            src.plugin_wasm("demo").unwrap()
        );
        let profiles = crate::profiles::list(&b).0;
        assert!(
            profiles
                .iter()
                .any(|l| l.profile.name == "my-radar" && !l.builtin)
        );
        let (uid, _) = dst
            .create_system_track(b.site, "aisstream", "z", Decision::new("engine", "create"))
            .unwrap();
        assert_eq!(uid.to_string(), "TST000000006", "track numbers carry on");
        // Exported again, it is the same configuration.
        let mut again: ConfigFile = serde_json::from_slice(&exported(&b, &mut dst)).unwrap();
        let first: ConfigFile = serde_json::from_slice(&bytes).unwrap();
        again.exported_at = first.exported_at;
        again.uid_sequences = first.uid_sequences.clone();
        for (x, y) in again.sources.iter_mut().zip(&first.sources) {
            x.updated_at_ms = y.updated_at_ms;
        }
        for (x, y) in again.plugins.iter_mut().zip(&first.plugins) {
            x.updated_at_ms = y.updated_at_ms;
        }
        // A source's spec is saved as the API normalises it (defaults
        // filled in), as the source editor would have saved it.
        assert_eq!(
            again.sources[0].spec["transport"]["subscribe"],
            first.sources[0].spec["transport"]["subscribe"]
        );
        again.sources = first.sources.clone();
        let (x, y) = (
            serde_json::to_value(&again).unwrap(),
            serde_json::to_value(&first).unwrap(),
        );
        for (k, v) in x.as_object().unwrap() {
            assert_eq!(v, &y[k], "{k}");
        }
        // And the import is in the audit record.
        let rows = dst
            .audit_rows(&ot_store::AuditFilter {
                ops: vec!["import_config".into()],
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn a_configured_node_refuses_an_import() {
        crate::fips::init().unwrap();
        let (a_dir, b_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (a, b) = (common(a_dir.path()), common(b_dir.path()));
        let bytes = exported(&a, &mut configured(&a));
        let mut dst = configured(&b);
        let before = dst.config_snapshot().unwrap();
        match import(&mut dst, &b, &bytes, "cli") {
            Err(ImportError::NotEmpty(there)) => {
                let all = there.join(", ");
                assert!(
                    all.contains("1 source(s)") && all.contains("2 account(s)"),
                    "{all}"
                );
                assert!(all.contains("imported tracker profile"), "{all}");
            }
            other => panic!("{other:?}"),
        }
        let mut after = dst.config_snapshot().unwrap();
        after.exported_at = before.exported_at;
        assert_eq!(after, before);
        // A profile alone makes a node non-empty.
        let c_dir = tempfile::tempdir().unwrap();
        let c = common(c_dir.path());
        let mut only = fresh(&c);
        let dir = crate::profiles::user_dir(&c);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("x.json"), "{}").unwrap();
        assert!(matches!(
            import(&mut only, &c, &bytes, "cli"),
            Err(ImportError::NotEmpty(_))
        ));
    }

    #[test]
    fn a_bad_file_is_refused_and_writes_nothing() {
        crate::fips::init().unwrap();
        let (a_dir, b_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (a, b) = (common(a_dir.path()), common(b_dir.path()));
        let good: Value = serde_json::from_slice(&exported(&a, &mut configured(&a))).unwrap();
        let mut dst = fresh(&b);
        let decisions = |db: &Db| -> i64 {
            db.connection()
                .query_row("SELECT count(*) FROM decisions", [], |r| r.get(0))
                .unwrap()
        };
        let n = decisions(&dst);
        let with = |f: &dyn Fn(&mut Value)| {
            let mut v = good.clone();
            f(&mut v);
            serde_json::to_vec(&v).unwrap()
        };
        let cases: Vec<(&str, Vec<u8>, &str)> = vec![
            ("not JSON", b"{not json".to_vec(), "not a JSON document"),
            (
                "version 1",
                with(&|v| {
                    let o = v.as_object_mut().unwrap();
                    o.remove("format");
                    o.remove("version");
                }),
                "version 1 export",
            ),
            ("version 3", with(&|v| v["version"] = json!(3)), "version 3"),
            (
                "another format",
                with(&|v| v["format"] = json!("x")),
                "not an OpenTrack",
            ),
            (
                "malformed",
                with(&|v| v["accounts"] = json!("everyone")),
                "malformed",
            ),
            (
                "bad source",
                with(&|v| v["sources"][0]["spec"]["transport"] = json!({"type": "carrier-pigeon"})),
                "source aisstream",
            ),
            (
                "bad settings",
                with(&|v| v["app_settings"]["banner"] = json!({"background": "red"})),
                "app_settings",
            ),
            (
                "tampered plugin",
                with(&|v| v["plugins"][0]["sha256"] = json!("00")),
                "SHA-256",
            ),
            (
                "no admin",
                with(&|v| v["accounts"][0]["role"] = json!("viewer")),
                "active admin",
            ),
            (
                "orphan token",
                with(&|v| v["api_tokens"][0]["user_id"] = json!("nobody")),
                "not in the file",
            ),
        ];
        for (what, bytes, says) in cases {
            match import(&mut dst, &b, &bytes, "cli") {
                Err(ImportError::Invalid(p)) => {
                    assert!(p.join("; ").contains(says), "{what}: {p:?}");
                }
                other => panic!("{what}: {other:?}"),
            }
        }
        assert!(present(&dst, &b).unwrap().is_empty(), "nothing written");
        assert_eq!(decisions(&dst), n);
        assert!(
            dst.user_by_email("admin@opentrack.local")
                .unwrap()
                .is_some()
        );
        assert!(data_dir(&b).join("initial-admin.txt").is_file());
    }
}
