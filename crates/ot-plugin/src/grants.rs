//! What a plugin may use beyond computing: an operator grants each plugin
//! its own (Settings → Plugins). By default a WebAssembly plugin has no
//! files, no network and no environment, 256 MB of memory and 5 s per call.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Grants {
    /// Most memory the plugin may grow to (MB).
    pub memory_mb: u64,
    /// Longest one call may run (ms) before it is stopped.
    pub call_timeout_ms: u64,
    /// Server directories the plugin can see (lookup tables, models).
    pub dirs: Vec<DirGrant>,
    /// Addresses the plugin may connect to (`host:port`), e.g. a model server.
    pub network: Vec<String>,
    /// Environment variables it sees.
    pub env: BTreeMap<String, String>,
}

impl Default for Grants {
    fn default() -> Self {
        Self {
            memory_mb: 256,
            call_timeout_ms: 5000,
            dirs: Vec::new(),
            network: Vec::new(),
            env: BTreeMap::new(),
        }
    }
}

/// A server directory a plugin can see.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirGrant {
    /// The directory on the server.
    pub path: String,
    /// Where the plugin sees it (default: the same path).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guest: Option<String>,
    /// It may also write there.
    #[serde(default)]
    pub write: bool,
}

impl Grants {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=65_536).contains(&self.memory_mb) {
            return Err("memory_mb: 1 to 65536".into());
        }
        if !(1..=600_000).contains(&self.call_timeout_ms) {
            return Err("call_timeout_ms: 1 to 600000".into());
        }
        for d in &self.dirs {
            if !std::path::Path::new(&d.path).is_absolute() {
                return Err(format!("directory {:?}: give an absolute path", d.path));
            }
        }
        for n in &self.network {
            if n.rsplit_once(':')
                .is_none_or(|(_, p)| p.parse::<u16>().is_err())
            {
                return Err(format!("network {n:?}: give host:port"));
            }
        }
        Ok(())
    }
}
