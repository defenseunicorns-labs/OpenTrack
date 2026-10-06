//! OpenTrack between nodes (see `docs/multi-node.md`).
//!
//! Nodes share one picture with no node in charge. What must not be lost is
//! what people decided: every track-management decision is an entry in a
//! log each node keeps of every node's decisions, under a global id
//! (`<site>:<seq>`) and a hybrid logical clock stamp ([`Hlc`]). Entries
//! reach other nodes in any order, late or twice; the rules here make every
//! node end up with the same result without asking anyone.

pub mod dr;
mod hlc;
mod log;
mod quality;
pub mod r2;
pub mod sign;
pub mod wire;

pub use hlc::{Clock, Hlc};
pub use log::{Entry, GlobalId, IdError, Verdict, judge, replicated, targets};
pub use quality::quality;
