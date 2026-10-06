//! SAPIENT (BSI Flex 335) codec for OpenTrack.
//!
//! Decodes SAPIENT protobuf messages into `serde_json::Value` trees for the
//! existing mapping pipeline. Uses runtime protobuf reflection via
//! `prost-reflect` and the file-descriptor-set shipped by `sapient-rs`, so no
//! compile-time codegen beyond `sapient-rs` itself is required.

mod codec;
mod error;

pub use codec::Decoder;
pub use error::SapientError;

/// Version tag reported in the plugin manifest.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
