//! Durable graph storage for PathHydra.
//!
//! Provisional node, relation-kind, and edge candidates remain outside the
//! confirmed graph until an external validator calls
//! [`Catalog::confirm_validated_candidate`].

mod catalog;
mod codec;
mod column_families;
mod error;
mod metrics;
mod operations;
mod options;
mod paths;

pub use catalog::{
    ActiveRoutingImage, Catalog, CatalogConfig, ConfirmedGraphRecords, ConfirmedGraphScan,
    ConfirmedRecordBatch, ScanError,
};
pub use error::{CatalogError, ConfirmedId, EdgeEndpoint, RecordKind};
pub use metrics::{
    ColumnFamilyMetrics, MaintenanceMetrics, MetricValue, ScanMetrics, StoreMetricsSnapshot,
    WriteMetrics, WriteOperationClass,
};
pub use operations::{
    CandidateCounts, CatalogSummary, CheckpointReport, CheckpointRequest, CompactionFamilyReport,
    CompactionReport, FlushMode, FlushReport, OperationError, ReadOnlyCatalog, ResourceKind,
    RestoreReport, RestoreRequest, StandaloneRestoreMetrics, VerificationLimits,
    VerificationReport, restore_checkpoint, standalone_restore_metrics,
};
pub use options::{WAL_SYNC_POLICY, WalSyncPolicy};
pub use pathhydra_core::{
    BaseWeight, Candidate, CandidateId, ConfirmedRecord, EdgeId, EdgeRecord, InvalidBaseWeight,
    MAX_NODE_PAYLOAD_BYTES, NodeId, NodeName, NodePayload, NodeRecord, RelationId, RelationName,
    RelationRecord,
};
pub use paths::{
    OperationalPathRequest, PathTarget, ResolvedOperationalPaths, validate_operational_paths,
};
