use std::time::Duration;

use pathhydra_routing::{NUMERIC_POLICY_ID, RoutingImageManifest, TIE_POLICY_ID};
use pathhydra_store::StoreMetricsSnapshot;

use crate::{
    CpuTopologyMode, CudaAvailability, EngineConfig, ImageBuildReport, LifecycleSnapshot,
    RetirementSnapshot, RoutingUnavailableReason,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceLimitsSnapshot {
    pub maximum_active_image_bytes: usize,
    pub maximum_concurrent_routes: usize,
    pub maximum_reserved_route_bytes: usize,
    pub maximum_destinations_per_request: usize,
    pub maximum_hydration_handles_per_request: usize,
    pub maximum_total_bundle_bytes: u64,
    pub target_partition_topology_bytes: u64,
    pub hard_maximum_partition_topology_bytes: u64,
    pub maximum_resident_image_metadata_bytes: u64,
    pub host_partition_cache_bytes: u64,
    pub host_partition_cache_entries: usize,
    pub routing_io_workers: usize,
    pub maximum_queued_partition_reads: usize,
    pub routing_io_staging_bytes: u64,
    pub maximum_retired_bundle_bytes: u64,
    pub maximum_retired_bundle_count: usize,
    pub maximum_concurrent_checkpoints: usize,
    pub maximum_maintenance_workers: usize,
    pub maximum_queued_maintenance: usize,
    pub maximum_verification_records: u64,
    pub maximum_verification_duration: Duration,
    pub shutdown_drain_timeout: Duration,
    pub cuda_maximum_topology_bytes: usize,
    pub cuda_maximum_concurrent_searches: usize,
    pub cuda_maximum_batch_lanes: usize,
    pub cuda_maximum_reserved_search_bytes: usize,
}

impl From<&EngineConfig> for ResourceLimitsSnapshot {
    fn from(config: &EngineConfig) -> Self {
        Self {
            maximum_active_image_bytes: config.max_active_image_bytes,
            maximum_concurrent_routes: config.max_concurrent_routes,
            maximum_reserved_route_bytes: config.max_reserved_route_bytes,
            maximum_destinations_per_request: config.max_destinations_per_request,
            maximum_hydration_handles_per_request: config.max_hydration_handles_per_request,
            maximum_total_bundle_bytes: config.max_total_bundle_bytes,
            target_partition_topology_bytes: config.target_partition_topology_bytes,
            hard_maximum_partition_topology_bytes: config.hard_maximum_partition_topology_bytes,
            maximum_resident_image_metadata_bytes: config.max_resident_image_metadata_bytes,
            host_partition_cache_bytes: config.host_partition_cache_bytes,
            host_partition_cache_entries: config.host_partition_cache_entries,
            routing_io_workers: config.routing_io_worker_count,
            maximum_queued_partition_reads: config.maximum_queued_partition_reads,
            routing_io_staging_bytes: config.routing_io_staging_bytes,
            maximum_retired_bundle_bytes: config.maximum_retired_bundle_bytes,
            maximum_retired_bundle_count: config.maximum_retired_bundle_count,
            maximum_concurrent_checkpoints: config.maximum_concurrent_checkpoints,
            maximum_maintenance_workers: config.maximum_maintenance_workers,
            maximum_queued_maintenance: config.maximum_queued_maintenance,
            maximum_verification_records: config.maximum_verification_records,
            maximum_verification_duration: config.maximum_verification_duration,
            shutdown_drain_timeout: config.shutdown_drain_timeout,
            cuda_maximum_topology_bytes: config.cuda.maximum_topology_bytes,
            cuda_maximum_concurrent_searches: config.cuda.maximum_concurrent_searches,
            cuda_maximum_batch_lanes: config.cuda.maximum_batch_lanes,
            cuda_maximum_reserved_search_bytes: config.cuda.maximum_reserved_search_bytes,
        }
    }
}

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
    pub cuda_parallel_strategy: &'static str,
    pub cuda_reset_mode: &'static str,
    pub cuda_target_mode: &'static str,
    pub cuda_profile_mode: &'static str,
    pub cuda_path_evidence_mode: &'static str,
    pub paths: bool,
    pub edge_budgets: bool,
    pub cancellation: bool,
    pub hydration: bool,
    pub subgraphs: bool,
    pub durable_routing_images: bool,
    pub checkpoint_backup: bool,
    pub validated_restore: bool,
    pub store_maintenance: bool,
    pub explicit_shutdown: bool,
    pub current_state_hydration: bool,
    pub numeric_policy_id: &'static str,
    pub tie_policy_id: &'static str,
    pub resource_limits: ResourceLimitsSnapshot,
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
            cuda_paths: true,
            cuda_finite_edge_budgets: false,
            cuda_full_residency_required: false,
            cuda_parallel_strategy: "relation-threads-atomic-binary64",
            cuda_reset_mode: "explicit-clear",
            cuda_target_mode: "sorted-sparse-host",
            cuda_profile_mode: "inline-exact",
            cuda_path_evidence_mode: "cpu-pass-same-image",
            paths: true,
            edge_budgets: true,
            cancellation: true,
            hydration: true,
            subgraphs: true,
            durable_routing_images: true,
            checkpoint_backup: true,
            validated_restore: true,
            store_maintenance: true,
            explicit_shutdown: true,
            current_state_hydration: true,
            numeric_policy_id: NUMERIC_POLICY_ID,
            tie_policy_id: TIE_POLICY_ID,
            resource_limits: ResourceLimitsSnapshot::from(&config),
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
    pub partitioned_topology: bool,
    pub device_topology_cache: Option<DeviceTopologyCacheHealth>,
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DeviceTopologyCacheHealth {
    pub capacity_bytes: usize,
    pub capacity_slots: usize,
    pub current_bytes: usize,
    pub high_water_bytes: usize,
    pub entries: usize,
    pub host_loading_entries: usize,
    pub copying_entries: usize,
    pub evicting_entries: usize,
    pub ready_entries: usize,
    pub failed_entries: usize,
    pub hits: u64,
    pub misses: u64,
    pub coalesced_waits: u64,
    pub copies: u64,
    pub evictions: u64,
    pub slot_waits: u64,
    pub completion_waits: u64,
    pub in_use_slots: usize,
    pub transfer_bytes: u64,
}

#[derive(Clone, Debug)]
pub enum RoutingHealth {
    Available,
    Unavailable(RoutingUnavailableReason),
    ShutDown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartupImageOutcome {
    ValidatedBundle,
    RebuiltFromCatalog,
    RebuildFailed,
}

#[derive(Clone, Debug)]
pub struct EngineHealth {
    pub durable_catalog_available: bool,
    pub lifecycle: LifecycleSnapshot,
    pub store: Option<StoreMetricsSnapshot>,
    pub routing: RoutingHealth,
    pub current_image_manifest: Option<RoutingImageManifest>,
    pub current_image_age: Option<Duration>,
    pub active_image_references: usize,
    pub current_bundle: Option<pathhydra_routing::BundleSnapshot>,
    pub startup_image_outcome: StartupImageOutcome,
    pub startup_image_duration: Duration,
    pub cpu_topology_mode: Option<CpuTopologyMode>,
    pub host_partition_cache: Option<pathhydra_routing::HostCacheSnapshot>,
    pub last_image_build: ImageBuildReport,
    pub last_image_corruption: Option<String>,
    pub last_cuda_degradation: Option<String>,
    pub last_cuda_recovery: Option<String>,
    pub active_routes: usize,
    pub peak_active_routes: usize,
    pub reserved_route_bytes: usize,
    pub peak_reserved_route_bytes: usize,
    pub cumulative_route_admissions: u64,
    pub cumulative_admission_rejections: u64,
    pub cumulative_cancellations: u64,
    pub cumulative_image_build_failures: u64,
    pub retired_bundles: RetirementSnapshot,
    pub cuda: CudaHealth,
}
