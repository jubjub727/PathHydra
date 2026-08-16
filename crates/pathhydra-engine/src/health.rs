use std::time::Duration;

use pathhydra_routing::{NUMERIC_POLICY_ID, RoutingImageManifest, TIE_POLICY_ID};

use crate::{CudaAvailability, EngineConfig, ImageBuildReport, RoutingUnavailableReason};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CudaDeviceSummary {
    pub ordinal: usize,
    pub name: String,
    pub compute_capability: (i32, i32),
    pub driver_version: i32,
    pub total_memory_bytes: usize,
    pub free_memory_bytes: usize,
    pub kernel_ptx_target: &'static str,
}

#[derive(Clone, Debug)]
pub struct EngineCapabilities {
    pub cpu_reference_routing: bool,
    pub gpu_routing: bool,
    pub cuda_support_compiled: bool,
    pub cuda_runtime: CudaAvailability,
    pub cuda_device: Option<CudaDeviceSummary>,
    pub cuda_algorithms: &'static [&'static str],
    pub cuda_distance_only: bool,
    pub cuda_paths: bool,
    pub cuda_finite_edge_budgets: bool,
    pub cuda_full_residency_required: bool,
    pub paths: bool,
    pub edge_budgets: bool,
    pub cancellation: bool,
    pub hydration: bool,
    pub subgraphs: bool,
    pub durable_routing_images: bool,
    pub numeric_policy_id: &'static str,
    pub tie_policy_id: &'static str,
    pub resource_limits: EngineConfig,
}

impl EngineCapabilities {
    pub(crate) fn new(
        config: EngineConfig,
        cuda_runtime: CudaAvailability,
        cuda_device: Option<CudaDeviceSummary>,
    ) -> Self {
        let gpu_routing = cfg!(feature = "cuda")
            && config.cuda.enabled
            && matches!(&cuda_runtime, CudaAvailability::Available);
        Self {
            cpu_reference_routing: true,
            gpu_routing,
            cuda_support_compiled: cfg!(feature = "cuda"),
            cuda_runtime,
            cuda_device,
            cuda_algorithms: &["frontier", "delta-stepping"],
            cuda_distance_only: true,
            cuda_paths: false,
            cuda_finite_edge_budgets: false,
            cuda_full_residency_required: true,
            paths: true,
            edge_budgets: true,
            cancellation: true,
            hydration: true,
            subgraphs: true,
            durable_routing_images: false,
            numeric_policy_id: NUMERIC_POLICY_ID,
            tie_policy_id: TIE_POLICY_ID,
            resource_limits: config,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CudaHealth {
    pub availability: CudaAvailability,
    pub device: Option<CudaDeviceSummary>,
    pub resident_node_count: usize,
    pub resident_adjacency_count: usize,
    pub resident_topology_bytes: usize,
    pub queued_lanes: usize,
    pub active_lanes: usize,
    pub peak_active_lanes: usize,
    pub reserved_search_bytes: usize,
    pub peak_reserved_search_bytes: usize,
    pub cumulative_admission_rejections: u64,
    pub worker_running: bool,
    pub cumulative_uploads: u64,
    pub cumulative_upload_failures: u64,
    pub cumulative_launches: u64,
    pub cumulative_launch_failures: u64,
    pub cumulative_fallbacks: u64,
    pub cumulative_cancellations: u64,
    pub cumulative_context_reinitializations: u64,
}

#[derive(Clone, Debug)]
pub enum RoutingHealth {
    Available,
    Unavailable(RoutingUnavailableReason),
}

#[derive(Clone, Debug)]
pub struct EngineHealth {
    pub durable_catalog_available: bool,
    pub routing: RoutingHealth,
    pub current_image_manifest: Option<RoutingImageManifest>,
    pub current_image_age: Option<Duration>,
    pub last_image_build: ImageBuildReport,
    pub active_routes: usize,
    pub peak_active_routes: usize,
    pub reserved_route_bytes: usize,
    pub peak_reserved_route_bytes: usize,
    pub cumulative_route_admissions: u64,
    pub cumulative_admission_rejections: u64,
    pub cumulative_cancellations: u64,
    pub cumulative_image_build_failures: u64,
    pub cuda: CudaHealth,
}
