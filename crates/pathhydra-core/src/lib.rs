//! Dependency-free graph domain types and invariants for PathHydra.
//!
//! Names are exact, case-sensitive UTF-8 strings. PathHydra does not trim,
//! normalize, case-fold, or otherwise rewrite them. Node payloads are opaque,
//! and base weights are normalized logical distances from zero through one.

mod candidate;
mod edge;
mod id;
mod name;
mod payload;
mod weight;

pub use candidate::{Candidate, ConfirmedRecord, NodeRecord, RelationRecord};
pub use edge::EdgeRecord;
pub use id::{CandidateId, EdgeId, NodeId, RelationId};
pub use name::{NodeName, RelationName};
pub use payload::{MAX_NODE_PAYLOAD_BYTES, NodePayload};
pub use weight::{BaseWeight, InvalidBaseWeight};
