//! OpenTrack source framework.
//!
//! A source is a transport plus a codec plus a mapping, all configuration.
//! Each stage hands the next a plain value: frames (bytes), records (decoded
//! trees), then observations (records in the track schema).

pub mod codec;
pub mod expr;
pub mod frame;
pub mod grpc;
pub mod mapping;
pub mod netlog;
pub mod path;
pub mod pipeline;
pub mod plugin;
pub mod probe;
pub mod proto;
pub mod registry;
pub mod schema;
/// The secret-hiding rules live in `ot-core` (the store masks recorded
/// decisions with them too).
pub use ot_core::secrets;
pub mod source;
pub mod tls;
pub mod trace;
pub mod tracker;
pub mod transport;
