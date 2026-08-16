use std::time::Duration;

use pathhydra_store::{RestoreReport, RestoreRequest};

use crate::{
    ActiveOperationCounts, DrainOutcome, EngineConfig, EngineLifecycleState, ImageBuildReport,
    RetirementSnapshot,
};

#[derive(Clone, Debug)]
pub struct EngineRestoreRequest {
    pub store: RestoreRequest,
    pub engine_config: EngineConfig,
}

#[derive(Clone, Debug)]
pub struct EngineRestoreReport {
    pub store: RestoreReport,
    pub routing_image: ImageBuildReport,
    pub smoke_catalog_verified: bool,
    pub smoke_route_verified: bool,
    pub smoke_hydration_verified: bool,
    pub cuda_initialized: bool,
    pub shutdown: ShutdownReport,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShutdownFailureStage {
    Drain,
    RoutingIo,
    CudaWorker,
    StoreFlush,
    StoreHandle,
    Lifecycle,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShutdownFailure {
    pub stage: ShutdownFailureStage,
    pub category: &'static str,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CudaWorkerShutdownReport {
    pub queued_at_request: usize,
    pub active_at_request: usize,
    pub queued_routes_rejected: usize,
    pub joined: bool,
}

#[derive(Clone, Debug)]
pub struct ShutdownReport {
    pub state_before: EngineLifecycleState,
    pub state_after: EngineLifecycleState,
    pub already_shut_down: bool,
    pub active_before: ActiveOperationCounts,
    pub drain: DrainOutcome,
    pub drained: ActiveOperationCounts,
    pub active_requests_signalled: usize,
    pub newly_signalled_requests: usize,
    pub partition_io_stopped: bool,
    pub cuda_worker: Option<CudaWorkerShutdownReport>,
    pub store_flush_duration: Option<Duration>,
    pub retired_bundles: RetirementSnapshot,
    pub duration: Duration,
    pub failures: Vec<ShutdownFailure>,
}

impl ShutdownReport {
    #[must_use]
    pub fn complete(&self) -> bool {
        self.state_after == EngineLifecycleState::ShutDown && self.failures.is_empty()
    }
}
