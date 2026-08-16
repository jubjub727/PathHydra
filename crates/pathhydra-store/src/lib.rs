//! Durable graph storage for PathHydra.
//!
//! Provisional node, relation-kind, and edge candidates remain outside the
//! confirmed graph until an external validator calls
//! [`Catalog::confirm_validated_candidate`].

mod catalog;
mod codec;
mod column_families;
mod error;

pub use catalog::{
    ActiveRoutingImage, Catalog, ConfirmedGraphRecords, ConfirmedGraphScan, ConfirmedRecordBatch,
    ScanError,
};
pub use error::{CatalogError, ConfirmedId, EdgeEndpoint, RecordKind};
pub use pathhydra_core::{
    BaseWeight, Candidate, CandidateId, ConfirmedRecord, EdgeId, EdgeRecord, InvalidBaseWeight,
    MAX_NODE_PAYLOAD_BYTES, NodeId, NodeName, NodePayload, NodeRecord, RelationId, RelationName,
    RelationRecord,
};
