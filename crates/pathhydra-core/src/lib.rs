//! Dependency-free domain types and invariants for PathHydra.
//!
//! Names are exact, case-sensitive UTF-8 strings. PathHydra does not trim,
//! normalize, case-fold, or otherwise rewrite them.

mod candidate;
mod id;
mod name;

pub use candidate::{Candidate, ConfirmedRecord, NodeRecord, RelationRecord};
pub use id::{CandidateId, NodeId, RelationId};
pub use name::{NodeName, RelationName};
