use std::{error::Error, fmt};

use pathhydra_routing::RoutingError;
use pathhydra_store::{CatalogError, OperationError};

use crate::{AdmissionRejection, RoutingUnavailableReason};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EngineConfigError {
    ZeroLimit {
        limit: &'static str,
    },
    InvalidCudaValue {
        field: &'static str,
        reason: &'static str,
    },
    InvalidRoutingImageValue {
        field: &'static str,
        reason: &'static str,
    },
    InvalidMaintenanceValue {
        field: &'static str,
        reason: &'static str,
    },
}

impl fmt::Display for EngineConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroLimit { limit } => write!(formatter, "engine limit {limit} must be nonzero"),
            Self::InvalidCudaValue { field, reason } => {
                write!(formatter, "invalid CUDA configuration {field}: {reason}")
            }
            Self::InvalidRoutingImageValue { field, reason } => write!(
                formatter,
                "invalid routing-image configuration {field}: {reason}"
            ),
            Self::InvalidMaintenanceValue { field, reason } => {
                write!(
                    formatter,
                    "invalid maintenance configuration {field}: {reason}"
                )
            }
        }
    }
}

impl Error for EngineConfigError {}

#[derive(Debug)]
pub enum EngineError {
    Configuration(EngineConfigError),
    Catalog(CatalogError),
    Operation(OperationError),
    RoutingUnavailable(RoutingUnavailableReason),
    DuplicateActiveRequest(crate::RequestId),
    Admission(AdmissionRejection),
    CudaIneligible(crate::CudaIneligibility),
    CudaFailure(String),
    RestoreSmokeFailed(&'static str),
    Routing(RoutingError),
    Hydration(crate::HydrationError),
    Lifecycle(crate::LifecycleError),
    LockPoisoned { lock: &'static str },
}

impl fmt::Display for EngineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Configuration(error) => error.fmt(formatter),
            Self::Catalog(error) => error.fmt(formatter),
            Self::Operation(error) => error.fmt(formatter),
            Self::RoutingUnavailable(reason) => write!(formatter, "routing unavailable: {reason}"),
            Self::DuplicateActiveRequest(id) => {
                write!(formatter, "request ID {id} is already active")
            }
            Self::Admission(reason) => reason.fmt(formatter),
            Self::CudaIneligible(reason) => write!(formatter, "CUDA route is ineligible: {reason}"),
            Self::CudaFailure(reason) => write!(formatter, "CUDA execution failed: {reason}"),
            Self::RestoreSmokeFailed(check) => {
                write!(formatter, "restored engine failed the {check} smoke check")
            }
            Self::Routing(error) => error.fmt(formatter),
            Self::Hydration(error) => error.fmt(formatter),
            Self::Lifecycle(error) => error.fmt(formatter),
            Self::LockPoisoned { lock } => write!(formatter, "{lock} lock is poisoned"),
        }
    }
}

impl Error for EngineError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Configuration(error) => Some(error),
            Self::Catalog(error) => Some(error),
            Self::Operation(error) => Some(error),
            Self::Routing(error) => Some(error),
            Self::Hydration(error) => Some(error),
            Self::Lifecycle(error) => Some(error),
            _ => None,
        }
    }
}

impl From<CatalogError> for EngineError {
    fn from(value: CatalogError) -> Self {
        Self::Catalog(value)
    }
}
impl From<OperationError> for EngineError {
    fn from(value: OperationError) -> Self {
        Self::Operation(value)
    }
}
impl From<RoutingError> for EngineError {
    fn from(value: RoutingError) -> Self {
        Self::Routing(value)
    }
}
impl From<crate::HydrationError> for EngineError {
    fn from(value: crate::HydrationError) -> Self {
        Self::Hydration(value)
    }
}

impl From<crate::LifecycleError> for EngineError {
    fn from(value: crate::LifecycleError) -> Self {
        Self::Lifecycle(value)
    }
}
