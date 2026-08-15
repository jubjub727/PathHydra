//! Durable exact-name identity catalogs for PathHydra.
//!
//! Provisional candidates remain outside confirmed lookup until an external
//! validator calls [`Catalog::confirm_validated_candidate`].

mod catalog;
mod codec;
mod column_families;
mod error;

pub use catalog::Catalog;
pub use error::{CatalogError, ConfirmedId, RecordKind};
pub use pathhydra_core::{
    Candidate, CandidateId, ConfirmedRecord, NodeId, NodeName, NodeRecord, RelationId,
    RelationName, RelationRecord,
};
