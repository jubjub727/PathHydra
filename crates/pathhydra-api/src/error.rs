use std::{error::Error, fmt};

use pathhydra_engine::{
    CudaIneligibility, EngineError, HydrationError, LifecycleError, RoutingUnavailableReason,
};
use pathhydra_routing::{ProfileError, RoutingError};
use pathhydra_store::{CatalogError, OperationError, RecordKind};
use pathhydra_subgraph::SubgraphError;

use crate::DtoValidationError;

/// Stable machine-readable classification for consumer-facing failures.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum ApiErrorCategory {
    InvalidInput,
    InvalidEncoding,
    InvalidProfile,
    InvalidWeight,
    MissingCandidate,
    MissingConfirmed,
    InvalidCandidateTransition,
    ExactNameConflict,
    DurableStore,
    Corruption,
    Recovery,
    RoutingUnavailable,
    RoutingImageCorruption,
    AdmissionLimit,
    ResourceLimit,
    Cancellation,
    HydrationUnavailable,
    HydrationIntegrity,
    SubgraphConflict,
    CudaUnavailable,
    CudaIneligible,
    CudaDeviceFailure,
    Backup,
    Restore,
    Maintenance,
    Shutdown,
    InternalInvariant,
}

/// A path-free, payload-free public failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApiError {
    category: ApiErrorCategory,
    code: &'static str,
    message: &'static str,
    retryable: bool,
}

impl ApiError {
    #[must_use]
    pub const fn new(
        category: ApiErrorCategory,
        code: &'static str,
        message: &'static str,
        retryable: bool,
    ) -> Self {
        Self {
            category,
            code,
            message,
            retryable,
        }
    }

    #[must_use]
    pub const fn category(&self) -> ApiErrorCategory {
        self.category
    }

    #[must_use]
    pub const fn code(&self) -> &'static str {
        self.code
    }

    #[must_use]
    pub const fn message(&self) -> &'static str {
        self.message
    }

    #[must_use]
    pub const fn retryable(&self) -> bool {
        self.retryable
    }

    pub(crate) const fn invalid_input(code: &'static str, message: &'static str) -> Self {
        Self::new(ApiErrorCategory::InvalidInput, code, message, false)
    }

    pub(crate) const fn cancellation(code: &'static str, message: &'static str) -> Self {
        Self::new(ApiErrorCategory::Cancellation, code, message, false)
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl Error for ApiError {}

impl From<EngineError> for ApiError {
    fn from(error: EngineError) -> Self {
        match error {
            EngineError::Configuration(_) => Self::invalid_input(
                "invalid_engine_configuration",
                "the engine configuration is invalid",
            ),
            EngineError::Catalog(error) => error.into(),
            EngineError::Operation(error) => error.into(),
            EngineError::RoutingUnavailable(reason) => match reason {
                RoutingUnavailableReason::TopologyLimit { .. }
                | RoutingUnavailableReason::MetadataLimit { .. } => Self::new(
                    ApiErrorCategory::ResourceLimit,
                    "routing_image_limit",
                    "the routing image exceeds a configured resource limit",
                    false,
                ),
                RoutingUnavailableReason::ImageCompilation(_) => Self::new(
                    ApiErrorCategory::RoutingUnavailable,
                    "routing_image_unavailable",
                    "the routing image is unavailable",
                    true,
                ),
                RoutingUnavailableReason::Bundle(_) => Self::new(
                    ApiErrorCategory::RoutingImageCorruption,
                    "routing_bundle_unavailable",
                    "the durable routing bundle is unavailable",
                    true,
                ),
            },
            EngineError::DuplicateActiveRequest(_) => Self::cancellation(
                "duplicate_request_id",
                "the request ID has already been admitted",
            ),
            EngineError::Admission(_) => Self::new(
                ApiErrorCategory::AdmissionLimit,
                "route_admission_refused",
                "the route was refused by a configured admission limit",
                true,
            ),
            EngineError::CudaIneligible(reason) => match reason {
                CudaIneligibility::Disabled | CudaIneligibility::SupportNotCompiled => Self::new(
                    ApiErrorCategory::CudaUnavailable,
                    "cuda_unavailable",
                    "CUDA execution is unavailable",
                    false,
                ),
                CudaIneligibility::ResourceRefusal(_) => Self::new(
                    ApiErrorCategory::ResourceLimit,
                    "cuda_resource_refused",
                    "CUDA execution was refused by a configured resource limit",
                    true,
                ),
                _ => Self::new(
                    ApiErrorCategory::CudaIneligible,
                    "cuda_request_ineligible",
                    "the request is ineligible for CUDA execution",
                    false,
                ),
            },
            EngineError::CudaFailure(_) => Self::new(
                ApiErrorCategory::CudaDeviceFailure,
                "cuda_device_failure",
                "CUDA execution failed",
                true,
            ),
            EngineError::RestoreSmokeFailed(_) => Self::new(
                ApiErrorCategory::Restore,
                "restore_smoke_check_failed",
                "the restored catalog failed a smoke check",
                false,
            ),
            EngineError::Routing(error) => error.into(),
            EngineError::Hydration(error) => error.into(),
            EngineError::Lifecycle(error) => error.into(),
            EngineError::LockPoisoned { .. } => Self::new(
                ApiErrorCategory::InternalInvariant,
                "internal_lock_failure",
                "an internal synchronization invariant failed",
                false,
            ),
        }
    }
}

impl From<CatalogError> for ApiError {
    fn from(error: CatalogError) -> Self {
        match error {
            CatalogError::NotFound {
                kind: RecordKind::Candidate,
                ..
            } => Self::new(
                ApiErrorCategory::MissingCandidate,
                "candidate_not_found",
                "the candidate was not found",
                false,
            ),
            CatalogError::NotFound { .. }
            | CatalogError::MissingEdgeEndpoint { .. }
            | CatalogError::MissingEdgeRelationKind { .. } => Self::new(
                ApiErrorCategory::MissingConfirmed,
                "confirmed_record_not_found",
                "a required confirmed record was not found",
                false,
            ),
            CatalogError::NameAlreadyConfirmed { .. } => Self::new(
                ApiErrorCategory::ExactNameConflict,
                "exact_name_conflict",
                "the exact name is already confirmed",
                false,
            ),
            CatalogError::InvalidBatch { .. } => {
                Self::invalid_input("invalid_candidate_batch", "the candidate batch is invalid")
            }
            CatalogError::InvalidCandidateDependency { .. } => Self::new(
                ApiErrorCategory::InvalidCandidateTransition,
                "invalid_candidate_dependency",
                "the candidate dependency closure is invalid",
                false,
            ),
            CatalogError::CorruptRecord { .. } => Self::new(
                ApiErrorCategory::Corruption,
                "durable_record_corrupt",
                "a durable record failed validation",
                false,
            ),
            CatalogError::NameTooLong { .. } | CatalogError::PayloadTooLong { .. } => Self::new(
                ApiErrorCategory::ResourceLimit,
                "record_limit_exceeded",
                "the record exceeds a configured boundary limit",
                false,
            ),
            CatalogError::InvalidBaseWeight(_) => Self::new(
                ApiErrorCategory::InvalidWeight,
                "invalid_base_weight",
                "the base weight is invalid",
                false,
            ),
            CatalogError::CounterOverflow { .. } | CatalogError::StorageExhausted { .. } => {
                Self::new(
                    ApiErrorCategory::ResourceLimit,
                    "durable_resource_exhausted",
                    "a durable resource limit was exhausted",
                    false,
                )
            }
            CatalogError::InvalidConfiguration { .. } => Self::invalid_input(
                "invalid_store_configuration",
                "the durable store configuration is invalid",
            ),
            CatalogError::ValidationAborted => Self::new(
                ApiErrorCategory::Maintenance,
                "verification_cancelled",
                "catalog verification was cancelled",
                true,
            ),
            CatalogError::Io(_) | CatalogError::RocksDb(_) => Self::new(
                ApiErrorCategory::DurableStore,
                "durable_store_failure",
                "the durable store operation failed",
                true,
            ),
            CatalogError::LockPoisoned { .. } => Self::new(
                ApiErrorCategory::InternalInvariant,
                "internal_store_lock_failure",
                "an internal store synchronization invariant failed",
                false,
            ),
        }
    }
}

impl From<OperationError> for ApiError {
    fn from(error: OperationError) -> Self {
        match error {
            OperationError::Catalog(error) => error.into(),
            OperationError::InvalidPath { .. } | OperationError::UnsafePathRelationship { .. } => {
                Self::invalid_input(
                    "invalid_resource_path",
                    "an operational resource path is invalid",
                )
            }
            OperationError::DestinationNotEmpty { .. }
            | OperationError::DestinationAlreadyExists { .. } => Self::new(
                ApiErrorCategory::Backup,
                "backup_destination_conflict",
                "the backup or restore destination is not fresh",
                false,
            ),
            OperationError::ResourceRefused { .. }
            | OperationError::StorageExhausted { .. }
            | OperationError::VerificationLimit { .. } => Self::new(
                ApiErrorCategory::ResourceLimit,
                "maintenance_resource_refused",
                "maintenance was refused by a configured resource limit",
                false,
            ),
            OperationError::ConcurrentCheckpoint { .. } => Self::new(
                ApiErrorCategory::Maintenance,
                "checkpoint_busy",
                "the bounded checkpoint worker is busy",
                true,
            ),
            OperationError::BackgroundFailure { .. }
            | OperationError::Io { .. }
            | OperationError::RocksDb(_) => Self::new(
                ApiErrorCategory::DurableStore,
                "maintenance_store_failure",
                "the durable maintenance operation failed",
                true,
            ),
        }
    }
}

impl From<RoutingError> for ApiError {
    fn from(error: RoutingError) -> Self {
        match error {
            RoutingError::MissingOrigin(_) => Self::new(
                ApiErrorCategory::MissingConfirmed,
                "routing_origin_not_found",
                "the routing origin is not confirmed in the current image",
                false,
            ),
            RoutingError::InvalidProfile(ProfileError::AllocationFailed) => Self::new(
                ApiErrorCategory::ResourceLimit,
                "profile_allocation_failed",
                "the relation profile could not be allocated",
                true,
            ),
            RoutingError::InvalidProfile(_) => Self::new(
                ApiErrorCategory::InvalidProfile,
                "invalid_relation_profile",
                "the relation profile is invalid for the current image",
                false,
            ),
            RoutingError::Arithmetic(_) => Self::new(
                ApiErrorCategory::InvalidWeight,
                "non_finite_route_weight",
                "routing produced a non-finite weight",
                false,
            ),
            RoutingError::ResourceEstimateOverflow | RoutingError::AllocationFailed { .. } => {
                Self::new(
                    ApiErrorCategory::ResourceLimit,
                    "routing_resource_refused",
                    "routing could not reserve its bounded working memory",
                    true,
                )
            }
            RoutingError::ImageAccess(_) => Self::new(
                ApiErrorCategory::RoutingImageCorruption,
                "routing_image_access_failed",
                "the current routing image could not be read",
                true,
            ),
            RoutingError::InternalInvariant { .. } => Self::new(
                ApiErrorCategory::InternalInvariant,
                "routing_invariant_failed",
                "an internal routing invariant failed",
                false,
            ),
        }
    }
}

impl From<HydrationError> for ApiError {
    fn from(error: HydrationError) -> Self {
        match error {
            HydrationError::HandleLimit { .. } | HydrationError::AllocationFailed => Self::new(
                ApiErrorCategory::ResourceLimit,
                "hydration_resource_refused",
                "hydration exceeds a configured resource limit",
                false,
            ),
            HydrationError::InvalidProfile(_) => Self::new(
                ApiErrorCategory::InvalidProfile,
                "invalid_hydration_profile",
                "the hydration relation profile is invalid",
                false,
            ),
            HydrationError::RoutingUnavailable(_) => Self::new(
                ApiErrorCategory::RoutingUnavailable,
                "hydration_routing_unavailable",
                "routing is unavailable for profile validation",
                true,
            ),
            HydrationError::DestinationPositionOutOfBounds { .. }
            | HydrationError::DestinationHasNoPath { .. } => Self::invalid_input(
                "path_not_hydratable",
                "the selected destination does not contain an exact returned path",
            ),
            HydrationError::HydrationUnavailable { .. } => Self::new(
                ApiErrorCategory::HydrationUnavailable,
                "current_records_unavailable",
                "current confirmed records are unavailable for one or more handles",
                false,
            ),
            HydrationError::Integrity { .. } => Self::new(
                ApiErrorCategory::HydrationIntegrity,
                "hydration_integrity_failed",
                "hydrated records do not match the requested structural evidence",
                false,
            ),
        }
    }
}

impl From<LifecycleError> for ApiError {
    fn from(error: LifecycleError) -> Self {
        match error {
            LifecycleError::Closing | LifecycleError::ShutDown => Self::new(
                ApiErrorCategory::Shutdown,
                "engine_not_running",
                "the graph engine is closing or shut down",
                false,
            ),
            LifecycleError::MaintenanceBusy { .. } => Self::new(
                ApiErrorCategory::Maintenance,
                "maintenance_busy",
                "the bounded maintenance worker is busy",
                true,
            ),
            LifecycleError::CounterOverflow | LifecycleError::LockPoisoned => Self::new(
                ApiErrorCategory::InternalInvariant,
                "lifecycle_invariant_failed",
                "an internal lifecycle invariant failed",
                false,
            ),
        }
    }
}

impl From<SubgraphError> for ApiError {
    fn from(error: SubgraphError) -> Self {
        match error {
            SubgraphError::EdgeIdentityConflict { .. } => Self::new(
                ApiErrorCategory::SubgraphConflict,
                "subgraph_edge_conflict",
                "an edge ID has conflicting endpoint evidence",
                false,
            ),
            SubgraphError::InvalidPath { .. } => Self::invalid_input(
                "invalid_route_path",
                "the route path is not structurally valid",
            ),
        }
    }
}

impl From<DtoValidationError> for ApiError {
    fn from(error: DtoValidationError) -> Self {
        match error {
            DtoValidationError::NonCanonicalDecimal { .. }
            | DtoValidationError::NonCanonicalHex { .. }
            | DtoValidationError::NonCanonicalBase64 => Self::new(
                ApiErrorCategory::InvalidEncoding,
                "noncanonical_boundary_value",
                "a boundary value is not canonically encoded",
                false,
            ),
            DtoValidationError::InvalidNumericEvidence { .. } => Self::new(
                ApiErrorCategory::InvalidWeight,
                "invalid_numeric_evidence",
                "numeric evidence is invalid",
                false,
            ),
            DtoValidationError::DuplicateProfileRelation { .. }
            | DtoValidationError::UnorderedProfile => Self::new(
                ApiErrorCategory::InvalidProfile,
                "invalid_relation_profile",
                "the relation profile is invalid",
                false,
            ),
            DtoValidationError::UnorderedSubgraphNodes
            | DtoValidationError::UnorderedSubgraphEdges
            | DtoValidationError::MissingSubgraphEndpoint { .. } => Self::new(
                ApiErrorCategory::SubgraphConflict,
                "invalid_subgraph_handles",
                "the subgraph handle evidence is invalid",
                false,
            ),
            DtoValidationError::InvalidPath { .. }
            | DtoValidationError::InvalidRoutingResponse { .. } => Self::invalid_input(
                "invalid_routing_evidence",
                "the routing evidence is invalid",
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use pathhydra_engine::{EngineError, HydrationError};
    use pathhydra_store::{CatalogError, OperationError, ResourceKind};

    use super::{ApiError, ApiErrorCategory};

    #[test]
    fn internal_paths_names_payloads_and_reasons_are_not_retained() {
        let secret_path = r"C:\private\tenant\secret.db";
        let secret_name = "private-candidate-payload";
        let errors = [
            ApiError::from(EngineError::Operation(OperationError::InvalidPath {
                resource: ResourceKind::Restore,
                reason: secret_path.to_owned(),
            })),
            ApiError::from(EngineError::Catalog(CatalogError::NameAlreadyConfirmed {
                name: secret_name.into(),
                existing_id: pathhydra_store::ConfirmedId::Node(pathhydra_core::NodeId::from_u64(
                    9,
                )),
            })),
            ApiError::from(EngineError::CudaFailure(secret_path.to_owned())),
            ApiError::from(EngineError::Hydration(HydrationError::Integrity {
                reason: secret_name.to_owned(),
            })),
        ];

        for error in errors {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains(secret_path));
            assert!(!rendered.contains(secret_name));
            assert!(error.source().is_none());
        }
    }

    #[test]
    fn stable_categories_preserve_actionable_distinctions() {
        assert_eq!(
            ApiError::from(EngineError::Catalog(CatalogError::NotFound {
                kind: pathhydra_store::RecordKind::Candidate,
                id: 1,
            }))
            .category(),
            ApiErrorCategory::MissingCandidate
        );
        assert_eq!(
            ApiError::from(EngineError::Hydration(
                HydrationError::HydrationUnavailable {
                    node_ids: Vec::new(),
                    edge_ids: Vec::new(),
                },
            ))
            .category(),
            ApiErrorCategory::HydrationUnavailable
        );
        assert_eq!(
            ApiError::from(EngineError::CudaFailure("private reason".to_owned())).category(),
            ApiErrorCategory::CudaDeviceFailure
        );
    }
}
