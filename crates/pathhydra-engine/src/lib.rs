//! Coherent CPU-side graph storage, publication, routing, and hydration.

mod admission;
mod cancellation;
mod engine;
mod error;
mod health;
mod hydration;
mod publication;

pub(crate) use admission::AdmissionController;
pub use admission::AdmissionRejection;
pub(crate) use cancellation::RequestRegistry;
pub use cancellation::{CancellationOutcome, RequestId};
pub use engine::{
    DEFAULT_MAX_ACTIVE_IMAGE_BYTES, DEFAULT_MAX_CONCURRENT_ROUTES,
    DEFAULT_MAX_DESTINATIONS_PER_REQUEST, DEFAULT_MAX_HYDRATION_HANDLES_PER_REQUEST,
    DEFAULT_MAX_RESERVED_ROUTE_BYTES, EngineConfig, EngineRoutingResponse, Executor, GraphEngine,
    RuntimeDiagnostics,
};
pub use error::{EngineConfigError, EngineError};
pub use health::{EngineCapabilities, EngineHealth, RoutingHealth};
pub use hydration::{
    EdgeEvaluation, HydratedEdge, HydratedEdgeResult, HydratedEdgeState, HydratedNodeResult,
    HydratedNodeState, HydratedPath, HydratedSubgraph, HydrationError, HydrationRequest,
    HydrationResponse,
};
pub use pathhydra_subgraph::{EdgeHandle, Subgraph, SubgraphError, SubgraphHandles};
pub(crate) use publication::RoutingState;
pub use publication::{
    ConfirmedMutation, ImageBuildOutcome, ImageBuildReport, PublicationOutcome,
    RoutingUnavailableReason,
};
