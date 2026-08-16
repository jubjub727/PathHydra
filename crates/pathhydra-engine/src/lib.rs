//! Coherent CPU-side graph storage, publication, routing, and hydration.

mod admission;
mod cancellation;
mod cuda;
mod engine;
mod error;
mod health;
mod hydration;
mod lifecycle;
mod operations;
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
    DEFAULT_MAX_ACTIVE_IMAGE_BYTES, DEFAULT_MAX_CONCURRENT_CHECKPOINTS,
    DEFAULT_MAX_CONCURRENT_ROUTES, DEFAULT_MAX_DESTINATIONS_PER_REQUEST,
    DEFAULT_MAX_HYDRATION_HANDLES_PER_REQUEST, DEFAULT_MAX_RESERVED_ROUTE_BYTES,
    DEFAULT_MAX_VERIFICATION_DURATION, DEFAULT_MAX_VERIFICATION_RECORDS,
    DEFAULT_MAXIMUM_MAINTENANCE_WORKERS, DEFAULT_MAXIMUM_QUEUED_MAINTENANCE,
    DEFAULT_SHUTDOWN_DRAIN_TIMEOUT, EngineConfig, EngineRoutingResponse, Executor, GraphEngine,
    RuntimeDiagnostics, StartupBundlePolicy,
};
pub use error::{EngineConfigError, EngineError};
pub use health::{
    CudaDeviceSummary, CudaHealth, DeviceTopologyCacheHealth, EngineCapabilities, EngineHealth,
    ResourceLimitsSnapshot, RoutingHealth, StartupImageOutcome,
};
pub use hydration::{
    EdgeEvaluation, HydratedEdge, HydratedEdgeResult, HydratedEdgeState, HydratedNodeResult,
    HydratedNodeState, HydratedPath, HydratedSubgraph, HydrationError, HydrationRequest,
    HydrationResponse,
};
pub(crate) use lifecycle::LifecycleController;
pub use lifecycle::{
    ActiveOperationCounts, DrainOutcome, EngineLifecycleState, LifecycleError, LifecycleSnapshot,
};
pub use operations::{
    CudaWorkerShutdownReport, EngineRestoreReport, EngineRestoreRequest, ShutdownFailure,
    ShutdownFailureStage, ShutdownReport,
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
