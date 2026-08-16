use std::{
    fmt,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
    time::{Duration, Instant},
};

use pathhydra_routing::{ChunkedRoutingImage, RoutingBundle, RoutingImage, RoutingImageManifest};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum PublicationStage {
    TemporaryDirectoryCreated = 1,
    BundleFilesSynchronized = 2,
    TemporaryBundleValidated = 3,
    FinalDirectoryRenamed = 4,
    DurablePointerCommitted = 5,
    RuntimeRepresentationConstructed = 6,
    BeforeEngineSwap = 7,
}

#[doc(hidden)]
#[derive(Default)]
pub struct PublicationFaultInjection {
    stage: AtomicU8,
}

impl PublicationFaultInjection {
    pub fn reset(&self) {
        self.stage.store(0, Ordering::Release);
    }

    pub fn fail_at(&self, stage: PublicationStage) {
        self.stage.store(stage as u8, Ordering::Release);
    }

    pub(crate) fn trip(&self, stage: PublicationStage) -> Result<(), RoutingUnavailableReason> {
        if self
            .stage
            .compare_exchange(stage as u8, 0, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            Err(RoutingUnavailableReason::Bundle(format!(
                "injected publication failure at {stage:?}"
            )))
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CpuTopologyMode {
    Resident,
    Partitioned,
}

#[derive(Clone)]
pub(crate) enum CpuTopology {
    Resident {
        image: Arc<RoutingImage>,
        bundle: Arc<RoutingBundle>,
    },
    Partitioned(Arc<ChunkedRoutingImage>),
}

impl CpuTopology {
    pub fn manifest(&self) -> RoutingImageManifest {
        match self {
            Self::Resident { image, .. } => image.manifest().clone(),
            Self::Partitioned(image) => image
                .routing_manifest()
                .expect("validated bundle counts remain representable"),
        }
    }
    pub fn resident(&self) -> Option<Arc<RoutingImage>> {
        match self {
            Self::Resident { image, .. } => Some(Arc::clone(image)),
            Self::Partitioned(_) => None,
        }
    }
    pub const fn mode(&self) -> CpuTopologyMode {
        match self {
            Self::Resident { .. } => CpuTopologyMode::Resident,
            Self::Partitioned(_) => CpuTopologyMode::Partitioned,
        }
    }
    pub fn host_cache(&self) -> Option<pathhydra_routing::HostCacheSnapshot> {
        match self {
            Self::Resident { .. } => None,
            Self::Partitioned(image) => Some(image.cache_snapshot()),
        }
    }
    pub fn bundle(&self) -> Arc<RoutingBundle> {
        match self {
            Self::Resident { bundle, .. } => Arc::clone(bundle),
            Self::Partitioned(image) => Arc::clone(image.bundle()),
        }
    }
}

pub(crate) struct PublishedExecutionImage {
    pub cpu: CpuTopology,
    #[cfg(feature = "cuda")]
    pub cuda: Option<CudaTopology>,
    pub cuda_unavailable_reason: Option<crate::CudaAvailability>,
}

#[cfg(feature = "cuda")]
#[derive(Clone)]
pub(crate) enum CudaTopology {
    Resident(Arc<pathhydra_cuda::CudaResidentImage>),
    Partitioned(Arc<pathhydra_cuda::CudaPartitionedImage>),
}

#[cfg(feature = "cuda")]
impl CudaTopology {
    pub fn node_count(&self) -> usize {
        match self {
            Self::Resident(image) => image.node_count(),
            Self::Partitioned(image) => image.node_count(),
        }
    }
    pub fn adjacency_count(&self) -> usize {
        match self {
            Self::Resident(image) => image.adjacency_count(),
            Self::Partitioned(image) => image.adjacency_count(),
        }
    }
    pub fn allocated_bytes(&self) -> usize {
        match self {
            Self::Resident(image) => image.allocated_bytes(),
            Self::Partitioned(image) => image.allocated_bytes(),
        }
    }
    pub const fn partitioned(&self) -> bool {
        matches!(self, Self::Partitioned(_))
    }
    pub fn device_cache(&self) -> Option<pathhydra_cuda::DeviceTopologyCacheSnapshot> {
        match self {
            Self::Resident(_) => None,
            Self::Partitioned(image) => Some(image.topology_cache_snapshot()),
        }
    }
}

impl PublishedExecutionImage {
    pub fn cpu_only(cpu: CpuTopology, reason: crate::CudaAvailability) -> Self {
        Self {
            cpu,
            #[cfg(feature = "cuda")]
            cuda: None,
            cuda_unavailable_reason: Some(reason),
        }
    }

    #[cfg(feature = "cuda")]
    pub fn with_cuda(cpu: CpuTopology, cuda: CudaTopology) -> Self {
        Self {
            cpu,
            cuda: Some(cuda),
            cuda_unavailable_reason: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RoutingUnavailableReason {
    ImageCompilation(String),
    TopologyLimit { required: usize, limit: usize },
    MetadataLimit { required: u64, limit: u64 },
    Bundle(String),
}

impl fmt::Display for RoutingUnavailableReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ImageCompilation(reason) => formatter.write_str(reason),
            Self::TopologyLimit { required, limit } => write!(
                formatter,
                "topology requires {required} bytes; limit is {limit}"
            ),
            Self::MetadataLimit { required, limit } => write!(
                formatter,
                "routing metadata requires {required} bytes; limit is {limit}"
            ),
            Self::Bundle(reason) => write!(formatter, "durable routing bundle failure: {reason}"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ImageBuildOutcome {
    Published,
    Failed(RoutingUnavailableReason),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImageBuildReport {
    duration: Duration,
    outcome: ImageBuildOutcome,
    node_count: usize,
    relation_kind_count: usize,
    adjacency_count: usize,
    bundle_metrics: Option<pathhydra_routing::BundleBuildMetrics>,
}

impl ImageBuildReport {
    pub(crate) fn new(
        duration: Duration,
        outcome: ImageBuildOutcome,
        counts: (usize, usize, usize),
        bundle_metrics: Option<pathhydra_routing::BundleBuildMetrics>,
    ) -> Self {
        Self {
            duration,
            outcome,
            node_count: counts.0,
            relation_kind_count: counts.1,
            adjacency_count: counts.2,
            bundle_metrics,
        }
    }
    #[must_use]
    pub const fn duration(&self) -> Duration {
        self.duration
    }
    #[must_use]
    pub const fn outcome(&self) -> &ImageBuildOutcome {
        &self.outcome
    }
    #[must_use]
    pub const fn node_count(&self) -> usize {
        self.node_count
    }
    #[must_use]
    pub const fn relation_kind_count(&self) -> usize {
        self.relation_kind_count
    }
    #[must_use]
    pub const fn adjacency_count(&self) -> usize {
        self.adjacency_count
    }
    #[must_use]
    pub const fn bundle_metrics(&self) -> Option<&pathhydra_routing::BundleBuildMetrics> {
        self.bundle_metrics.as_ref()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PublicationOutcome {
    Published(ImageBuildReport),
    RoutingUnavailable(RoutingUnavailableReason, ImageBuildReport),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfirmedMutation<T> {
    durable_result: T,
    publication: PublicationOutcome,
}
impl<T> ConfirmedMutation<T> {
    pub(crate) fn new(durable_result: T, publication: PublicationOutcome) -> Self {
        Self {
            durable_result,
            publication,
        }
    }
    #[must_use]
    pub const fn durable_result(&self) -> &T {
        &self.durable_result
    }
    #[must_use]
    pub const fn publication(&self) -> &PublicationOutcome {
        &self.publication
    }
    #[must_use]
    pub fn into_parts(self) -> (T, PublicationOutcome) {
        (self.durable_result, self.publication)
    }
}

pub(crate) enum RoutingState {
    Available {
        image: Arc<PublishedExecutionImage>,
        published_at: Instant,
        last_build: ImageBuildReport,
    },
    Unavailable {
        reason: RoutingUnavailableReason,
        last_build: ImageBuildReport,
    },
}

impl RoutingState {
    pub fn image(&self) -> Result<Arc<RoutingImage>, RoutingUnavailableReason> {
        match self {
            Self::Available { image, .. } => image.cpu.resident().ok_or_else(|| {
                RoutingUnavailableReason::Bundle("current CPU topology is partitioned".into())
            }),
            Self::Unavailable { reason, .. } => Err(reason.clone()),
        }
    }
    pub fn manifest(&self) -> Option<RoutingImageManifest> {
        match self {
            Self::Available { image, .. } => Some(image.cpu.manifest()),
            Self::Unavailable { .. } => None,
        }
    }

    pub fn execution(&self) -> Result<Arc<PublishedExecutionImage>, RoutingUnavailableReason> {
        match self {
            Self::Available { image, .. } => Ok(Arc::clone(image)),
            Self::Unavailable { reason, .. } => Err(reason.clone()),
        }
    }
    pub fn execution_lease_count(&self) -> usize {
        match self {
            Self::Available { image, .. } => Arc::strong_count(image),
            Self::Unavailable { .. } => 0,
        }
    }
    pub fn age(&self) -> Option<Duration> {
        match self {
            Self::Available { published_at, .. } => Some(published_at.elapsed()),
            Self::Unavailable { .. } => None,
        }
    }
    pub fn last_build(&self) -> &ImageBuildReport {
        match self {
            Self::Available { last_build, .. } | Self::Unavailable { last_build, .. } => last_build,
        }
    }
    pub fn unavailable_reason(&self) -> Option<&RoutingUnavailableReason> {
        match self {
            Self::Unavailable { reason, .. } => Some(reason),
            Self::Available { .. } => None,
        }
    }
}
