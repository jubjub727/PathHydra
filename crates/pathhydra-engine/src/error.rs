use std::{error::Error, fmt};

use pathhydra_routing::RoutingError;
use pathhydra_store::CatalogError;

use crate::{AdmissionRejection, RoutingUnavailableReason};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EngineConfigError {
    ZeroLimit { limit: &'static str },
}

impl fmt::Display for EngineConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroLimit { limit } => write!(formatter, "engine limit {limit} must be nonzero"),
        }
    }
}

impl Error for EngineConfigError {}

#[derive(Debug)]
pub enum EngineError {
    Configuration(EngineConfigError),
    Catalog(CatalogError),
    RoutingUnavailable(RoutingUnavailableReason),
    DuplicateActiveRequest(crate::RequestId),
    Admission(AdmissionRejection),
    Routing(RoutingError),
    Hydration(crate::HydrationError),
    LockPoisoned { lock: &'static str },
}

impl fmt::Display for EngineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Configuration(error) => error.fmt(formatter),
            Self::Catalog(error) => error.fmt(formatter),
            Self::RoutingUnavailable(reason) => write!(formatter, "routing unavailable: {reason}"),
            Self::DuplicateActiveRequest(id) => {
                write!(formatter, "request ID {id} is already active")
            }
            Self::Admission(reason) => reason.fmt(formatter),
            Self::Routing(error) => error.fmt(formatter),
            Self::Hydration(error) => error.fmt(formatter),
            Self::LockPoisoned { lock } => write!(formatter, "{lock} lock is poisoned"),
        }
    }
}

impl Error for EngineError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Configuration(error) => Some(error),
            Self::Catalog(error) => Some(error),
            Self::Routing(error) => Some(error),
            Self::Hydration(error) => Some(error),
            _ => None,
        }
    }
}

impl From<CatalogError> for EngineError {
    fn from(value: CatalogError) -> Self {
        Self::Catalog(value)
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
