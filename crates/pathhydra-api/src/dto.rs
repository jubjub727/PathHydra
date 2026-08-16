//! Owned consumer-facing data transfer objects.

use std::{fmt, time::Duration};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DtoValidationError {
    NonCanonicalDecimal { field: &'static str },
    NonCanonicalHex { field: &'static str },
    NonCanonicalBase64,
    InvalidNumericEvidence { field: &'static str },
    DuplicateProfileRelation { relation_kind: RelationIdDto },
    UnorderedProfile,
    UnorderedSubgraphNodes,
    UnorderedSubgraphEdges,
    MissingSubgraphEndpoint { edge: EdgeIdDto, node: NodeIdDto },
    InvalidPath { reason: &'static str },
    InvalidRoutingResponse { reason: &'static str },
}

impl fmt::Display for DtoValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonCanonicalDecimal { field } => {
                write!(formatter, "{field} is not a canonical decimal u64")
            }
            Self::NonCanonicalHex { field } => write!(
                formatter,
                "{field} is not canonical fixed-width hexadecimal text"
            ),
            Self::NonCanonicalBase64 => {
                formatter.write_str("payload is not canonical padded base64")
            }
            Self::InvalidNumericEvidence { field } => {
                write!(formatter, "{field} contains invalid numeric evidence")
            }
            Self::DuplicateProfileRelation { relation_kind } => {
                write!(formatter, "profile repeats relation kind {relation_kind}")
            }
            Self::UnorderedProfile => {
                formatter.write_str("profile relation kinds are not strictly increasing")
            }
            Self::UnorderedSubgraphNodes => {
                formatter.write_str("subgraph node IDs are not strictly increasing")
            }
            Self::UnorderedSubgraphEdges => {
                formatter.write_str("subgraph edge IDs are not strictly increasing")
            }
            Self::MissingSubgraphEndpoint { edge, node } => write!(
                formatter,
                "subgraph edge {edge} references absent node {node}"
            ),
            Self::InvalidPath { reason } => write!(formatter, "invalid path evidence: {reason}"),
            Self::InvalidRoutingResponse { reason } => {
                write!(formatter, "invalid routing response: {reason}")
            }
        }
    }
}

impl std::error::Error for DtoValidationError {}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum ApiErrorCategoryDto {
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiErrorDto {
    pub category: ApiErrorCategoryDto,
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

fn parse_decimal(value: &str, field: &'static str) -> Result<u64, DtoValidationError> {
    if value.is_empty()
        || (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(DtoValidationError::NonCanonicalDecimal { field });
    }
    let parsed = value
        .parse::<u64>()
        .map_err(|_| DtoValidationError::NonCanonicalDecimal { field })?;
    if parsed.to_string() != value {
        return Err(DtoValidationError::NonCanonicalDecimal { field });
    }
    Ok(parsed)
}

macro_rules! decimal_wrapper {
    ($name:ident, $field:literal) => {
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            #[must_use]
            pub fn from_u64(value: u64) -> Self {
                Self(value.to_string())
            }
            pub fn parse(value: impl Into<String>) -> Result<Self, DtoValidationError> {
                let value = value.into();
                parse_decimal(&value, $field)?;
                Ok(Self(value))
            }
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
            pub fn as_u64(&self) -> Result<u64, DtoValidationError> {
                parse_decimal(&self.0, $field)
            }
        }
        impl Default for $name {
            fn default() -> Self {
                Self::from_u64(0)
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(&self.0)
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                Self::parse(String::deserialize(deserializer)?).map_err(de::Error::custom)
            }
        }
    };
}

decimal_wrapper!(DecimalU64Dto, "decimal value");
decimal_wrapper!(CandidateIdDto, "candidate ID");
decimal_wrapper!(NodeIdDto, "node ID");
decimal_wrapper!(RelationIdDto, "relation-kind ID");
decimal_wrapper!(EdgeIdDto, "edge ID");
decimal_wrapper!(RequestIdDto, "request ID");

fn parse_hex<const DIGITS: usize>(
    value: &str,
    field: &'static str,
) -> Result<u64, DtoValidationError> {
    let Some(hex) = value.strip_prefix("0x") else {
        return Err(DtoValidationError::NonCanonicalHex { field });
    };
    if hex.len() != DIGITS
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(DtoValidationError::NonCanonicalHex { field });
    }
    u64::from_str_radix(hex, 16).map_err(|_| DtoValidationError::NonCanonicalHex { field })
}

macro_rules! hex_wrapper {
    ($name:ident, $bits:ty, $digits:expr, $field:literal) => {
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);
        impl $name {
            #[must_use]
            pub fn from_bits(bits: $bits) -> Self {
                Self(format!(concat!("0x{:0", stringify!($digits), "x}"), bits))
            }
            pub fn parse(value: impl Into<String>) -> Result<Self, DtoValidationError> {
                let value = value.into();
                let parsed = <$bits>::try_from(parse_hex::<$digits>(&value, $field)?)
                    .map_err(|_| DtoValidationError::NonCanonicalHex { field: $field })?;
                if Self::from_bits(parsed).0 != value {
                    return Err(DtoValidationError::NonCanonicalHex { field: $field });
                }
                Ok(Self(value))
            }
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
            pub fn bits(&self) -> Result<$bits, DtoValidationError> {
                <$bits>::try_from(parse_hex::<$digits>(&self.0, $field)?)
                    .map_err(|_| DtoValidationError::NonCanonicalHex { field: $field })
            }
        }
        impl Default for $name {
            fn default() -> Self {
                Self::from_bits(0)
            }
        }
        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(&self.0)
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                Self::parse(String::deserialize(deserializer)?).map_err(de::Error::custom)
            }
        }
    };
}

hex_wrapper!(Binary32Dto, u32, 8, "binary32 bits");
hex_wrapper!(Binary64Dto, u64, 16, "binary64 bits");

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PayloadDto(String);

impl PayloadDto {
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self(STANDARD.encode(bytes))
    }
    pub fn parse(value: impl Into<String>) -> Result<Self, DtoValidationError> {
        let value = value.into();
        let decoded = STANDARD
            .decode(&value)
            .map_err(|_| DtoValidationError::NonCanonicalBase64)?;
        if STANDARD.encode(decoded) != value {
            return Err(DtoValidationError::NonCanonicalBase64);
        }
        Ok(Self(value))
    }
    pub fn decode(&self) -> Result<Vec<u8>, DtoValidationError> {
        let decoded = STANDARD
            .decode(&self.0)
            .map_err(|_| DtoValidationError::NonCanonicalBase64)?;
        if STANDARD.encode(&decoded) != self.0 {
            return Err(DtoValidationError::NonCanonicalBase64);
        }
        Ok(decoded)
    }
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl Serialize for PayloadDto {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}
impl<'de> Deserialize<'de> for PayloadDto {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::parse(String::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DurationDto {
    pub seconds: DecimalU64Dto,
    pub nanoseconds: u32,
}
impl DurationDto {
    #[must_use]
    pub fn from_duration(value: Duration) -> Self {
        Self {
            seconds: DecimalU64Dto::from_u64(value.as_secs()),
            nanoseconds: value.subsec_nanos(),
        }
    }
    pub fn to_duration(&self) -> Result<Duration, DtoValidationError> {
        if self.nanoseconds >= 1_000_000_000 {
            return Err(DtoValidationError::InvalidNumericEvidence {
                field: "duration nanoseconds",
            });
        }
        Ok(Duration::new(self.seconds.as_u64()?, self.nanoseconds))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
pub enum CandidateDto {
    Node {
        id: CandidateIdDto,
        name: String,
        payload: PayloadDto,
    },
    RelationKind {
        id: CandidateIdDto,
        name: String,
    },
    Edge {
        id: CandidateIdDto,
        source: NodeIdDto,
        destination: NodeIdDto,
        relation_kind: RelationIdDto,
        base_weight: Binary32Dto,
    },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeRecordDto {
    pub id: NodeIdDto,
    pub name: String,
    pub payload: PayloadDto,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelationKindRecordDto {
    pub id: RelationIdDto,
    pub name: String,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EdgeRecordDto {
    pub id: EdgeIdDto,
    pub source: NodeIdDto,
    pub destination: NodeIdDto,
    pub relation_kind: RelationIdDto,
    pub base_weight: Binary32Dto,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    deny_unknown_fields,
    tag = "kind",
    content = "record",
    rename_all = "snake_case"
)]
pub enum ConfirmedRecordDto {
    Node(NodeRecordDto),
    RelationKind(RelationKindRecordDto),
    Edge(EdgeRecordDto),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "state", rename_all = "snake_case")]
pub enum RelationUseDto {
    Disabled,
    Enabled { multiplier: Binary32Dto },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelationProfileEntryDto {
    pub relation_kind: RelationIdDto,
    pub relation_use: RelationUseDto,
}
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelationProfileDto {
    pub entries: Vec<RelationProfileEntryDto>,
}
impl RelationProfileDto {
    pub fn validate(&self) -> Result<(), DtoValidationError> {
        let mut previous = None;
        for entry in &self.entries {
            let current = entry.relation_kind.as_u64()?;
            if previous == Some(current) {
                return Err(DtoValidationError::DuplicateProfileRelation {
                    relation_kind: entry.relation_kind.clone(),
                });
            }
            if previous.is_some_and(|value| value > current) {
                return Err(DtoValidationError::UnorderedProfile);
            }
            previous = Some(current);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
pub enum SearchBudgetDto {
    Unlimited,
    ExaminedEdges { maximum: DecimalU64Dto },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum TiePolicyDto {
    StablePredecessor,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum NumericPolicyDto {
    Binary32OperandsSeparateBinary64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum CompletionReasonDto {
    AllDestinationsFinalized,
    FrontierExhausted,
    BudgetExhausted,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutingRequestDto {
    pub origin: NodeIdDto,
    pub destinations: Vec<NodeIdDto>,
    pub profile: RelationProfileDto,
    pub return_paths: bool,
    pub budget: SearchBudgetDto,
    pub tie_policy: TiePolicyDto,
}
impl RoutingRequestDto {
    pub fn validate(&self) -> Result<(), DtoValidationError> {
        self.profile.validate()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathStepDto {
    pub edge: EdgeIdDto,
    pub source: NodeIdDto,
    pub destination: NodeIdDto,
    pub relation_kind: RelationIdDto,
    pub base_weight: Binary32Dto,
    pub multiplier: Binary32Dto,
    pub effective_weight: Binary64Dto,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutePathDto {
    pub origin: NodeIdDto,
    pub destination: NodeIdDto,
    pub logical_distance: Binary64Dto,
    pub steps: Vec<PathStepDto>,
}
impl RoutePathDto {
    pub fn validate(&self) -> Result<(), DtoValidationError> {
        if self.steps.is_empty() {
            if self.origin != self.destination {
                return Err(DtoValidationError::InvalidPath {
                    reason: "a nontrivial path has no steps",
                });
            }
        } else if self.steps[0].source != self.origin {
            return Err(DtoValidationError::InvalidPath {
                reason: "first step does not start at the origin",
            });
        } else if self
            .steps
            .last()
            .is_some_and(|step| step.destination != self.destination)
        {
            return Err(DtoValidationError::InvalidPath {
                reason: "last step does not end at the destination",
            });
        } else if self
            .steps
            .windows(2)
            .any(|pair| pair[0].destination != pair[1].source)
        {
            return Err(DtoValidationError::InvalidPath {
                reason: "step endpoints are discontinuous",
            });
        }
        validate_nonnegative_f64(&self.logical_distance, "logical distance")?;
        for step in &self.steps {
            validate_base_weight(&step.base_weight)?;
            validate_multiplier(&step.multiplier)?;
            validate_nonnegative_f64(&step.effective_weight, "effective weight")?;
        }
        Ok(())
    }
}
fn validate_base_weight(value: &Binary32Dto) -> Result<(), DtoValidationError> {
    let value = f32::from_bits(value.bits()?);
    if !value.is_finite()
        || !(0.0..=1.0).contains(&value)
        || value == 0.0 && value.is_sign_negative()
    {
        return Err(DtoValidationError::InvalidNumericEvidence {
            field: "base weight",
        });
    }
    Ok(())
}
fn validate_multiplier(value: &Binary32Dto) -> Result<(), DtoValidationError> {
    let value = f32::from_bits(value.bits()?);
    if !value.is_finite() || value < 0.0 || value == 0.0 && value.is_sign_negative() {
        return Err(DtoValidationError::InvalidNumericEvidence {
            field: "relation multiplier",
        });
    }
    Ok(())
}
fn validate_nonnegative_f64(
    value: &Binary64Dto,
    field: &'static str,
) -> Result<(), DtoValidationError> {
    let value = f64::from_bits(value.bits()?);
    if !value.is_finite() || value < 0.0 || value == 0.0 && value.is_sign_negative() {
        return Err(DtoValidationError::InvalidNumericEvidence { field });
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "state", rename_all = "snake_case")]
pub enum DestinationStateDto {
    Exact {
        logical_distance: Binary64Dto,
        path: Option<RoutePathDto>,
    },
    Unreachable,
    MissingNode,
    Incomplete,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DestinationResultDto {
    pub destination: NodeIdDto,
    pub state: DestinationStateDto,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutingResponseDto {
    pub origin: NodeIdDto,
    pub results: Vec<DestinationResultDto>,
    pub profile: RelationProfileDto,
    pub numeric_policy: NumericPolicyDto,
    pub tie_policy: TiePolicyDto,
    pub paths_requested: bool,
    pub examined_edges: DecimalU64Dto,
    pub finalized_nodes: DecimalU64Dto,
    pub completion_reason: CompletionReasonDto,
}
impl RoutingResponseDto {
    pub fn validate(&self) -> Result<(), DtoValidationError> {
        self.profile.validate()?;
        for result in &self.results {
            if let DestinationStateDto::Exact {
                logical_distance,
                path,
            } = &result.state
            {
                validate_nonnegative_f64(logical_distance, "logical distance")?;
                if let Some(path) = path {
                    path.validate()?;
                    if path.origin != self.origin
                        || path.destination != result.destination
                        || path.logical_distance != *logical_distance
                    {
                        return Err(DtoValidationError::InvalidRoutingResponse {
                            reason: "path identity or distance differs from its destination result",
                        });
                    }
                }
                if self.paths_requested != path.is_some() {
                    return Err(DtoValidationError::InvalidRoutingResponse {
                        reason: "exact path presence differs from paths_requested",
                    });
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum ExecutorDto {
    CpuReference,
    NvidiaCuda,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum ExecutorSelectionReasonDto {
    CpuOnlyPolicy,
    PreferredCudaEligible,
    RequiredCudaEligible,
    AutomaticPolicySelectedCpu,
    CpuFallback,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "reason", rename_all = "snake_case")]
pub enum CudaIneligibilityDto {
    Disabled,
    SupportNotCompiled,
    FiniteEdgeBudgetUnsupportedByCuda,
    NoResidentImage,
    ResourceRefusal,
    AutomaticPolicySelectedCpu,
    Unhealthy,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CpuSearchDiagnosticsDto {
    pub examined_edges: DecimalU64Dto,
    pub relaxation_updates: DecimalU64Dto,
    pub finalized_nodes: DecimalU64Dto,
    pub frontier_high_water_mark: DecimalU64Dto,
    pub unique_present_destinations: DecimalU64Dto,
    pub exact_destinations: DecimalU64Dto,
    pub unreachable_destinations: DecimalU64Dto,
    pub missing_destinations: DecimalU64Dto,
    pub incomplete_destinations: DecimalU64Dto,
    pub path_reconstruction_steps: DecimalU64Dto,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartitionedCpuDiagnosticsDto {
    pub partitions: DecimalU64Dto,
    pub file_bytes: DecimalU64Dto,
    pub io_wait: DurationDto,
    pub cache_hits: DecimalU64Dto,
    pub cache_misses: DecimalU64Dto,
    pub cache_current_bytes: DecimalU64Dto,
    pub cache_high_water_bytes: DecimalU64Dto,
    pub cache_entries: DecimalU64Dto,
    pub cache_queue_depth: DecimalU64Dto,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CudaRequestDiagnosticsDto {
    pub algorithm: String,
    pub delta: Option<Binary64Dto>,
    pub queue_duration: DurationDto,
    pub batch_collection_duration: DurationDto,
    pub batch_width: DecimalU64Dto,
    pub lane_index: DecimalU64Dto,
    pub topology_bytes: DecimalU64Dto,
    pub search_bytes: DecimalU64Dto,
    pub host_to_device_bytes: DecimalU64Dto,
    pub device_to_host_bytes: DecimalU64Dto,
    pub kernel_launches: DecimalU64Dto,
    pub synchronized_execution_duration: DurationDto,
    pub examined_edges: DecimalU64Dto,
    pub relaxation_attempts: DecimalU64Dto,
    pub relaxation_updates: DecimalU64Dto,
    pub phases: DecimalU64Dto,
    pub partitions_required: DecimalU64Dto,
    pub host_cache_hits: DecimalU64Dto,
    pub device_cache_hits: DecimalU64Dto,
    pub file_bytes: DecimalU64Dto,
    pub staged_bytes: DecimalU64Dto,
    pub transfer_bytes: DecimalU64Dto,
    pub parallel_strategy: String,
    pub reset_mode: String,
    pub target_mode: String,
    pub profile_mode: String,
    pub path_evidence_mode: String,
    pub state_initialization_duration: DurationDto,
    pub partition_scheduling_duration: DurationDto,
    pub relation_relaxation_duration: DurationDto,
    pub response_transfer_duration: DurationDto,
    pub frontier_compaction_duration: DurationDto,
    pub compacted_task_count: DecimalU64Dto,
    pub destination_completion_duration: DurationDto,
    pub destination_count_checked: DecimalU64Dto,
    pub atomic_cas_retries: DecimalU64Dto,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeDiagnosticsDto {
    pub request_id: RequestIdDto,
    pub executor: ExecutorDto,
    pub selection_reason: ExecutorSelectionReasonDto,
    pub attempted_cuda: bool,
    pub cuda_fallback_reason: Option<CudaIneligibilityDto>,
    pub numeric_policy: NumericPolicyDto,
    pub tie_policy: TiePolicyDto,
    pub image_node_count: DecimalU64Dto,
    pub image_relation_kind_count: DecimalU64Dto,
    pub image_adjacency_count: DecimalU64Dto,
    pub reserved_working_bytes: DecimalU64Dto,
    pub admission_duration: DurationDto,
    pub execution_duration: DurationDto,
    pub completion_reason: CompletionReasonDto,
    pub search: CpuSearchDiagnosticsDto,
    pub partitioned_cpu: Option<PartitionedCpuDiagnosticsDto>,
    pub cuda: Option<CudaRequestDiagnosticsDto>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineRoutingResponseDto {
    pub response: RoutingResponseDto,
    pub diagnostics: RuntimeDiagnosticsDto,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HydrationRequestDto {
    pub node_ids: Vec<NodeIdDto>,
    pub edge_ids: Vec<EdgeIdDto>,
    pub profile: Option<RelationProfileDto>,
}
impl HydrationRequestDto {
    pub fn validate(&self) -> Result<(), DtoValidationError> {
        self.profile
            .as_ref()
            .map_or(Ok(()), RelationProfileDto::validate)
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "state", rename_all = "snake_case")]
pub enum HydratedNodeStateDto {
    Found { node: NodeRecordDto },
    Missing,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HydratedNodeResultDto {
    pub requested_node_id: NodeIdDto,
    pub state: HydratedNodeStateDto,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "state", rename_all = "snake_case")]
pub enum EdgeEvaluationDto {
    Unprofiled,
    Disabled,
    Enabled {
        multiplier: Binary32Dto,
        effective_weight: Binary64Dto,
    },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HydratedEdgeDto {
    pub edge: EdgeRecordDto,
    pub relation_kind: RelationKindRecordDto,
    pub evaluation: EdgeEvaluationDto,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "state", rename_all = "snake_case")]
pub enum HydratedEdgeStateDto {
    Found { edge: Box<HydratedEdgeDto> },
    Missing,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HydratedEdgeResultDto {
    pub requested_edge_id: EdgeIdDto,
    pub state: HydratedEdgeStateDto,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HydrationResponseDto {
    pub nodes: Vec<HydratedNodeResultDto>,
    pub edges: Vec<HydratedEdgeResultDto>,
    pub profile: Option<RelationProfileDto>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HydratedPathDto {
    pub nodes: Vec<NodeRecordDto>,
    pub edges: Vec<HydratedEdgeDto>,
    pub logical_distance: Binary64Dto,
    pub numeric_policy: NumericPolicyDto,
    pub tie_policy: TiePolicyDto,
    pub profile: RelationProfileDto,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EdgeHandleDto {
    pub edge: EdgeIdDto,
    pub source: NodeIdDto,
    pub destination: NodeIdDto,
}
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubgraphHandlesDto {
    pub nodes: Vec<NodeIdDto>,
    pub edges: Vec<EdgeHandleDto>,
}
impl SubgraphHandlesDto {
    pub fn validate(&self) -> Result<(), DtoValidationError> {
        let mut previous_node = None;
        for node in &self.nodes {
            let current = node.as_u64()?;
            if previous_node.is_some_and(|previous| previous >= current) {
                return Err(DtoValidationError::UnorderedSubgraphNodes);
            }
            previous_node = Some(current);
        }
        let mut previous_edge = None;
        for edge in &self.edges {
            let current = edge.edge.as_u64()?;
            if previous_edge.is_some_and(|previous| previous >= current) {
                return Err(DtoValidationError::UnorderedSubgraphEdges);
            }
            previous_edge = Some(current);
            for endpoint in [&edge.source, &edge.destination] {
                if self.nodes.binary_search(endpoint).is_err() {
                    return Err(DtoValidationError::MissingSubgraphEndpoint {
                        edge: edge.edge.clone(),
                        node: endpoint.clone(),
                    });
                }
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HydratedSubgraphDto {
    pub nodes: Vec<NodeRecordDto>,
    pub edges: Vec<HydratedEdgeDto>,
    pub missing_node_ids: Vec<NodeIdDto>,
    pub missing_edge_ids: Vec<EdgeIdDto>,
    pub profile: Option<RelationProfileDto>,
    pub complete: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum CudaExecutorPolicyDto {
    CpuOnly,
    PreferCuda,
    RequireCuda,
    Auto,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
pub enum CudaAlgorithmDto {
    Frontier,
    DeltaStepping { delta: Binary64Dto },
    Automatic,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CudaConfigDto {
    pub enabled: bool,
    pub device_ordinal: DecimalU64Dto,
    pub executor_policy: CudaExecutorPolicyDto,
    pub maximum_topology_bytes: DecimalU64Dto,
    pub maximum_partitioned_topology_cache_bytes: DecimalU64Dto,
    pub maximum_partitioned_topology_cache_slots: DecimalU64Dto,
    pub maximum_partitioned_host_staging_bytes: DecimalU64Dto,
    pub minimum_free_memory_headroom: DecimalU64Dto,
    pub maximum_concurrent_searches: DecimalU64Dto,
    pub maximum_batch_lanes: DecimalU64Dto,
    pub maximum_reserved_search_bytes: DecimalU64Dto,
    pub batch_collection_delay: DurationDto,
    pub algorithm: CudaAlgorithmDto,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum StartupBundlePolicyDto {
    ValidateOrRebuild,
    RequireValidBundle,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathHydraConfigDto {
    pub maximum_active_image_bytes: DecimalU64Dto,
    pub maximum_concurrent_routes: DecimalU64Dto,
    pub maximum_reserved_route_bytes: DecimalU64Dto,
    pub maximum_destinations_per_request: DecimalU64Dto,
    pub maximum_hydration_handles_per_request: DecimalU64Dto,
    pub maximum_total_bundle_bytes: DecimalU64Dto,
    pub target_partition_topology_bytes: DecimalU64Dto,
    pub hard_maximum_partition_topology_bytes: DecimalU64Dto,
    pub maximum_resident_image_metadata_bytes: DecimalU64Dto,
    pub host_partition_cache_bytes: DecimalU64Dto,
    pub host_partition_cache_entries: DecimalU64Dto,
    pub routing_io_workers: DecimalU64Dto,
    pub maximum_queued_partition_reads: DecimalU64Dto,
    pub routing_io_staging_bytes: DecimalU64Dto,
    pub maximum_retired_bundle_bytes: DecimalU64Dto,
    pub maximum_retired_bundle_count: DecimalU64Dto,
    pub maximum_concurrent_checkpoints: DecimalU64Dto,
    pub maximum_maintenance_workers: DecimalU64Dto,
    pub maximum_queued_maintenance: DecimalU64Dto,
    pub maximum_verification_records: DecimalU64Dto,
    pub maximum_verification_duration: DurationDto,
    pub shutdown_drain_timeout: DurationDto,
    pub startup_bundle_policy: StartupBundlePolicyDto,
    pub cuda: CudaConfigDto,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerificationLimitsDto {
    pub maximum_records: Option<DecimalU64Dto>,
    pub maximum_duration: Option<DurationDto>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum CancellationOutcomeDto {
    Signalled,
    AlreadySignalled,
    AlreadyCompleted,
    UnknownRequest,
    ShuttingDown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "reason", rename_all = "snake_case")]
pub enum RoutingUnavailableReasonDto {
    ImageCompilation,
    TopologyLimit {
        required: DecimalU64Dto,
        limit: DecimalU64Dto,
    },
    MetadataLimit {
        required: DecimalU64Dto,
        limit: DecimalU64Dto,
    },
    Bundle,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "outcome", rename_all = "snake_case")]
pub enum ImageBuildOutcomeDto {
    Published,
    Failed { reason: RoutingUnavailableReasonDto },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageBuildReportDto {
    pub duration: DurationDto,
    pub outcome: ImageBuildOutcomeDto,
    pub node_count: DecimalU64Dto,
    pub relation_kind_count: DecimalU64Dto,
    pub adjacency_count: DecimalU64Dto,
    pub bundle_bytes: Option<DecimalU64Dto>,
    pub partition_count: Option<DecimalU64Dto>,
    pub segment_count: Option<DecimalU64Dto>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "outcome", rename_all = "snake_case")]
pub enum PublicationOutcomeDto {
    Published {
        report: ImageBuildReportDto,
    },
    RoutingUnavailable {
        reason: RoutingUnavailableReasonDto,
        report: ImageBuildReportDto,
    },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    deny_unknown_fields,
    tag = "kind",
    content = "record",
    rename_all = "snake_case"
)]
pub enum MutationDurableResultDto {
    Confirmed(ConfirmedRecordDto),
    EdgeRemoved(EdgeIdDto),
    NodeRemoved(NodeIdDto),
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MutationOutcomeDto {
    pub durable_result: MutationDurableResultDto,
    pub publication: PublicationOutcomeDto,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum CudaAvailabilityDto {
    Disabled,
    SupportNotCompiled,
    Available,
    Degraded,
    Unavailable,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceLimitsDto {
    pub maximum_active_image_bytes: DecimalU64Dto,
    pub maximum_concurrent_routes: DecimalU64Dto,
    pub maximum_reserved_route_bytes: DecimalU64Dto,
    pub maximum_destinations_per_request: DecimalU64Dto,
    pub maximum_hydration_handles_per_request: DecimalU64Dto,
    pub maximum_total_bundle_bytes: DecimalU64Dto,
    pub target_partition_topology_bytes: DecimalU64Dto,
    pub hard_maximum_partition_topology_bytes: DecimalU64Dto,
    pub maximum_resident_image_metadata_bytes: DecimalU64Dto,
    pub host_partition_cache_bytes: DecimalU64Dto,
    pub host_partition_cache_entries: DecimalU64Dto,
    pub routing_io_workers: DecimalU64Dto,
    pub maximum_queued_partition_reads: DecimalU64Dto,
    pub routing_io_staging_bytes: DecimalU64Dto,
    pub maximum_retired_bundle_bytes: DecimalU64Dto,
    pub maximum_retired_bundle_count: DecimalU64Dto,
    pub maximum_concurrent_checkpoints: DecimalU64Dto,
    pub maximum_maintenance_workers: DecimalU64Dto,
    pub maximum_queued_maintenance: DecimalU64Dto,
    pub maximum_verification_records: DecimalU64Dto,
    pub maximum_verification_duration: DurationDto,
    pub shutdown_drain_timeout: DurationDto,
    pub cuda_maximum_topology_bytes: DecimalU64Dto,
    pub cuda_maximum_concurrent_searches: DecimalU64Dto,
    pub cuda_maximum_batch_lanes: DecimalU64Dto,
    pub cuda_maximum_reserved_search_bytes: DecimalU64Dto,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiLimitsDto {
    pub maximum_encoded_bytes: DecimalU64Dto,
    pub maximum_name_bytes: DecimalU64Dto,
    pub maximum_payload_bytes: DecimalU64Dto,
    pub maximum_destinations: DecimalU64Dto,
    pub maximum_profile_entries: DecimalU64Dto,
    pub maximum_path_steps: DecimalU64Dto,
    pub maximum_hydration_handles: DecimalU64Dto,
    pub maximum_subgraph_nodes: DecimalU64Dto,
    pub maximum_subgraph_edges: DecimalU64Dto,
    pub maximum_diagnostic_text_bytes: DecimalU64Dto,
    pub maximum_nesting_depth: DecimalU64Dto,
    pub maximum_json_values: DecimalU64Dto,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CudaDeviceCapabilitiesDto {
    pub compute_capability_major: i32,
    pub compute_capability_minor: i32,
    pub total_memory_bytes: DecimalU64Dto,
    pub kernel_ptx_target: String,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilitiesDto {
    pub cpu_reference_routing: bool,
    pub gpu_routing: bool,
    pub cuda_support_compiled: bool,
    pub cuda_runtime: CudaAvailabilityDto,
    pub cuda_device: Option<CudaDeviceCapabilitiesDto>,
    pub cuda_algorithms: Vec<String>,
    pub cuda_distance_only: bool,
    pub cuda_paths: bool,
    pub cuda_finite_edge_budgets: bool,
    pub cuda_partitioned_topology: bool,
    pub cuda_parallel_strategy: String,
    pub cuda_reset_mode: String,
    pub cuda_target_mode: String,
    pub cuda_profile_mode: String,
    pub cuda_path_evidence_mode: String,
    pub paths: bool,
    pub edge_budgets: bool,
    pub cancellation: bool,
    pub hydration: bool,
    pub current_state_hydration: bool,
    pub subgraphs: bool,
    pub canonical_subgraph_encoding: bool,
    pub durable_routing_images: bool,
    pub checkpoint_backup: bool,
    pub validated_restore: bool,
    pub store_maintenance: bool,
    pub explicit_shutdown: bool,
    pub numeric_policy_id: String,
    pub tie_policy_id: String,
    pub resource_limits: ResourceLimitsDto,
    pub api_limits: ApiLimitsDto,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum EngineLifecycleStateDto {
    Running,
    Closing,
    ShutDown,
}
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActiveOperationCountsDto {
    pub routes: DecimalU64Dto,
    pub mutations: DecimalU64Dto,
    pub checkpoints: DecimalU64Dto,
    pub maintenance: DecimalU64Dto,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleSnapshotDto {
    pub state: EngineLifecycleStateDto,
    pub active: ActiveOperationCountsDto,
    pub shutdown_active_before: Option<ActiveOperationCountsDto>,
    pub rejected_after_close: DecimalU64Dto,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "state", rename_all = "snake_case")]
pub enum RoutingHealthDto {
    Available,
    Unavailable { reason: RoutingUnavailableReasonDto },
    ShutDown,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoreHealthDto {
    pub wal_enabled: bool,
    pub ordinary_writes_sync: bool,
    pub write_attempts: DecimalU64Dto,
    pub write_failures: DecimalU64Dto,
    pub committed_bytes: DecimalU64Dto,
    pub completed_scans: DecimalU64Dto,
    pub scan_failures: DecimalU64Dto,
    pub checkpoint_attempts: DecimalU64Dto,
    pub checkpoint_failures: DecimalU64Dto,
    pub restore_attempts: DecimalU64Dto,
    pub restore_failures: DecimalU64Dto,
    pub flush_attempts: DecimalU64Dto,
    pub flush_failures: DecimalU64Dto,
    pub compaction_attempts: DecimalU64Dto,
    pub compaction_failures: DecimalU64Dto,
    pub last_verification_succeeded: Option<bool>,
    pub last_maintenance_succeeded: Option<bool>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetirementHealthDto {
    pub bundle_count: DecimalU64Dto,
    pub bundle_bytes: DecimalU64Dto,
    pub maximum_bundle_count: DecimalU64Dto,
    pub maximum_bundle_bytes: DecimalU64Dto,
    pub limit_exceeded: bool,
    pub cleanup_failure_present: bool,
    pub cumulative_backpressure_waits: DecimalU64Dto,
    pub cumulative_backpressure_duration: DurationDto,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CudaHealthDto {
    pub availability: CudaAvailabilityDto,
    pub resident_node_count: DecimalU64Dto,
    pub resident_adjacency_count: DecimalU64Dto,
    pub resident_topology_bytes: DecimalU64Dto,
    pub partitioned_topology: bool,
    pub queued_lanes: DecimalU64Dto,
    pub active_lanes: DecimalU64Dto,
    pub peak_active_lanes: DecimalU64Dto,
    pub reserved_search_bytes: DecimalU64Dto,
    pub peak_reserved_search_bytes: DecimalU64Dto,
    pub cumulative_admission_rejections: DecimalU64Dto,
    pub worker_running: bool,
    pub cumulative_uploads: DecimalU64Dto,
    pub cumulative_upload_failures: DecimalU64Dto,
    pub cumulative_launches: DecimalU64Dto,
    pub cumulative_launch_failures: DecimalU64Dto,
    pub cumulative_fallbacks: DecimalU64Dto,
    pub cumulative_cancellations: DecimalU64Dto,
    pub cumulative_context_reinitializations: DecimalU64Dto,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthDto {
    pub durable_catalog_available: bool,
    pub lifecycle: LifecycleSnapshotDto,
    pub store: Option<StoreHealthDto>,
    pub routing: RoutingHealthDto,
    pub current_image_age: Option<DurationDto>,
    pub active_image_references: DecimalU64Dto,
    pub image_node_count: Option<DecimalU64Dto>,
    pub image_relation_kind_count: Option<DecimalU64Dto>,
    pub image_adjacency_count: Option<DecimalU64Dto>,
    pub startup_image_duration: DurationDto,
    pub last_image_build: ImageBuildReportDto,
    pub image_corruption_present: bool,
    pub cuda_degradation_present: bool,
    pub cuda_recovery_present: bool,
    pub active_routes: DecimalU64Dto,
    pub peak_active_routes: DecimalU64Dto,
    pub reserved_route_bytes: DecimalU64Dto,
    pub peak_reserved_route_bytes: DecimalU64Dto,
    pub cumulative_route_admissions: DecimalU64Dto,
    pub cumulative_admission_rejections: DecimalU64Dto,
    pub cumulative_cancellations: DecimalU64Dto,
    pub cumulative_image_build_failures: DecimalU64Dto,
    pub retired_bundles: RetirementHealthDto,
    pub cuda: CudaHealthDto,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogSummaryDto {
    pub candidate_nodes: DecimalU64Dto,
    pub candidate_relation_kinds: DecimalU64Dto,
    pub candidate_edges: DecimalU64Dto,
    pub confirmed_nodes: DecimalU64Dto,
    pub relation_kinds: DecimalU64Dto,
    pub confirmed_edges: DecimalU64Dto,
    pub node_name_entries: DecimalU64Dto,
    pub relation_name_entries: DecimalU64Dto,
    pub outgoing_entries: DecimalU64Dto,
    pub incoming_entries: DecimalU64Dto,
    pub routing_pointer_present: bool,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerificationReportDto {
    pub summary: CatalogSummaryDto,
    pub records_examined: DecimalU64Dto,
    pub decoded_bytes: DecimalU64Dto,
    pub catalog_checksum: Binary64Dto,
    pub duration: DurationDto,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointReportDto {
    pub catalog: VerificationReportDto,
    pub file_count: DecimalU64Dto,
    pub bytes: DecimalU64Dto,
    pub content_checksum: Binary64Dto,
    pub duration: DurationDto,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestoreReportDto {
    pub source_file_count: DecimalU64Dto,
    pub source_bytes: DecimalU64Dto,
    pub source_checksum: Binary64Dto,
    pub cleared_routing_pointer: bool,
    pub restored_catalog: VerificationReportDto,
    pub duration: DurationDto,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineRestoreReportDto {
    pub store: RestoreReportDto,
    pub routing_image: ImageBuildReportDto,
    pub smoke_catalog_verified: bool,
    pub smoke_route_verified: bool,
    pub smoke_hydration_verified: bool,
    pub cuda_initialized: bool,
    pub shutdown: ShutdownReportDto,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompactionFamilyReportDto {
    pub name: String,
    pub live_sst_bytes_before: Option<DecimalU64Dto>,
    pub live_sst_bytes_after: Option<DecimalU64Dto>,
    pub total_sst_bytes_before: Option<DecimalU64Dto>,
    pub total_sst_bytes_after: Option<DecimalU64Dto>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompactionReportDto {
    pub families: Vec<CompactionFamilyReportDto>,
    pub duration: DurationDto,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum ShutdownFailureStageDto {
    Drain,
    RoutingIo,
    CudaWorker,
    StoreFlush,
    StoreHandle,
    Lifecycle,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShutdownFailureDto {
    pub stage: ShutdownFailureStageDto,
    pub category: String,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DrainOutcomeDto {
    pub drained: bool,
    pub remaining: ActiveOperationCountsDto,
    pub duration: DurationDto,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CudaWorkerShutdownReportDto {
    pub queued_at_request: DecimalU64Dto,
    pub active_at_request: DecimalU64Dto,
    pub queued_routes_rejected: DecimalU64Dto,
    pub joined: bool,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShutdownReportDto {
    pub state_before: EngineLifecycleStateDto,
    pub state_after: EngineLifecycleStateDto,
    pub already_shut_down: bool,
    pub active_before: ActiveOperationCountsDto,
    pub drain: DrainOutcomeDto,
    pub drained: ActiveOperationCountsDto,
    pub active_requests_signalled: DecimalU64Dto,
    pub newly_signalled_requests: DecimalU64Dto,
    pub partition_io_stopped: bool,
    pub cuda_worker: Option<CudaWorkerShutdownReportDto>,
    pub store_flush_duration: Option<DurationDto>,
    pub retired_bundles: RetirementHealthDto,
    pub duration: DurationDto,
    pub failures: Vec<ShutdownFailureDto>,
}
