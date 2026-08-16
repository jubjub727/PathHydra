use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    sync::{Arc, Mutex, MutexGuard, RwLock},
    time::{Duration, Instant},
};

use pathhydra_core::{
    BaseWeight, Candidate, CandidateId, ConfirmedRecord, EdgeId, EdgeRecord, NodeId, NodeName,
    NodePayload, NodeRecord, RelationId, RelationName, RelationRecord,
};
use pathhydra_routing::{
    BundleConfig, ChunkedRoutingImage, CompletionReason, CpuSearchDiagnostics, DestinationState,
    HostCacheConfig, NumericPolicy, RelationMultiplier, RelationProfile, RelationUse,
    RoutingRequest, RoutingResponse, SearchBudget, TiePolicy, compile_bundle,
    estimate_cpu_working_set, open_bundle, route_controlled, route_partitioned_controlled,
};
use pathhydra_store::{
    ActiveRoutingImage, Catalog, CatalogConfig, CatalogSummary, CheckpointReport,
    CheckpointRequest, CompactionReport, OperationalPathRequest, PathTarget, StoreMetricsSnapshot,
    VerificationLimits, VerificationReport, validate_operational_paths,
};

#[cfg(feature = "cuda")]
use pathhydra_routing::RoutingImage;

use crate::{
    AdmissionController, CancellationOutcome, ConfirmedMutation, CpuTopology,
    CudaAlgorithmSelection, CudaAvailability, CudaConfig, CudaDeviceSummary, CudaExecutorPolicy,
    CudaHealth, CudaIneligibility, CudaRequestDiagnostics, CudaWorkerShutdownReport,
    EngineCapabilities, EngineConfigError, EngineError, EngineHealth, EngineRestoreReport,
    EngineRestoreRequest, ExecutorSelectionReason, HydratedNodeState, HydrationRequest,
    ImageBuildOutcome, ImageBuildReport, LifecycleController, PublicationFaultInjection,
    PublicationOutcome, PublicationStage, PublishedExecutionImage, RequestId, RequestRegistry,
    RetirementManager, RoutingHealth, RoutingState, RoutingUnavailableReason, ShutdownFailure,
    ShutdownFailureStage, ShutdownReport, StartupImageOutcome,
};

#[cfg(feature = "cuda")]
use crate::CudaTopology;

#[cfg(feature = "cuda")]
struct CudaRuntime {
    context: Arc<pathhydra_cuda::CudaContextOwner>,
    worker: pathhydra_cuda::CudaWorker,
    config: CudaConfig,
    admission: pathhydra_cuda::CudaAdmissionController,
    healthy: AtomicBool,
    last_failure: Mutex<Option<String>>,
}

#[cfg(feature = "cuda")]
impl CudaRuntime {
    fn initialize(config: CudaConfig) -> Result<Self, pathhydra_cuda::CudaError> {
        let context = pathhydra_cuda::CudaContextOwner::initialize(config.device_ordinal)?;
        let worker = pathhydra_cuda::CudaWorker::start(
            config.maximum_batch_lanes,
            config.batch_collection_delay,
        );
        let admission =
            pathhydra_cuda::CudaAdmissionController::new(pathhydra_cuda::CudaMemoryLimits {
                maximum_search_bytes: config.maximum_reserved_search_bytes,
                maximum_concurrent_searches: config.maximum_concurrent_searches,
            });
        Ok(Self {
            context,
            worker,
            config,
            admission,
            healthy: AtomicBool::new(true),
            last_failure: Mutex::new(None),
        })
    }

    fn upload(
        &self,
        image: Arc<RoutingImage>,
    ) -> Result<Arc<pathhydra_cuda::CudaResidentImage>, pathhydra_cuda::CudaError> {
        pathhydra_cuda::CudaResidentImage::upload(
            Arc::clone(&self.context),
            image,
            self.config.maximum_topology_bytes,
            self.config.minimum_free_memory_headroom,
        )
    }

    fn upload_topology(
        &self,
        cpu: &CpuTopology,
    ) -> Result<CudaTopology, pathhydra_cuda::CudaError> {
        match cpu {
            CpuTopology::Resident { image, .. } => {
                self.upload(Arc::clone(image)).map(CudaTopology::Resident)
            }
            CpuTopology::Partitioned(image) => pathhydra_cuda::CudaPartitionedImage::upload(
                Arc::clone(&self.context),
                Arc::clone(image),
                pathhydra_cuda::CudaPartitionedConfig {
                    maximum_topology_cache_bytes: self
                        .config
                        .maximum_partitioned_topology_cache_bytes,
                    maximum_topology_cache_slots: self
                        .config
                        .maximum_partitioned_topology_cache_slots,
                    maximum_host_staging_bytes: self.config.maximum_partitioned_host_staging_bytes,
                    minimum_free_memory_headroom: self.config.minimum_free_memory_headroom,
                    reserved_concurrent_search_bytes: self.config.maximum_reserved_search_bytes,
                    reverse_partition_order: false,
                },
            )
            .map(CudaTopology::Partitioned),
        }
    }

    fn algorithm(&self) -> pathhydra_cuda::CudaAlgorithm {
        match self.config.algorithm {
            CudaAlgorithmSelection::Frontier | CudaAlgorithmSelection::Automatic => {
                pathhydra_cuda::CudaAlgorithm::Frontier
            }
            CudaAlgorithmSelection::DeltaStepping { delta } => {
                pathhydra_cuda::CudaAlgorithm::DeltaStepping(
                    pathhydra_cuda::DeltaConfiguration::new(delta)
                        .expect("validated engine CUDA delta remains positive and finite"),
                )
            }
        }
    }

    fn failure(&self, error: &pathhydra_cuda::CudaError) {
        if error.poisons_context() {
            self.healthy.store(false, Ordering::Release);
        }
        if let Ok(mut failure) = self.last_failure.lock() {
            *failure = Some(error.to_string());
        }
    }

    fn healthy(&self) -> Result<(), CudaIneligibility> {
        if self.healthy.load(Ordering::Acquire) {
            Ok(())
        } else {
            let reason = self
                .last_failure
                .lock()
                .ok()
                .and_then(|failure| failure.clone())
                .unwrap_or_else(|| "CUDA context is poisoned".to_owned());
            Err(CudaIneligibility::Unhealthy(reason))
        }
    }
}

pub const DEFAULT_MAX_ACTIVE_IMAGE_BYTES: usize = 512 * 1024 * 1024;
pub const DEFAULT_MAX_CONCURRENT_ROUTES: usize = 8;
pub const DEFAULT_MAX_RESERVED_ROUTE_BYTES: usize = 512 * 1024 * 1024;
pub const DEFAULT_MAX_DESTINATIONS_PER_REQUEST: usize = 16_384;
pub const DEFAULT_MAX_HYDRATION_HANDLES_PER_REQUEST: usize = 65_536;
pub const DEFAULT_MAX_RESIDENT_IMAGE_METADATA_BYTES: u64 = 256 * 1024 * 1024;
pub const DEFAULT_HOST_PARTITION_CACHE_BYTES: u64 = 256 * 1024 * 1024;
pub const DEFAULT_MAX_RETIRED_BUNDLE_BYTES: u64 = 128 * 1024 * 1024 * 1024;
pub const DEFAULT_MAX_RETIRED_BUNDLE_COUNT: usize = 32;
pub const DEFAULT_MAX_CONCURRENT_CHECKPOINTS: usize = 1;
pub const DEFAULT_MAXIMUM_MAINTENANCE_WORKERS: usize = 1;
pub const DEFAULT_MAXIMUM_QUEUED_MAINTENANCE: usize = 0;
pub const DEFAULT_MAX_VERIFICATION_RECORDS: u64 = 100_000_000;
pub const DEFAULT_MAX_VERIFICATION_DURATION: Duration = Duration::from_secs(300);
pub const DEFAULT_SHUTDOWN_DRAIN_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum StartupBundlePolicy {
    #[default]
    ValidateOrRebuild,
    RequireValidBundle,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EngineConfig {
    pub max_active_image_bytes: usize,
    pub max_concurrent_routes: usize,
    pub max_reserved_route_bytes: usize,
    pub max_destinations_per_request: usize,
    pub max_hydration_handles_per_request: usize,
    pub routing_image_root: Option<PathBuf>,
    pub max_total_bundle_bytes: u64,
    pub target_partition_topology_bytes: u64,
    pub hard_maximum_partition_topology_bytes: u64,
    pub max_resident_image_metadata_bytes: u64,
    pub host_partition_cache_bytes: u64,
    pub host_partition_cache_entries: usize,
    pub routing_io_worker_count: usize,
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
    pub startup_bundle_policy: StartupBundlePolicy,
    pub cuda: CudaConfig,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            max_active_image_bytes: DEFAULT_MAX_ACTIVE_IMAGE_BYTES,
            max_concurrent_routes: DEFAULT_MAX_CONCURRENT_ROUTES,
            max_reserved_route_bytes: DEFAULT_MAX_RESERVED_ROUTE_BYTES,
            max_destinations_per_request: DEFAULT_MAX_DESTINATIONS_PER_REQUEST,
            max_hydration_handles_per_request: DEFAULT_MAX_HYDRATION_HANDLES_PER_REQUEST,
            routing_image_root: None,
            max_total_bundle_bytes: 64 * 1024 * 1024 * 1024,
            target_partition_topology_bytes: 4 * 1024 * 1024,
            hard_maximum_partition_topology_bytes: 8 * 1024 * 1024,
            max_resident_image_metadata_bytes: DEFAULT_MAX_RESIDENT_IMAGE_METADATA_BYTES,
            host_partition_cache_bytes: DEFAULT_HOST_PARTITION_CACHE_BYTES,
            host_partition_cache_entries: 64,
            routing_io_worker_count: 2,
            maximum_queued_partition_reads: 64,
            routing_io_staging_bytes: 32 * 1024 * 1024,
            maximum_retired_bundle_bytes: DEFAULT_MAX_RETIRED_BUNDLE_BYTES,
            maximum_retired_bundle_count: DEFAULT_MAX_RETIRED_BUNDLE_COUNT,
            maximum_concurrent_checkpoints: DEFAULT_MAX_CONCURRENT_CHECKPOINTS,
            maximum_maintenance_workers: DEFAULT_MAXIMUM_MAINTENANCE_WORKERS,
            maximum_queued_maintenance: DEFAULT_MAXIMUM_QUEUED_MAINTENANCE,
            maximum_verification_records: DEFAULT_MAX_VERIFICATION_RECORDS,
            maximum_verification_duration: DEFAULT_MAX_VERIFICATION_DURATION,
            shutdown_drain_timeout: DEFAULT_SHUTDOWN_DRAIN_TIMEOUT,
            startup_bundle_policy: StartupBundlePolicy::ValidateOrRebuild,
            cuda: CudaConfig::default(),
        }
    }
}

impl EngineConfig {
    pub fn validate(self) -> Result<Self, EngineConfigError> {
        for (name, value) in [
            ("max_concurrent_routes", self.max_concurrent_routes),
            ("max_reserved_route_bytes", self.max_reserved_route_bytes),
            (
                "max_destinations_per_request",
                self.max_destinations_per_request,
            ),
            (
                "max_hydration_handles_per_request",
                self.max_hydration_handles_per_request,
            ),
            (
                "host_partition_cache_entries",
                self.host_partition_cache_entries,
            ),
            ("routing_io_worker_count", self.routing_io_worker_count),
            (
                "maximum_queued_partition_reads",
                self.maximum_queued_partition_reads,
            ),
            (
                "maximum_retired_bundle_count",
                self.maximum_retired_bundle_count,
            ),
            (
                "maximum_concurrent_checkpoints",
                self.maximum_concurrent_checkpoints,
            ),
            (
                "maximum_maintenance_workers",
                self.maximum_maintenance_workers,
            ),
        ] {
            if value == 0 {
                return Err(EngineConfigError::ZeroLimit { limit: name });
            }
        }
        if self.maximum_queued_maintenance != 0 {
            return Err(EngineConfigError::InvalidMaintenanceValue {
                field: "maximum_queued_maintenance",
                reason: "the current caller-executed maintenance policy has a zero-length queue",
            });
        }
        if self.maximum_verification_records == 0 {
            return Err(EngineConfigError::ZeroLimit {
                limit: "maximum_verification_records",
            });
        }
        if self.maximum_verification_duration.is_zero() {
            return Err(EngineConfigError::ZeroLimit {
                limit: "maximum_verification_duration",
            });
        }
        if self.shutdown_drain_timeout.is_zero() {
            return Err(EngineConfigError::ZeroLimit {
                limit: "shutdown_drain_timeout",
            });
        }
        for (name, value) in [
            ("max_total_bundle_bytes", self.max_total_bundle_bytes),
            (
                "target_partition_topology_bytes",
                self.target_partition_topology_bytes,
            ),
            (
                "hard_maximum_partition_topology_bytes",
                self.hard_maximum_partition_topology_bytes,
            ),
            (
                "max_resident_image_metadata_bytes",
                self.max_resident_image_metadata_bytes,
            ),
            (
                "host_partition_cache_bytes",
                self.host_partition_cache_bytes,
            ),
            (
                "maximum_retired_bundle_bytes",
                self.maximum_retired_bundle_bytes,
            ),
            ("routing_io_staging_bytes", self.routing_io_staging_bytes),
        ] {
            if value == 0 {
                return Err(EngineConfigError::ZeroLimit { limit: name });
            }
        }
        if self.target_partition_topology_bytes > self.hard_maximum_partition_topology_bytes {
            return Err(EngineConfigError::InvalidRoutingImageValue {
                field: "target_partition_topology_bytes",
                reason: "cannot exceed hard_maximum_partition_topology_bytes",
            });
        }
        if self.cuda.maximum_topology_bytes == 0 {
            return Err(EngineConfigError::ZeroLimit {
                limit: "cuda.maximum_topology_bytes",
            });
        }
        for (name, value) in [
            (
                "cuda.maximum_partitioned_topology_cache_bytes",
                self.cuda.maximum_partitioned_topology_cache_bytes,
            ),
            (
                "cuda.maximum_partitioned_topology_cache_slots",
                self.cuda.maximum_partitioned_topology_cache_slots,
            ),
            (
                "cuda.maximum_partitioned_host_staging_bytes",
                self.cuda.maximum_partitioned_host_staging_bytes,
            ),
        ] {
            if value == 0 {
                return Err(EngineConfigError::ZeroLimit { limit: name });
            }
        }
        for (name, value) in [
            (
                "cuda.maximum_concurrent_searches",
                self.cuda.maximum_concurrent_searches,
            ),
            ("cuda.maximum_batch_lanes", self.cuda.maximum_batch_lanes),
            (
                "cuda.maximum_reserved_search_bytes",
                self.cuda.maximum_reserved_search_bytes,
            ),
        ] {
            if value == 0 {
                return Err(EngineConfigError::ZeroLimit { limit: name });
            }
        }
        if self.cuda.maximum_batch_lanes > self.cuda.maximum_concurrent_searches {
            return Err(EngineConfigError::InvalidCudaValue {
                field: "maximum_batch_lanes",
                reason: "cannot exceed maximum_concurrent_searches",
            });
        }
        let selected_delta = match self.cuda.algorithm {
            CudaAlgorithmSelection::DeltaStepping { delta } => Some(delta),
            _ => None,
        };
        if selected_delta
            .into_iter()
            .any(|delta| !delta.is_finite() || delta <= 0.0)
        {
            return Err(EngineConfigError::InvalidCudaValue {
                field: "delta",
                reason: "must be positive and finite",
            });
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Executor {
    CpuReference,
    NvidiaCuda,
}

#[derive(Clone, Debug)]
pub struct RuntimeDiagnostics {
    pub request_id: RequestId,
    pub executor: Executor,
    pub selection_reason: ExecutorSelectionReason,
    pub attempted_cuda: bool,
    pub cuda_fallback_reason: Option<CudaIneligibility>,
    pub numeric_policy: NumericPolicy,
    pub tie_policy: TiePolicy,
    pub image_node_count: usize,
    pub image_relation_kind_count: usize,
    pub image_adjacency_count: usize,
    pub reserved_working_bytes: usize,
    pub admission_duration: Duration,
    pub execution_duration: Duration,
    pub completion_reason: CompletionReason,
    pub search: CpuSearchDiagnostics,
    pub partitioned_cpu: Option<pathhydra_routing::PartitionedCpuDiagnostics>,
    pub cuda: Option<CudaRequestDiagnostics>,
}

#[derive(Clone, Debug)]
pub struct EngineRoutingResponse {
    pub response: RoutingResponse,
    pub diagnostics: RuntimeDiagnostics,
}

/// The sole owner of a durable catalog and its current published CPU image.
///
/// Open an engine and inspect concrete capabilities:
///
/// ```no_run
/// use pathhydra_engine::{EngineConfig, GraphEngine};
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let engine = GraphEngine::open("graph.db", EngineConfig::default())?;
/// assert!(engine.capabilities().cpu_reference_routing);
/// assert!(!engine.capabilities().gpu_routing);
/// # Ok(()) }
/// ```
///
/// Prefer CUDA and inspect runtime capability after opening:
///
/// ```no_run
/// use pathhydra_engine::{CudaConfig, CudaExecutorPolicy, EngineConfig, GraphEngine};
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let config = EngineConfig {
///     cuda: CudaConfig { enabled: true, executor_policy: CudaExecutorPolicy::PreferCuda, ..CudaConfig::default() },
///     ..EngineConfig::default()
/// };
/// let engine = GraphEngine::open("graph.db", config)?;
/// println!("{:?}", engine.capabilities().cuda_runtime);
/// # Ok(()) }
/// ```
///
/// An eligible unlimited-budget request preserves the common routing response.
/// CUDA supplies exact distances; when paths are requested, the same acquired
/// CPU image supplies cancellation-aware, bit-verified edge evidence:
///
/// ```no_run
/// # use pathhydra_engine::{CudaConfig, CudaExecutorPolicy, EngineConfig, GraphEngine, RequestId};
/// # use pathhydra_routing::RoutingRequest;
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// # let config = EngineConfig { cuda: CudaConfig { enabled: true, executor_policy: CudaExecutorPolicy::PreferCuda, ..CudaConfig::default() }, ..EngineConfig::default() };
/// # let engine = GraphEngine::open("graph.db", config)?;
/// # let request: RoutingRequest = unimplemented!();
/// let routed = engine.route(RequestId::new(1), &request)?;
/// println!("{:?}", routed.diagnostics.executor);
/// # Ok(()) }
/// ```
///
/// Path requests under a permissive policy execute on the CPU oracle:
///
/// ```no_run
/// # use pathhydra_engine::{CudaConfig, CudaExecutorPolicy, EngineConfig, Executor, GraphEngine, RequestId};
/// # use pathhydra_routing::RoutingRequest;
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// # let engine = GraphEngine::open("graph.db", EngineConfig { cuda: CudaConfig { enabled: true, executor_policy: CudaExecutorPolicy::PreferCuda, ..CudaConfig::default() }, ..EngineConfig::default() })?;
/// # let path_request: RoutingRequest = unimplemented!();
/// let routed = engine.route(RequestId::new(2), &path_request)?;
/// assert_eq!(routed.diagnostics.executor, Executor::CpuReference);
/// # Ok(()) }
/// ```
///
/// Required CUDA returns typed ineligibility instead of changing semantics:
///
/// ```no_run
/// # use pathhydra_engine::{CudaConfig, CudaExecutorPolicy, EngineConfig, EngineError, GraphEngine, RequestId};
/// # use pathhydra_routing::RoutingRequest;
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// # let engine = GraphEngine::open("graph.db", EngineConfig { cuda: CudaConfig { enabled: true, executor_policy: CudaExecutorPolicy::RequireCuda, ..CudaConfig::default() }, ..EngineConfig::default() })?;
/// # let unsupported_request: RoutingRequest = unimplemented!();
/// assert!(matches!(engine.route(RequestId::new(3), &unsupported_request), Err(EngineError::CudaIneligible(_))));
/// # Ok(()) }
/// ```
///
/// After device loss, permissive calls use CPU and recovery is explicit:
///
/// ```no_run
/// # use pathhydra_engine::{CudaConfig, CudaExecutorPolicy, EngineConfig, GraphEngine};
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let engine = GraphEngine::open("graph.db", EngineConfig { cuda: CudaConfig { enabled: true, executor_policy: CudaExecutorPolicy::PreferCuda, ..CudaConfig::default() }, ..EngineConfig::default() })?;
/// // After health reports a poisoned/lost context:
/// engine.reinitialize_cuda()?;
/// # Ok(()) }
/// ```
///
/// Promotion reports publication separately from its durable result:
///
/// ```no_run
/// use pathhydra_engine::{EngineConfig, GraphEngine, PublicationOutcome};
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let engine = GraphEngine::open("graph.db", EngineConfig::default())?;
/// let candidate = engine.insert_node_candidate("exact name")?;
/// let mutation = engine.confirm_validated_candidate(candidate)?;
/// assert!(matches!(mutation.publication(), PublicationOutcome::Published(_)));
/// # Ok(()) }
/// ```
///
/// Callers may signal a synchronous route from another thread:
///
/// ```no_run
/// use std::sync::Arc;
/// use pathhydra_engine::{CancellationOutcome, EngineConfig, GraphEngine, RequestId};
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let engine = Arc::new(GraphEngine::open("graph.db", EngineConfig::default())?);
/// let id = RequestId::new(42);
/// // A caller-owned worker can invoke `engine.route(id, &request)`.
/// let outcome = engine.cancel(id)?;
/// assert!(matches!(outcome, CancellationOutcome::NotActive));
/// # Ok(()) }
/// ```
///
/// Exact returned paths are hydrated from their own response evidence:
///
/// ```no_run
/// # use pathhydra_engine::{EngineConfig, GraphEngine};
/// # use pathhydra_routing::RoutingResponse;
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// # let engine = GraphEngine::open("graph.db", EngineConfig::default())?;
/// # let response: RoutingResponse = unimplemented!();
/// let path = engine.hydrate_path(&response, 0)?;
/// println!("{}", path.logical_distance);
/// # Ok(()) }
/// ```
///
/// Caller-owned subgraphs can be unioned and hydrated without store mutation:
///
/// ```no_run
/// use pathhydra_core::{EdgeId, NodeId};
/// use pathhydra_engine::{EngineConfig, GraphEngine, Subgraph};
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let engine = GraphEngine::open("graph.db", EngineConfig::default())?;
/// let mut selected = Subgraph::new();
/// selected.add_edge(EdgeId::from_u64(1), NodeId::from_u64(1), NodeId::from_u64(2))?;
/// let hydrated = engine.hydrate_subgraph(&selected, None)?;
/// assert!(!hydrated.complete); // the example handles are not confirmed
/// # Ok(()) }
/// ```
///
/// A committed mutation can explicitly report an unavailable replacement:
///
/// ```no_run
/// use pathhydra_engine::{EngineConfig, GraphEngine, PublicationOutcome};
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let config = EngineConfig { max_active_image_bytes: 8, ..EngineConfig::default() };
/// let engine = GraphEngine::open("small.db", config)?;
/// let candidate = engine.insert_node_candidate("commits before publication")?;
/// let mutation = engine.confirm_validated_candidate(candidate)?;
/// assert!(matches!(mutation.publication(), PublicationOutcome::RoutingUnavailable(_, _)));
/// # Ok(()) }
/// ```
pub struct GraphEngine {
    pub(crate) catalog: Mutex<Option<Arc<Catalog>>>,
    pub(crate) config: EngineConfig,
    pub(crate) routing: RwLock<RoutingState>,
    routing_image_root: PathBuf,
    admission: AdmissionController,
    requests: RequestRegistry,
    pub(crate) lifecycle: LifecycleController,
    retirement: RetirementManager,
    publication_faults: PublicationFaultInjection,
    startup_image_outcome: StartupImageOutcome,
    startup_image_duration: Duration,
    cancellations: AtomicU64,
    image_build_failures: AtomicU64,
    publication_active: AtomicBool,
    cuda_availability: RwLock<CudaAvailability>,
    last_image_corruption: RwLock<Option<String>>,
    last_cuda_degradation: RwLock<Option<String>>,
    last_cuda_recovery: RwLock<Option<String>>,
    cuda_uploads: AtomicU64,
    cuda_upload_failures: AtomicU64,
    cuda_fallbacks: AtomicU64,
    cuda_context_reinitializations: AtomicU64,
    #[cfg(feature = "cuda")]
    cuda_runtime: RwLock<Option<Arc<CudaRuntime>>>,
}

struct PublicationActivity<'a>(&'a AtomicBool);

impl Drop for PublicationActivity<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl GraphEngine {
    pub fn open(path: impl AsRef<Path>, config: EngineConfig) -> Result<Self, EngineError> {
        let config = config.validate().map_err(EngineError::Configuration)?;
        let database_path = if path.as_ref().is_absolute() {
            path.as_ref().to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|error| {
                    EngineError::RoutingUnavailable(RoutingUnavailableReason::Bundle(
                        error.to_string(),
                    ))
                })?
                .join(path)
        };
        let routing_image_root =
            resolve_routing_image_root(&database_path, config.routing_image_root.as_deref())?;
        let database_root = database_path.parent().ok_or(EngineError::Configuration(
            EngineConfigError::InvalidRoutingImageValue {
                field: "database_path",
                reason: "database path has no parent",
            },
        ))?;
        let routing_root = routing_image_root
            .parent()
            .ok_or(EngineError::Configuration(
                EngineConfigError::InvalidRoutingImageValue {
                    field: "routing_image_root",
                    reason: "routing-image path has no parent",
                },
            ))?;
        validate_operational_paths(&OperationalPathRequest {
            database: Some(PathTarget {
                root: database_root,
                path: &database_path,
                must_exist: false,
                must_be_fresh: false,
            }),
            routing_images: Some(PathTarget {
                root: routing_root,
                path: &routing_image_root,
                must_exist: false,
                must_be_fresh: false,
            }),
            ..OperationalPathRequest::default()
        })?;
        let catalog = Catalog::open_with_config(
            &database_path,
            CatalogConfig {
                maximum_concurrent_checkpoints: config.maximum_concurrent_checkpoints,
            },
        )?;
        fs::create_dir_all(&routing_image_root).map_err(|error| {
            EngineError::RoutingUnavailable(RoutingUnavailableReason::Bundle(error.to_string()))
        })?;
        let current_bundle_name = catalog
            .active_routing_image()?
            .map(|pointer| pointer.relative_bundle().to_owned());
        cleanup_startup_children(&routing_image_root, current_bundle_name.as_deref())?;
        let (routing, failed, startup_image_outcome, startup_image_corruption) =
            Self::initial_routing_state(&catalog, &routing_image_root, &config)?;
        let startup_image_duration = routing.last_build().duration();
        let current_bundle_name = catalog
            .active_routing_image()?
            .map(|pointer| pointer.relative_bundle().to_owned());
        cleanup_startup_children(&routing_image_root, current_bundle_name.as_deref())?;
        let initial_cuda_availability = if !config.cuda.enabled {
            CudaAvailability::Disabled
        } else if cfg!(feature = "cuda") {
            CudaAvailability::Unavailable("CUDA initialization has not completed".to_owned())
        } else {
            CudaAvailability::SupportNotCompiled
        };
        let engine = Self {
            catalog: Mutex::new(Some(Arc::new(catalog))),
            config: config.clone(),
            routing: RwLock::new(routing),
            routing_image_root: routing_image_root.clone(),
            admission: AdmissionController::new(
                config.max_concurrent_routes,
                config.max_reserved_route_bytes,
            ),
            requests: RequestRegistry::new(),
            lifecycle: LifecycleController::new(config.maximum_maintenance_workers),
            retirement: RetirementManager::new(
                routing_image_root.clone(),
                config.maximum_retired_bundle_count,
                config.maximum_retired_bundle_bytes,
            ),
            publication_faults: PublicationFaultInjection::default(),
            startup_image_outcome,
            startup_image_duration,
            cancellations: AtomicU64::new(0),
            image_build_failures: AtomicU64::new(u64::from(failed)),
            publication_active: AtomicBool::new(false),
            cuda_availability: RwLock::new(initial_cuda_availability),
            last_image_corruption: RwLock::new(startup_image_corruption),
            last_cuda_degradation: RwLock::new(None),
            last_cuda_recovery: RwLock::new(None),
            cuda_uploads: AtomicU64::new(0),
            cuda_upload_failures: AtomicU64::new(0),
            cuda_fallbacks: AtomicU64::new(0),
            cuda_context_reinitializations: AtomicU64::new(0),
            #[cfg(feature = "cuda")]
            cuda_runtime: RwLock::new(None),
        };
        if engine.config.cuda.enabled {
            engine.initialize_cuda_for_current();
        }
        Ok(engine)
    }

    fn catalog_guard(&self) -> Result<MutexGuard<'_, Option<Arc<Catalog>>>, EngineError> {
        self.catalog.lock().map_err(|_| EngineError::LockPoisoned {
            lock: "catalog ownership",
        })
    }

    pub(crate) fn with_catalog<T>(
        &self,
        operation: impl FnOnce(&Catalog) -> Result<T, EngineError>,
    ) -> Result<T, EngineError> {
        let catalog = self
            .catalog_guard()?
            .as_ref()
            .cloned()
            .ok_or(crate::LifecycleError::ShutDown)?;
        operation(&catalog)
    }

    #[must_use]
    pub fn capabilities(&self) -> EngineCapabilities {
        let availability = self.cuda_availability.read().map_or(
            CudaAvailability::Unavailable("CUDA health lock is poisoned".to_owned()),
            |value| value.clone(),
        );
        EngineCapabilities::new(
            self.config.clone(),
            availability,
            self.cuda_device_summary(),
        )
    }

    pub fn health(&self) -> Result<EngineHealth, EngineError> {
        self.retirement.reap();
        let lifecycle = self.lifecycle.snapshot()?;
        let routing = self.routing.read().map_err(|_| EngineError::LockPoisoned {
            lock: "published routing state",
        })?;
        let admission = self.admission.snapshot()?;
        let cuda = self.cuda_health(&routing)?;
        let (durable_catalog_available, store) = {
            let catalog = self.catalog_guard()?.as_ref().cloned();
            match catalog {
                Some(catalog) => (true, Some(catalog.metrics_snapshot()?)),
                None => (false, None),
            }
        };
        Ok(EngineHealth {
            durable_catalog_available,
            lifecycle,
            store,
            routing: if lifecycle.state == crate::EngineLifecycleState::ShutDown {
                RoutingHealth::ShutDown
            } else {
                routing
                    .unavailable_reason()
                    .cloned()
                    .map_or(RoutingHealth::Available, RoutingHealth::Unavailable)
            },
            current_image_manifest: routing.manifest(),
            current_image_age: routing.age(),
            active_image_references: routing.execution_lease_count(),
            current_bundle: routing
                .execution()
                .ok()
                .map(|execution| execution.cpu.bundle().snapshot()),
            startup_image_outcome: self.startup_image_outcome,
            startup_image_duration: self.startup_image_duration,
            cpu_topology_mode: routing.execution().ok().map(|image| image.cpu.mode()),
            host_partition_cache: routing
                .execution()
                .ok()
                .and_then(|image| image.cpu.host_cache()),
            last_image_build: routing.last_build().clone(),
            last_image_corruption: self
                .last_image_corruption
                .read()
                .map_err(|_| EngineError::LockPoisoned {
                    lock: "last image corruption",
                })?
                .clone(),
            last_cuda_degradation: self
                .last_cuda_degradation
                .read()
                .map_err(|_| EngineError::LockPoisoned {
                    lock: "last CUDA degradation",
                })?
                .clone(),
            last_cuda_recovery: self
                .last_cuda_recovery
                .read()
                .map_err(|_| EngineError::LockPoisoned {
                    lock: "last CUDA recovery",
                })?
                .clone(),
            active_routes: admission.active,
            peak_active_routes: admission.peak_active,
            reserved_route_bytes: admission.reserved,
            peak_reserved_route_bytes: admission.peak_reserved,
            cumulative_route_admissions: admission.admissions,
            cumulative_admission_rejections: admission.rejections,
            cumulative_cancellations: self.cancellations.load(Ordering::Relaxed),
            cumulative_image_build_failures: self.image_build_failures.load(Ordering::Relaxed),
            retired_bundles: self.retirement.snapshot(),
            cuda,
        })
    }

    pub fn lifecycle_snapshot(&self) -> Result<crate::LifecycleSnapshot, EngineError> {
        Ok(self.lifecycle.snapshot()?)
    }

    #[must_use]
    pub fn publication_in_progress(&self) -> bool {
        self.publication_active.load(Ordering::Acquire)
    }

    pub fn catalog_summary(&self) -> Result<CatalogSummary, EngineError> {
        self.with_catalog(|catalog| Ok(catalog.summary()?))
    }

    pub fn verify_catalog(
        &self,
        limits: VerificationLimits,
    ) -> Result<VerificationReport, EngineError> {
        let _operation = self.lifecycle.begin_maintenance()?;
        self.with_catalog(|catalog| Ok(catalog.verify(self.bounded_verification_limits(limits))?))
    }

    pub fn store_metrics(&self) -> Result<StoreMetricsSnapshot, EngineError> {
        self.with_catalog(|catalog| Ok(catalog.metrics_snapshot()?))
    }

    pub fn create_checkpoint(
        &self,
        request: &CheckpointRequest,
    ) -> Result<CheckpointReport, EngineError> {
        let _operation = self.lifecycle.begin_checkpoint()?;
        self.with_catalog(|catalog| Ok(catalog.create_checkpoint(request)?))
    }

    pub fn compact_store(&self) -> Result<CompactionReport, EngineError> {
        let _operation = self.lifecycle.begin_maintenance()?;
        self.with_catalog(|catalog| Ok(catalog.compact_all()?))
    }

    /// Restores a database-only checkpoint into a fresh destination, opens the
    /// restored catalog through the production engine path, rebuilds routing,
    /// performs aggregate smoke verification, and explicitly shuts the
    /// temporary engine down. Caller-controlled cutover remains separate.
    pub fn restore_checkpoint(
        request: &EngineRestoreRequest,
    ) -> Result<EngineRestoreReport, EngineError> {
        let mut store_request = request.store.clone();
        store_request.verification_limits = bounded_verification_limits(
            store_request.verification_limits,
            request.engine_config.maximum_verification_records,
            request.engine_config.maximum_verification_duration,
        );
        let store = pathhydra_store::restore_checkpoint(&store_request)?;
        let engine = Self::open(&request.store.destination, request.engine_config.clone())?;
        let summary = engine.catalog_summary()?;
        let health = engine.health()?;
        let restored_summary = &store.restored_catalog.summary;
        let smoke_catalog_verified = summary.candidates == restored_summary.candidates
            && summary.confirmed_nodes == restored_summary.confirmed_nodes
            && summary.relation_kinds == restored_summary.relation_kinds
            && summary.confirmed_edges == restored_summary.confirmed_edges
            && summary.node_name_entries == restored_summary.node_name_entries
            && summary.relation_name_entries == restored_summary.relation_name_entries
            && summary.outgoing_entries == restored_summary.outgoing_entries
            && summary.incoming_entries == restored_summary.incoming_entries
            && matches!(health.routing, RoutingHealth::Available);
        let records = engine.with_catalog(|catalog| Ok(catalog.confirmed_graph_records()?))?;
        let profile = RelationProfile::new(records.relation_kinds().iter().map(|relation| {
            (
                relation.id(),
                RelationUse::Enabled(RelationMultiplier::new(1.0).expect("one is valid")),
            )
        }));
        let (smoke_route_verified, smoke_hydration_verified) =
            if let Some(node) = records.nodes().first() {
                let routed = engine.route(
                    RequestId::new(u64::MAX - 1),
                    &RoutingRequest::new(
                        node.id(),
                        [node.id()],
                        profile.clone(),
                        true,
                        SearchBudget::Unlimited,
                        TiePolicy::StablePredecessor,
                    ),
                )?;
                let route_ok = matches!(
                    routed.response.results().first().map(|result| result.state()),
                    Some(DestinationState::Exact(exact))
                        if exact.logical_distance().to_bits() == 0.0_f64.to_bits()
                );
                let hydrated = engine.hydrate(&HydrationRequest::new(
                    [node.id()],
                    records.edges().first().map(EdgeRecord::id),
                    Some(profile),
                ))?;
                let hydration_ok = matches!(
                    hydrated.nodes.first().map(|result| &result.state),
                    Some(HydratedNodeState::Found(record)) if record.id() == node.id()
                );
                (route_ok, hydration_ok)
            } else {
                (true, true)
            };
        let cuda_initialized = !request.engine_config.cuda.enabled
            || matches!(health.cuda.availability, CudaAvailability::Available);
        let routing_image = health.last_image_build;
        let shutdown = engine.shutdown()?;
        if !smoke_catalog_verified {
            return Err(EngineError::RestoreSmokeFailed("catalog"));
        }
        if !smoke_route_verified {
            return Err(EngineError::RestoreSmokeFailed("route"));
        }
        if !smoke_hydration_verified {
            return Err(EngineError::RestoreSmokeFailed("hydration"));
        }
        if !cuda_initialized {
            return Err(EngineError::RestoreSmokeFailed("CUDA initialization"));
        }
        if !shutdown.complete() {
            return Err(EngineError::RestoreSmokeFailed("shutdown"));
        }
        Ok(EngineRestoreReport {
            store,
            routing_image,
            smoke_catalog_verified,
            smoke_route_verified,
            smoke_hydration_verified,
            cuda_initialized,
            shutdown,
        })
    }

    /// Closes admission, cancels and drains active work, joins owned workers,
    /// flushes RocksDB, and releases the catalog handle. A timed-out or failed
    /// call leaves the engine closing; calling `shutdown` again safely retries.
    pub fn shutdown(&self) -> Result<ShutdownReport, EngineError> {
        let started = Instant::now();
        let before = self.lifecycle.snapshot()?;
        if before.state == crate::EngineLifecycleState::ShutDown {
            return Ok(ShutdownReport {
                state_before: before.state,
                state_after: before.state,
                already_shut_down: true,
                active_before: before.active,
                drain: crate::DrainOutcome {
                    drained: true,
                    remaining: before.active,
                    duration: Duration::ZERO,
                },
                drained: crate::ActiveOperationCounts::default(),
                active_requests_signalled: 0,
                newly_signalled_requests: 0,
                partition_io_stopped: false,
                cuda_worker: None,
                store_flush_duration: None,
                retired_bundles: self.retirement.snapshot(),
                duration: started.elapsed(),
                failures: Vec::new(),
            });
        }

        self.lifecycle.close_admission()?;
        let shutdown_active_before = self
            .lifecycle
            .snapshot()?
            .shutdown_active_before
            .unwrap_or(before.active);
        let cancelled = self.requests.cancel_all()?;
        let drain = self
            .lifecycle
            .wait_for_drain(self.config.shutdown_drain_timeout)?;
        if !drain.drained {
            return Ok(ShutdownReport {
                state_before: before.state,
                state_after: crate::EngineLifecycleState::Closing,
                already_shut_down: false,
                active_before: shutdown_active_before,
                drain,
                drained: drained_operations(shutdown_active_before, drain.remaining),
                active_requests_signalled: cancelled.active,
                newly_signalled_requests: cancelled.newly_signalled,
                partition_io_stopped: false,
                cuda_worker: None,
                store_flush_duration: None,
                retired_bundles: self.retirement.snapshot(),
                duration: started.elapsed(),
                failures: vec![ShutdownFailure {
                    stage: ShutdownFailureStage::Drain,
                    category: "drain-timeout",
                }],
            });
        }

        let partition_io_shutdown = {
            let mut state = self
                .routing
                .write()
                .map_err(|_| EngineError::LockPoisoned {
                    lock: "published routing state",
                })?;
            let last_build = state.last_build().clone();
            let previous = std::mem::replace(&mut *state, RoutingState::ShutDown { last_build });
            let stopped = previous
                .execution()
                .map_or(Ok(false), |execution| execution.cpu.shutdown_io());
            drop(previous);
            stopped
        };

        #[cfg(feature = "cuda")]
        let cuda_worker = {
            let mut slot = self
                .cuda_runtime
                .write()
                .map_err(|_| EngineError::LockPoisoned {
                    lock: "CUDA runtime",
                })?;
            slot.take().map(|runtime| match Arc::try_unwrap(runtime) {
                Ok(mut runtime) => {
                    let report = runtime.worker.shutdown();
                    CudaWorkerShutdownReport {
                        queued_at_request: report.queued_at_request,
                        active_at_request: report.active_at_request,
                        queued_routes_rejected: report.queued_routes_rejected,
                        joined: report.joined,
                    }
                }
                Err(runtime) => {
                    *slot = Some(runtime);
                    CudaWorkerShutdownReport {
                        joined: false,
                        ..CudaWorkerShutdownReport::default()
                    }
                }
            })
        };
        #[cfg(not(feature = "cuda"))]
        let cuda_worker: Option<CudaWorkerShutdownReport> = None;

        let mut failures = Vec::new();
        let partition_io_stopped = match partition_io_shutdown {
            Ok(stopped) => stopped,
            Err(_) => {
                failures.push(ShutdownFailure {
                    stage: ShutdownFailureStage::RoutingIo,
                    category: "worker-join-failed",
                });
                false
            }
        };
        if cuda_worker.as_ref().is_some_and(|report| !report.joined) {
            failures.push(ShutdownFailure {
                stage: ShutdownFailureStage::CudaWorker,
                category: "worker-reference-still-active",
            });
        }

        let store_flush_duration = {
            let mut catalog = self.catalog_guard()?;
            match catalog.take() {
                None => None,
                Some(value) => match Arc::try_unwrap(value) {
                    Err(value) => {
                        *catalog = Some(value);
                        failures.push(ShutdownFailure {
                            stage: ShutdownFailureStage::StoreHandle,
                            category: "catalog-reference-still-active",
                        });
                        None
                    }
                    Ok(value) => match value.prepare_for_close() {
                        Ok(report) => {
                            let duration = report.duration;
                            drop(value);
                            Some(duration)
                        }
                        Err(_) => {
                            *catalog = Some(Arc::new(value));
                            failures.push(ShutdownFailure {
                                stage: ShutdownFailureStage::StoreFlush,
                                category: "durability-flush-failed",
                            });
                            None
                        }
                    },
                },
            }
        };

        self.retirement.reap();
        if failures.is_empty() {
            self.lifecycle.mark_shut_down()?;
        }
        let after = self.lifecycle.snapshot()?.state;
        Ok(ShutdownReport {
            state_before: before.state,
            state_after: after,
            already_shut_down: false,
            active_before: shutdown_active_before,
            drain,
            drained: drained_operations(shutdown_active_before, drain.remaining),
            active_requests_signalled: cancelled.active,
            newly_signalled_requests: cancelled.newly_signalled,
            partition_io_stopped,
            cuda_worker,
            store_flush_duration,
            retired_bundles: self.retirement.snapshot(),
            duration: started.elapsed(),
            failures,
        })
    }

    fn bounded_verification_limits(&self, requested: VerificationLimits) -> VerificationLimits {
        bounded_verification_limits(
            requested,
            self.config.maximum_verification_records,
            self.config.maximum_verification_duration,
        )
    }

    pub fn rebuild_routing_image(&self) -> Result<ImageBuildReport, EngineError> {
        let _operation = self.lifecycle.begin_maintenance()?;
        let mut state = self
            .routing
            .write()
            .map_err(|_| EngineError::LockPoisoned {
                lock: "published routing state",
            })?;
        self.publication_active.store(true, Ordering::Release);
        let _publication_activity = PublicationActivity(&self.publication_active);
        self.await_retirement_capacity(&state);
        let (result, report) = self.compile_current_image();
        match result {
            Ok(image) => {
                let execution = self.execution_image(image);
                retire_available_state(&self.retirement, &state, execution.cpu.bundle());
                *state = RoutingState::Available {
                    image: Arc::new(execution),
                    published_at: Instant::now(),
                    last_build: report.clone(),
                }
            }
            Err(reason) => {
                saturating_increment(&self.image_build_failures);
                match &mut *state {
                    RoutingState::Available { last_build, .. } => *last_build = report.clone(),
                    RoutingState::Unavailable {
                        reason: old_reason,
                        last_build,
                    } => {
                        *old_reason = reason;
                        *last_build = report.clone();
                    }
                    RoutingState::ShutDown { last_build } => *last_build = report.clone(),
                }
            }
        }
        Ok(report)
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn publication_fault_injection(&self) -> &PublicationFaultInjection {
        &self.publication_faults
    }

    #[doc(hidden)]
    pub fn routing_bundle_fault_injection(
        &self,
    ) -> Result<Arc<pathhydra_routing::BundleFaultInjection>, EngineError> {
        let execution = self
            .routing
            .read()
            .map_err(|_| EngineError::LockPoisoned {
                lock: "published routing state",
            })?
            .execution()
            .map_err(EngineError::RoutingUnavailable)?;
        Ok(execution.cpu.bundle().fault_injection())
    }

    #[cfg(feature = "cuda")]
    #[doc(hidden)]
    pub fn cuda_fault_injection(
        &self,
    ) -> Result<Arc<pathhydra_cuda::CudaFaultInjection>, EngineError> {
        let execution = self
            .routing
            .read()
            .map_err(|_| EngineError::LockPoisoned {
                lock: "published routing state",
            })?
            .execution()
            .map_err(EngineError::RoutingUnavailable)?;
        match execution.cuda.as_ref() {
            Some(CudaTopology::Partitioned(image)) => Ok(image.fault_injection()),
            Some(CudaTopology::Resident(image)) => Ok(image.fault_injection()),
            None => Err(EngineError::CudaIneligible(
                CudaIneligibility::NoResidentImage("CUDA image is unavailable".to_owned()),
            )),
        }
    }

    /// Re-uploads only the current CPU image to the existing healthy CUDA context.
    pub fn rebuild_cuda_residency(&self) -> Result<(), EngineError> {
        let _operation = self.lifecycle.begin_maintenance()?;
        if !self.config.cuda.enabled {
            return Err(EngineError::CudaIneligible(CudaIneligibility::Disabled));
        }
        #[cfg(feature = "cuda")]
        {
            let mut state = self
                .routing
                .write()
                .map_err(|_| EngineError::LockPoisoned {
                    lock: "published routing state",
                })?;
            let cpu = state
                .execution()
                .map_err(EngineError::RoutingUnavailable)?
                .cpu
                .clone();
            let runtime = self
                .cuda_runtime
                .read()
                .map_err(|_| EngineError::LockPoisoned {
                    lock: "CUDA runtime",
                })?
                .clone()
                .ok_or_else(|| {
                    EngineError::CudaIneligible(CudaIneligibility::NoResidentImage(
                        "CUDA runtime is absent; use reinitialize_cuda".to_owned(),
                    ))
                })?;
            let last_build = state.last_build().clone();
            let cuda = match runtime.upload_topology(&cpu) {
                Ok(cuda) => cuda,
                Err(error) => {
                    let reason = CudaAvailability::Degraded(error.to_string());
                    *state = RoutingState::Available {
                        image: Arc::new(PublishedExecutionImage::cpu_only(cpu, reason.clone())),
                        published_at: Instant::now(),
                        last_build,
                    };
                    self.set_cuda_availability(reason);
                    saturating_increment(&self.cuda_upload_failures);
                    return Err(EngineError::CudaFailure(error.to_string()));
                }
            };
            *state = RoutingState::Available {
                image: Arc::new(PublishedExecutionImage::with_cuda(cpu, cuda)),
                published_at: Instant::now(),
                last_build,
            };
            self.set_cuda_availability(CudaAvailability::Available);
            saturating_increment(&self.cuda_uploads);
            Ok(())
        }
        #[cfg(not(feature = "cuda"))]
        {
            Err(EngineError::CudaIneligible(
                CudaIneligibility::SupportNotCompiled,
            ))
        }
    }

    /// Creates a fresh CUDA context and restores residency from the current CPU image.
    pub fn reinitialize_cuda(&self) -> Result<(), EngineError> {
        let _operation = self.lifecycle.begin_maintenance()?;
        if !self.config.cuda.enabled {
            return Err(EngineError::CudaIneligible(CudaIneligibility::Disabled));
        }
        #[cfg(feature = "cuda")]
        {
            let runtime = Arc::new(
                CudaRuntime::initialize(self.config.cuda)
                    .map_err(|error| EngineError::CudaFailure(error.to_string()))?,
            );
            let mut state = self
                .routing
                .write()
                .map_err(|_| EngineError::LockPoisoned {
                    lock: "published routing state",
                })?;
            let cpu = state
                .execution()
                .map_err(EngineError::RoutingUnavailable)?
                .cpu
                .clone();
            let last_build = state.last_build().clone();
            let cuda = match runtime.upload_topology(&cpu) {
                Ok(cuda) => cuda,
                Err(error) => {
                    let reason = CudaAvailability::Degraded(error.to_string());
                    *state = RoutingState::Available {
                        image: Arc::new(PublishedExecutionImage::cpu_only(cpu, reason.clone())),
                        published_at: Instant::now(),
                        last_build,
                    };
                    self.set_cuda_availability(reason);
                    saturating_increment(&self.cuda_upload_failures);
                    return Err(EngineError::CudaFailure(error.to_string()));
                }
            };
            {
                let mut slot =
                    self.cuda_runtime
                        .write()
                        .map_err(|_| EngineError::LockPoisoned {
                            lock: "CUDA runtime",
                        })?;
                *slot = Some(runtime);
            }
            *state = RoutingState::Available {
                image: Arc::new(PublishedExecutionImage::with_cuda(cpu, cuda)),
                published_at: Instant::now(),
                last_build,
            };
            self.set_cuda_availability(CudaAvailability::Available);
            saturating_increment(&self.cuda_uploads);
            saturating_increment(&self.cuda_context_reinitializations);
            Ok(())
        }
        #[cfg(not(feature = "cuda"))]
        {
            Err(EngineError::CudaIneligible(
                CudaIneligibility::SupportNotCompiled,
            ))
        }
    }

    pub fn route(
        &self,
        request_id: RequestId,
        request: &RoutingRequest,
    ) -> Result<EngineRoutingResponse, EngineError> {
        let _operation = self.lifecycle.begin_route()?;
        if request.destinations().len() > self.config.max_destinations_per_request {
            return Err(self.admission.reject_destinations(
                request.destinations().len(),
                self.config.max_destinations_per_request,
            )?);
        }
        let registration = self.requests.register(request_id)?;
        let admission_started = Instant::now();
        let mut permit = self.admission.acquire_slot()?;
        let execution = {
            let state = self.routing.read().map_err(|_| EngineError::LockPoisoned {
                lock: "published routing state",
            })?;
            state.execution().map_err(EngineError::RoutingUnavailable)?
        };
        let manifest = execution.cpu.manifest();
        let policy = self.config.cuda.executor_policy;
        let cuda_shape_refusal = if !self.config.cuda.enabled {
            Some(CudaIneligibility::Disabled)
        } else if request.budget() != pathhydra_routing::SearchBudget::Unlimited {
            Some(CudaIneligibility::FiniteEdgeBudgetUnsupportedByCuda)
        } else {
            None
        };
        let mut fallback_reason = cuda_shape_refusal;
        let try_cuda = match policy {
            CudaExecutorPolicy::CpuOnly => false,
            CudaExecutorPolicy::Auto => {
                fallback_reason = Some(CudaIneligibility::AutomaticPolicySelectedCpu);
                false
            }
            CudaExecutorPolicy::PreferCuda | CudaExecutorPolicy::RequireCuda => {
                fallback_reason.is_none()
            }
        };

        if try_cuda {
            #[cfg(feature = "cuda")]
            'cuda_attempt: {
                let cuda_topology = execution.cuda.clone().ok_or_else(|| {
                    CudaIneligibility::NoResidentImage(format!(
                        "{:?}",
                        execution.cuda_unavailable_reason
                    ))
                });
                let runtime = self
                    .cuda_runtime
                    .read()
                    .map_err(|_| EngineError::LockPoisoned {
                        lock: "CUDA runtime",
                    })?
                    .clone()
                    .ok_or_else(|| {
                        CudaIneligibility::NoResidentImage("runtime is absent".to_owned())
                    });
                match cuda_topology
                    .and_then(|cuda_topology| runtime.map(|runtime| (cuda_topology, runtime)))
                {
                    Ok((cuda_topology, runtime)) => {
                        if let Err(reason) = runtime.healthy() {
                            if policy == CudaExecutorPolicy::RequireCuda {
                                return Err(EngineError::CudaIneligible(reason));
                            }
                            fallback_reason = Some(reason);
                            break 'cuda_attempt;
                        }
                        let algorithm = runtime.algorithm();
                        let cpu_evidence_working_bytes = if request.return_paths() {
                            Some(match &execution.cpu {
                                CpuTopology::Resident { image, .. } => {
                                    estimate_cpu_working_set(image, request)?.bytes()
                                }
                                CpuTopology::Partitioned(image) => {
                                    image.estimate_working_set(request)?.bytes()
                                }
                            })
                        } else {
                            None
                        };
                        let search_bytes = pathhydra_cuda::estimate_search_bytes_for_request(
                            manifest.node_count(),
                            manifest.relation_kind_count(),
                            manifest.adjacency_count(),
                            request.destinations().len(),
                            algorithm,
                            cpu_evidence_working_bytes,
                        )
                        .map_err(|error| {
                            EngineError::CudaIneligible(CudaIneligibility::ResourceRefusal(
                                error.to_string(),
                            ))
                        })?;
                        let reservation = runtime.admission.reserve(search_bytes);
                        match reservation {
                            Ok(reservation) => {
                                let admission_duration = admission_started.elapsed();
                                let execution_started = Instant::now();
                                let routed = match &cuda_topology {
                                    CudaTopology::Resident(image) => runtime.worker.submit(
                                        Arc::clone(image),
                                        request,
                                        algorithm,
                                        registration.flag_arc(),
                                        reservation.bytes(),
                                    ),
                                    CudaTopology::Partitioned(image) => {
                                        runtime.worker.submit_partitioned(
                                            Arc::clone(image),
                                            request,
                                            algorithm,
                                            registration.flag_arc(),
                                            reservation.bytes(),
                                        )
                                    }
                                };
                                match routed {
                                    Ok(output) => {
                                        let execution_duration = execution_started.elapsed();
                                        let device = runtime.context.capabilities();
                                        let algorithm_name = match algorithm {
                                            pathhydra_cuda::CudaAlgorithm::Frontier => "frontier",
                                            pathhydra_cuda::CudaAlgorithm::DeltaStepping(_) => {
                                                "delta-stepping"
                                            }
                                        };
                                        let delta = match algorithm {
                                            pathhydra_cuda::CudaAlgorithm::Frontier => None,
                                            pathhydra_cuda::CudaAlgorithm::DeltaStepping(delta) => {
                                                Some(delta.delta())
                                            }
                                        };
                                        let cuda = output.diagnostics;
                                        let diagnostics = RuntimeDiagnostics {
                                            request_id,
                                            executor: Executor::NvidiaCuda,
                                            selection_reason: if policy
                                                == CudaExecutorPolicy::RequireCuda
                                            {
                                                ExecutorSelectionReason::RequiredCudaEligible
                                            } else {
                                                ExecutorSelectionReason::PreferredCudaEligible
                                            },
                                            attempted_cuda: true,
                                            cuda_fallback_reason: None,
                                            numeric_policy: output.response.numeric_policy(),
                                            tie_policy: output.response.tie_policy(),
                                            image_node_count: manifest.node_count(),
                                            image_relation_kind_count: manifest
                                                .relation_kind_count(),
                                            image_adjacency_count: manifest.adjacency_count(),
                                            reserved_working_bytes: reservation.bytes(),
                                            admission_duration,
                                            execution_duration,
                                            completion_reason: output.response.completion_reason(),
                                            search: CpuSearchDiagnostics::default(),
                                            partitioned_cpu: None,
                                            cuda: Some(CudaRequestDiagnostics {
                                                algorithm: algorithm_name,
                                                delta,
                                                device_ordinal: device.device_ordinal,
                                                device_name: device.device_name.clone(),
                                                queue_duration: cuda.queue_duration,
                                                batch_collection_duration: cuda
                                                    .batch_collection_duration,
                                                batch_width: cuda.batch_width,
                                                lane_index: cuda.lane_index,
                                                topology_bytes: cuda_topology.allocated_bytes(),
                                                search_bytes: cuda.reserved_search_bytes,
                                                host_to_device_bytes: cuda.host_to_device_bytes,
                                                device_to_host_bytes: cuda.device_to_host_bytes,
                                                kernel_launches: cuda.kernel_launches,
                                                synchronized_execution_duration: cuda
                                                    .synchronized_execution_duration,
                                                examined_edges: cuda.examined_edges,
                                                relaxation_attempts: cuda.relaxation_attempts,
                                                relaxation_updates: cuda.relaxation_updates,
                                                phases: cuda.phases,
                                                partitions_required: cuda.partitions_required,
                                                host_cache_hits: cuda.host_cache_hits,
                                                device_cache_hits: cuda.device_cache_hits,
                                                file_bytes: cuda.file_bytes,
                                                staged_bytes: cuda.staged_bytes,
                                                transfer_bytes: cuda.transfer_bytes,
                                                parallel_strategy: "relation-threads-atomic-binary64",
                                                reset_mode: "explicit-clear",
                                                target_mode: "sorted-sparse-host",
                                                profile_mode: "inline-exact",
                                                path_evidence_mode: if request.return_paths() {
                                                    "cpu-pass-same-image"
                                                } else {
                                                    "not-requested"
                                                },
                                                state_initialization_duration: cuda
                                                    .state_initialization_duration,
                                                partition_scheduling_duration: cuda
                                                    .partition_scheduling_duration,
                                                relation_relaxation_duration: cuda
                                                    .relation_relaxation_duration,
                                                response_transfer_duration: cuda
                                                    .response_transfer_duration,
                                                frontier_compaction_duration: cuda
                                                    .frontier_compaction_duration,
                                                compacted_task_count: cuda.compacted_task_count,
                                                destination_completion_duration: cuda
                                                    .destination_completion_duration,
                                                destination_count_checked: cuda
                                                    .destination_count_checked,
                                                atomic_cas_retries: cuda.atomic_cas_retries,
                                            }),
                                        };
                                        return Ok(EngineRoutingResponse {
                                            response: output.response,
                                            diagnostics,
                                        });
                                    }
                                    Err(error) => {
                                        runtime.failure(&error);
                                        if error.poisons_context() {
                                            self.set_cuda_availability(CudaAvailability::Degraded(
                                                error.to_string(),
                                            ));
                                        }
                                        if error.kind()
                                            == pathhydra_cuda::CudaFailureKind::ImageAccess
                                        {
                                            self.record_image_corruption(error.to_string());
                                            let _ = self.rebuild_routing_image();
                                            return Err(EngineError::RoutingUnavailable(
                                                RoutingUnavailableReason::Bundle(error.to_string()),
                                            ));
                                        }
                                        if policy == CudaExecutorPolicy::RequireCuda {
                                            return Err(EngineError::CudaFailure(
                                                error.to_string(),
                                            ));
                                        }
                                        fallback_reason =
                                            Some(CudaIneligibility::Unhealthy(error.to_string()));
                                    }
                                }
                            }
                            Err(error) => {
                                if policy == CudaExecutorPolicy::RequireCuda {
                                    return Err(EngineError::CudaIneligible(
                                        CudaIneligibility::ResourceRefusal(error.to_string()),
                                    ));
                                }
                                fallback_reason =
                                    Some(CudaIneligibility::ResourceRefusal(error.to_string()));
                            }
                        }
                    }
                    Err(reason) => {
                        if policy == CudaExecutorPolicy::RequireCuda {
                            return Err(EngineError::CudaIneligible(reason));
                        }
                        fallback_reason = Some(reason);
                    }
                }
            }
            #[cfg(not(feature = "cuda"))]
            {
                if policy == CudaExecutorPolicy::RequireCuda {
                    return Err(EngineError::CudaIneligible(
                        CudaIneligibility::SupportNotCompiled,
                    ));
                }
                fallback_reason = Some(CudaIneligibility::SupportNotCompiled);
            }
        } else if policy == CudaExecutorPolicy::RequireCuda {
            return Err(EngineError::CudaIneligible(
                fallback_reason.unwrap_or(CudaIneligibility::Disabled),
            ));
        }

        if fallback_reason.is_some() && policy != CudaExecutorPolicy::CpuOnly {
            saturating_increment(&self.cuda_fallbacks);
        }
        let estimate = match &execution.cpu {
            CpuTopology::Resident { image, .. } => estimate_cpu_working_set(image, request)?,
            CpuTopology::Partitioned(image) => image.estimate_working_set(request)?,
        };
        permit.reserve(estimate.bytes())?;
        let admission_duration = admission_started.elapsed();
        let execution_started = Instant::now();
        let routed = match &execution.cpu {
            CpuTopology::Resident { image, .. } => {
                route_controlled(image, request, registration.flag())
                    .map(|(response, search)| (response, search, None))
            }
            CpuTopology::Partitioned(image) => {
                route_partitioned_controlled(image, request, registration.flag())
                    .map(|(response, search, diagnostics)| (response, search, Some(diagnostics)))
            }
        };
        let (response, search, partitioned_cpu) = match routed {
            Ok(result) => result,
            Err(pathhydra_routing::RoutingError::ImageAccess(reason)) => {
                self.record_image_corruption(reason.clone());
                let _ = self.rebuild_routing_image();
                return Err(EngineError::RoutingUnavailable(
                    RoutingUnavailableReason::Bundle(reason),
                ));
            }
            Err(error) => return Err(error.into()),
        };
        let execution_duration = execution_started.elapsed();
        let diagnostics = RuntimeDiagnostics {
            request_id,
            executor: Executor::CpuReference,
            selection_reason: if policy == CudaExecutorPolicy::Auto {
                ExecutorSelectionReason::AutomaticPolicySelectedCpu
            } else if fallback_reason.is_some() {
                ExecutorSelectionReason::CpuFallback
            } else {
                ExecutorSelectionReason::CpuOnlyPolicy
            },
            attempted_cuda: try_cuda,
            cuda_fallback_reason: fallback_reason,
            numeric_policy: response.numeric_policy(),
            tie_policy: response.tie_policy(),
            image_node_count: manifest.node_count(),
            image_relation_kind_count: manifest.relation_kind_count(),
            image_adjacency_count: manifest.adjacency_count(),
            reserved_working_bytes: estimate.bytes(),
            admission_duration,
            execution_duration,
            completion_reason: response.completion_reason(),
            search,
            partitioned_cpu,
            cuda: None,
        };
        Ok(EngineRoutingResponse {
            response,
            diagnostics,
        })
    }

    pub fn cancel(&self, request_id: RequestId) -> Result<CancellationOutcome, EngineError> {
        let outcome = self.requests.cancel(request_id)?;
        if outcome == CancellationOutcome::Signalled {
            saturating_increment(&self.cancellations);
        }
        Ok(outcome)
    }

    pub fn insert_node_candidate(
        &self,
        name: impl Into<NodeName>,
    ) -> Result<CandidateId, EngineError> {
        let _operation = self.lifecycle.begin_mutation()?;
        self.with_catalog(|catalog| Ok(catalog.insert_node_candidate(name)?))
    }
    pub fn insert_node_candidate_with_payload(
        &self,
        name: impl Into<NodeName>,
        payload: impl Into<NodePayload>,
    ) -> Result<CandidateId, EngineError> {
        let _operation = self.lifecycle.begin_mutation()?;
        self.with_catalog(|catalog| Ok(catalog.insert_node_candidate_with_payload(name, payload)?))
    }
    pub fn insert_relation_candidate(
        &self,
        name: impl Into<RelationName>,
    ) -> Result<CandidateId, EngineError> {
        let _operation = self.lifecycle.begin_mutation()?;
        self.with_catalog(|catalog| Ok(catalog.insert_relation_candidate(name)?))
    }
    pub fn insert_edge_candidate(
        &self,
        source: NodeId,
        destination: NodeId,
        relation_kind: RelationId,
        base_weight: f32,
    ) -> Result<CandidateId, EngineError> {
        let _operation = self.lifecycle.begin_mutation()?;
        self.with_catalog(|catalog| {
            Ok(catalog.insert_edge_candidate(source, destination, relation_kind, base_weight)?)
        })
    }
    pub fn insert_edge_candidate_with_base_weight(
        &self,
        source: NodeId,
        destination: NodeId,
        relation_kind: RelationId,
        base_weight: BaseWeight,
    ) -> Result<CandidateId, EngineError> {
        let _operation = self.lifecycle.begin_mutation()?;
        self.with_catalog(|catalog| {
            Ok(catalog.insert_edge_candidate_with_base_weight(
                source,
                destination,
                relation_kind,
                base_weight,
            )?)
        })
    }
    pub fn get_candidate(&self, id: CandidateId) -> Result<Candidate, EngineError> {
        self.with_catalog(|catalog| Ok(catalog.get_candidate(id)?))
    }
    pub fn lookup_node_exact(&self, name: &str) -> Result<Option<NodeId>, EngineError> {
        self.with_catalog(|catalog| Ok(catalog.lookup_node_exact(name)?))
    }
    pub fn lookup_relation_exact(&self, name: &str) -> Result<Option<RelationId>, EngineError> {
        self.with_catalog(|catalog| Ok(catalog.lookup_relation_exact(name)?))
    }
    pub fn get_node(&self, id: NodeId) -> Result<NodeRecord, EngineError> {
        self.with_catalog(|catalog| Ok(catalog.get_node(id)?))
    }
    pub fn get_relation(&self, id: RelationId) -> Result<RelationRecord, EngineError> {
        self.with_catalog(|catalog| Ok(catalog.get_relation(id)?))
    }
    pub fn get_edge(&self, id: EdgeId) -> Result<EdgeRecord, EngineError> {
        self.with_catalog(|catalog| Ok(catalog.get_edge(id)?))
    }

    pub fn confirm_validated_candidate(
        &self,
        id: CandidateId,
    ) -> Result<ConfirmedMutation<ConfirmedRecord>, EngineError> {
        self.confirmed_mutation(|catalog| catalog.confirm_validated_candidate(id))
    }
    pub fn remove_edge(&self, id: EdgeId) -> Result<ConfirmedMutation<()>, EngineError> {
        self.confirmed_mutation(|catalog| catalog.remove_edge(id))
    }
    pub fn remove_node(&self, id: NodeId) -> Result<ConfirmedMutation<()>, EngineError> {
        self.confirmed_mutation(|catalog| catalog.remove_node(id))
    }

    fn confirmed_mutation<T>(
        &self,
        mutation: impl FnOnce(&Catalog) -> Result<T, pathhydra_store::CatalogError>,
    ) -> Result<ConfirmedMutation<T>, EngineError> {
        let _operation = self.lifecycle.begin_mutation()?;
        let mut state = self
            .routing
            .write()
            .map_err(|_| EngineError::LockPoisoned {
                lock: "published routing state",
            })?;
        self.publication_active.store(true, Ordering::Release);
        let _publication_activity = PublicationActivity(&self.publication_active);
        self.await_retirement_capacity(&state);
        let catalog = self
            .catalog_guard()?
            .as_ref()
            .cloned()
            .ok_or(crate::LifecycleError::ShutDown)?;
        let durable_result = mutation(&catalog)?;
        let (compiled, report) = self.compile_current_image();
        let publication = match compiled {
            Ok(image) => {
                let execution = self.execution_image(image);
                retire_available_state(&self.retirement, &state, execution.cpu.bundle());
                *state = RoutingState::Available {
                    image: Arc::new(execution),
                    published_at: Instant::now(),
                    last_build: report.clone(),
                };
                PublicationOutcome::Published(report)
            }
            Err(reason) => {
                saturating_increment(&self.image_build_failures);
                *state = RoutingState::Unavailable {
                    reason: reason.clone(),
                    last_build: report.clone(),
                };
                PublicationOutcome::RoutingUnavailable(reason, report)
            }
        };
        drop(state);
        self.retirement.reap();
        Ok(ConfirmedMutation::new(durable_result, publication))
    }

    fn initial_routing_state(
        catalog: &Catalog,
        root: &Path,
        config: &EngineConfig,
    ) -> Result<(RoutingState, bool, StartupImageOutcome, Option<String>), EngineError> {
        let started = Instant::now();
        let mut startup_image_corruption = None;
        let reopened = {
            let scan = catalog.confirmed_graph_scan()?;
            match scan.active_routing_image() {
                Ok(Some(pointer)) => {
                    let result = validate_bundle_child(root, pointer.relative_bundle())
                        .and_then(|path| open_bundle(&path).map_err(bundle_reason))
                        .and_then(|bundle| {
                            if bundle.manifest_checksum() != pointer.manifest_checksum() {
                                return Err(RoutingUnavailableReason::Bundle(
                                    "manifest checksum disagrees with the durable pointer".into(),
                                ));
                            }
                            select_cpu_topology(bundle, config)
                        });
                    if let Err(reason) = &result {
                        startup_image_corruption = Some(reason.to_string());
                        scan.clear_active_routing_image()?;
                    }
                    result.ok()
                }
                Ok(None) => None,
                Err(_) => {
                    startup_image_corruption =
                        Some("durable routing-image pointer is corrupt".to_owned());
                    scan.clear_active_routing_image()?;
                    None
                }
            }
        };
        if let Some(cpu) = reopened {
            let manifest = cpu.manifest();
            let counts = (
                manifest.node_count(),
                manifest.relation_kind_count(),
                manifest.adjacency_count(),
            );
            let report = ImageBuildReport::new(
                started.elapsed(),
                ImageBuildOutcome::Published,
                counts,
                None,
            );
            return Ok((
                RoutingState::Available {
                    image: Arc::new(PublishedExecutionImage::cpu_only(
                        cpu,
                        CudaAvailability::Disabled,
                    )),
                    published_at: Instant::now(),
                    last_build: report,
                },
                false,
                StartupImageOutcome::ValidatedBundle,
                startup_image_corruption,
            ));
        }
        if config.startup_bundle_policy == StartupBundlePolicy::RequireValidBundle {
            let reason = RoutingUnavailableReason::Bundle(
                "startup requires a valid durable routing bundle; none could be opened".into(),
            );
            let report = ImageBuildReport::new(
                started.elapsed(),
                ImageBuildOutcome::Failed(reason.clone()),
                (0, 0, 0),
                None,
            );
            return Ok((
                RoutingState::Unavailable {
                    reason,
                    last_build: report,
                },
                true,
                StartupImageOutcome::RebuildFailed,
                startup_image_corruption,
            ));
        }
        let (result, counts, bundle_metrics) = build_current_bundle(catalog, root, config, None);
        match result {
            Ok(cpu) => {
                let report = ImageBuildReport::new(
                    started.elapsed(),
                    ImageBuildOutcome::Published,
                    counts,
                    bundle_metrics,
                );
                Ok((
                    RoutingState::Available {
                        image: Arc::new(PublishedExecutionImage::cpu_only(
                            cpu,
                            CudaAvailability::Disabled,
                        )),
                        published_at: Instant::now(),
                        last_build: report,
                    },
                    false,
                    StartupImageOutcome::RebuiltFromCatalog,
                    startup_image_corruption,
                ))
            }
            Err(reason) => {
                let report = ImageBuildReport::new(
                    started.elapsed(),
                    ImageBuildOutcome::Failed(reason.clone()),
                    counts,
                    bundle_metrics,
                );
                Ok((
                    RoutingState::Unavailable {
                        reason,
                        last_build: report,
                    },
                    true,
                    StartupImageOutcome::RebuildFailed,
                    startup_image_corruption,
                ))
            }
        }
    }

    fn compile_current_image(
        &self,
    ) -> (
        Result<CpuTopology, RoutingUnavailableReason>,
        ImageBuildReport,
    ) {
        let started = Instant::now();
        let catalog = match self.catalog_guard() {
            Ok(guard) => guard.as_ref().cloned(),
            Err(error) => {
                let reason = RoutingUnavailableReason::Bundle(error.to_string());
                let report = ImageBuildReport::new(
                    started.elapsed(),
                    ImageBuildOutcome::Failed(reason.clone()),
                    (0, 0, 0),
                    None,
                );
                return (Err(reason), report);
            }
        };
        let Some(catalog) = catalog else {
            let reason = RoutingUnavailableReason::Bundle(
                "the durable catalog is unavailable after shutdown".to_owned(),
            );
            let report = ImageBuildReport::new(
                started.elapsed(),
                ImageBuildOutcome::Failed(reason.clone()),
                (0, 0, 0),
                None,
            );
            return (Err(reason), report);
        };
        let (mut result, counts, bundle_metrics) = build_current_bundle(
            &catalog,
            &self.routing_image_root,
            &self.config,
            Some(&self.publication_faults),
        );
        if result.is_ok()
            && let Err(error) = self
                .publication_faults
                .trip(PublicationStage::BeforeEngineSwap)
        {
            result = Err(error);
        }
        let outcome = result
            .as_ref()
            .map(|_| ImageBuildOutcome::Published)
            .unwrap_or_else(|reason| ImageBuildOutcome::Failed(reason.clone()));
        let report = ImageBuildReport::new(started.elapsed(), outcome, counts, bundle_metrics);
        (result, report)
    }

    fn initialize_cuda_for_current(&self) {
        #[cfg(feature = "cuda")]
        {
            match CudaRuntime::initialize(self.config.cuda) {
                Ok(runtime) => {
                    if let Ok(mut slot) = self.cuda_runtime.write() {
                        *slot = Some(Arc::new(runtime));
                    } else {
                        self.set_cuda_availability(CudaAvailability::Unavailable(
                            "CUDA runtime lock is poisoned".to_owned(),
                        ));
                        return;
                    }
                    if let Err(error) = self.rebuild_cuda_residency() {
                        self.set_cuda_availability(CudaAvailability::Degraded(error.to_string()));
                    }
                }
                Err(error) => {
                    self.set_cuda_availability(CudaAvailability::Unavailable(error.to_string()));
                }
            }
        }
        #[cfg(not(feature = "cuda"))]
        self.set_cuda_availability(CudaAvailability::SupportNotCompiled);
    }

    fn execution_image(&self, cpu: CpuTopology) -> PublishedExecutionImage {
        if !self.config.cuda.enabled {
            return PublishedExecutionImage::cpu_only(cpu, CudaAvailability::Disabled);
        }
        #[cfg(feature = "cuda")]
        {
            let runtime = self
                .cuda_runtime
                .read()
                .ok()
                .and_then(|runtime| runtime.clone());
            let Some(runtime) = runtime else {
                return PublishedExecutionImage::cpu_only(
                    cpu,
                    CudaAvailability::Unavailable("CUDA runtime is absent".to_owned()),
                );
            };
            match runtime.upload_topology(&cpu) {
                Ok(cuda) => {
                    saturating_increment(&self.cuda_uploads);
                    self.set_cuda_availability(CudaAvailability::Available);
                    PublishedExecutionImage::with_cuda(cpu, cuda)
                }
                Err(error) => {
                    saturating_increment(&self.cuda_upload_failures);
                    let availability = CudaAvailability::Degraded(error.to_string());
                    self.set_cuda_availability(availability.clone());
                    PublishedExecutionImage::cpu_only(cpu, availability)
                }
            }
        }
        #[cfg(not(feature = "cuda"))]
        {
            PublishedExecutionImage::cpu_only(cpu, CudaAvailability::SupportNotCompiled)
        }
    }

    fn set_cuda_availability(&self, availability: CudaAvailability) {
        match &availability {
            CudaAvailability::Available => {
                if let Ok(mut recovery) = self.last_cuda_recovery.write() {
                    *recovery = Some("CUDA context and matching topology are available".to_owned());
                }
            }
            CudaAvailability::Degraded(reason) | CudaAvailability::Unavailable(reason) => {
                if let Ok(mut degradation) = self.last_cuda_degradation.write() {
                    *degradation = Some(reason.clone());
                }
            }
            CudaAvailability::Disabled | CudaAvailability::SupportNotCompiled => {}
        }
        if let Ok(mut current) = self.cuda_availability.write() {
            *current = availability;
        }
    }

    fn record_image_corruption(&self, reason: String) {
        if let Ok(mut corruption) = self.last_image_corruption.write() {
            *corruption = Some(reason);
        }
    }

    fn await_retirement_capacity(&self, state: &RoutingState) {
        let Ok(execution) = state.execution() else {
            return;
        };
        let bundle_bytes = execution.cpu.bundle().total_bytes();
        drop(execution);
        while state.execution_lease_count() > 1 && !self.retirement.capacity_available(bundle_bytes)
        {
            let waited = Duration::from_millis(5);
            std::thread::sleep(waited);
            self.retirement.record_backpressure_wait(waited);
        }
    }

    fn cuda_device_summary(&self) -> Option<CudaDeviceSummary> {
        #[cfg(feature = "cuda")]
        {
            let runtime = self.cuda_runtime.read().ok()?.clone()?;
            let capabilities = runtime.context.capabilities();
            let (free, total) = runtime.context.current_memory().ok()?;
            Some(CudaDeviceSummary {
                ordinal: capabilities.device_ordinal,
                name: capabilities.device_name.clone(),
                compute_capability: capabilities.compute_capability,
                driver_version: capabilities.driver_version,
                total_memory_bytes: total,
                free_memory_bytes: free,
                kernel_ptx_target: capabilities.kernel_ptx_target,
            })
        }
        #[cfg(not(feature = "cuda"))]
        {
            None
        }
    }

    fn cuda_health(&self, routing: &RoutingState) -> Result<CudaHealth, EngineError> {
        let configured = self
            .cuda_availability
            .read()
            .map_err(|_| EngineError::LockPoisoned {
                lock: "CUDA health",
            })?
            .clone();
        let snapshot_availability = routing
            .execution()
            .ok()
            .and_then(|execution| execution.cuda_unavailable_reason.clone());
        let availability = snapshot_availability.unwrap_or(configured);
        #[cfg(feature = "cuda")]
        {
            let execution = routing.execution().ok();
            let resident = execution
                .as_ref()
                .and_then(|execution| execution.cuda.as_ref());
            let runtime = self
                .cuda_runtime
                .read()
                .ok()
                .and_then(|runtime| runtime.clone());
            let worker = runtime
                .as_ref()
                .map(|runtime| runtime.worker.snapshot())
                .unwrap_or_default();
            let cuda_admission = runtime
                .as_ref()
                .and_then(|runtime| runtime.admission.snapshot().ok())
                .unwrap_or_default();
            Ok(CudaHealth {
                availability,
                device: self.cuda_device_summary(),
                resident_node_count: resident.map_or(0, |resident| resident.node_count()),
                resident_adjacency_count: resident.map_or(0, |resident| resident.adjacency_count()),
                resident_topology_bytes: resident.map_or(0, |resident| resident.allocated_bytes()),
                partitioned_topology: resident.is_some_and(CudaTopology::partitioned),
                device_topology_cache: resident.and_then(CudaTopology::device_cache).map(|cache| {
                    crate::DeviceTopologyCacheHealth {
                        capacity_bytes: cache.capacity_bytes,
                        capacity_slots: cache.capacity_slots,
                        current_bytes: cache.current_bytes,
                        high_water_bytes: cache.high_water_bytes,
                        entries: cache.entries,
                        host_loading_entries: cache.host_loading_entries,
                        copying_entries: cache.copying_entries,
                        evicting_entries: cache.evicting_entries,
                        ready_entries: cache.ready_entries,
                        failed_entries: cache.failed_entries,
                        hits: cache.hits,
                        misses: cache.misses,
                        coalesced_waits: cache.coalesced_waits,
                        copies: cache.copies,
                        evictions: cache.evictions,
                        slot_waits: cache.slot_waits,
                        completion_waits: cache.completion_waits,
                        in_use_slots: cache.in_use_slots,
                        transfer_bytes: cache.transfer_bytes,
                    }
                }),
                queued_lanes: worker.queued_lanes,
                active_lanes: worker.active_lanes,
                peak_active_lanes: worker.peak_active_lanes,
                reserved_search_bytes: cuda_admission.reserved_bytes,
                peak_reserved_search_bytes: cuda_admission.peak_reserved_bytes,
                cumulative_admission_rejections: cuda_admission.rejections,
                worker_running: worker.running,
                cumulative_uploads: self.cuda_uploads.load(Ordering::Relaxed),
                cumulative_upload_failures: self.cuda_upload_failures.load(Ordering::Relaxed),
                cumulative_launches: worker.cumulative_launches,
                cumulative_launch_failures: worker.cumulative_failures,
                cumulative_fallbacks: self.cuda_fallbacks.load(Ordering::Relaxed),
                cumulative_cancellations: worker.cumulative_cancellations,
                cumulative_context_reinitializations: self
                    .cuda_context_reinitializations
                    .load(Ordering::Relaxed),
            })
        }
        #[cfg(not(feature = "cuda"))]
        {
            Ok(CudaHealth {
                availability,
                device: None,
                resident_node_count: 0,
                resident_adjacency_count: 0,
                resident_topology_bytes: 0,
                partitioned_topology: false,
                device_topology_cache: None,
                queued_lanes: 0,
                active_lanes: 0,
                peak_active_lanes: 0,
                reserved_search_bytes: 0,
                peak_reserved_search_bytes: 0,
                cumulative_admission_rejections: 0,
                worker_running: false,
                cumulative_uploads: self.cuda_uploads.load(Ordering::Relaxed),
                cumulative_upload_failures: self.cuda_upload_failures.load(Ordering::Relaxed),
                cumulative_launches: 0,
                cumulative_launch_failures: 0,
                cumulative_fallbacks: self.cuda_fallbacks.load(Ordering::Relaxed),
                cumulative_cancellations: 0,
                cumulative_context_reinitializations: self
                    .cuda_context_reinitializations
                    .load(Ordering::Relaxed),
            })
        }
    }
}

static BUNDLE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn resolve_routing_image_root(
    database: &Path,
    configured: Option<&Path>,
) -> Result<PathBuf, EngineError> {
    let parent = database.parent().ok_or({
        EngineError::Configuration(EngineConfigError::InvalidRoutingImageValue {
            field: "routing_image_root",
            reason: "database path has no parent",
        })
    })?;
    let root = match configured {
        Some(path) if path.is_absolute() => path.to_path_buf(),
        Some(path) => parent.join(path),
        None => {
            let name = database.file_name().and_then(|name| name.to_str()).ok_or({
                EngineError::Configuration(EngineConfigError::InvalidRoutingImageValue {
                    field: "routing_image_root",
                    reason: "database name is not valid UTF-8",
                })
            })?;
            parent.join(format!("{name}.routing-images"))
        }
    };
    if root.parent().is_none() {
        return Err(EngineError::Configuration(
            EngineConfigError::InvalidRoutingImageValue {
                field: "routing_image_root",
                reason: "must not be a filesystem root",
            },
        ));
    }
    Ok(root)
}

fn cleanup_startup_children(root: &Path, current: Option<&str>) -> Result<(), EngineError> {
    for entry in fs::read_dir(root).map_err(|error| {
        EngineError::RoutingUnavailable(RoutingUnavailableReason::Bundle(error.to_string()))
    })? {
        let entry = entry.map_err(|error| {
            EngineError::RoutingUnavailable(RoutingUnavailableReason::Bundle(error.to_string()))
        })?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let positively_identified =
            name.starts_with(".tmp-") || (name.starts_with("bundle-") && Some(name) != current);
        if positively_identified
            && entry
                .file_type()
                .map_err(|error| {
                    EngineError::RoutingUnavailable(RoutingUnavailableReason::Bundle(
                        error.to_string(),
                    ))
                })?
                .is_dir()
        {
            fs::remove_dir_all(entry.path()).map_err(|error| {
                EngineError::RoutingUnavailable(RoutingUnavailableReason::Bundle(error.to_string()))
            })?;
        }
    }
    Ok(())
}

fn retire_available_state(
    retirement: &RetirementManager,
    state: &RoutingState,
    replacement: Arc<pathhydra_routing::RoutingBundle>,
) {
    let Ok(execution) = state.execution() else {
        return;
    };
    let externally_leased = state.execution_lease_count() > 2;
    let current = execution.cpu.bundle();
    if !Arc::ptr_eq(&current, &replacement) && current.root() != replacement.root() {
        retirement.retire(&current, externally_leased);
    }
}

fn validate_bundle_child(root: &Path, name: &str) -> Result<PathBuf, RoutingUnavailableReason> {
    let path = Path::new(name);
    let mut components = path.components();
    if !matches!(components.next(), Some(std::path::Component::Normal(_)))
        || components.next().is_some()
    {
        return Err(RoutingUnavailableReason::Bundle(
            "durable bundle name is not one relative child".into(),
        ));
    }
    Ok(root.join(path))
}

fn bundle_reason(error: pathhydra_routing::BundleError) -> RoutingUnavailableReason {
    RoutingUnavailableReason::Bundle(error.to_string())
}

fn select_cpu_topology(
    bundle: pathhydra_routing::RoutingBundle,
    config: &EngineConfig,
) -> Result<CpuTopology, RoutingUnavailableReason> {
    let metadata = bundle.identity_directory_bytes();
    if metadata > config.max_resident_image_metadata_bytes {
        return Err(RoutingUnavailableReason::MetadataLimit {
            required: metadata,
            limit: config.max_resident_image_metadata_bytes,
        });
    }
    let logical = bundle
        .routing_manifest()
        .map_err(bundle_reason)?
        .byte_counts()
        .checked_total()
        .ok_or_else(|| {
            RoutingUnavailableReason::Bundle("resident image byte count overflow".into())
        })?;
    let bundle = Arc::new(bundle);
    if logical <= config.max_active_image_bytes {
        return bundle
            .to_resident_image()
            .map(|image| CpuTopology::Resident {
                image: Arc::new(image),
                bundle: Arc::clone(&bundle),
            })
            .map_err(bundle_reason);
    }
    let cache = HostCacheConfig {
        maximum_bytes: config.host_partition_cache_bytes,
        maximum_staging_bytes: config.routing_io_staging_bytes,
        maximum_entries: config.host_partition_cache_entries,
        io_worker_count: config.routing_io_worker_count,
        maximum_queued_reads: config.maximum_queued_partition_reads,
    };
    ChunkedRoutingImage::open_shared(bundle, cache)
        .map(|image| CpuTopology::Partitioned(Arc::new(image)))
        .map_err(bundle_reason)
}

fn build_current_bundle(
    catalog: &Catalog,
    root: &Path,
    config: &EngineConfig,
    faults: Option<&PublicationFaultInjection>,
) -> (
    Result<CpuTopology, RoutingUnavailableReason>,
    (usize, usize, usize),
    Option<pathhydra_routing::BundleBuildMetrics>,
) {
    let scan = match catalog.confirmed_graph_scan() {
        Ok(scan) => scan,
        Err(error) => {
            return (
                Err(RoutingUnavailableReason::Bundle(error.to_string())),
                (0, 0, 0),
                None,
            );
        }
    };
    let sequence = BUNDLE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary_name = format!(".tmp-{}-{sequence}", std::process::id());
    let temporary = root.join(&temporary_name);
    let bundle_config = BundleConfig {
        target_partition_topology_bytes: config.target_partition_topology_bytes,
        hard_maximum_partition_topology_bytes: config.hard_maximum_partition_topology_bytes,
        maximum_total_bundle_bytes: config.max_total_bundle_bytes,
    };
    let result = (|| {
        fs::create_dir(&temporary)
            .map_err(|error| RoutingUnavailableReason::Bundle(error.to_string()))?;
        trip_publication(faults, PublicationStage::TemporaryDirectoryCreated)?;
        let (written, metrics) =
            compile_bundle(&scan, &temporary, bundle_config).map_err(bundle_reason)?;
        trip_publication(faults, PublicationStage::BundleFilesSynchronized)?;
        let checked = open_bundle(&temporary).map_err(bundle_reason)?;
        if checked.manifest() != &written {
            return Err(RoutingUnavailableReason::Bundle(
                "reopened manifest disagrees with the streaming compiler".into(),
            ));
        }
        trip_publication(faults, PublicationStage::TemporaryBundleValidated)?;
        let checksum = *checked.manifest_checksum();
        let checksum_name = checksum
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let mut suffix = sequence;
        let final_name = loop {
            let candidate = format!("bundle-{checksum_name}-{}-{suffix}", std::process::id());
            if !root.join(&candidate).exists() {
                break candidate;
            }
            suffix = suffix.saturating_add(1);
        };
        let final_path = root.join(&final_name);
        fs::rename(&temporary, &final_path)
            .map_err(|error| RoutingUnavailableReason::Bundle(error.to_string()))?;
        trip_publication(faults, PublicationStage::FinalDirectoryRenamed)?;
        let final_bundle = open_bundle(&final_path).map_err(bundle_reason)?;
        let counts = (
            final_bundle.manifest().node_count as usize,
            final_bundle.manifest().relation_kind_count as usize,
            final_bundle.manifest().adjacency_count as usize,
        );
        scan.set_active_routing_image(&ActiveRoutingImage::new(final_name, checksum))
            .map_err(|error| RoutingUnavailableReason::Bundle(error.to_string()))?;
        trip_publication(faults, PublicationStage::DurablePointerCommitted)?;
        match select_cpu_topology(final_bundle, config) {
            Ok(cpu) => {
                trip_publication(faults, PublicationStage::RuntimeRepresentationConstructed)?;
                Ok((cpu, counts, metrics))
            }
            Err(error) => {
                let _ = scan.clear_active_routing_image();
                Err(error)
            }
        }
    })();
    match result {
        Ok((cpu, counts, metrics)) => (Ok(cpu), counts, Some(metrics)),
        Err(error) => (Err(error), (0, 0, 0), None),
    }
}

fn trip_publication(
    faults: Option<&PublicationFaultInjection>,
    stage: PublicationStage,
) -> Result<(), RoutingUnavailableReason> {
    faults.map_or(Ok(()), |faults| faults.trip(stage))
}

fn bounded_verification_limits(
    requested: VerificationLimits,
    maximum_records: u64,
    maximum_duration: Duration,
) -> VerificationLimits {
    VerificationLimits {
        maximum_records: Some(
            requested
                .maximum_records
                .map_or(maximum_records, |value| value.min(maximum_records)),
        ),
        maximum_duration: Some(
            requested
                .maximum_duration
                .map_or(maximum_duration, |value| value.min(maximum_duration)),
        ),
    }
}

fn drained_operations(
    before: crate::ActiveOperationCounts,
    remaining: crate::ActiveOperationCounts,
) -> crate::ActiveOperationCounts {
    crate::ActiveOperationCounts {
        routes: before.routes.saturating_sub(remaining.routes),
        mutations: before.mutations.saturating_sub(remaining.mutations),
        checkpoints: before.checkpoints.saturating_sub(remaining.checkpoints),
        maintenance: before.maintenance.saturating_sub(remaining.maintenance),
    }
}

impl Drop for GraphEngine {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn saturating_increment(counter: &AtomicU64) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
        Some(value.saturating_add(1))
    });
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pathhydra_core::ConfirmedRecord;
    use pathhydra_routing::{
        DestinationState, RelationMultiplier, RelationProfile, RelationUse, SearchBudget,
    };

    use super::*;

    fn confirm_node(engine: &GraphEngine, name: &str) -> NodeRecord {
        let candidate = engine.insert_node_candidate(name).unwrap();
        let ConfirmedRecord::Node(node) = engine
            .confirm_validated_candidate(candidate)
            .unwrap()
            .into_parts()
            .0
        else {
            panic!()
        };
        node
    }

    #[test]
    fn acquired_image_survives_publication_but_new_routes_use_the_replacement() {
        let directory = tempfile::tempdir().unwrap();
        let engine = GraphEngine::open(directory.path(), EngineConfig::default()).unwrap();
        let source = confirm_node(&engine, "source");
        let destination = confirm_node(&engine, "destination");
        let relation_candidate = engine.insert_relation_candidate("kind").unwrap();
        let ConfirmedRecord::Relation(relation) = engine
            .confirm_validated_candidate(relation_candidate)
            .unwrap()
            .into_parts()
            .0
        else {
            panic!()
        };
        let edge_candidate = engine
            .insert_edge_candidate(source.id(), destination.id(), relation.id(), 0.5)
            .unwrap();
        engine.confirm_validated_candidate(edge_candidate).unwrap();
        let old_image = engine.routing.read().unwrap().image().unwrap();
        let request = RoutingRequest::new(
            source.id(),
            [destination.id()],
            RelationProfile::new([(
                relation.id(),
                RelationUse::Enabled(RelationMultiplier::new(1.0).unwrap()),
            )]),
            false,
            SearchBudget::Unlimited,
            TiePolicy::StablePredecessor,
        );
        engine.remove_node(destination.id()).unwrap();

        let old_response = pathhydra_routing::route(&old_image, &request).unwrap();
        assert!(matches!(
            old_response.results()[0].state(),
            DestinationState::Exact(_)
        ));
        let new_response = engine.route(RequestId::new(1), &request).unwrap();
        assert!(matches!(
            new_response.response.results()[0].state(),
            DestinationState::MissingNode
        ));
    }

    #[test]
    fn simultaneous_confirmed_mutations_serialize_publication() {
        let directory = tempfile::tempdir().unwrap();
        let engine =
            Arc::new(GraphEngine::open(directory.path(), EngineConfig::default()).unwrap());
        let candidates: Vec<_> = (0..8)
            .map(|index| {
                engine
                    .insert_node_candidate(format!("node-{index}"))
                    .unwrap()
            })
            .collect();
        let workers: Vec<_> = candidates
            .into_iter()
            .map(|candidate| {
                let engine = Arc::clone(&engine);
                std::thread::spawn(move || engine.confirm_validated_candidate(candidate).unwrap())
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(
            engine
                .health()
                .unwrap()
                .current_image_manifest
                .unwrap()
                .node_count(),
            8
        );
    }

    #[test]
    fn shutdown_cancels_a_real_active_partitioned_route_and_retry_joins_io() {
        let root = tempfile::tempdir().unwrap();
        let engine = Arc::new(
            GraphEngine::open(
                root.path().join("catalog"),
                EngineConfig {
                    max_active_image_bytes: 8,
                    target_partition_topology_bytes: 48,
                    hard_maximum_partition_topology_bytes: 48,
                    host_partition_cache_bytes: 112,
                    host_partition_cache_entries: 1,
                    routing_io_staging_bytes: 56,
                    routing_io_worker_count: 1,
                    maximum_queued_partition_reads: 1,
                    routing_image_root: Some(root.path().join("routing")),
                    shutdown_drain_timeout: Duration::from_millis(1),
                    ..EngineConfig::default()
                },
            )
            .unwrap(),
        );
        let source = confirm_node(&engine, "source");
        let destination = confirm_node(&engine, "destination");
        let relation_candidate = engine.insert_relation_candidate("relation").unwrap();
        let ConfirmedRecord::Relation(relation) = engine
            .confirm_validated_candidate(relation_candidate)
            .unwrap()
            .into_parts()
            .0
        else {
            panic!("expected relation")
        };
        let edge = engine
            .insert_edge_candidate(source.id(), destination.id(), relation.id(), 1.0)
            .unwrap();
        engine.confirm_validated_candidate(edge).unwrap();
        let request = RoutingRequest::new(
            source.id(),
            [destination.id()],
            RelationProfile::new([(
                relation.id(),
                RelationUse::Enabled(RelationMultiplier::new(1.0).unwrap()),
            )]),
            false,
            SearchBudget::Unlimited,
            TiePolicy::StablePredecessor,
        );
        engine
            .routing_bundle_fault_injection()
            .unwrap()
            .delay_reads(Duration::from_secs(1));
        let route = {
            let engine = Arc::clone(&engine);
            std::thread::spawn(move || engine.route(RequestId::new(900), &request))
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let active_routes = engine.lifecycle.snapshot().unwrap().active.routes;
            let cache = engine
                .routing
                .read()
                .unwrap()
                .execution()
                .unwrap()
                .cpu
                .host_cache()
                .unwrap();
            if active_routes == 1 && cache.misses > 0 && cache.queue_depth > 0 {
                break;
            }
            assert!(
                !route.is_finished(),
                "route finished before entering partition I/O"
            );
            assert!(
                Instant::now() < deadline,
                "route never entered partition I/O"
            );
            std::thread::yield_now();
        }

        let first = engine.shutdown().unwrap();
        assert!(!first.complete());
        assert!(!first.drain.drained);
        assert_eq!(first.active_requests_signalled, 1);
        assert_eq!(first.newly_signalled_requests, 1);
        assert_eq!(first.drain.remaining.routes, 1);
        assert_eq!(first.failures[0].stage, ShutdownFailureStage::Drain);
        let _ = route.join().unwrap();

        let retried = engine.shutdown().unwrap();
        assert!(retried.complete(), "{:#?}", retried.failures);
        assert!(retried.drain.drained);
        assert_eq!(retried.active_before.routes, 1);
        assert_eq!(retried.drained.routes, 1);
        assert!(retried.partition_io_stopped);
    }

    #[test]
    fn maintenance_contention_is_refused_through_the_engine_api() {
        let root = tempfile::tempdir().unwrap();
        let engine = GraphEngine::open(
            root.path().join("catalog"),
            EngineConfig {
                routing_image_root: Some(root.path().join("routing")),
                maximum_maintenance_workers: 1,
                maximum_queued_maintenance: 0,
                ..EngineConfig::default()
            },
        )
        .unwrap();
        let active = engine.lifecycle.begin_maintenance().unwrap();
        assert!(matches!(
            engine.verify_catalog(VerificationLimits::default()),
            Err(EngineError::Lifecycle(
                crate::LifecycleError::MaintenanceBusy { maximum_workers: 1 }
            ))
        ));
        assert!(matches!(
            engine.compact_store(),
            Err(EngineError::Lifecycle(
                crate::LifecycleError::MaintenanceBusy { maximum_workers: 1 }
            ))
        ));
        drop(active);
        assert!(engine.verify_catalog(VerificationLimits::default()).is_ok());
        assert!(engine.shutdown().unwrap().complete());
    }

    #[test]
    fn request_identity_and_cancellation_are_isolated_and_released() {
        let directory = tempfile::tempdir().unwrap();
        let engine = GraphEngine::open(directory.path(), EngineConfig::default()).unwrap();
        let origin = confirm_node(&engine, "origin");
        let id = RequestId::new(44);
        let registration = engine.requests.register(id).unwrap();
        let request = RoutingRequest::new(
            origin.id(),
            [origin.id()],
            RelationProfile::default(),
            false,
            SearchBudget::Unlimited,
            TiePolicy::StablePredecessor,
        );
        assert!(matches!(
            engine.route(id, &request),
            Err(EngineError::DuplicateActiveRequest(found)) if found == id
        ));
        assert_eq!(engine.cancel(id).unwrap(), CancellationOutcome::Signalled);
        assert_eq!(
            engine.cancel(id).unwrap(),
            CancellationOutcome::AlreadySignalled
        );
        assert_eq!(engine.health().unwrap().cumulative_cancellations, 1);
        drop(registration);
        assert_eq!(engine.cancel(id).unwrap(), CancellationOutcome::NotActive);
        assert!(engine.route(id, &request).is_ok());
    }

    #[test]
    fn partitioned_bundle_retirement_waits_for_lease_and_retries_delete_failure() {
        let directory = tempfile::tempdir().unwrap();
        let config = EngineConfig {
            max_active_image_bytes: 8,
            target_partition_topology_bytes: 48,
            hard_maximum_partition_topology_bytes: 48,
            host_partition_cache_bytes: 112,
            host_partition_cache_entries: 1,
            ..EngineConfig::default()
        };
        let engine = GraphEngine::open(directory.path().join("graph"), config).unwrap();
        let source = confirm_node(&engine, "source");
        let destination = confirm_node(&engine, "destination");
        let relation_candidate = engine.insert_relation_candidate("kind").unwrap();
        let ConfirmedRecord::Relation(relation) = engine
            .confirm_validated_candidate(relation_candidate)
            .unwrap()
            .into_parts()
            .0
        else {
            panic!()
        };
        let edge = engine
            .insert_edge_candidate(source.id(), destination.id(), relation.id(), 1.0)
            .unwrap();
        engine.confirm_validated_candidate(edge).unwrap();
        let old = engine.routing.read().unwrap().execution().unwrap();
        let old_bundle = old.cpu.bundle();
        let old_path = old_bundle.root().to_path_buf();
        let old_partitioned = match &old.cpu {
            CpuTopology::Partitioned(image) => Arc::clone(image),
            CpuTopology::Resident { .. } => panic!("fixture must force partitioned CPU"),
        };
        drop(old_bundle);
        let request = RoutingRequest::new(
            source.id(),
            [destination.id()],
            RelationProfile::new([(
                relation.id(),
                RelationUse::Enabled(RelationMultiplier::new(1.0).unwrap()),
            )]),
            false,
            SearchBudget::Unlimited,
            TiePolicy::StablePredecessor,
        );
        engine.remove_node(destination.id()).unwrap();
        assert!(old_path.exists());
        assert!(matches!(
            pathhydra_routing::route_partitioned(&old_partitioned, &request)
                .unwrap()
                .results()[0]
                .state(),
            DestinationState::Exact(_)
        ));
        assert!(engine.health().unwrap().retired_bundles.bundle_count >= 1);
        drop(old_partitioned);
        drop(old);

        engine.retirement.fail_next_delete();
        let failed = engine.health().unwrap().retired_bundles;
        assert!(old_path.exists());
        assert!(failed.last_cleanup_failure.is_some());
        let recovered = engine.health().unwrap().retired_bundles;
        assert!(!old_path.exists());
        assert!(recovered.last_cleanup_failure.is_none());
    }

    #[test]
    fn retirement_count_limit_backpressures_publication_without_overrun() {
        let directory = tempfile::tempdir().unwrap();
        let engine = Arc::new(
            GraphEngine::open(
                directory.path(),
                EngineConfig {
                    maximum_retired_bundle_count: 1,
                    maximum_retired_bundle_bytes: u64::MAX,
                    ..EngineConfig::default()
                },
            )
            .unwrap(),
        );
        let first_lease = engine.routing.read().unwrap().execution().unwrap();
        confirm_node(&engine, "first replacement");
        assert_eq!(engine.health().unwrap().retired_bundles.bundle_count, 1);

        let second_lease = engine.routing.read().unwrap().execution().unwrap();
        let candidate = engine.insert_node_candidate("second replacement").unwrap();
        let worker = {
            let engine = Arc::clone(&engine);
            std::thread::spawn(move || engine.confirm_validated_candidate(candidate).unwrap())
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while engine.retirement.snapshot().cumulative_backpressure_waits == 0 {
            assert!(
                Instant::now() < deadline,
                "publication did not enter retirement backpressure"
            );
            std::thread::yield_now();
        }
        assert!(!worker.is_finished(), "publication must wait at the limit");
        assert_eq!(engine.retirement.snapshot().bundle_count, 1);
        drop(first_lease);
        worker.join().unwrap();

        let pressured = engine.health().unwrap().retired_bundles;
        assert_eq!(pressured.bundle_count, 1);
        assert!(!pressured.limit_exceeded);
        assert!(pressured.cumulative_backpressure_waits > 0);
        drop(second_lease);
        assert_eq!(engine.health().unwrap().retired_bundles.bundle_count, 0);
    }

    #[test]
    fn retirement_byte_limit_waits_for_the_current_external_lease() {
        let directory = tempfile::tempdir().unwrap();
        let engine = Arc::new(
            GraphEngine::open(
                directory.path(),
                EngineConfig {
                    maximum_retired_bundle_count: 8,
                    maximum_retired_bundle_bytes: 1,
                    ..EngineConfig::default()
                },
            )
            .unwrap(),
        );
        let lease = engine.routing.read().unwrap().execution().unwrap();
        let candidate = engine.insert_node_candidate("replacement").unwrap();
        let worker = {
            let engine = Arc::clone(&engine);
            std::thread::spawn(move || engine.confirm_validated_candidate(candidate).unwrap())
        };
        std::thread::sleep(Duration::from_millis(30));
        assert!(
            !worker.is_finished(),
            "oversized retirement must backpressure"
        );
        drop(lease);
        worker.join().unwrap();
        let snapshot = engine.health().unwrap().retired_bundles;
        assert_eq!(snapshot.bundle_count, 0);
        assert!(!snapshot.limit_exceeded);
        assert!(snapshot.cumulative_backpressure_waits > 0);
    }
}
