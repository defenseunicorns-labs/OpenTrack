//! Plugins over the API (Settings → Plugins): list the built-in and added
//! ones with where each is used; add a WebAssembly component (the body,
//! `Content-Type: application/wasm`) or an external plugin (`{"address"}`);
//! enable, disable and grant; check; delete. A plugin is loaded and its
//! manifest read before it is stored, so a broken file never gets in.
//!
//! An external plugin shares a secret with OpenTrack, which both prove
//! they hold on every connection (`ot_plugin::handshake`). OpenTrack makes
//! one (`POST /plugins/secret`, shown that once) or the admin gives their
//! own; it is sent with the address or later (`PUT {"secret"}`), and never
//! returned: the API says only whether one is set.

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::routing::{get, post};
use axum::{Json, Router};
use ot_plugin::{Grants, Source};
use ot_store::{AuditEvent, PluginWrite};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::api::actor;
use crate::auth::access::audit;
use crate::control::{ApiError, AppState};

/// Largest component accepted (a Python plugin bundles its interpreter).
const MAX_PLUGIN_BYTES: usize = 128 * 1024 * 1024;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/plugins",
            get(list)
                .post(add)
                .layer(DefaultBodyLimit::max(MAX_PLUGIN_BYTES)),
        )
        .route("/plugins/secret", post(new_secret))
        .route("/plugins/{name}", get(view).put(configure).delete(delete))
        .route("/plugins/{name}/check", post(check))
}

/// Where each plugin is used: sources' codecs and tracker stages, and the
/// correlation scorer.
async fn uses(s: &AppState) -> Result<Vec<(String, Value)>, ApiError> {
    s.with_db(|db| {
        let mut out = Vec::new();
        for src in db.list_sources()? {
            for (what, ptr) in [
                ("codec", "/pipeline/codec/plugin"),
                ("tracker", "/pipeline/tracker/plugin"),
            ] {
                if let Some(name) = src.spec.pointer(ptr).and_then(Value::as_str) {
                    out.push((
                        name.to_owned(),
                        json!({"source": src.id, "name": src.name, "as": what, "enabled": src.enabled}),
                    ));
                }
            }
        }
        if let Some(settings) = db.correlation_settings()?
            && let Some(name) = settings.pointer("/scorer/plugin").and_then(Value::as_str)
        {
            out.push((name.to_owned(), json!({"correlation": true, "as": "scorer"})));
        }
        Ok(out)
    })
    .await
}

fn used_by(uses: &[(String, Value)], name: &str) -> Vec<Value> {
    uses.iter()
        .filter(|(n, _)| n == name)
        .map(|(_, u)| u.clone())
        .collect()
}

/// Every plugin: built in, then added (with whether it is running here).
async fn list(State(s): State<AppState>) -> Result<Json<Value>, ApiError> {
    let uses = uses(&s).await?;
    let rows = s.with_db(|db| db.list_plugins()).await?;
    let mut plugins: Vec<Value> = ot_source::plugin::plugins()
        .into_iter()
        .filter(|p| ot_source::plugin::describe(p.as_ref())["builtin"] == true)
        .map(|p| {
            let mut v = ot_source::plugin::describe(p.as_ref());
            v["runtime"] = json!("builtin");
            v["enabled"] = json!(true);
            v["status"] = json!("loaded");
            v["used_by"] = json!(used_by(&uses, &p.manifest().name));
            v
        })
        .collect();
    for r in rows {
        plugins.push(row_view(&r, &uses));
    }
    Ok(Json(json!({ "plugins": plugins })))
}

fn row_view(r: &ot_store::PluginRow, uses: &[(String, Value)]) -> Value {
    let mut v = r.manifest.clone();
    if !v.is_object() {
        v = json!({});
    }
    let manifest: Option<ot_source::plugin::Manifest> =
        serde_json::from_value(r.manifest.clone()).ok();
    v["default_options"] = manifest
        .as_ref()
        .map_or(json!({}), ot_source::plugin::default_options);
    v["builtin"] = json!(false);
    v["name"] = json!(r.name);
    v["version"] = json!(r.version);
    v["runtime"] = json!(r.runtime);
    v["sha256"] = json!(r.sha256);
    v["size"] = json!(r.size);
    v["address"] = json!(r.address);
    // Whether a secret is set, never the secret.
    if r.runtime == "external" {
        v["secret_set"] = json!(r.secret.is_some());
    }
    v["grants"] = r.grants.clone();
    v["enabled"] = json!(r.enabled);
    v["updated_at_ms"] = json!(r.updated_at_ms);
    let status = match (r.enabled, crate::plugins::load_error(&r.name)) {
        (false, _) => json!("disabled"),
        (true, Some(e)) => {
            v["error"] = json!(e);
            json!("error")
        }
        (true, None) if crate::plugins::is_loaded(&r.name) => json!("loaded"),
        (true, None) => json!("loading"),
    };
    v["status"] = status;
    v["used_by"] = json!(used_by(uses, &r.name));
    v
}

async fn view(
    State(s): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let uses = uses(&s).await?;
    let key = name.clone();
    let row = s
        .with_db(move |db| db.get_plugin(&key))
        .await?
        .ok_or_else(|| ApiError::not_found(format!("plugin {name}")))?;
    Ok(Json(row_view(&row, &uses)))
}

#[derive(Deserialize)]
struct AddQuery {
    /// Replace a plugin of the same name (a new build or address).
    #[serde(default)]
    replace: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExternalBody {
    address: String,
    /// The secret it shares with OpenTrack, or a `${env:NAME}` reference.
    #[serde(default)]
    secret: Option<String>,
    #[serde(default)]
    grants: Option<Grants>,
}

/// A secret as given: at least 32 characters, or an environment reference
/// (resolved, and checked, when the plugin is connected to). Empty: none.
fn secret_arg(secret: Option<String>) -> Result<Option<String>, ApiError> {
    let Some(s) = secret
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
    else {
        return Ok(None);
    };
    let env_ref = s.starts_with("${env:") && s.ends_with('}') && !s[2..].contains('$');
    if !env_ref {
        ot_plugin::handshake::check_secret(&s).map_err(ApiError::unprocessable)?;
    }
    Ok(Some(s))
}

/// When replacing an external plugin at the same address, the secret it
/// has: the new build is checked with it, and keeps it.
async fn replace_secret(
    s: &AppState,
    address: &str,
    replace: bool,
) -> Result<Option<String>, ApiError> {
    if !replace {
        return Ok(None);
    }
    let address = address.to_owned();
    s.with_db(move |db| {
        Ok(db
            .list_plugins()?
            .into_iter()
            .find(|r| r.runtime == "external" && r.address.as_deref() == Some(address.as_str()))
            .and_then(|r| r.secret))
    })
    .await
}

/// A new secret for an external plugin, from the FIPS module's DRBG. It is
/// not stored: send it with the plugin's address (or in `PUT`) once the
/// plugin has it. This is the only time it is shown.
async fn new_secret() -> Json<Value> {
    Json(json!({ "secret": crate::fips::random_hex::<32>() }))
}

/// Load a plugin off the async threads (compiling a component takes a
/// moment). An external plugin that fails authentication is audited.
async fn try_load(
    s: &AppState,
    actor: &str,
    source: Source,
    grants: Grants,
) -> Result<ot_source::plugin::Manifest, ApiError> {
    let address = match &source {
        Source::External(e) => e.address.clone(),
        Source::Wasm(_) => String::new(),
    };
    let loaded = tokio::task::spawn_blocking(move || ot_plugin::load(&source, &grants))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    match loaded {
        Ok(p) => Ok(p.manifest().clone()),
        Err(e) => {
            if crate::plugins::auth_failed(&e) {
                audit(
                    s,
                    crate::plugins::auth_failed_event(actor, "", &address, &e),
                )
                .await;
            }
            Err(ApiError::unprocessable(format!(
                "the plugin did not load: {e:#}"
            )))
        }
    }
}

async fn add(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<AddQuery>,
    body: Bytes,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let wasm = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|t| {
            t.starts_with("application/wasm") || t.starts_with("application/octet-stream")
        });
    let actor = actor(&headers);
    let mut secret = None;
    let (source, grants, sha) = if wasm {
        let sha = ot_plugin::sha256_hex(&body);
        (Source::Wasm(body.to_vec()), Grants::default(), Some(sha))
    } else {
        let b: ExternalBody = serde_json::from_slice(&body).map_err(|e| {
            ApiError::bad_request(format!(
                "send a component (application/wasm) or {{\"address\"}}: {e}"
            ))
        })?;
        let grants = b.grants.unwrap_or_default();
        grants.validate().map_err(ApiError::unprocessable)?;
        secret = secret_arg(b.secret)?;
        let mut stored = None;
        if secret.is_none() {
            // A new build of one that has a secret keeps it.
            stored = replace_secret(&s, &b.address, q.replace).await?;
        }
        let source = crate::plugins::external(&s.common, &b.address, secret.clone().or(stored));
        (source, grants, None)
    };
    let manifest = try_load(&s, &actor, source.clone(), grants.clone()).await?;
    let name = manifest.name.clone();
    if ot_source::plugin::is_builtin(&name) {
        return Err(ApiError::conflict(format!("{name} is a built-in plugin")));
    }
    let secret_given = secret.is_some();
    let manifest_json =
        serde_json::to_value(&manifest).map_err(|e| ApiError::internal(e.to_string()))?;
    let grants_json =
        serde_json::to_value(&grants).map_err(|e| ApiError::internal(e.to_string()))?;
    let replace = q.replace;
    let audit_actor = actor.clone();
    let row = s
        .with_db(move |db| {
            if !replace && db.get_plugin(&manifest.name)?.is_some() {
                return Err(ot_store::StoreError::Conflict(format!(
                    "plugin {} exists: replace it to add this build",
                    manifest.name
                )));
            }
            let (wasm, address) = match &source {
                Source::Wasm(b) => (Some(b.as_slice()), None),
                Source::External(e) => (None, Some(e.address.as_str())),
            };
            db.put_plugin(
                &PluginWrite {
                    name: &manifest.name,
                    version: &manifest.version,
                    manifest: &manifest_json,
                    wasm: wasm.zip(sha.as_deref()),
                    address,
                    grants: &grants_json,
                    secret: secret.as_deref(),
                },
                &audit_actor,
            )
        })
        .await?;
    if secret_given {
        audit(
            &s,
            AuditEvent::new(&actor, "plugin_secret_set").detail(json!({ "plugin": name })),
        )
        .await;
    }
    crate::plugins::sync(&s.common)
        .await
        .map_err(|e| ApiError::internal(format!("{e:#}")))?;
    let uses = uses(&s).await?;
    tracing::info!(plugin = %name, runtime = %row.runtime, "plugin added");
    Ok((StatusCode::CREATED, Json(row_view(&row, &uses))))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigureBody {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    grants: Option<Grants>,
    /// A new secret for an external plugin (or a `${env:NAME}` reference);
    /// empty clears it.
    #[serde(default)]
    secret: Option<String>,
}

async fn configure(
    State(s): State<AppState>,
    Path(name): Path<String>,
    headers: HeaderMap,
    Json(b): Json<ConfigureBody>,
) -> Result<Json<Value>, ApiError> {
    if let Some(g) = &b.grants {
        g.validate().map_err(ApiError::unprocessable)?;
    }
    let key = name.clone();
    let mut row = s
        .with_db(move |db| db.get_plugin(&key))
        .await?
        .ok_or_else(|| ApiError::not_found(format!("plugin {name}")))?;
    let actor = actor(&headers);
    // Some(None): clear it.
    let secret = match b.secret {
        None => None,
        Some(_) if row.runtime != "external" => {
            return Err(ApiError::unprocessable(
                "only an external plugin has a secret",
            ));
        }
        given => Some(secret_arg(given)?),
    };
    // Check it still loads with what it will run with.
    if b.enabled.unwrap_or(row.enabled) {
        let key = name.clone();
        let wasm = s.with_db(move |db| db.plugin_wasm(&key)).await?;
        let mut trial = row.clone();
        if let Some(sec) = &secret {
            trial.secret.clone_from(sec);
        }
        let (source, mut grants) = crate::plugins::source_of(&s.common, &trial, wasm)
            .map_err(|e| ApiError::internal(format!("{e:#}")))?;
        if let Some(g) = &b.grants {
            grants = g.clone();
        }
        try_load(&s, &actor, source, grants).await?;
    }
    if let Some(sec) = secret {
        let (key, who) = (name.clone(), actor.clone());
        let set = sec.is_some();
        row = s
            .with_db(move |db| db.set_plugin_secret(&key, sec.as_deref(), &who))
            .await?;
        let op = if set {
            "plugin_secret_set"
        } else {
            "plugin_secret_cleared"
        };
        audit(
            &s,
            AuditEvent::new(&actor, op).detail(json!({ "plugin": name })),
        )
        .await;
    }
    if b.enabled.is_some() || b.grants.is_some() {
        let grants = b
            .grants
            .map(|g| serde_json::to_value(g).unwrap_or_default());
        let key = name.clone();
        row = s
            .with_db(move |db| db.set_plugin(&key, b.enabled, grants.as_ref(), &actor))
            .await?;
    }
    crate::plugins::sync(&s.common)
        .await
        .map_err(|e| ApiError::internal(format!("{e:#}")))?;
    let uses = uses(&s).await?;
    Ok(Json(row_view(&row, &uses)))
}

#[derive(Deserialize)]
struct DeleteQuery {
    /// Delete even while sources or correlation use it.
    #[serde(default)]
    force: bool,
}

async fn delete(
    State(s): State<AppState>,
    Path(name): Path<String>,
    headers: HeaderMap,
    Query(q): Query<DeleteQuery>,
) -> Result<StatusCode, ApiError> {
    let users = used_by(&uses(&s).await?, &name);
    if !users.is_empty() && !q.force {
        return Err(ApiError::conflict(format!(
            "plugin {name} is in use ({}): change those first, or delete with force",
            users
                .iter()
                .map(|u| u["source"]
                    .as_str()
                    .map_or("correlation".to_owned(), |x| format!("source {x}")))
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    let actor = actor(&headers);
    s.with_db(move |db| db.delete_plugin(&name, &actor)).await?;
    crate::plugins::sync(&s.common)
        .await
        .map_err(|e| ApiError::internal(format!("{e:#}")))?;
    Ok(StatusCode::NO_CONTENT)
}

/// Load a stored plugin afresh and open each kind it provides with its
/// default options: whether it works, without changing anything.
async fn check(
    State(s): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let key = name.clone();
    let (row, wasm) = s
        .with_db(move |db| Ok((db.get_plugin(&key)?, db.plugin_wasm(&key)?)))
        .await?;
    let row = row.ok_or_else(|| ApiError::not_found(format!("plugin {name}")))?;
    let (source, grants) = crate::plugins::source_of(&s.common, &row, wasm)
        .map_err(|e| ApiError::internal(format!("{e:#}")))?;
    let report = tokio::task::spawn_blocking(move || crate::plugin_cli::smoke(&source, &grants))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(report))
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    use axum::body::Body;
    use axum::http::Request;
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD as B64;
    use http_body_util::BodyExt;
    use ot_plugin::handshake::{self, Role};
    use tower::ServiceExt;

    use super::*;
    use crate::auth::{Auth, AuthSettings};

    /// The plugin loader is one per process: these tests take turns.
    static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// The router (sign-in off) over a database in `dir`; `None` (test
    /// skipped) without `OT_TEST_REDIS_URL`.
    async fn app(dir: &std::path::Path) -> Option<(Router, AppState)> {
        let url = std::env::var("OT_TEST_REDIS_URL").ok()?;
        let mut common = crate::config_backup::tests::common(dir);
        common.redis = url.clone();
        common.redis_namespace =
            format!("ot-plugins-test-{}", chrono::Utc::now().timestamp_micros());
        let db = common.open_new_db().unwrap();
        let state = AppState {
            db: Arc::new(Mutex::new(db)),
            redis: ot_store::RedisStore::connect(
                &url,
                ot_store::Keys::new(common.redis_namespace.clone()),
            )
            .await
            .unwrap(),
            nats: common.connect_nats().await.unwrap(),
            auth: Arc::new(Auth::new(
                vec![1; 32],
                true,
                false,
                None,
                AuthSettings::default(),
            )),
            common,
        };
        Some((crate::control::router(state.clone(), None), state))
    }

    async fn call(
        app: &Router,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value, String) {
        let mut req = Request::builder().method(method).uri(uri);
        let body = match body {
            Some(b) => {
                req = req.header("content-type", "application/json");
                Body::from(b.to_string())
            }
            None => Body::empty(),
        };
        let res = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8_lossy(&bytes).into_owned();
        (
            status,
            serde_json::from_str(&text).unwrap_or(Value::Null),
            text,
        )
    }

    /// A scorer plugin that authenticates with `secret`.
    fn fake(name: &'static str, secret: String) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let secret = secret.clone();
                std::thread::spawn(move || {
                    let mut r = BufReader::new(stream.try_clone().unwrap());
                    let mut w = stream;
                    let (mut line, mut nonces, mut ok) = (String::new(), None, false);
                    while r.read_line(&mut line).unwrap_or(0) > 0 {
                        let req: Value = serde_json::from_str(&line).unwrap();
                        line.clear();
                        let p = &req["params"];
                        let reply = match req["method"].as_str().unwrap_or_default() {
                            "hello" => {
                                let c = B64.decode(p["challenge"].as_str().unwrap()).unwrap();
                                let n = handshake::nonce().to_vec();
                                let proof = handshake::proof(&secret, Role::Plugin, &c, &n);
                                let out = json!({"result": {"proof": B64.encode(proof), "challenge": B64.encode(&n)}});
                                nonces = Some((c, n));
                                out
                            }
                            "verify" => {
                                let (c, n) = nonces.take().unwrap();
                                let tag = B64.decode(p["proof"].as_str().unwrap()).unwrap();
                                ok = handshake::verify(&secret, Role::OpenTrack, &c, &n, &tag);
                                if ok {
                                    json!({"result": null})
                                } else {
                                    json!({"error": "wrong proof"})
                                }
                            }
                            _ if !ok => json!({"error": "not authenticated"}),
                            "describe" => {
                                json!({"result": {"name": name, "version": "1", "kinds": ["scorer"]}})
                            }
                            _ => json!({"result": null}),
                        };
                        let mut reply = reply;
                        reply["id"] = req["id"].clone();
                        if writeln!(w, "{reply}").is_err() {
                            return;
                        }
                    }
                });
            }
        });
        address
    }

    fn audit_ops(s: &AppState) -> Vec<(String, bool, String)> {
        s.db.lock()
            .unwrap()
            .audit_rows(&ot_store::AuditFilter {
                limit: 1000,
                ..Default::default()
            })
            .unwrap()
            .into_iter()
            .map(|r| (r.op, r.outcome == "success", r.detail.to_string()))
            .collect()
    }

    #[tokio::test]
    async fn an_external_plugins_secret_is_made_shown_once_checked_and_never_returned() {
        let _turn = SERIAL.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let Some((app, s)) = app(dir.path()).await else {
            eprintln!("skipped: OT_TEST_REDIS_URL not set");
            return;
        };
        // OpenTrack makes a secret: 32 random bytes as hex, different each time.
        let (st, made, _) = call(&app, "POST", "/api/v1/plugins/secret", None).await;
        assert_eq!(st, StatusCode::OK);
        let secret = made["secret"].as_str().unwrap().to_owned();
        assert_eq!(secret.len(), 64);
        assert!(secret.chars().all(|c| c.is_ascii_hexdigit()));
        let (_, again, _) = call(&app, "POST", "/api/v1/plugins/secret", None).await;
        assert_ne!(again["secret"], made["secret"]);

        let address = fake("fake-authenticated", secret.clone());
        // No secret over TCP: refused, saying why.
        let (st, body, _) = call(
            &app,
            "POST",
            "/api/v1/plugins",
            Some(json!({"address": address})),
        )
        .await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            body["error"].as_str().unwrap().contains("no secret"),
            "{body}"
        );
        // Too short.
        let (st, ..) = call(
            &app,
            "POST",
            "/api/v1/plugins",
            Some(json!({"address": address, "secret": "short"})),
        )
        .await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        // The wrong one: refused and audited.
        let wrong = "f".repeat(64);
        let (st, body, _) = call(
            &app,
            "POST",
            "/api/v1/plugins",
            Some(json!({"address": address, "secret": wrong})),
        )
        .await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("authentication failed"),
            "{body}"
        );
        let ops = audit_ops(&s);
        assert!(
            ops.iter()
                .any(|(op, ok, d)| op == "plugin_auth_failed" && !ok && d.contains(&address)),
            "{ops:?}"
        );

        // The right one: added, and never returned.
        let (st, body, text) = call(
            &app,
            "POST",
            "/api/v1/plugins",
            Some(json!({"address": address, "secret": secret})),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{body}");
        assert_eq!(body["secret_set"], true);
        assert!(!text.contains(&secret));
        for uri in ["/api/v1/plugins", "/api/v1/plugins/fake-authenticated"] {
            let (st, body, text) = call(&app, "GET", uri, None).await;
            assert_eq!(st, StatusCode::OK);
            assert!(!text.contains(&secret), "{uri}");
            if uri.ends_with("authenticated") {
                assert_eq!(
                    (body["secret_set"].as_bool(), body["status"].as_str()),
                    (Some(true), Some("loaded"))
                );
            }
        }
        let row =
            s.db.lock()
                .unwrap()
                .get_plugin("fake-authenticated")
                .unwrap()
                .unwrap();
        assert_eq!(row.secret.as_deref(), Some(secret.as_str()));
        assert!(
            audit_ops(&s)
                .iter()
                .any(|(op, ok, _)| op == "plugin_secret_set" && *ok)
        );
        // Neither the audit record nor the decision log holds it.
        assert!(!format!("{:?}", audit_ops(&s)).contains(&secret));
        let decisions: String =
            s.db.lock()
                .unwrap()
                .connection()
                .query_row(
                    "SELECT group_concat(coalesce(after, ''), ' ') FROM decisions",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
        assert!(!decisions.contains(&secret));

        // A new secret the plugin does not have is refused, and the old one kept.
        let (st, ..) = call(
            &app,
            "PUT",
            "/api/v1/plugins/fake-authenticated",
            Some(json!({"secret": wrong})),
        )
        .await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        let row =
            s.db.lock()
                .unwrap()
                .get_plugin("fake-authenticated")
                .unwrap()
                .unwrap();
        assert_eq!(row.secret.as_deref(), Some(secret.as_str()));
        let (st, ..) = call(&app, "DELETE", "/api/v1/plugins/fake-authenticated", None).await;
        assert_eq!(st, StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn a_stored_plugin_without_a_secret_or_with_the_wrong_one_does_not_load() {
        let _turn = SERIAL.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let Some((app, s)) = app(dir.path()).await else {
            eprintln!("skipped: OT_TEST_REDIS_URL not set");
            return;
        };
        // As an external plugin added before 0.4.5 is after the upgrade.
        let address = fake("fake-legacy", "a".repeat(64));
        s.db.lock()
            .unwrap()
            .put_plugin(
                &PluginWrite {
                    name: "fake-legacy",
                    version: "1",
                    manifest: &json!({"name": "fake-legacy", "version": "1", "kinds": ["scorer"]}),
                    wasm: None,
                    address: Some(&address),
                    grants: &json!({}),
                    secret: None,
                },
                "op:test",
            )
            .unwrap();
        crate::plugins::sync(&s.common).await.unwrap();
        let (_, body, _) = call(&app, "GET", "/api/v1/plugins/fake-legacy", None).await;
        assert_eq!(body["status"], "error");
        assert_eq!(body["secret_set"], false);
        assert!(
            body["error"].as_str().unwrap().contains("no secret"),
            "{body}"
        );
        // A wrong secret: refused when loading, and audited by the system.
        // (Loaders notice a change by its time: let the clock move.)
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        s.db.lock()
            .unwrap()
            .set_plugin_secret("fake-legacy", Some(&"b".repeat(64)), "op:test")
            .unwrap();
        crate::plugins::sync(&s.common).await.unwrap();
        let (_, body, _) = call(&app, "GET", "/api/v1/plugins/fake-legacy", None).await;
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("authentication failed"),
            "{body}"
        );
        let rows =
            s.db.lock()
                .unwrap()
                .audit_rows(&ot_store::AuditFilter {
                    ops: vec!["plugin_auth_failed".into()],
                    limit: 10,
                    ..Default::default()
                })
                .unwrap();
        assert_eq!((rows.len(), rows[0].actor.as_str()), (1, "system"));
        assert_eq!(rows[0].detail["plugin"], "fake-legacy");
        // Its secret set through the API, it loads.
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let (st, body, _) = call(
            &app,
            "PUT",
            "/api/v1/plugins/fake-legacy",
            Some(json!({"secret": "a".repeat(64)})),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert_eq!(
            (body["status"].as_str(), body["secret_set"].as_bool()),
            (Some("loaded"), Some(true))
        );
    }
}
