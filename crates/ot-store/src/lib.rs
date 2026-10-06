//! OpenTrack storage.
//!
//! SQLite holds everything a person decided or configured: sources, schema,
//! mappings, the registry, the decision log and the temporal track graph.
//! Redis holds everything the feeds produce: observation streams, live source
//! and system track state, history and metrics. SQLite writes stay at human
//! speed while Redis absorbs thousands of observations per second.

mod app_settings;
pub mod audit;
pub mod auth;
pub mod backup;
pub mod correlation;
pub mod graph;
pub mod groups;
pub mod keys;
pub mod otel;
pub mod plugins;
pub mod probe;
pub mod redis_store;
pub mod registry;
pub mod schema;
pub mod sources;
pub mod sqlite;
pub mod sync;
pub mod uid_counter;
pub mod undo;

pub use audit::{AuditEvent, AuditFilter, AuditRow, AuditVerify};
pub use auth::{ApiToken, NewUser, Session, User};
pub use correlation::{DecisionRow, Suggestion};
pub use graph::{EdgeKind, NodeKind};
pub use groups::{Group, GroupSpec};
pub use keys::Keys;
pub use plugins::{PluginRow, PluginWrite};
pub use redis_store::{
    GroupBacklog, HistoryPoint, OutboxEntry, OutboxOp, RedisStore, RedisTls, TrackWrite,
};
pub use registry::{
    AttrType, Attribute, Entity, EntityRevision, Publish, RegistryIdentifier, RegistryRow,
    new_entity_id,
};
pub use schema::SchemaVersion;
pub use sources::{SourceRevision, SourceRow, SourceWrite};
pub use sqlite::{Db, Decision, StoreError};
pub use sync::SyncStatus;
pub use uid_counter::{UidCounterAdvanced, UidSighting};
pub use undo::Undone;
