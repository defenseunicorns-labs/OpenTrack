//! OpenTrack plugin hosts. A plugin (`wit/plugin.wit`) runs either as a
//! WebAssembly component inside OpenTrack ([`wasm`]: sandboxed, with the
//! [`Grants`] an operator gives it) or as its own program that serves the
//! same interface over a socket ([`external`]: for code that cannot run as
//! WebAssembly, such as Python with numpy, or a GPU). Either way it becomes
//! an [`ot_source::plugin::Plugin`], installed where sources and the engine
//! look plugins up.

use std::path::PathBuf;
use std::sync::Arc;

use ot_source::plugin::Plugin;

pub mod external;
mod grants;
pub mod handshake;
pub mod wasm;

pub use external::{AuthFailed, NoSecret};
pub use grants::{DirGrant, Grants};

/// Where a plugin comes from.
#[derive(Debug, Clone)]
pub enum Source {
    /// A WebAssembly component (the file's bytes).
    Wasm(Vec<u8>),
    /// An external plugin.
    External(External),
}

/// An external plugin: where it listens, and how it proves itself.
#[derive(Clone, Default)]
pub struct External {
    /// `host:port`, `tcp://host:port` or `unix:/path/to/socket`.
    pub address: String,
    /// The secret shared with the plugin (a `${env:NAME}` reference is
    /// resolved when connecting). None: the plugin is only reached over a
    /// unix socket under `data_dir`.
    pub secret: Option<String>,
    /// OpenTrack's data directory: a unix socket under it needs no secret,
    /// as only accounts that can write there can listen on it.
    pub data_dir: Option<PathBuf>,
}

impl External {
    /// An address with a secret.
    pub fn new(address: impl Into<String>, secret: Option<String>) -> Self {
        Self {
            address: address.into(),
            secret,
            data_dir: None,
        }
    }
}

// Never print the secret.
impl std::fmt::Debug for External {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("External")
            .field("address", &self.address)
            .field("secret", &self.secret.as_ref().map(|_| "(set)"))
            .field("data_dir", &self.data_dir)
            .finish()
    }
}

/// Load a plugin, ask for its manifest and check it.
pub fn load(source: &Source, grants: &Grants) -> anyhow::Result<Arc<dyn Plugin>> {
    let plugin: Arc<dyn Plugin> = match source {
        Source::Wasm(bytes) => Arc::new(wasm::WasmPlugin::load(bytes, grants)?),
        Source::External(ext) => Arc::new(external::ExternalPlugin::connect(ext, grants)?),
    };
    plugin
        .manifest()
        .validate()
        .map_err(|e| anyhow::anyhow!(e))?;
    Ok(plugin)
}

/// The SHA-256 of a plugin file, in hex: how the API and the decision log
/// name exactly which build ran.
pub fn sha256_hex(bytes: &[u8]) -> String {
    aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
