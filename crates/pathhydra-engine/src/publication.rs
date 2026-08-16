use std::{
    fmt,
    sync::Arc,
    time::{Duration, Instant},
};

use pathhydra_routing::{ChunkedRoutingImage, RoutingImage, RoutingImageManifest};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CpuTopologyMode {
    Resident,
    Partitioned,
}

pub(crate) enum CpuTopology {
    Resident(Arc<RoutingImage>),
    Partitioned(Arc<ChunkedRoutingImage>),
}

impl CpuTopology {
    pub fn manifest(&self) -> RoutingImageManifest {
        match self {
            Self::Resident(image) => image.manifest().clone(),
            Self::Partitioned(image) => image
                .routing_manifest()
                .expect("validated bundle counts remain representable"),
        }
    }
    pub fn resident(&self) -> Option<Arc<RoutingImage>> {
        match self {
            Self::Resident(image) => Some(Arc::clone(image)),
            Self::Partitioned(_) => None,
        }
    }
    pub const fn mode(&self) -> CpuTopologyMode {
        match self {
            Self::Resident(_) => CpuTopologyMode::Resident,
            Self::Partitioned(_) => CpuTopologyMode::Partitioned,
        }
    }
    pub fn host_cache(&self) -> Option<pathhydra_routing::HostCacheSnapshot> {
        match self {
            Self::Resident(_) => None,
            Self::Partitioned(image) => Some(image.cache_snapshot()),
        }
    }
}

pub(crate) struct PublishedExecutionImage {
    pub cpu: CpuTopology,
    #[cfg(feature = "cuda")]
    pub cuda: Option<Arc<pathhydra_cuda::CudaResidentImage>>,
    pub cuda_unavailable_reason: Option<crate::CudaAvailability>,
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
    pub fn with_cuda(
        image: Arc<RoutingImage>,
        cuda: Arc<pathhydra_cuda::CudaResidentImage>,
    ) -> Self {
        Self {
            cpu: CpuTopology::Resident(image),
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
}

impl ImageBuildReport {
    pub(crate) fn new(
        duration: Duration,
        outcome: ImageBuildOutcome,
        counts: (usize, usize, usize),
    ) -> Self {
        Self {
            duration,
            outcome,
            node_count: counts.0,
            relation_kind_count: counts.1,
            adjacency_count: counts.2,
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
