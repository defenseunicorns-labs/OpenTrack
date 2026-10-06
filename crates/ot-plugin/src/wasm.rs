//! WebAssembly component plugins, run by wasmtime inside OpenTrack.
//!
//! The component is compiled once when loaded. Every decoder, tracker or
//! scorer a source or the engine opens gets an instance of its own (its own
//! memory and state), so one stream cannot disturb another. Each instance
//! has the plugin's [`Grants`]: a memory ceiling, a time limit per call
//! (epoch interruption), and only the directories, network addresses and
//! environment it was given (WASI). A call that traps or overruns fails;
//! the instance is not used again.

use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::LazyLock;
use std::time::Duration;

use anyhow::Context;
use chrono::{DateTime, Utc};
use ot_core::Observation;
use ot_source::plugin::{
    Manifest, Plugin, PluginDecoder, PluginScorer, PluginTracker, Score, ScoreCandidate,
    StreamHints,
};
use serde_json::Value;
use wasmtime::component::{Component, HasSelf, Linker, ResourceAny, ResourceTable};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder};
use wasmtime_wasi::{FsPerms, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

use crate::Grants;

mod bindings {
    wasmtime::component::bindgen!({ path: "../../wit", world: "plugin" });
}

use bindings::opentrack::plugin::host::Level;
use bindings::{Plugin as World, PluginPre as WorldPre};

/// How often the epoch advances: the resolution of call time limits.
const TICK: Duration = Duration::from_millis(10);

/// One engine for every plugin, with a thread that advances its epoch.
static ENGINE: LazyLock<Engine> = LazyLock::new(|| {
    let mut config = Config::new();
    config.epoch_interruption(true);
    let engine = Engine::new(&config).expect("a wasmtime engine");
    let ticker = engine.clone();
    std::thread::Builder::new()
        .name("plugin-epoch".into())
        .spawn(move || {
            loop {
                std::thread::sleep(TICK);
                ticker.increment_epoch();
            }
        })
        .expect("the plugin epoch thread");
    engine
});

struct State {
    plugin: String,
    wasi: WasiCtx,
    table: ResourceTable,
    limits: StoreLimits,
}

impl WasiView for State {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl bindings::opentrack::plugin::host::Host for State {
    fn log(&mut self, level: Level, message: String) {
        let plugin = self.plugin.as_str();
        match level {
            Level::Debug => tracing::debug!(%plugin, "{message}"),
            Level::Info => tracing::info!(%plugin, "{message}"),
            Level::Warn => tracing::warn!(%plugin, "{message}"),
            Level::Error => tracing::error!(%plugin, "{message}"),
        }
    }
}

/// A loaded WebAssembly plugin.
pub struct WasmPlugin {
    manifest: Manifest,
    pre: WorldPre<State>,
    grants: Grants,
    /// Resolved network grants.
    allowed: Vec<SocketAddr>,
}

impl WasmPlugin {
    /// Compile a component and read its manifest.
    pub fn load(bytes: &[u8], grants: &Grants) -> anyhow::Result<Self> {
        grants.validate().map_err(|e| anyhow::anyhow!(e))?;
        let component = Component::new(&ENGINE, bytes).ctx("not a WebAssembly component")?;
        let mut linker: Linker<State> = Linker::new(&ENGINE);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;
        World::add_to_linker::<State, HasSelf<State>>(&mut linker, |s| s)?;
        let pre = WorldPre::new(linker.instantiate_pre(&component)?)
            .ctx("the component does not implement opentrack:plugin/plugin")?;
        let allowed = resolve(&grants.network);
        let mut plugin = Self {
            manifest: Manifest {
                name: "loading".into(),
                version: String::new(),
                description: String::new(),
                kinds: Vec::new(),
                options: Vec::new(),
                framing: None,
            },
            pre,
            grants: grants.clone(),
            allowed,
        };
        let (mut store, bindings) = plugin.instance()?;
        let raw = plugin
            .call(&mut store, |s| {
                bindings.opentrack_plugin_meta().call_describe(s)
            })
            .context("describe")?;
        plugin.manifest = serde_json::from_str(&raw).context("the plugin's manifest")?;
        Ok(plugin)
    }

    fn instance(&self) -> anyhow::Result<(Store<State>, World)> {
        let g = &self.grants;
        let mut wasi = WasiCtxBuilder::new();
        wasi.inherit_stderr();
        for (k, v) in &g.env {
            wasi.env(k, v);
        }
        for d in &g.dirs {
            let perms = if d.write {
                FsPerms::ReadWrite
            } else {
                FsPerms::ReadOnly
            };
            wasi.preopened_dir(&d.path, d.guest.as_deref().unwrap_or(&d.path), perms)
                .ctx(&format!("granted directory {}", d.path))?;
        }
        if !self.allowed.is_empty() {
            let allowed = self.allowed.clone();
            wasi.inherit_network();
            wasi.allow_ip_name_lookup(true);
            wasi.socket_addr_check(move |addr, _| {
                let ok = allowed.contains(&addr);
                Box::pin(async move { ok })
            });
        }
        let limits = StoreLimitsBuilder::new()
            .memory_size(usize::try_from(g.memory_mb * 1024 * 1024).unwrap_or(usize::MAX))
            .build();
        let mut store = Store::new(
            &ENGINE,
            State {
                plugin: self.manifest.name.clone(),
                wasi: wasi.build(),
                table: ResourceTable::new(),
                limits,
            },
        );
        store.limiter(|s| &mut s.limits);
        store.set_epoch_deadline(u64::MAX / 2);
        let bindings = self.pre.instantiate(&mut store)?;
        Ok((store, bindings))
    }

    /// Run one call inside the plugin's time limit.
    fn call<T>(
        &self,
        store: &mut Store<State>,
        f: impl FnOnce(&mut Store<State>) -> wasmtime::Result<T>,
    ) -> anyhow::Result<T> {
        call(store, self.grants.call_timeout_ms, f)
    }
}

fn call<T>(
    store: &mut Store<State>,
    timeout_ms: u64,
    f: impl FnOnce(&mut Store<State>) -> wasmtime::Result<T>,
) -> anyhow::Result<T> {
    store.set_epoch_deadline(timeout_ms.div_ceil(TICK.as_millis() as u64).max(1));
    let out = f(store);
    store.set_epoch_deadline(u64::MAX / 2);
    out.map_err(|e| match e.downcast_ref::<wasmtime::Trap>() {
        Some(wasmtime::Trap::Interrupt) => anyhow::anyhow!("the call ran past {timeout_ms} ms"),
        _ => anyhow::Error::from(e),
    })
}

/// `context` for wasmtime's own error type.
trait Ctx<T> {
    fn ctx(self, what: &str) -> anyhow::Result<T>;
}

impl<T> Ctx<T> for wasmtime::Result<T> {
    fn ctx(self, what: &str) -> anyhow::Result<T> {
        self.map_err(|e| anyhow::Error::from(e).context(what.to_owned()))
    }
}

fn resolve(network: &[String]) -> Vec<SocketAddr> {
    network
        .iter()
        .flat_map(|a| match a.to_socket_addrs() {
            Ok(addrs) => addrs.collect::<Vec<_>>(),
            Err(e) => {
                tracing::warn!(address = %a, error = %e, "granted network address does not resolve");
                Vec::new()
            }
        })
        .collect()
}

fn ms(t: DateTime<Utc>) -> i64 {
    t.timestamp_millis()
}

/// An instance with one open resource (a decoder, tracker or scorer).
struct Open {
    store: Store<State>,
    bindings: World,
    resource: ResourceAny,
    timeout_ms: u64,
    /// A call trapped or overran: the instance is not used again.
    broken: Option<String>,
}

impl Open {
    fn call<T>(
        &mut self,
        f: impl FnOnce(&World, &mut Store<State>, ResourceAny) -> wasmtime::Result<Result<T, String>>,
    ) -> Result<T, String> {
        if let Some(e) = &self.broken {
            return Err(format!("stopped after an earlier failure: {e}"));
        }
        let (bindings, resource) = (&self.bindings, self.resource);
        match call(&mut self.store, self.timeout_ms, |s| {
            f(bindings, s, resource)
        }) {
            Ok(r) => r,
            Err(e) => {
                let e = format!("{e:#}");
                self.broken = Some(e.clone());
                Err(e)
            }
        }
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        if self.broken.is_none() {
            let _ = self.resource.resource_drop(&mut self.store);
        }
    }
}

impl WasmPlugin {
    fn open(
        &self,
        f: impl FnOnce(&World, &mut Store<State>) -> wasmtime::Result<Result<ResourceAny, String>>,
    ) -> Result<Open, String> {
        let (mut store, bindings) = self.instance().map_err(|e| format!("{e:#}"))?;
        let resource = self
            .call(&mut store, |s| f(&bindings, s))
            .map_err(|e| format!("{e:#}"))??;
        Ok(Open {
            store,
            bindings,
            resource,
            timeout_ms: self.grants.call_timeout_ms,
            broken: None,
        })
    }
}

fn options_json(options: &Value) -> String {
    if options.is_null() {
        "{}".into()
    } else {
        options.to_string()
    }
}

impl Plugin for WasmPlugin {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn decoder(&self, options: &Value) -> Result<Box<dyn PluginDecoder>, String> {
        let options = options_json(options);
        let open = self.open(|b, s| b.opentrack_plugin_codec().call_open_decoder(s, &options))?;
        Ok(Box::new(Decoder { open, hints: None }))
    }

    fn tracker(&self, options: &Value) -> Result<Box<dyn PluginTracker>, String> {
        let options = options_json(options);
        let open = self.open(|b, s| b.opentrack_plugin_tracker().call_open_tracker(s, &options))?;
        Ok(Box::new(Tracker(open)))
    }

    fn scorer(&self, options: &Value) -> Result<Box<dyn PluginScorer>, String> {
        let options = options_json(options);
        let open = self.open(|b, s| b.opentrack_plugin_scorer().call_open_scorer(s, &options))?;
        Ok(Box::new(Scorer(open)))
    }
}

struct Decoder {
    open: Open,
    /// Read after each frame: `hints` cannot call into the instance.
    hints: Option<StreamHints>,
}

impl PluginDecoder for Decoder {
    fn decode(&mut self, bytes: &[u8], received_at: DateTime<Utc>) -> Result<Vec<Value>, String> {
        let records = self.open.call(|b, s, r| {
            b.opentrack_plugin_codec()
                .decoder()
                .call_decode(s, r, bytes, ms(received_at))
        })?;
        if let Ok(Some(h)) = self.open.call(|b, s, r| {
            b.opentrack_plugin_codec()
                .decoder()
                .call_hints(s, r)
                .map(Ok)
        }) {
            self.hints = serde_json::from_str(&h)
                .ok()
                .and_then(|v| StreamHints::from_json(&v));
        }
        records
            .iter()
            .map(|r| serde_json::from_str(r).map_err(|e| format!("a record is not JSON: {e}")))
            .collect()
    }

    fn hints(&self) -> Option<StreamHints> {
        self.hints.clone()
    }
}

struct Tracker(Open);

impl PluginTracker for Tracker {
    fn push(&mut self, plot: Observation, received_at: DateTime<Utc>) {
        let Ok(plot) = serde_json::to_string(&plot) else {
            return;
        };
        let _ = self.0.call(|b, s, r| {
            b.opentrack_plugin_tracker()
                .tracker()
                .call_push(s, r, &plot, ms(received_at))
                .map(Ok)
        });
    }

    fn run(&mut self, now: DateTime<Utc>, force: bool) -> Result<Vec<Observation>, String> {
        let tracks = self.0.call(|b, s, r| {
            b.opentrack_plugin_tracker()
                .tracker()
                .call_run(s, r, ms(now), force)
        })?;
        tracks
            .iter()
            .map(|t| {
                serde_json::from_str(t).map_err(|e| format!("a track is not an observation: {e}"))
            })
            .collect()
    }
}

struct Scorer(Open);

impl PluginScorer for Scorer {
    fn score(
        &mut self,
        report: &Observation,
        candidates: &[ScoreCandidate],
    ) -> Result<Vec<Score>, String> {
        let report = serde_json::to_string(report).map_err(|e| e.to_string())?;
        let candidates: Vec<String> = candidates
            .iter()
            .map(|c| serde_json::to_string(c).unwrap_or_default())
            .collect();
        let scores = self.0.call(|b, s, r| {
            b.opentrack_plugin_scorer()
                .scorer()
                .call_score(s, r, &report, &candidates)
        })?;
        scores
            .iter()
            .map(|x| serde_json::from_str(x).map_err(|e| format!("a score: {e}")))
            .collect()
    }
}
