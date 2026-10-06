//! Keeps this process's plugins in step with the database: every role that
//! runs plugins (serve, sources, engine) loads the enabled ones at start and
//! again whenever one is added, changed or removed, and installs them where
//! sources and the engine look plugins up. What failed to load, and why, is
//! kept for the API; an external plugin that fails authentication is also
//! logged as an error and audited.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use std::time::Duration;

use ot_plugin::{External, Grants, Source};
use ot_store::{AuditEvent, PluginRow};
use serde_json::json;

use crate::config::Common;

/// How often the database is checked for plugin changes.
const POLL: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Loader {
    version: String,
    /// Each enabled plugin, by the row version (`updated_at_ms`) last tried:
    /// a plugin is loaded again only when its row changes.
    tried: BTreeMap<String, i64>,
    /// Those that failed, and why.
    errors: BTreeMap<String, String>,
}

static LOADER: LazyLock<Mutex<Loader>> = LazyLock::new(|| Mutex::new(Loader::default()));

/// Why a stored plugin is not running in this process, if it is not.
pub fn load_error(name: &str) -> Option<String> {
    LOADER.lock().ok()?.errors.get(name).cloned()
}

/// Whether a stored plugin is running in this process.
pub fn is_loaded(name: &str) -> bool {
    LOADER
        .lock()
        .is_ok_and(|l| l.tried.contains_key(name) && !l.errors.contains_key(name))
}

/// The data directory (beside the database): a unix socket under it needs
/// no plugin secret.
pub fn data_dir(common: &Common) -> PathBuf {
    common
        .sqlite
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."))
        .to_path_buf()
}

/// An external plugin at `address`, as this node connects to it.
pub fn external(common: &Common, address: &str, secret: Option<String>) -> Source {
    Source::External(External {
        address: address.to_owned(),
        secret,
        data_dir: Some(data_dir(common)),
    })
}

/// A stored plugin as it would run: its component or address (and secret),
/// and grants.
pub fn source_of(
    common: &Common,
    row: &PluginRow,
    wasm: Option<Vec<u8>>,
) -> anyhow::Result<(Source, Grants)> {
    let grants: Grants = serde_json::from_value(row.grants.clone())
        .map_err(|e| anyhow::anyhow!("plugin {}: grants: {e}", row.name))?;
    let source = match (row.runtime.as_str(), wasm, &row.address) {
        ("wasm", Some(bytes), _) => Source::Wasm(bytes),
        ("external", _, Some(a)) => external(common, a, row.secret.clone()),
        _ => anyhow::bail!("plugin {}: nothing to load", row.name),
    };
    Ok((source, grants))
}

/// Whether a load failed because the plugin failed authentication.
pub fn auth_failed(e: &anyhow::Error) -> bool {
    e.chain().any(|c| c.is::<ot_plugin::AuthFailed>())
}

/// The audit event for a plugin that failed authentication.
pub fn auth_failed_event(actor: &str, name: &str, address: &str, e: &anyhow::Error) -> AuditEvent {
    AuditEvent::new(actor, "plugin_auth_failed")
        .failure()
        .detail(json!({ "plugin": name, "address": address, "error": format!("{e:#}") }))
}

/// Load what changed since the last look. Compiling a component takes a
/// moment, so this runs off the async threads.
pub async fn sync(common: &Common) -> anyhow::Result<()> {
    let c = common.clone();
    tokio::task::spawn_blocking(move || sync_blocking(&c)).await?
}

fn sync_blocking(common: &Common) -> anyhow::Result<()> {
    let mut db = common.open_db()?;
    let version = db.plugins_version()?;
    if LOADER.lock().is_ok_and(|l| l.version == version) {
        return Ok(());
    }
    let rows = db.list_plugins()?;
    let mut loader = LOADER
        .lock()
        .map_err(|_| anyhow::anyhow!("plugin loader poisoned"))?;
    let wanted: BTreeMap<&str, &PluginRow> = rows
        .iter()
        .filter(|r| r.enabled)
        .map(|r| (r.name.as_str(), r))
        .collect();
    let gone: Vec<String> = loader
        .tried
        .keys()
        .filter(|n| !wanted.contains_key(n.as_str()))
        .cloned()
        .collect();
    for name in gone {
        ot_source::plugin::uninstall(&name);
        loader.tried.remove(&name);
        loader.errors.remove(&name);
        tracing::info!(plugin = %name, "plugin removed");
    }
    for (name, row) in wanted {
        if loader.tried.get(name) == Some(&row.updated_at_ms) {
            continue;
        }
        loader.tried.insert(name.to_owned(), row.updated_at_ms);
        let wasm = if row.runtime == "wasm" {
            db.plugin_wasm(name)?
        } else {
            None
        };
        let loaded = source_of(common, row, wasm).and_then(|(source, grants)| {
            let p = ot_plugin::load(&source, &grants)?;
            if p.manifest().name != *name {
                anyhow::bail!(
                    "the plugin now calls itself {:?}, not {name:?}: add it again",
                    p.manifest().name
                );
            }
            ot_source::plugin::install(p).map_err(anyhow::Error::msg)
        });
        match loaded {
            Ok(()) => {
                tracing::info!(plugin = %name, version = %row.version, runtime = %row.runtime, "plugin loaded");
                loader.errors.remove(name);
            }
            Err(e) => {
                if auth_failed(&e) {
                    tracing::error!(plugin = %name, error = %format!("{e:#}"), "plugin failed authentication: refused");
                    let address = row.address.as_deref().unwrap_or_default();
                    if let Err(err) = db.audit(&auth_failed_event("system", name, address, &e)) {
                        tracing::warn!(error = %err, "audit event not recorded");
                    }
                } else {
                    tracing::warn!(plugin = %name, error = %format!("{e:#}"), "plugin failed to load");
                }
                ot_source::plugin::uninstall(name);
                loader.errors.insert(name.to_owned(), format!("{e:#}"));
            }
        }
    }
    loader.version = version;
    Ok(())
}

/// Load the plugins now, then keep them in step in the background (once per
/// process, however many roles it runs).
pub async fn start(common: &Common) {
    static STARTED: OnceLock<()> = OnceLock::new();
    if let Err(e) = sync(common).await {
        tracing::warn!(error = %format!("{e:#}"), "plugins not loaded");
    }
    if STARTED.set(()).is_err() {
        return;
    }
    let common = Arc::new(common.clone());
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(POLL);
        loop {
            tick.tick().await;
            if let Err(e) = sync(&common).await {
                tracing::warn!(error = %format!("{e:#}"), "plugin refresh failed");
            }
        }
    });
}
