//! Immutable routing images and deterministic exact CPU routing for PathHydra.
//!
//! A routing image is a caller-owned point-in-time value. It does not update
//! when the catalog changes and is not a durable file format.
//!
//! ```no_run
//! use pathhydra_routing::{
//!     DestinationState, RelationMultiplier, RelationProfile, RelationUse, RoutingImage,
//!     RoutingRequest, SearchBudget, TiePolicy, route,
//! };
//! use pathhydra_store::{Catalog, ConfirmedRecord};
//!
//! # fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let catalog = Catalog::open("pathhydra.db")?;
//! let source_candidate = catalog.insert_node_candidate("source")?;
//! let middle_candidate = catalog.insert_node_candidate("middle")?;
//! let target_candidate = catalog.insert_node_candidate("target")?;
//! let fast_candidate = catalog.insert_relation_candidate("fast")?;
//! let scenic_candidate = catalog.insert_relation_candidate("scenic")?;
//! let ConfirmedRecord::Node(source) = catalog.confirm_validated_candidate(source_candidate)? else { unreachable!() };
//! let ConfirmedRecord::Node(middle) = catalog.confirm_validated_candidate(middle_candidate)? else { unreachable!() };
//! let ConfirmedRecord::Node(target) = catalog.confirm_validated_candidate(target_candidate)? else { unreachable!() };
//! let ConfirmedRecord::Relation(fast) = catalog.confirm_validated_candidate(fast_candidate)? else { unreachable!() };
//! let ConfirmedRecord::Relation(scenic) = catalog.confirm_validated_candidate(scenic_candidate)? else { unreachable!() };
//! for (from, to, relation, weight) in [
//!     (source.id(), target.id(), fast.id(), 0.8),
//!     (source.id(), middle.id(), scenic.id(), 0.3),
//!     (middle.id(), target.id(), scenic.id(), 0.3),
//! ] {
//!     let candidate = catalog.insert_edge_candidate(from, to, relation, weight)?;
//!     catalog.confirm_validated_candidate(candidate)?;
//! }
//! let image = RoutingImage::compile(&catalog.confirmed_graph_records()?)?;
//! let profile = |fast_multiplier, scenic_multiplier| RelationProfile::new([
//!     (fast.id(), RelationUse::Enabled(RelationMultiplier::new(fast_multiplier).unwrap())),
//!     (scenic.id(), RelationUse::Enabled(RelationMultiplier::new(scenic_multiplier).unwrap())),
//! ]);
//! let request = |profile| RoutingRequest::new(
//!     source.id(), [target.id()], profile, true, SearchBudget::Unlimited,
//!     TiePolicy::StablePredecessor,
//! );
//! let direct = route(&image, &request(profile(0.5, 1.0)))?;
//! let scenic = route(&image, &request(profile(1.0, 0.5)))?;
//! let DestinationState::Exact(direct) = direct.results()[0].state() else { unreachable!() };
//! let DestinationState::Exact(scenic) = scenic.results()[0].state() else { unreachable!() };
//! assert_eq!(direct.path().unwrap().steps().len(), 1);
//! assert_eq!(scenic.path().unwrap().steps().len(), 2);
//! # Ok(())
//! # }
//! ```

pub mod bundle;
mod compile;
mod cpu;
mod error;
mod image;
mod profile;
mod request;

pub use bundle::{
    BundleBuildMetrics, BundleConfig, BundleError, BundleManifest, ChunkedRoutingImage,
    HostCacheConfig, HostCacheSnapshot, PartitionedCpuDiagnostics, RoutingBundle, compile_bundle,
    open_bundle, route_partitioned, route_partitioned_controlled,
};
pub use compile::{compile_routing_image, compile_routing_image_with_limit};
pub use cpu::{
    CancellationSignal, CpuSearchDiagnostics, CpuWorkingSetEstimate, NeverCancelled,
    accumulate_distance, effective_weight, estimate_cpu_working_set, route, route_controlled,
};
pub use error::{ArithmeticError, ArithmeticOperation, CompileError, ProfileError, RoutingError};
pub use image::{
    DenseNodeId, ElementWidths, GpuTopologyManifest, ImageByteCounts, NUMERIC_POLICY_ID,
    OutgoingEdge, RoutingImage, RoutingImageArrays, RoutingImageManifest, TIE_POLICY_ID,
};
pub use profile::{
    InvalidRelationMultiplier, PackedRelationProfile, RelationMultiplier, RelationProfile,
    RelationUse,
};
pub use request::{
    CompletionReason, DestinationResult, DestinationState, ExactRoute, NumericPolicy, PathStep,
    RoutePath, RoutingRequest, RoutingResponse, SearchBudget, TiePolicy,
};

/// Representation-independent semantic agreement used by CPU/CUDA fixtures.
/// Executor timing and cache diagnostics live outside `RoutingResponse`.
#[must_use]
pub fn responses_agree(left: &RoutingResponse, right: &RoutingResponse) -> bool {
    left == right
}
