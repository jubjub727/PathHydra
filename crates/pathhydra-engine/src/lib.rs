//! Coherent CPU-side graph storage, publication, routing, and hydration.

mod admission;
mod cancellation;
mod cuda;
mod engine;
mod error;
mod health;
mod hydration;
mod publication;
mod retirement;

pub(crate) use admission::AdmissionController;
pub use admission::AdmissionRejection;
pub(crate) use cancellation::RequestRegistry;
pub use cancellation::{CancellationOutcome, RequestId};
pub use cuda::{
    CudaAlgorithmSelection, CudaAvailability, CudaConfig, CudaExecutorPolicy, CudaIneligibility,
    CudaRequestDiagnostics, ExecutorSelectionReason,
};
pub use engine::{
    DEFAULT_MAX_ACTIVE_IMAGE_BYTES, DEFAULT_MAX_CONCURRENT_ROUTES,
    DEFAULT_MAX_DESTINATIONS_PER_REQUEST, DEFAULT_MAX_HYDRATION_HANDLES_PER_REQUEST,
    DEFAULT_MAX_RESERVED_ROUTE_BYTES, EngineConfig, EngineRoutingResponse, Executor, GraphEngine,
    RuntimeDiagnostics, StartupBundlePolicy,
};
pub use error::{EngineConfigError, EngineError};
pub use health::{
    CudaDeviceSummary, CudaHealth, DeviceTopologyCacheHealth, EngineCapabilities, EngineHealth,
    RoutingHealth, StartupImageOutcome,
};
pub use hydration::{
    EdgeEvaluation, HydratedEdge, HydratedEdgeResult, HydratedEdgeState, HydratedNodeResult,
    HydratedNodeState, HydratedPath, HydratedSubgraph, HydrationError, HydrationRequest,
    HydrationResponse,
};
pub use pathhydra_subgraph::{EdgeHandle, Subgraph, SubgraphError, SubgraphHandles};
#[cfg(feature = "cuda")]
pub(crate) use publication::CudaTopology;
pub use publication::{
    ConfirmedMutation, CpuTopologyMode, ImageBuildOutcome, ImageBuildReport,
    PublicationFaultInjection, PublicationOutcome, PublicationStage, RoutingUnavailableReason,
};
pub(crate) use publication::{CpuTopology, PublishedExecutionImage, RoutingState};
pub(crate) use retirement::RetirementManager;
pub use retirement::RetirementSnapshot;
