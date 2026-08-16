mod codec;
mod compiler;
mod layout;
mod manifest;
mod reader;

pub use compiler::{
    AnalyticParallelBundleConfig, BundleBuildMetrics, BundleConfig, compile_bundle,
    generate_analytic_parallel_bundle,
};
pub use layout::{PartitionDescriptor, SegmentDescriptor};
pub use manifest::BundleManifest;
pub use reader::{
    BundleFaultInjection, BundleSnapshot, ChunkedRoutingImage, HostCacheConfig, HostCacheSnapshot,
    PartitionLease, PartitionSourceSegment, PartitionedCpuDiagnostics, RoutingBundle, open_bundle,
    route_partitioned, route_partitioned_controlled,
};

use std::{error::Error, fmt, io};

#[derive(Debug)]
pub enum BundleError {
    Io(io::Error),
    Store(pathhydra_store::CatalogError),
    Invalid(String),
    Limit {
        resource: &'static str,
        required: u64,
        limit: u64,
    },
    Routing(crate::RoutingError),
}

impl fmt::Display for BundleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "routing bundle I/O failure: {e}"),
            Self::Store(e) => write!(f, "confirmed graph scan failed: {e}"),
            Self::Invalid(reason) => write!(f, "invalid routing bundle: {reason}"),
            Self::Limit {
                resource,
                required,
                limit,
            } => write!(
                f,
                "routing bundle {resource} requires {required} bytes; limit is {limit}"
            ),
            Self::Routing(e) => e.fmt(f),
        }
    }
}

impl Error for BundleError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Store(e) => Some(e),
            Self::Routing(e) => Some(e),
            _ => None,
        }
    }
}
impl From<io::Error> for BundleError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}
impl From<pathhydra_store::CatalogError> for BundleError {
    fn from(value: pathhydra_store::CatalogError) -> Self {
        Self::Store(value)
    }
}
impl From<crate::RoutingError> for BundleError {
    fn from(value: crate::RoutingError) -> Self {
        Self::Routing(value)
    }
}
