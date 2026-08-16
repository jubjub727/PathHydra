use std::{
    fmt,
    sync::Arc,
    time::{Duration, Instant},
};

use pathhydra_routing::{RoutingImage, RoutingImageManifest};

#[derive(Debug)]
pub(crate) struct PublishedExecutionImage {
    pub cpu: Arc<RoutingImage>,
    #[cfg(feature = "cuda")]
    pub cuda: Option<Arc<pathhydra_cuda::CudaResidentImage>>,
    pub cuda_unavailable_reason: Option<crate::CudaAvailability>,
}

impl PublishedExecutionImage {
    pub fn cpu_only(image: Arc<RoutingImage>, reason: crate::CudaAvailability) -> Self {
        Self {
            cpu: image,
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
            cpu: image,
            cuda: Some(cuda),
            cuda_unavailable_reason: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RoutingUnavailableReason {
    ImageCompilation(String),
    TopologyLimit { required: usize, limit: usize },
}

impl fmt::Display for RoutingUnavailableReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ImageCompilation(reason) => formatter.write_str(reason),
            Self::TopologyLimit { required, limit } => write!(
                formatter,
                "topology requires {required} bytes; limit is {limit}"
            ),
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
            Self::Available { image, .. } => Ok(Arc::clone(&image.cpu)),
            Self::Unavailable { reason, .. } => Err(reason.clone()),
        }
    }
    pub fn manifest(&self) -> Option<&RoutingImageManifest> {
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
