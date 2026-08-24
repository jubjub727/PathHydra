use pathhydra_core::{
    BaseWeight, Candidate, CandidateId, CandidateNodeReference, CandidateRelationReference,
    ConfirmedRecord, EdgeId, EdgeRecord, NodeId, NodeRecord, RelationId, RelationRecord,
};
use pathhydra_engine::{
    ActiveOperationCounts, CancellationOutcome, CudaAlgorithmSelection, CudaAvailability,
    CudaConfig, CudaExecutorPolicy, CudaHealth, CudaIneligibility, CudaRequestDiagnostics,
    CudaWorkerShutdownReport, DrainOutcome, EdgeEvaluation, EngineCapabilities, EngineConfig,
    EngineHealth, EngineLifecycleState, EngineRestoreReport, EngineRoutingResponse, Executor,
    ExecutorSelectionReason, HydratedEdge, HydratedEdgeState, HydratedNodeState, HydratedPath,
    HydratedSubgraph, HydrationRequest, HydrationResponse, ImageBuildOutcome, ImageBuildReport,
    LifecycleSnapshot, PublicationOutcome, ResourceLimitsSnapshot, RoutingHealth,
    RoutingUnavailableReason, RuntimeDiagnostics, ShutdownFailure, ShutdownFailureStage,
    ShutdownReport,
};
use pathhydra_routing::{
    CompletionReason, CpuSearchDiagnostics, DestinationState, NumericPolicy,
    PartitionedCpuDiagnostics, RelationMultiplier, RelationProfile, RelationUse, RoutePath,
    RoutingRequest, RoutingResponse, SearchBudget, TiePolicy,
};
use pathhydra_store::{
    CandidateBatchCounts, CatalogSummary, CheckpointReport, CompactionFamilyReport,
    CompactionReport, DeletionResult, MetricValue, RelationKindUsage, RestoreReport,
    StoreMetricsSnapshot, VerificationLimits, VerificationReport,
};
use pathhydra_subgraph::{Subgraph, SubgraphHandles};

use crate::dto::*;
use crate::{ApiError, ApiErrorCategory, ApiLimits};

impl From<ApiErrorCategory> for ApiErrorCategoryDto {
    fn from(value: ApiErrorCategory) -> Self {
        match value {
            ApiErrorCategory::InvalidInput => Self::InvalidInput,
            ApiErrorCategory::InvalidEncoding => Self::InvalidEncoding,
            ApiErrorCategory::InvalidProfile => Self::InvalidProfile,
            ApiErrorCategory::InvalidWeight => Self::InvalidWeight,
            ApiErrorCategory::MissingCandidate => Self::MissingCandidate,
            ApiErrorCategory::MissingConfirmed => Self::MissingConfirmed,
            ApiErrorCategory::InvalidCandidateTransition => Self::InvalidCandidateTransition,
            ApiErrorCategory::ExactNameConflict => Self::ExactNameConflict,
            ApiErrorCategory::DurableStore => Self::DurableStore,
            ApiErrorCategory::Corruption => Self::Corruption,
            ApiErrorCategory::Recovery => Self::Recovery,
            ApiErrorCategory::RoutingUnavailable => Self::RoutingUnavailable,
            ApiErrorCategory::RoutingImageCorruption => Self::RoutingImageCorruption,
            ApiErrorCategory::AdmissionLimit => Self::AdmissionLimit,
            ApiErrorCategory::ResourceLimit => Self::ResourceLimit,
            ApiErrorCategory::Cancellation => Self::Cancellation,
            ApiErrorCategory::HydrationUnavailable => Self::HydrationUnavailable,
            ApiErrorCategory::HydrationIntegrity => Self::HydrationIntegrity,
            ApiErrorCategory::SubgraphConflict => Self::SubgraphConflict,
            ApiErrorCategory::CudaUnavailable => Self::CudaUnavailable,
            ApiErrorCategory::CudaIneligible => Self::CudaIneligible,
            ApiErrorCategory::CudaDeviceFailure => Self::CudaDeviceFailure,
            ApiErrorCategory::Backup => Self::Backup,
            ApiErrorCategory::Restore => Self::Restore,
            ApiErrorCategory::Maintenance => Self::Maintenance,
            ApiErrorCategory::Shutdown => Self::Shutdown,
            ApiErrorCategory::InternalInvariant => Self::InternalInvariant,
        }
    }
}

impl From<&ApiError> for ApiErrorDto {
    fn from(value: &ApiError) -> Self {
        Self {
            category: value.category().into(),
            code: value.code().to_owned(),
            message: value.message().to_owned(),
            retryable: value.retryable(),
        }
    }
}

fn count(value: usize) -> DecimalU64Dto {
    DecimalU64Dto::from_u64(u64::try_from(value).unwrap_or(u64::MAX))
}

fn usize_from(value: &DecimalU64Dto, field: &'static str) -> Result<usize, DtoValidationError> {
    usize::try_from(value.as_u64()?)
        .map_err(|_| DtoValidationError::InvalidNumericEvidence { field })
}

macro_rules! id_conversions {
    ($internal:ty, $dto:ty) => {
        impl From<$internal> for $dto {
            fn from(value: $internal) -> Self {
                Self::from_u64(value.as_u64())
            }
        }
        impl TryFrom<&$dto> for $internal {
            type Error = DtoValidationError;
            fn try_from(value: &$dto) -> Result<Self, Self::Error> {
                Ok(Self::from_u64(value.as_u64()?))
            }
        }
    };
}

id_conversions!(CandidateId, CandidateIdDto);
id_conversions!(NodeId, NodeIdDto);
id_conversions!(RelationId, RelationIdDto);
id_conversions!(EdgeId, EdgeIdDto);
impl From<pathhydra_engine::RequestId> for RequestIdDto {
    fn from(value: pathhydra_engine::RequestId) -> Self {
        Self::from_u64(value.get())
    }
}
impl TryFrom<&RequestIdDto> for pathhydra_engine::RequestId {
    type Error = DtoValidationError;
    fn try_from(value: &RequestIdDto) -> Result<Self, Self::Error> {
        Ok(Self::new(value.as_u64()?))
    }
}

impl From<&Candidate> for CandidateDto {
    fn from(value: &Candidate) -> Self {
        match value {
            Candidate::Node {
                id,
                name,
                payload,
                incoming_reference_count,
            } => Self::Node {
                id: (*id).into(),
                name: name.as_str().to_owned(),
                payload: PayloadDto::from_bytes(payload.as_bytes()),
                incoming_reference_count: DecimalU64Dto::from_u64(*incoming_reference_count),
            },
            Candidate::Relation {
                id,
                name,
                incoming_reference_count,
            } => Self::RelationKind {
                id: (*id).into(),
                name: name.as_str().to_owned(),
                incoming_reference_count: DecimalU64Dto::from_u64(*incoming_reference_count),
            },
            Candidate::Edge {
                id,
                source,
                destination,
                relation_kind,
                base_weight,
            } => Self::Edge {
                id: (*id).into(),
                source: (*source).into(),
                destination: (*destination).into(),
                relation_kind: (*relation_kind).into(),
                base_weight: Binary32Dto::from_bits(base_weight.to_bits()),
            },
        }
    }
}

impl From<CandidateNodeReference> for CandidateNodeReferenceDto {
    fn from(value: CandidateNodeReference) -> Self {
        match value {
            CandidateNodeReference::Confirmed(id) => Self::ConfirmedNode { id: id.into() },
            CandidateNodeReference::Candidate(id) => Self::Candidate { id: id.into() },
        }
    }
}

impl From<CandidateRelationReference> for CandidateRelationReferenceDto {
    fn from(value: CandidateRelationReference) -> Self {
        match value {
            CandidateRelationReference::Confirmed(id) => {
                Self::ConfirmedRelationKind { id: id.into() }
            }
            CandidateRelationReference::Candidate(id) => Self::Candidate { id: id.into() },
        }
    }
}

impl From<CandidateBatchCounts> for CandidateBatchCountsDto {
    fn from(value: CandidateBatchCounts) -> Self {
        Self {
            nodes: count(value.nodes),
            relation_kinds: count(value.relation_kinds),
            edges: count(value.edges),
        }
    }
}

impl From<&RelationKindUsage> for RelationKindUsageDto {
    fn from(value: &RelationKindUsage) -> Self {
        let record = &value.relation_kind;
        Self {
            relation_kind: record.into(),
            provisional_reference_count: DecimalU64Dto::from_u64(
                record.provisional_reference_count(),
            ),
            confirmed_edge_count: DecimalU64Dto::from_u64(record.confirmed_edge_count()),
            total_reference_count: DecimalU64Dto::from_u64(
                record
                    .total_reference_count()
                    .expect("verified relation usage total remains representable"),
            ),
        }
    }
}

impl From<&NodeRecord> for NodeRecordDto {
    fn from(value: &NodeRecord) -> Self {
        Self {
            id: value.id().into(),
            name: value.name().as_str().to_owned(),
            payload: PayloadDto::from_bytes(value.payload().as_bytes()),
        }
    }
}
impl From<&RelationRecord> for RelationKindRecordDto {
    fn from(value: &RelationRecord) -> Self {
        Self {
            id: value.id().into(),
            name: value.name().as_str().to_owned(),
        }
    }
}
impl From<&EdgeRecord> for EdgeRecordDto {
    fn from(value: &EdgeRecord) -> Self {
        Self {
            id: value.id().into(),
            source: value.source().into(),
            destination: value.destination().into(),
            relation_kind: value.relation_kind().into(),
            base_weight: Binary32Dto::from_bits(value.base_weight().to_bits()),
        }
    }
}
impl From<&ConfirmedRecord> for ConfirmedRecordDto {
    fn from(value: &ConfirmedRecord) -> Self {
        match value {
            ConfirmedRecord::Node(value) => Self::Node(value.into()),
            ConfirmedRecord::Relation(value) => Self::RelationKind(value.into()),
            ConfirmedRecord::Edge(value) => Self::Edge(value.into()),
        }
    }
}

impl TryFrom<&Binary32Dto> for BaseWeight {
    type Error = DtoValidationError;
    fn try_from(value: &Binary32Dto) -> Result<Self, Self::Error> {
        BaseWeight::from_bits(value.bits()?).map_err(|_| {
            DtoValidationError::InvalidNumericEvidence {
                field: "base weight",
            }
        })
    }
}
impl TryFrom<&Binary32Dto> for RelationMultiplier {
    type Error = DtoValidationError;
    fn try_from(value: &Binary32Dto) -> Result<Self, Self::Error> {
        RelationMultiplier::from_bits(value.bits()?).map_err(|_| {
            DtoValidationError::InvalidNumericEvidence {
                field: "relation multiplier",
            }
        })
    }
}

impl From<RelationUse> for RelationUseDto {
    fn from(value: RelationUse) -> Self {
        match value {
            RelationUse::Disabled => Self::Disabled,
            RelationUse::Enabled(value) => Self::Enabled {
                multiplier: Binary32Dto::from_bits(value.to_bits()),
            },
        }
    }
}
impl TryFrom<&RelationUseDto> for RelationUse {
    type Error = DtoValidationError;
    fn try_from(value: &RelationUseDto) -> Result<Self, Self::Error> {
        match value {
            RelationUseDto::Disabled => Ok(Self::Disabled),
            RelationUseDto::Enabled { multiplier } => Ok(Self::Enabled(multiplier.try_into()?)),
        }
    }
}
impl From<&RelationProfile> for RelationProfileDto {
    fn from(value: &RelationProfile) -> Self {
        let mut entries: Vec<_> = value
            .entries()
            .iter()
            .map(|(id, use_)| RelationProfileEntryDto {
                relation_kind: (*id).into(),
                relation_use: (*use_).into(),
            })
            .collect();
        entries.sort_by(|left, right| left.relation_kind.cmp(&right.relation_kind));
        Self { entries }
    }
}
impl TryFrom<&RelationProfileDto> for RelationProfile {
    type Error = DtoValidationError;
    fn try_from(value: &RelationProfileDto) -> Result<Self, Self::Error> {
        value.validate()?;
        value
            .entries
            .iter()
            .map(|entry| {
                Ok((
                    (&entry.relation_kind).try_into()?,
                    (&entry.relation_use).try_into()?,
                ))
            })
            .collect::<Result<Vec<_>, DtoValidationError>>()
            .map(Self::new)
    }
}

impl From<SearchBudget> for SearchBudgetDto {
    fn from(value: SearchBudget) -> Self {
        match value {
            SearchBudget::Unlimited => Self::Unlimited,
            SearchBudget::ExaminedEdges(maximum) => Self::ExaminedEdges {
                maximum: DecimalU64Dto::from_u64(maximum),
            },
        }
    }
}
impl TryFrom<&SearchBudgetDto> for SearchBudget {
    type Error = DtoValidationError;
    fn try_from(value: &SearchBudgetDto) -> Result<Self, Self::Error> {
        Ok(match value {
            SearchBudgetDto::Unlimited => Self::Unlimited,
            SearchBudgetDto::ExaminedEdges { maximum } => Self::ExaminedEdges(maximum.as_u64()?),
        })
    }
}
impl From<TiePolicy> for TiePolicyDto {
    fn from(_: TiePolicy) -> Self {
        Self::StablePredecessor
    }
}
impl From<&TiePolicyDto> for TiePolicy {
    fn from(_: &TiePolicyDto) -> Self {
        Self::StablePredecessor
    }
}
impl From<NumericPolicy> for NumericPolicyDto {
    fn from(_: NumericPolicy) -> Self {
        Self::Binary32OperandsSeparateBinary64
    }
}
impl From<CompletionReason> for CompletionReasonDto {
    fn from(value: CompletionReason) -> Self {
        match value {
            CompletionReason::AllDestinationsFinalized => Self::AllDestinationsFinalized,
            CompletionReason::FrontierExhausted => Self::FrontierExhausted,
            CompletionReason::BudgetExhausted => Self::BudgetExhausted,
            CompletionReason::Cancelled => Self::Cancelled,
        }
    }
}

impl TryFrom<&RoutingRequestDto> for RoutingRequest {
    type Error = DtoValidationError;
    fn try_from(value: &RoutingRequestDto) -> Result<Self, Self::Error> {
        value.validate()?;
        Ok(Self::new(
            (&value.origin).try_into()?,
            value
                .destinations
                .iter()
                .map(NodeId::try_from)
                .collect::<Result<Vec<_>, _>>()?,
            (&value.profile).try_into()?,
            value.return_paths,
            (&value.budget).try_into()?,
            (&value.tie_policy).into(),
        ))
    }
}

fn route_path(value: &RoutePath) -> RoutePathDto {
    RoutePathDto {
        origin: value.origin().into(),
        destination: value.destination().into(),
        logical_distance: Binary64Dto::from_bits(value.logical_distance().to_bits()),
        steps: value
            .steps()
            .iter()
            .map(|step| PathStepDto {
                edge: step.edge_id().into(),
                source: step.source().into(),
                destination: step.destination().into(),
                relation_kind: step.relation_id().into(),
                base_weight: Binary32Dto::from_bits(step.base_weight().to_bits()),
                multiplier: Binary32Dto::from_bits(step.multiplier().to_bits()),
                effective_weight: Binary64Dto::from_bits(step.effective_weight().to_bits()),
            })
            .collect(),
    }
}
impl From<&RoutingResponse> for RoutingResponseDto {
    fn from(value: &RoutingResponse) -> Self {
        Self {
            origin: value.origin().into(),
            results: value
                .results()
                .iter()
                .map(|result| DestinationResultDto {
                    destination: result.destination().into(),
                    state: match result.state() {
                        DestinationState::Exact(exact) => DestinationStateDto::Exact {
                            logical_distance: Binary64Dto::from_bits(
                                exact.logical_distance().to_bits(),
                            ),
                            path: exact.path().map(route_path),
                        },
                        DestinationState::Unreachable => DestinationStateDto::Unreachable,
                        DestinationState::MissingNode => DestinationStateDto::MissingNode,
                        DestinationState::Incomplete => DestinationStateDto::Incomplete,
                    },
                })
                .collect(),
            profile: value.profile().into(),
            numeric_policy: value.numeric_policy().into(),
            tie_policy: value.tie_policy().into(),
            paths_requested: value.paths_requested(),
            examined_edges: DecimalU64Dto::from_u64(value.examined_edges()),
            finalized_nodes: DecimalU64Dto::from_u64(value.finalized_nodes()),
            completion_reason: value.completion_reason().into(),
        }
    }
}

impl TryFrom<&HydrationRequestDto> for HydrationRequest {
    type Error = DtoValidationError;
    fn try_from(value: &HydrationRequestDto) -> Result<Self, Self::Error> {
        value.validate()?;
        Ok(Self::new(
            value
                .node_ids
                .iter()
                .map(NodeId::try_from)
                .collect::<Result<Vec<_>, _>>()?,
            value
                .edge_ids
                .iter()
                .map(EdgeId::try_from)
                .collect::<Result<Vec<_>, _>>()?,
            value
                .profile
                .as_ref()
                .map(RelationProfile::try_from)
                .transpose()?,
        ))
    }
}

fn hydrated_edge(value: &HydratedEdge) -> HydratedEdgeDto {
    HydratedEdgeDto {
        edge: (&value.edge).into(),
        relation_kind: (&value.relation_kind).into(),
        evaluation: match value.evaluation {
            EdgeEvaluation::Unprofiled => EdgeEvaluationDto::Unprofiled,
            EdgeEvaluation::Disabled => EdgeEvaluationDto::Disabled,
            EdgeEvaluation::Enabled {
                multiplier,
                effective_weight,
            } => EdgeEvaluationDto::Enabled {
                multiplier: Binary32Dto::from_bits(multiplier.to_bits()),
                effective_weight: Binary64Dto::from_bits(effective_weight.to_bits()),
            },
        },
    }
}

fn hydration_diagnostics(
    requested_nodes: usize,
    requested_edges: usize,
    found_nodes: usize,
    found_edges: usize,
) -> HydrationDiagnosticsDto {
    HydrationDiagnosticsDto {
        execution_duration: DurationDto::default(),
        requested_nodes: count(requested_nodes),
        requested_edges: count(requested_edges),
        found_nodes: count(found_nodes),
        missing_nodes: count(requested_nodes.saturating_sub(found_nodes)),
        found_edges: count(found_edges),
        missing_edges: count(requested_edges.saturating_sub(found_edges)),
    }
}

impl From<&HydrationResponse> for HydrationResponseDto {
    fn from(value: &HydrationResponse) -> Self {
        let found_nodes = value
            .nodes
            .iter()
            .filter(|result| matches!(&result.state, HydratedNodeState::Found(_)))
            .count();
        let found_edges = value
            .edges
            .iter()
            .filter(|result| matches!(&result.state, HydratedEdgeState::Found(_)))
            .count();
        Self {
            nodes: value
                .nodes
                .iter()
                .map(|result| HydratedNodeResultDto {
                    requested_node_id: result.requested_node_id.into(),
                    state: match &result.state {
                        HydratedNodeState::Found(node) => {
                            HydratedNodeStateDto::Found { node: node.into() }
                        }
                        HydratedNodeState::Missing => HydratedNodeStateDto::Missing,
                    },
                })
                .collect(),
            edges: value
                .edges
                .iter()
                .map(|result| HydratedEdgeResultDto {
                    requested_edge_id: result.requested_edge_id.into(),
                    state: match &result.state {
                        HydratedEdgeState::Found(edge) => HydratedEdgeStateDto::Found {
                            edge: Box::new(hydrated_edge(edge)),
                        },
                        HydratedEdgeState::Missing => HydratedEdgeStateDto::Missing,
                    },
                })
                .collect(),
            profile: value.profile.as_ref().map(Into::into),
            diagnostics: hydration_diagnostics(
                value.nodes.len(),
                value.edges.len(),
                found_nodes,
                found_edges,
            ),
        }
    }
}
impl From<&HydratedPath> for HydratedPathDto {
    fn from(value: &HydratedPath) -> Self {
        Self {
            nodes: value.nodes.iter().map(Into::into).collect(),
            edges: value.edges.iter().map(hydrated_edge).collect(),
            logical_distance: Binary64Dto::from_bits(value.logical_distance.to_bits()),
            numeric_policy: value.numeric_policy.into(),
            tie_policy: value.tie_policy.into(),
            profile: (&value.profile).into(),
            diagnostics: hydration_diagnostics(
                value.nodes.len(),
                value.edges.len(),
                value.nodes.len(),
                value.edges.len(),
            ),
        }
    }
}
impl From<&HydratedSubgraph> for HydratedSubgraphDto {
    fn from(value: &HydratedSubgraph) -> Self {
        Self {
            nodes: value.nodes.iter().map(Into::into).collect(),
            edges: value.edges.iter().map(hydrated_edge).collect(),
            missing_node_ids: value
                .missing_node_ids
                .iter()
                .copied()
                .map(Into::into)
                .collect(),
            missing_edge_ids: value
                .missing_edge_ids
                .iter()
                .copied()
                .map(Into::into)
                .collect(),
            profile: value.profile.as_ref().map(Into::into),
            complete: value.complete,
            diagnostics: hydration_diagnostics(
                value
                    .nodes
                    .len()
                    .saturating_add(value.missing_node_ids.len()),
                value
                    .edges
                    .len()
                    .saturating_add(value.missing_edge_ids.len()),
                value.nodes.len(),
                value.edges.len(),
            ),
        }
    }
}

impl From<&SubgraphHandles> for SubgraphHandlesDto {
    fn from(value: &SubgraphHandles) -> Self {
        Self {
            nodes: value.nodes().iter().copied().map(Into::into).collect(),
            edges: value
                .edges()
                .iter()
                .map(|edge| EdgeHandleDto {
                    edge: edge.edge_id().into(),
                    source: edge.source().into(),
                    destination: edge.destination().into(),
                })
                .collect(),
        }
    }
}
impl TryFrom<&SubgraphHandlesDto> for Subgraph {
    type Error = DtoValidationError;
    fn try_from(value: &SubgraphHandlesDto) -> Result<Self, Self::Error> {
        value.validate()?;
        let mut subgraph = Self::new();
        for node in &value.nodes {
            subgraph.add_node(node.try_into()?);
        }
        for edge in &value.edges {
            subgraph
                .add_edge(
                    (&edge.edge).try_into()?,
                    (&edge.source).try_into()?,
                    (&edge.destination).try_into()?,
                )
                .map_err(|_| DtoValidationError::InvalidRoutingResponse {
                    reason: "subgraph edge identity conflict",
                })?;
        }
        Ok(subgraph)
    }
}

impl From<Executor> for ExecutorDto {
    fn from(value: Executor) -> Self {
        match value {
            Executor::CpuReference => Self::CpuReference,
            Executor::NvidiaCuda => Self::NvidiaCuda,
        }
    }
}
impl From<ExecutorSelectionReason> for ExecutorSelectionReasonDto {
    fn from(value: ExecutorSelectionReason) -> Self {
        match value {
            ExecutorSelectionReason::CpuOnlyPolicy => Self::CpuOnlyPolicy,
            ExecutorSelectionReason::PreferredCudaEligible => Self::PreferredCudaEligible,
            ExecutorSelectionReason::RequiredCudaEligible => Self::RequiredCudaEligible,
            ExecutorSelectionReason::AutomaticPolicySelectedCpu => Self::AutomaticPolicySelectedCpu,
            ExecutorSelectionReason::CpuFallback => Self::CpuFallback,
        }
    }
}
impl From<&CudaIneligibility> for CudaIneligibilityDto {
    fn from(value: &CudaIneligibility) -> Self {
        match value {
            CudaIneligibility::Disabled => Self::Disabled,
            CudaIneligibility::SupportNotCompiled => Self::SupportNotCompiled,
            CudaIneligibility::FiniteEdgeBudgetUnsupportedByCuda => {
                Self::FiniteEdgeBudgetUnsupportedByCuda
            }
            CudaIneligibility::NoResidentImage(_) => Self::NoResidentImage,
            CudaIneligibility::ResourceRefusal(_) => Self::ResourceRefusal,
            CudaIneligibility::AutomaticPolicySelectedCpu => Self::AutomaticPolicySelectedCpu,
            CudaIneligibility::Unhealthy(_) => Self::Unhealthy,
        }
    }
}
impl From<&CpuSearchDiagnostics> for CpuSearchDiagnosticsDto {
    fn from(value: &CpuSearchDiagnostics) -> Self {
        Self {
            examined_edges: DecimalU64Dto::from_u64(value.examined_edges),
            relaxation_updates: DecimalU64Dto::from_u64(value.relaxation_updates),
            finalized_nodes: DecimalU64Dto::from_u64(value.finalized_nodes),
            frontier_high_water_mark: count(value.frontier_high_water_mark),
            unique_present_destinations: count(value.unique_present_destinations),
            exact_destinations: count(value.exact_destinations),
            unreachable_destinations: count(value.unreachable_destinations),
            missing_destinations: count(value.missing_destinations),
            incomplete_destinations: count(value.incomplete_destinations),
            path_reconstruction_steps: DecimalU64Dto::from_u64(value.path_reconstruction_steps),
            first_destination_duration: value
                .first_destination_duration
                .map(DurationDto::from_duration),
            path_reconstruction_duration: DurationDto::from_duration(
                value.path_reconstruction_duration,
            ),
        }
    }
}
impl From<&PartitionedCpuDiagnostics> for PartitionedCpuDiagnosticsDto {
    fn from(value: &PartitionedCpuDiagnostics) -> Self {
        Self {
            partitions: DecimalU64Dto::from_u64(value.partitions),
            file_bytes: DecimalU64Dto::from_u64(value.file_bytes),
            io_wait: DurationDto::from_duration(value.io_wait),
            cache_hits: DecimalU64Dto::from_u64(value.cache.hits),
            cache_misses: DecimalU64Dto::from_u64(value.cache.misses),
            cache_current_bytes: DecimalU64Dto::from_u64(value.cache.current_bytes),
            cache_high_water_bytes: DecimalU64Dto::from_u64(value.cache.high_water_bytes),
            cache_entries: count(value.cache.entries),
            cache_queue_depth: count(value.cache.queue_depth),
        }
    }
}
impl From<&CudaRequestDiagnostics> for CudaRequestDiagnosticsDto {
    fn from(value: &CudaRequestDiagnostics) -> Self {
        Self {
            algorithm: value.algorithm.to_owned(),
            delta: value.delta.map(|v| Binary64Dto::from_bits(v.to_bits())),
            queue_duration: DurationDto::from_duration(value.queue_duration),
            batch_collection_duration: DurationDto::from_duration(value.batch_collection_duration),
            batch_width: count(value.batch_width),
            lane_index: count(value.lane_index),
            topology_bytes: count(value.topology_bytes),
            search_bytes: count(value.search_bytes),
            host_to_device_bytes: count(value.host_to_device_bytes),
            device_to_host_bytes: count(value.device_to_host_bytes),
            kernel_launches: DecimalU64Dto::from_u64(value.kernel_launches),
            synchronized_execution_duration: DurationDto::from_duration(
                value.synchronized_execution_duration,
            ),
            examined_edges: DecimalU64Dto::from_u64(value.examined_edges),
            relaxation_attempts: DecimalU64Dto::from_u64(value.relaxation_attempts),
            relaxation_updates: DecimalU64Dto::from_u64(value.relaxation_updates),
            phases: DecimalU64Dto::from_u64(value.phases),
            partitions_required: DecimalU64Dto::from_u64(value.partitions_required),
            host_cache_hits: DecimalU64Dto::from_u64(value.host_cache_hits),
            device_cache_hits: DecimalU64Dto::from_u64(value.device_cache_hits),
            file_bytes: DecimalU64Dto::from_u64(value.file_bytes),
            staged_bytes: DecimalU64Dto::from_u64(value.staged_bytes),
            transfer_bytes: DecimalU64Dto::from_u64(value.transfer_bytes),
            parallel_strategy: value.parallel_strategy.to_owned(),
            reset_mode: value.reset_mode.to_owned(),
            target_mode: value.target_mode.to_owned(),
            profile_mode: value.profile_mode.to_owned(),
            path_evidence_mode: value.path_evidence_mode.to_owned(),
            state_initialization_duration: DurationDto::from_duration(
                value.state_initialization_duration,
            ),
            partition_scheduling_duration: DurationDto::from_duration(
                value.partition_scheduling_duration,
            ),
            relation_relaxation_duration: DurationDto::from_duration(
                value.relation_relaxation_duration,
            ),
            response_transfer_duration: DurationDto::from_duration(
                value.response_transfer_duration,
            ),
            frontier_compaction_duration: DurationDto::from_duration(
                value.frontier_compaction_duration,
            ),
            compacted_task_count: DecimalU64Dto::from_u64(u64::from(value.compacted_task_count)),
            destination_completion_duration: DurationDto::from_duration(
                value.destination_completion_duration,
            ),
            destination_count_checked: count(value.destination_count_checked),
            atomic_cas_retries: DecimalU64Dto::from_u64(value.atomic_cas_retries),
            first_destination_duration: value
                .first_destination_duration
                .map(DurationDto::from_duration),
            path_reconstruction_duration: DurationDto::from_duration(
                value.path_reconstruction_duration,
            ),
        }
    }
}
impl From<&RuntimeDiagnostics> for RuntimeDiagnosticsDto {
    fn from(value: &RuntimeDiagnostics) -> Self {
        Self {
            request_id: value.request_id.into(),
            executor: value.executor.into(),
            selection_reason: value.selection_reason.into(),
            attempted_cuda: value.attempted_cuda,
            cuda_fallback_reason: value.cuda_fallback_reason.as_ref().map(Into::into),
            numeric_policy: value.numeric_policy.into(),
            tie_policy: value.tie_policy.into(),
            image_node_count: count(value.image_node_count),
            image_relation_kind_count: count(value.image_relation_kind_count),
            image_adjacency_count: count(value.image_adjacency_count),
            reserved_working_bytes: count(value.reserved_working_bytes),
            admission_duration: DurationDto::from_duration(value.admission_duration),
            execution_duration: DurationDto::from_duration(value.execution_duration),
            first_destination_duration: value
                .first_destination_duration
                .map(DurationDto::from_duration),
            reconstruction_duration: DurationDto::from_duration(value.reconstruction_duration),
            completion_reason: value.completion_reason.into(),
            search: (&value.search).into(),
            partitioned_cpu: value.partitioned_cpu.as_ref().map(Into::into),
            cuda: value.cuda.as_ref().map(Into::into),
        }
    }
}
impl From<&EngineRoutingResponse> for EngineRoutingResponseDto {
    fn from(value: &EngineRoutingResponse) -> Self {
        Self {
            response: (&value.response).into(),
            diagnostics: (&value.diagnostics).into(),
        }
    }
}

impl From<CancellationOutcome> for CancellationOutcomeDto {
    fn from(value: CancellationOutcome) -> Self {
        match value {
            CancellationOutcome::Signalled => Self::Signalled,
            CancellationOutcome::AlreadySignalled => Self::AlreadySignalled,
            CancellationOutcome::NotActive => Self::UnknownRequest,
        }
    }
}
impl From<CudaExecutorPolicy> for CudaExecutorPolicyDto {
    fn from(value: CudaExecutorPolicy) -> Self {
        match value {
            CudaExecutorPolicy::CpuOnly => Self::CpuOnly,
            CudaExecutorPolicy::PreferCuda => Self::PreferCuda,
            CudaExecutorPolicy::RequireCuda => Self::RequireCuda,
            CudaExecutorPolicy::Auto => Self::Auto,
        }
    }
}
impl From<&CudaExecutorPolicyDto> for CudaExecutorPolicy {
    fn from(value: &CudaExecutorPolicyDto) -> Self {
        match value {
            CudaExecutorPolicyDto::CpuOnly => Self::CpuOnly,
            CudaExecutorPolicyDto::PreferCuda => Self::PreferCuda,
            CudaExecutorPolicyDto::RequireCuda => Self::RequireCuda,
            CudaExecutorPolicyDto::Auto => Self::Auto,
        }
    }
}
impl From<CudaAlgorithmSelection> for CudaAlgorithmDto {
    fn from(value: CudaAlgorithmSelection) -> Self {
        match value {
            CudaAlgorithmSelection::Frontier => Self::Frontier,
            CudaAlgorithmSelection::DeltaStepping { delta } => Self::DeltaStepping {
                delta: Binary64Dto::from_bits(delta.to_bits()),
            },
            CudaAlgorithmSelection::Automatic => Self::Automatic,
        }
    }
}
impl TryFrom<&CudaAlgorithmDto> for CudaAlgorithmSelection {
    type Error = DtoValidationError;
    fn try_from(value: &CudaAlgorithmDto) -> Result<Self, Self::Error> {
        Ok(match value {
            CudaAlgorithmDto::Frontier => Self::Frontier,
            CudaAlgorithmDto::Automatic => Self::Automatic,
            CudaAlgorithmDto::DeltaStepping { delta } => {
                let delta = f64::from_bits(delta.bits()?);
                if !delta.is_finite() || delta <= 0.0 {
                    return Err(DtoValidationError::InvalidNumericEvidence {
                        field: "CUDA delta",
                    });
                }
                Self::DeltaStepping { delta }
            }
        })
    }
}
impl From<&CudaConfig> for CudaConfigDto {
    fn from(value: &CudaConfig) -> Self {
        Self {
            enabled: value.enabled,
            device_ordinal: count(value.device_ordinal),
            executor_policy: value.executor_policy.into(),
            maximum_topology_bytes: count(value.maximum_topology_bytes),
            maximum_partitioned_topology_cache_bytes: count(
                value.maximum_partitioned_topology_cache_bytes,
            ),
            maximum_partitioned_topology_cache_slots: count(
                value.maximum_partitioned_topology_cache_slots,
            ),
            maximum_partitioned_host_staging_bytes: count(
                value.maximum_partitioned_host_staging_bytes,
            ),
            minimum_free_memory_headroom: count(value.minimum_free_memory_headroom),
            maximum_concurrent_searches: count(value.maximum_concurrent_searches),
            maximum_batch_lanes: count(value.maximum_batch_lanes),
            maximum_reserved_search_bytes: count(value.maximum_reserved_search_bytes),
            batch_collection_delay: DurationDto::from_duration(value.batch_collection_delay),
            algorithm: value.algorithm.into(),
        }
    }
}
impl TryFrom<&CudaConfigDto> for CudaConfig {
    type Error = DtoValidationError;
    fn try_from(value: &CudaConfigDto) -> Result<Self, Self::Error> {
        Ok(Self {
            enabled: value.enabled,
            device_ordinal: usize_from(&value.device_ordinal, "CUDA device ordinal")?,
            executor_policy: (&value.executor_policy).into(),
            maximum_topology_bytes: usize_from(
                &value.maximum_topology_bytes,
                "maximum CUDA topology bytes",
            )?,
            maximum_partitioned_topology_cache_bytes: usize_from(
                &value.maximum_partitioned_topology_cache_bytes,
                "maximum CUDA partition cache bytes",
            )?,
            maximum_partitioned_topology_cache_slots: usize_from(
                &value.maximum_partitioned_topology_cache_slots,
                "maximum CUDA partition cache slots",
            )?,
            maximum_partitioned_host_staging_bytes: usize_from(
                &value.maximum_partitioned_host_staging_bytes,
                "maximum CUDA host staging bytes",
            )?,
            minimum_free_memory_headroom: usize_from(
                &value.minimum_free_memory_headroom,
                "minimum CUDA memory headroom",
            )?,
            maximum_concurrent_searches: usize_from(
                &value.maximum_concurrent_searches,
                "maximum CUDA searches",
            )?,
            maximum_batch_lanes: usize_from(
                &value.maximum_batch_lanes,
                "maximum CUDA batch lanes",
            )?,
            maximum_reserved_search_bytes: usize_from(
                &value.maximum_reserved_search_bytes,
                "maximum CUDA search bytes",
            )?,
            batch_collection_delay: value.batch_collection_delay.to_duration()?,
            algorithm: (&value.algorithm).try_into()?,
        })
    }
}
impl From<&EngineConfig> for PathHydraConfigDto {
    fn from(value: &EngineConfig) -> Self {
        Self {
            maximum_active_image_bytes: count(value.max_active_image_bytes),
            maximum_concurrent_routes: count(value.max_concurrent_routes),
            maximum_reserved_route_bytes: count(value.max_reserved_route_bytes),
            maximum_destinations_per_request: count(value.max_destinations_per_request),
            maximum_hydration_handles_per_request: count(value.max_hydration_handles_per_request),
            maximum_total_bundle_bytes: DecimalU64Dto::from_u64(value.max_total_bundle_bytes),
            target_partition_topology_bytes: DecimalU64Dto::from_u64(
                value.target_partition_topology_bytes,
            ),
            hard_maximum_partition_topology_bytes: DecimalU64Dto::from_u64(
                value.hard_maximum_partition_topology_bytes,
            ),
            maximum_resident_image_metadata_bytes: DecimalU64Dto::from_u64(
                value.max_resident_image_metadata_bytes,
            ),
            host_partition_cache_bytes: DecimalU64Dto::from_u64(value.host_partition_cache_bytes),
            host_partition_cache_entries: count(value.host_partition_cache_entries),
            routing_io_workers: count(value.routing_io_worker_count),
            maximum_queued_partition_reads: count(value.maximum_queued_partition_reads),
            routing_io_staging_bytes: DecimalU64Dto::from_u64(value.routing_io_staging_bytes),
            maximum_retired_bundle_bytes: DecimalU64Dto::from_u64(
                value.maximum_retired_bundle_bytes,
            ),
            maximum_retired_bundle_count: count(value.maximum_retired_bundle_count),
            maximum_concurrent_checkpoints: count(value.maximum_concurrent_checkpoints),
            maximum_maintenance_workers: count(value.maximum_maintenance_workers),
            maximum_queued_maintenance: count(value.maximum_queued_maintenance),
            maximum_verification_records: DecimalU64Dto::from_u64(
                value.maximum_verification_records,
            ),
            maximum_verification_duration: DurationDto::from_duration(
                value.maximum_verification_duration,
            ),
            shutdown_drain_timeout: DurationDto::from_duration(value.shutdown_drain_timeout),
            startup_bundle_policy: value.startup_bundle_policy.into(),
            cuda: (&value.cuda).into(),
        }
    }
}
impl TryFrom<&PathHydraConfigDto> for EngineConfig {
    type Error = DtoValidationError;
    fn try_from(value: &PathHydraConfigDto) -> Result<Self, Self::Error> {
        Ok(Self {
            max_active_image_bytes: usize_from(
                &value.maximum_active_image_bytes,
                "maximum active image bytes",
            )?,
            max_concurrent_routes: usize_from(
                &value.maximum_concurrent_routes,
                "maximum concurrent routes",
            )?,
            max_reserved_route_bytes: usize_from(
                &value.maximum_reserved_route_bytes,
                "maximum reserved route bytes",
            )?,
            max_destinations_per_request: usize_from(
                &value.maximum_destinations_per_request,
                "maximum destinations",
            )?,
            max_hydration_handles_per_request: usize_from(
                &value.maximum_hydration_handles_per_request,
                "maximum hydration handles",
            )?,
            routing_image_root: None,
            max_total_bundle_bytes: value.maximum_total_bundle_bytes.as_u64()?,
            target_partition_topology_bytes: value.target_partition_topology_bytes.as_u64()?,
            hard_maximum_partition_topology_bytes: value
                .hard_maximum_partition_topology_bytes
                .as_u64()?,
            max_resident_image_metadata_bytes: value
                .maximum_resident_image_metadata_bytes
                .as_u64()?,
            host_partition_cache_bytes: value.host_partition_cache_bytes.as_u64()?,
            host_partition_cache_entries: usize_from(
                &value.host_partition_cache_entries,
                "host partition cache entries",
            )?,
            routing_io_worker_count: usize_from(&value.routing_io_workers, "routing IO workers")?,
            maximum_queued_partition_reads: usize_from(
                &value.maximum_queued_partition_reads,
                "queued partition reads",
            )?,
            routing_io_staging_bytes: value.routing_io_staging_bytes.as_u64()?,
            maximum_retired_bundle_bytes: value.maximum_retired_bundle_bytes.as_u64()?,
            maximum_retired_bundle_count: usize_from(
                &value.maximum_retired_bundle_count,
                "retired bundle count",
            )?,
            maximum_concurrent_checkpoints: usize_from(
                &value.maximum_concurrent_checkpoints,
                "concurrent checkpoints",
            )?,
            maximum_maintenance_workers: usize_from(
                &value.maximum_maintenance_workers,
                "maintenance workers",
            )?,
            maximum_queued_maintenance: usize_from(
                &value.maximum_queued_maintenance,
                "queued maintenance",
            )?,
            maximum_verification_records: value.maximum_verification_records.as_u64()?,
            maximum_verification_duration: value.maximum_verification_duration.to_duration()?,
            shutdown_drain_timeout: value.shutdown_drain_timeout.to_duration()?,
            startup_bundle_policy: value.startup_bundle_policy.into(),
            cuda: (&value.cuda).try_into()?,
        })
    }
}
impl From<pathhydra_engine::StartupBundlePolicy> for StartupBundlePolicyDto {
    fn from(value: pathhydra_engine::StartupBundlePolicy) -> Self {
        match value {
            pathhydra_engine::StartupBundlePolicy::ValidateOrRebuild => Self::ValidateOrRebuild,
            pathhydra_engine::StartupBundlePolicy::RequireValidBundle => Self::RequireValidBundle,
        }
    }
}
impl From<StartupBundlePolicyDto> for pathhydra_engine::StartupBundlePolicy {
    fn from(value: StartupBundlePolicyDto) -> Self {
        match value {
            StartupBundlePolicyDto::ValidateOrRebuild => Self::ValidateOrRebuild,
            StartupBundlePolicyDto::RequireValidBundle => Self::RequireValidBundle,
        }
    }
}
impl TryFrom<&VerificationLimitsDto> for VerificationLimits {
    type Error = DtoValidationError;
    fn try_from(value: &VerificationLimitsDto) -> Result<Self, Self::Error> {
        Ok(Self {
            maximum_records: value
                .maximum_records
                .as_ref()
                .map(DecimalU64Dto::as_u64)
                .transpose()?,
            maximum_duration: value
                .maximum_duration
                .as_ref()
                .map(DurationDto::to_duration)
                .transpose()?,
        })
    }
}

impl From<&RoutingUnavailableReason> for RoutingUnavailableReasonDto {
    fn from(value: &RoutingUnavailableReason) -> Self {
        match value {
            RoutingUnavailableReason::ImageCompilation(_) => Self::ImageCompilation,
            RoutingUnavailableReason::TopologyLimit { required, limit } => Self::TopologyLimit {
                required: count(*required),
                limit: count(*limit),
            },
            RoutingUnavailableReason::MetadataLimit { required, limit } => Self::MetadataLimit {
                required: DecimalU64Dto::from_u64(*required),
                limit: DecimalU64Dto::from_u64(*limit),
            },
            RoutingUnavailableReason::Bundle(_) => Self::Bundle,
        }
    }
}
impl From<&ImageBuildReport> for ImageBuildReportDto {
    fn from(value: &ImageBuildReport) -> Self {
        let metrics = value.bundle_metrics();
        Self {
            duration: DurationDto::from_duration(value.duration()),
            outcome: match value.outcome() {
                ImageBuildOutcome::Published => ImageBuildOutcomeDto::Published,
                ImageBuildOutcome::Failed(reason) => ImageBuildOutcomeDto::Failed {
                    reason: reason.into(),
                },
            },
            node_count: count(value.node_count()),
            relation_kind_count: count(value.relation_kind_count()),
            adjacency_count: count(value.adjacency_count()),
            bundle_bytes: metrics.map(|m| DecimalU64Dto::from_u64(m.bundle_bytes)),
            partition_count: metrics.map(|m| DecimalU64Dto::from_u64(m.partition_count)),
            segment_count: metrics.map(|m| DecimalU64Dto::from_u64(m.segment_count)),
        }
    }
}
impl From<&PublicationOutcome> for PublicationOutcomeDto {
    fn from(value: &PublicationOutcome) -> Self {
        match value {
            PublicationOutcome::NotRequired => Self::NotRequired,
            PublicationOutcome::Published(report) => Self::Published {
                report: report.into(),
            },
            PublicationOutcome::RoutingUnavailable(reason, report) => Self::RoutingUnavailable {
                reason: reason.into(),
                report: report.into(),
            },
        }
    }
}
impl From<&pathhydra_engine::ConfirmedMutation<ConfirmedRecord>> for MutationOutcomeDto {
    fn from(value: &pathhydra_engine::ConfirmedMutation<ConfirmedRecord>) -> Self {
        Self {
            durable_result: MutationDurableResultDto::Confirmed(value.durable_result().into()),
            publication: value.publication().into(),
        }
    }
}
pub(crate) fn edge_removal_outcome(
    edge: EdgeId,
    value: &pathhydra_engine::ConfirmedMutation<DeletionResult>,
) -> MutationOutcomeDto {
    MutationOutcomeDto {
        durable_result: MutationDurableResultDto::EdgeRemoved {
            edge: edge.into(),
            removed_relation_kinds: value
                .durable_result()
                .removed_relation_kinds
                .iter()
                .copied()
                .map(Into::into)
                .collect(),
        },
        publication: value.publication().into(),
    }
}
pub(crate) fn node_removal_outcome(
    node: NodeId,
    value: &pathhydra_engine::ConfirmedMutation<DeletionResult>,
) -> MutationOutcomeDto {
    MutationOutcomeDto {
        durable_result: MutationDurableResultDto::NodeRemoved {
            node: node.into(),
            removed_relation_kinds: value
                .durable_result()
                .removed_relation_kinds
                .iter()
                .copied()
                .map(Into::into)
                .collect(),
        },
        publication: value.publication().into(),
    }
}

impl From<&ResourceLimitsSnapshot> for ResourceLimitsDto {
    fn from(v: &ResourceLimitsSnapshot) -> Self {
        Self {
            maximum_active_image_bytes: count(v.maximum_active_image_bytes),
            maximum_concurrent_routes: count(v.maximum_concurrent_routes),
            maximum_reserved_route_bytes: count(v.maximum_reserved_route_bytes),
            maximum_destinations_per_request: count(v.maximum_destinations_per_request),
            maximum_hydration_handles_per_request: count(v.maximum_hydration_handles_per_request),
            maximum_total_bundle_bytes: DecimalU64Dto::from_u64(v.maximum_total_bundle_bytes),
            target_partition_topology_bytes: DecimalU64Dto::from_u64(
                v.target_partition_topology_bytes,
            ),
            hard_maximum_partition_topology_bytes: DecimalU64Dto::from_u64(
                v.hard_maximum_partition_topology_bytes,
            ),
            maximum_resident_image_metadata_bytes: DecimalU64Dto::from_u64(
                v.maximum_resident_image_metadata_bytes,
            ),
            host_partition_cache_bytes: DecimalU64Dto::from_u64(v.host_partition_cache_bytes),
            host_partition_cache_entries: count(v.host_partition_cache_entries),
            routing_io_workers: count(v.routing_io_workers),
            maximum_queued_partition_reads: count(v.maximum_queued_partition_reads),
            routing_io_staging_bytes: DecimalU64Dto::from_u64(v.routing_io_staging_bytes),
            maximum_retired_bundle_bytes: DecimalU64Dto::from_u64(v.maximum_retired_bundle_bytes),
            maximum_retired_bundle_count: count(v.maximum_retired_bundle_count),
            maximum_concurrent_checkpoints: count(v.maximum_concurrent_checkpoints),
            maximum_maintenance_workers: count(v.maximum_maintenance_workers),
            maximum_queued_maintenance: count(v.maximum_queued_maintenance),
            maximum_verification_records: DecimalU64Dto::from_u64(v.maximum_verification_records),
            maximum_verification_duration: DurationDto::from_duration(
                v.maximum_verification_duration,
            ),
            shutdown_drain_timeout: DurationDto::from_duration(v.shutdown_drain_timeout),
            cuda_maximum_topology_bytes: count(v.cuda_maximum_topology_bytes),
            cuda_maximum_concurrent_searches: count(v.cuda_maximum_concurrent_searches),
            cuda_maximum_batch_lanes: count(v.cuda_maximum_batch_lanes),
            cuda_maximum_reserved_search_bytes: count(v.cuda_maximum_reserved_search_bytes),
        }
    }
}
impl From<&ApiLimits> for ApiLimitsDto {
    fn from(v: &ApiLimits) -> Self {
        Self {
            maximum_encoded_bytes: count(v.maximum_encoded_bytes),
            maximum_name_bytes: count(v.maximum_name_bytes),
            maximum_payload_bytes: count(v.maximum_payload_bytes),
            maximum_destinations: count(v.maximum_destinations),
            maximum_profile_entries: count(v.maximum_profile_entries),
            maximum_path_steps: count(v.maximum_path_steps),
            maximum_hydration_handles: count(v.maximum_hydration_handles),
            maximum_subgraph_nodes: count(v.maximum_subgraph_nodes),
            maximum_subgraph_edges: count(v.maximum_subgraph_edges),
            maximum_diagnostic_text_bytes: count(v.maximum_diagnostic_text_bytes),
            maximum_nesting_depth: count(v.maximum_nesting_depth),
            maximum_json_values: count(v.maximum_json_values),
            maximum_batch_entries: count(v.maximum_batch_entries),
            maximum_batch_node_entries: count(v.maximum_batch_node_entries),
            maximum_batch_relation_kind_entries: count(v.maximum_batch_relation_kind_entries),
            maximum_batch_edge_entries: count(v.maximum_batch_edge_entries),
            maximum_batch_name_bytes: count(v.maximum_batch_name_bytes),
            maximum_batch_payload_bytes: count(v.maximum_batch_payload_bytes),
            maximum_batch_references: count(v.maximum_batch_references),
            maximum_batch_estimated_bytes: count(v.maximum_batch_estimated_bytes),
            maximum_relation_kind_usage_results: count(v.maximum_relation_kind_usage_results),
        }
    }
}
impl TryFrom<&ApiLimitsDto> for ApiLimits {
    type Error = DtoValidationError;

    fn try_from(v: &ApiLimitsDto) -> Result<Self, Self::Error> {
        Ok(Self {
            maximum_encoded_bytes: usize_from(&v.maximum_encoded_bytes, "maximum encoded bytes")?,
            maximum_name_bytes: usize_from(&v.maximum_name_bytes, "maximum name bytes")?,
            maximum_payload_bytes: usize_from(&v.maximum_payload_bytes, "maximum payload bytes")?,
            maximum_destinations: usize_from(&v.maximum_destinations, "maximum destinations")?,
            maximum_profile_entries: usize_from(
                &v.maximum_profile_entries,
                "maximum profile entries",
            )?,
            maximum_path_steps: usize_from(&v.maximum_path_steps, "maximum path steps")?,
            maximum_hydration_handles: usize_from(
                &v.maximum_hydration_handles,
                "maximum hydration handles",
            )?,
            maximum_subgraph_nodes: usize_from(
                &v.maximum_subgraph_nodes,
                "maximum subgraph nodes",
            )?,
            maximum_subgraph_edges: usize_from(
                &v.maximum_subgraph_edges,
                "maximum subgraph edges",
            )?,
            maximum_diagnostic_text_bytes: usize_from(
                &v.maximum_diagnostic_text_bytes,
                "maximum diagnostic text bytes",
            )?,
            maximum_nesting_depth: usize_from(&v.maximum_nesting_depth, "maximum nesting depth")?,
            maximum_json_values: usize_from(&v.maximum_json_values, "maximum JSON values")?,
            maximum_batch_entries: usize_from(&v.maximum_batch_entries, "maximum batch entries")?,
            maximum_batch_node_entries: usize_from(
                &v.maximum_batch_node_entries,
                "maximum batch node entries",
            )?,
            maximum_batch_relation_kind_entries: usize_from(
                &v.maximum_batch_relation_kind_entries,
                "maximum batch relation-kind entries",
            )?,
            maximum_batch_edge_entries: usize_from(
                &v.maximum_batch_edge_entries,
                "maximum batch edge entries",
            )?,
            maximum_batch_name_bytes: usize_from(
                &v.maximum_batch_name_bytes,
                "maximum batch name bytes",
            )?,
            maximum_batch_payload_bytes: usize_from(
                &v.maximum_batch_payload_bytes,
                "maximum batch payload bytes",
            )?,
            maximum_batch_references: usize_from(
                &v.maximum_batch_references,
                "maximum batch references",
            )?,
            maximum_batch_estimated_bytes: usize_from(
                &v.maximum_batch_estimated_bytes,
                "maximum estimated batch bytes",
            )?,
            maximum_relation_kind_usage_results: usize_from(
                &v.maximum_relation_kind_usage_results,
                "maximum relation-kind usage results",
            )?,
        })
    }
}
fn cuda_availability(value: &CudaAvailability) -> CudaAvailabilityDto {
    match value {
        CudaAvailability::Disabled => CudaAvailabilityDto::Disabled,
        CudaAvailability::SupportNotCompiled => CudaAvailabilityDto::SupportNotCompiled,
        CudaAvailability::Available => CudaAvailabilityDto::Available,
        CudaAvailability::Degraded(_) => CudaAvailabilityDto::Degraded,
        CudaAvailability::Unavailable(_) => CudaAvailabilityDto::Unavailable,
    }
}
pub(crate) fn capabilities(v: &EngineCapabilities, api_limits: &ApiLimits) -> CapabilitiesDto {
    CapabilitiesDto {
        cpu_reference_routing: v.cpu_reference_routing,
        gpu_routing: v.gpu_routing,
        cuda_support_compiled: v.cuda_support_compiled,
        cuda_runtime: cuda_availability(&v.cuda_runtime),
        cuda_device: v
            .cuda_device
            .as_ref()
            .map(|device| CudaDeviceCapabilitiesDto {
                compute_capability_major: device.compute_capability.0,
                compute_capability_minor: device.compute_capability.1,
                total_memory_bytes: count(device.total_memory_bytes),
                kernel_ptx_target: device.kernel_ptx_target.to_owned(),
            }),
        cuda_algorithms: v
            .cuda_algorithms
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
        cuda_distance_only: v.cuda_distance_only,
        cuda_paths: v.cuda_paths,
        cuda_finite_edge_budgets: v.cuda_finite_edge_budgets,
        cuda_partitioned_topology: !v.cuda_full_residency_required,
        cuda_parallel_strategy: v.cuda_parallel_strategy.to_owned(),
        cuda_reset_mode: v.cuda_reset_mode.to_owned(),
        cuda_target_mode: v.cuda_target_mode.to_owned(),
        cuda_profile_mode: v.cuda_profile_mode.to_owned(),
        cuda_path_evidence_mode: v.cuda_path_evidence_mode.to_owned(),
        paths: v.paths,
        edge_budgets: v.edge_budgets,
        cancellation: v.cancellation,
        hydration: v.hydration,
        current_state_hydration: v.current_state_hydration,
        subgraphs: v.subgraphs,
        canonical_subgraph_encoding: true,
        durable_routing_images: v.durable_routing_images,
        checkpoint_backup: v.checkpoint_backup,
        validated_restore: v.validated_restore,
        store_maintenance: v.store_maintenance,
        explicit_shutdown: v.explicit_shutdown,
        numeric_policy_id: v.numeric_policy_id.to_owned(),
        tie_policy_id: v.tie_policy_id.to_owned(),
        resource_limits: (&v.resource_limits).into(),
        api_limits: api_limits.into(),
    }
}
impl From<EngineLifecycleState> for EngineLifecycleStateDto {
    fn from(value: EngineLifecycleState) -> Self {
        match value {
            EngineLifecycleState::Running => Self::Running,
            EngineLifecycleState::Closing => Self::Closing,
            EngineLifecycleState::ShutDown => Self::ShutDown,
        }
    }
}
impl From<ActiveOperationCounts> for ActiveOperationCountsDto {
    fn from(v: ActiveOperationCounts) -> Self {
        Self {
            routes: count(v.routes),
            mutations: count(v.mutations),
            checkpoints: count(v.checkpoints),
            maintenance: count(v.maintenance),
        }
    }
}
impl From<&LifecycleSnapshot> for LifecycleSnapshotDto {
    fn from(v: &LifecycleSnapshot) -> Self {
        Self {
            state: v.state.into(),
            active: v.active.into(),
            shutdown_active_before: v.shutdown_active_before.map(Into::into),
            rejected_after_close: DecimalU64Dto::from_u64(v.rejected_after_close),
        }
    }
}
fn store_health(v: &StoreMetricsSnapshot) -> StoreHealthDto {
    let (attempts, failures, bytes) = v.writes.values().fold((0_u64, 0_u64, 0_u64), |a, m| {
        (
            a.0.saturating_add(m.attempts),
            a.1.saturating_add(m.failures),
            a.2.saturating_add(m.committed_bytes),
        )
    });
    StoreHealthDto {
        wal_enabled: v.wal_enabled,
        ordinary_writes_sync: v.ordinary_writes_sync,
        write_attempts: DecimalU64Dto::from_u64(attempts),
        write_failures: DecimalU64Dto::from_u64(failures),
        committed_bytes: DecimalU64Dto::from_u64(bytes),
        completed_scans: DecimalU64Dto::from_u64(v.scans.completed_scans),
        scan_failures: DecimalU64Dto::from_u64(v.scans.failures),
        checkpoint_attempts: DecimalU64Dto::from_u64(v.maintenance.checkpoint_attempts),
        checkpoint_failures: DecimalU64Dto::from_u64(v.maintenance.checkpoint_failures),
        restore_attempts: DecimalU64Dto::from_u64(v.maintenance.restore_attempts),
        restore_failures: DecimalU64Dto::from_u64(v.maintenance.restore_failures),
        flush_attempts: DecimalU64Dto::from_u64(v.maintenance.flush_attempts),
        flush_failures: DecimalU64Dto::from_u64(v.maintenance.flush_failures),
        compaction_attempts: DecimalU64Dto::from_u64(v.maintenance.compaction_attempts),
        compaction_failures: DecimalU64Dto::from_u64(v.maintenance.compaction_failures),
        last_verification_succeeded: v.maintenance.last_verification_succeeded,
        last_maintenance_succeeded: v.maintenance.last_maintenance_succeeded,
    }
}
fn retirement(v: &pathhydra_engine::RetirementSnapshot) -> RetirementHealthDto {
    RetirementHealthDto {
        bundle_count: count(v.bundle_count),
        bundle_bytes: DecimalU64Dto::from_u64(v.bundle_bytes),
        maximum_bundle_count: count(v.maximum_bundle_count),
        maximum_bundle_bytes: DecimalU64Dto::from_u64(v.maximum_bundle_bytes),
        limit_exceeded: v.limit_exceeded,
        cleanup_failure_present: v.last_cleanup_failure.is_some(),
        cumulative_backpressure_waits: DecimalU64Dto::from_u64(v.cumulative_backpressure_waits),
        cumulative_backpressure_duration: DurationDto::from_duration(
            v.cumulative_backpressure_duration,
        ),
    }
}
fn cuda_health(v: &CudaHealth) -> CudaHealthDto {
    CudaHealthDto {
        availability: cuda_availability(&v.availability),
        resident_node_count: count(v.resident_node_count),
        resident_adjacency_count: count(v.resident_adjacency_count),
        resident_topology_bytes: count(v.resident_topology_bytes),
        partitioned_topology: v.partitioned_topology,
        queued_lanes: count(v.queued_lanes),
        active_lanes: count(v.active_lanes),
        peak_active_lanes: count(v.peak_active_lanes),
        reserved_search_bytes: count(v.reserved_search_bytes),
        peak_reserved_search_bytes: count(v.peak_reserved_search_bytes),
        cumulative_admission_rejections: DecimalU64Dto::from_u64(v.cumulative_admission_rejections),
        worker_running: v.worker_running,
        cumulative_uploads: DecimalU64Dto::from_u64(v.cumulative_uploads),
        cumulative_upload_failures: DecimalU64Dto::from_u64(v.cumulative_upload_failures),
        cumulative_launches: DecimalU64Dto::from_u64(v.cumulative_launches),
        cumulative_launch_failures: DecimalU64Dto::from_u64(v.cumulative_launch_failures),
        cumulative_fallbacks: DecimalU64Dto::from_u64(v.cumulative_fallbacks),
        cumulative_cancellations: DecimalU64Dto::from_u64(v.cumulative_cancellations),
        cumulative_context_reinitializations: DecimalU64Dto::from_u64(
            v.cumulative_context_reinitializations,
        ),
    }
}
impl From<&EngineHealth> for HealthDto {
    fn from(v: &EngineHealth) -> Self {
        Self {
            durable_catalog_available: v.durable_catalog_available,
            lifecycle: (&v.lifecycle).into(),
            store: v.store.as_ref().map(store_health),
            routing: match &v.routing {
                RoutingHealth::Available => RoutingHealthDto::Available,
                RoutingHealth::Unavailable(reason) => RoutingHealthDto::Unavailable {
                    reason: reason.into(),
                },
                RoutingHealth::ShutDown => RoutingHealthDto::ShutDown,
            },
            current_image_age: v.current_image_age.map(DurationDto::from_duration),
            active_image_references: count(v.active_image_references),
            image_node_count: v
                .current_image_manifest
                .as_ref()
                .map(|m| count(m.node_count())),
            image_relation_kind_count: v
                .current_image_manifest
                .as_ref()
                .map(|m| count(m.relation_kind_count())),
            image_adjacency_count: v
                .current_image_manifest
                .as_ref()
                .map(|m| count(m.adjacency_count())),
            startup_image_duration: DurationDto::from_duration(v.startup_image_duration),
            last_image_build: (&v.last_image_build).into(),
            image_corruption_present: v.last_image_corruption.is_some(),
            cuda_degradation_present: v.last_cuda_degradation.is_some(),
            cuda_recovery_present: v.last_cuda_recovery.is_some(),
            active_routes: count(v.active_routes),
            peak_active_routes: count(v.peak_active_routes),
            reserved_route_bytes: count(v.reserved_route_bytes),
            peak_reserved_route_bytes: count(v.peak_reserved_route_bytes),
            cumulative_route_admissions: DecimalU64Dto::from_u64(v.cumulative_route_admissions),
            cumulative_admission_rejections: DecimalU64Dto::from_u64(
                v.cumulative_admission_rejections,
            ),
            cumulative_cancellations: DecimalU64Dto::from_u64(v.cumulative_cancellations),
            cumulative_image_build_failures: DecimalU64Dto::from_u64(
                v.cumulative_image_build_failures,
            ),
            retired_bundles: retirement(&v.retired_bundles),
            cuda: cuda_health(&v.cuda),
        }
    }
}

impl From<&CatalogSummary> for CatalogSummaryDto {
    fn from(v: &CatalogSummary) -> Self {
        Self {
            candidate_nodes: DecimalU64Dto::from_u64(v.candidates.nodes),
            candidate_relation_kinds: DecimalU64Dto::from_u64(v.candidates.relation_kinds),
            candidate_edges: DecimalU64Dto::from_u64(v.candidates.edges),
            confirmed_nodes: DecimalU64Dto::from_u64(v.confirmed_nodes),
            relation_kinds: DecimalU64Dto::from_u64(v.relation_kinds),
            confirmed_edges: DecimalU64Dto::from_u64(v.confirmed_edges),
            node_name_entries: DecimalU64Dto::from_u64(v.node_name_entries),
            relation_name_entries: DecimalU64Dto::from_u64(v.relation_name_entries),
            outgoing_entries: DecimalU64Dto::from_u64(v.outgoing_entries),
            incoming_entries: DecimalU64Dto::from_u64(v.incoming_entries),
            routing_pointer_present: v.routing_pointer_present,
        }
    }
}
impl From<&VerificationReport> for VerificationReportDto {
    fn from(v: &VerificationReport) -> Self {
        Self {
            summary: (&v.summary).into(),
            records_examined: DecimalU64Dto::from_u64(v.records_examined),
            decoded_bytes: DecimalU64Dto::from_u64(v.decoded_bytes),
            catalog_checksum: Binary64Dto::from_bits(v.catalog_checksum),
            duration: DurationDto::from_duration(v.duration),
        }
    }
}
impl From<&CheckpointReport> for CheckpointReportDto {
    fn from(v: &CheckpointReport) -> Self {
        Self {
            catalog: (&v.catalog).into(),
            file_count: DecimalU64Dto::from_u64(v.file_count),
            bytes: DecimalU64Dto::from_u64(v.bytes),
            content_checksum: Binary64Dto::from_bits(v.content_checksum),
            duration: DurationDto::from_duration(v.duration),
        }
    }
}
impl From<&RestoreReport> for RestoreReportDto {
    fn from(v: &RestoreReport) -> Self {
        Self {
            source_file_count: DecimalU64Dto::from_u64(v.source_file_count),
            source_bytes: DecimalU64Dto::from_u64(v.source_bytes),
            source_checksum: Binary64Dto::from_bits(v.source_checksum),
            cleared_routing_pointer: v.cleared_routing_pointer,
            restored_catalog: (&v.restored_catalog).into(),
            duration: DurationDto::from_duration(v.duration),
        }
    }
}
impl From<&EngineRestoreReport> for EngineRestoreReportDto {
    fn from(v: &EngineRestoreReport) -> Self {
        Self {
            store: (&v.store).into(),
            routing_image: (&v.routing_image).into(),
            smoke_catalog_verified: v.smoke_catalog_verified,
            smoke_route_verified: v.smoke_route_verified,
            smoke_hydration_verified: v.smoke_hydration_verified,
            cuda_initialized: v.cuda_initialized,
            shutdown: (&v.shutdown).into(),
        }
    }
}
fn metric(value: &MetricValue<u64>) -> Option<DecimalU64Dto> {
    match value {
        MetricValue::Available(value) => Some(DecimalU64Dto::from_u64(*value)),
        MetricValue::Unavailable => None,
    }
}
impl From<&CompactionFamilyReport> for CompactionFamilyReportDto {
    fn from(v: &CompactionFamilyReport) -> Self {
        Self {
            name: v.name.to_owned(),
            live_sst_bytes_before: metric(&v.live_sst_bytes_before),
            live_sst_bytes_after: metric(&v.live_sst_bytes_after),
            total_sst_bytes_before: metric(&v.total_sst_bytes_before),
            total_sst_bytes_after: metric(&v.total_sst_bytes_after),
        }
    }
}
impl From<&CompactionReport> for CompactionReportDto {
    fn from(v: &CompactionReport) -> Self {
        Self {
            families: v.families.iter().map(Into::into).collect(),
            duration: DurationDto::from_duration(v.duration),
        }
    }
}
impl From<ShutdownFailureStage> for ShutdownFailureStageDto {
    fn from(v: ShutdownFailureStage) -> Self {
        match v {
            ShutdownFailureStage::Drain => Self::Drain,
            ShutdownFailureStage::RoutingIo => Self::RoutingIo,
            ShutdownFailureStage::CudaWorker => Self::CudaWorker,
            ShutdownFailureStage::StoreFlush => Self::StoreFlush,
            ShutdownFailureStage::StoreHandle => Self::StoreHandle,
            ShutdownFailureStage::Lifecycle => Self::Lifecycle,
        }
    }
}
impl From<&ShutdownFailure> for ShutdownFailureDto {
    fn from(v: &ShutdownFailure) -> Self {
        Self {
            stage: v.stage.into(),
            category: v.category.to_owned(),
        }
    }
}
impl From<DrainOutcome> for DrainOutcomeDto {
    fn from(v: DrainOutcome) -> Self {
        Self {
            drained: v.drained,
            remaining: v.remaining.into(),
            duration: DurationDto::from_duration(v.duration),
        }
    }
}
impl From<CudaWorkerShutdownReport> for CudaWorkerShutdownReportDto {
    fn from(v: CudaWorkerShutdownReport) -> Self {
        Self {
            queued_at_request: count(v.queued_at_request),
            active_at_request: count(v.active_at_request),
            queued_routes_rejected: count(v.queued_routes_rejected),
            joined: v.joined,
        }
    }
}
impl From<&ShutdownReport> for ShutdownReportDto {
    fn from(v: &ShutdownReport) -> Self {
        Self {
            state_before: v.state_before.into(),
            state_after: v.state_after.into(),
            already_shut_down: v.already_shut_down,
            active_before: v.active_before.into(),
            drain: v.drain.into(),
            drained: v.drained.into(),
            active_requests_signalled: count(v.active_requests_signalled),
            newly_signalled_requests: count(v.newly_signalled_requests),
            partition_io_stopped: v.partition_io_stopped,
            cuda_worker: v.cuda_worker.map(Into::into),
            store_flush_duration: v.store_flush_duration.map(DurationDto::from_duration),
            retired_bundles: retirement(&v.retired_bundles),
            duration: DurationDto::from_duration(v.duration),
            failures: v.failures.iter().map(Into::into).collect(),
        }
    }
}
