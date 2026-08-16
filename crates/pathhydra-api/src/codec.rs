use std::{fmt, io};

use serde::{Serialize, de::DeserializeOwned};

use crate::limits::ApiLimits;

/// The finite limits reported by the canonical boundary codec.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodecLimit {
    EncodedBytes,
    NestingDepth,
    JsonValues,
    NameBytes,
    PayloadBytes,
    Destinations,
    ProfileEntries,
    PathSteps,
    HydrationHandles,
    SubgraphNodes,
    SubgraphEdges,
    DiagnosticTextBytes,
}

/// Syntax classes callers may safely branch on without inspecting input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MalformedKind {
    InvalidUtf8,
    ByteOrderMark,
    JsonSyntax,
    DuplicateField,
    UnknownField,
    TrailingDocument,
    NonCanonical,
    ContextualValidation,
}

/// Bounded canonical encoding and decoding failures.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CodecError {
    Allocation {
        requested_bytes: usize,
    },
    LimitExceeded {
        limit: CodecLimit,
        maximum: usize,
        observed_at_least: usize,
    },
    Malformed {
        kind: MalformedKind,
        line: Option<usize>,
        column: Option<usize>,
    },
}

pub type ApiCodecError = CodecError;

/// Semantic validation hook implemented by every boundary DTO.
///
/// Syntax scanning bounds allocations globally; this hook enforces the
/// field-specific name, payload, cardinality, ordering, and contextual limits
/// before decoded input can reach the facade.
pub trait CanonicalDto: Serialize + DeserializeOwned {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError>;
}

impl CodecError {
    #[must_use]
    pub const fn limit(limit: CodecLimit, maximum: usize, observed_at_least: usize) -> Self {
        Self::LimitExceeded {
            limit,
            maximum,
            observed_at_least,
        }
    }

    #[must_use]
    pub const fn contextual_validation() -> Self {
        Self::Malformed {
            kind: MalformedKind::ContextualValidation,
            line: None,
            column: None,
        }
    }
}

impl fmt::Display for CodecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Allocation { requested_bytes } => write!(
                formatter,
                "canonical codec could not reserve {requested_bytes} bytes"
            ),
            Self::LimitExceeded {
                limit,
                maximum,
                observed_at_least,
            } => write!(
                formatter,
                "canonical codec {limit:?} limit is {maximum}; observed at least {observed_at_least}"
            ),
            Self::Malformed { kind, line, column } => {
                write!(formatter, "malformed canonical JSON: {kind:?}")?;
                if let (Some(line), Some(column)) = (line, column) {
                    write!(formatter, " at line {line}, column {column}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for CodecError {}

/// A decoded value paired with its one canonical byte representation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanonicalDocument<T> {
    pub value: T,
    pub canonical_bytes: Vec<u8>,
    pub input_was_canonical: bool,
}

/// Encode compact UTF-8 JSON using serde struct declaration order.
///
/// Boundary DTOs prohibit nondeterministically iterated maps and implement
/// canonical ID, bit-pattern, and payload wrappers. The bounded writer starts
/// with a small fallible reserve and grows fallibly without exceeding the
/// configured byte ceiling.
pub fn encode_canonical<T: CanonicalDto>(
    value: &T,
    limits: &ApiLimits,
) -> Result<Vec<u8>, CodecError> {
    value.validate_boundary(limits)?;
    let mut writer = BoundedWriter::new(limits.maximum_encoded_bytes)?;
    let result = {
        let mut serializer = serde_json::Serializer::new(&mut writer);
        value.serialize(&mut serializer)
    };
    if let Err(error) = result {
        if writer.overflowed {
            return Err(CodecError::limit(
                CodecLimit::EncodedBytes,
                limits.maximum_encoded_bytes,
                writer.observed_at_least,
            ));
        }
        if writer.allocation_failed {
            return Err(CodecError::Allocation {
                requested_bytes: writer.requested_bytes,
            });
        }
        return Err(malformed_from_serde(&error));
    }
    Ok(writer.bytes)
}

pub use encode_canonical as encode;

/// Decode one JSON document, apply contextual validation, and return its
/// canonical re-encoding.
pub fn decode_and_reencode<T>(
    input: &[u8],
    limits: &ApiLimits,
) -> Result<CanonicalDocument<T>, CodecError>
where
    T: CanonicalDto,
{
    scan_before_deserialize(input, limits)?;
    let mut deserializer = serde_json::Deserializer::from_slice(input);
    let value = T::deserialize(&mut deserializer).map_err(|error| malformed_from_serde(&error))?;
    deserializer.end().map_err(|error| {
        let mut malformed = malformed_from_serde(&error);
        if let CodecError::Malformed { kind, .. } = &mut malformed {
            *kind = MalformedKind::TrailingDocument;
        }
        malformed
    })?;
    value.validate_boundary(limits)?;
    let canonical_bytes = encode_canonical(&value, limits)?;
    let input_was_canonical = canonical_bytes == input;
    Ok(CanonicalDocument {
        value,
        canonical_bytes,
        input_was_canonical,
    })
}

pub fn decode<T: CanonicalDto>(input: &[u8], limits: &ApiLimits) -> Result<T, CodecError> {
    decode_and_reencode(input, limits).map(|document| document.value)
}

/// Require an already canonical document. Ordinary ingestion may instead
/// accept whitespace or field reordering and canonicalize it explicitly.
pub fn decode_strict_canonical<T: CanonicalDto>(
    input: &[u8],
    limits: &ApiLimits,
) -> Result<T, CodecError> {
    let document = decode_and_reencode(input, limits)?;
    if !document.input_was_canonical {
        return Err(CodecError::Malformed {
            kind: MalformedKind::NonCanonical,
            line: None,
            column: None,
        });
    }
    Ok(document.value)
}

fn scan_before_deserialize(input: &[u8], limits: &ApiLimits) -> Result<(), CodecError> {
    if input.len() > limits.maximum_encoded_bytes {
        return Err(CodecError::limit(
            CodecLimit::EncodedBytes,
            limits.maximum_encoded_bytes,
            input.len(),
        ));
    }
    let text = std::str::from_utf8(input).map_err(|_| CodecError::Malformed {
        kind: MalformedKind::InvalidUtf8,
        line: None,
        column: None,
    })?;
    if text.starts_with('\u{feff}') {
        return Err(CodecError::Malformed {
            kind: MalformedKind::ByteOrderMark,
            line: Some(1),
            column: Some(1),
        });
    }

    let bytes = text.as_bytes();
    let mut index = 0_usize;
    let mut depth = 0_usize;
    let mut values = 0_usize;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                let end = scan_string(bytes, index)?;
                let mut after = end;
                while after < bytes.len() && bytes[after].is_ascii_whitespace() {
                    after += 1;
                }
                if after >= bytes.len() || bytes[after] != b':' {
                    increment_values(&mut values, limits)?;
                }
                index = end;
            }
            b'{' | b'[' => {
                increment_values(&mut values, limits)?;
                depth = depth.saturating_add(1);
                if depth > limits.maximum_nesting_depth {
                    return Err(CodecError::limit(
                        CodecLimit::NestingDepth,
                        limits.maximum_nesting_depth,
                        depth,
                    ));
                }
                index += 1;
            }
            b'}' | b']' => {
                let Some(next_depth) = depth.checked_sub(1) else {
                    return Err(CodecError::Malformed {
                        kind: MalformedKind::JsonSyntax,
                        line: None,
                        column: None,
                    });
                };
                depth = next_depth;
                index += 1;
            }
            b'-' | b'0'..=b'9' | b't' | b'f' | b'n' => {
                increment_values(&mut values, limits)?;
                index += 1;
                while index < bytes.len()
                    && !matches!(bytes[index], b',' | b']' | b'}')
                    && !bytes[index].is_ascii_whitespace()
                {
                    index += 1;
                }
            }
            _ => index += 1,
        }
    }
    if depth != 0 {
        return Err(CodecError::Malformed {
            kind: MalformedKind::JsonSyntax,
            line: None,
            column: None,
        });
    }
    Ok(())
}

fn scan_string(bytes: &[u8], start: usize) -> Result<usize, CodecError> {
    let mut index = start + 1;
    let mut escaped = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if escaped {
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else if byte == b'"' {
            return Ok(index + 1);
        }
        index += 1;
    }
    Err(CodecError::Malformed {
        kind: MalformedKind::JsonSyntax,
        line: None,
        column: None,
    })
}

fn increment_values(values: &mut usize, limits: &ApiLimits) -> Result<(), CodecError> {
    *values = values.saturating_add(1);
    if *values > limits.maximum_json_values {
        return Err(CodecError::limit(
            CodecLimit::JsonValues,
            limits.maximum_json_values,
            *values,
        ));
    }
    Ok(())
}

fn malformed_from_serde(error: &serde_json::Error) -> CodecError {
    let rendered = error.to_string();
    let kind = if rendered.contains("duplicate field") {
        MalformedKind::DuplicateField
    } else if rendered.contains("unknown field") {
        MalformedKind::UnknownField
    } else if rendered.contains("trailing characters") {
        MalformedKind::TrailingDocument
    } else {
        MalformedKind::JsonSyntax
    };
    CodecError::Malformed {
        kind,
        line: Some(error.line()),
        column: Some(error.column()),
    }
}

struct BoundedWriter {
    bytes: Vec<u8>,
    maximum: usize,
    overflowed: bool,
    allocation_failed: bool,
    requested_bytes: usize,
    observed_at_least: usize,
}

impl BoundedWriter {
    fn new(maximum: usize) -> Result<Self, CodecError> {
        let mut bytes = Vec::new();
        let initial = maximum.min(4 * 1024);
        bytes
            .try_reserve_exact(initial)
            .map_err(|_| CodecError::Allocation {
                requested_bytes: initial,
            })?;
        Ok(Self {
            bytes,
            maximum,
            overflowed: false,
            allocation_failed: false,
            requested_bytes: initial,
            observed_at_least: 0,
        })
    }
}

impl io::Write for BoundedWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let attempted = self.bytes.len().saturating_add(buffer.len());
        if attempted > self.maximum {
            self.overflowed = true;
            self.observed_at_least = attempted;
            return Err(io::Error::other("canonical JSON byte limit exceeded"));
        }
        if attempted > self.bytes.capacity() {
            let additional = attempted.saturating_sub(self.bytes.len());
            if self.bytes.try_reserve_exact(additional).is_err() {
                self.allocation_failed = true;
                self.requested_bytes = attempted;
                return Err(io::Error::other("canonical JSON allocation failed"));
            }
        }
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

use crate::dto::*;

fn dto_validation(result: Result<(), DtoValidationError>) -> Result<(), ApiCodecError> {
    result.map_err(|_| ApiCodecError::contextual_validation())
}

fn bounded_len(limit: CodecLimit, maximum: usize, observed: usize) -> Result<(), ApiCodecError> {
    if observed > maximum {
        Err(ApiCodecError::limit(limit, maximum, observed))
    } else {
        Ok(())
    }
}

fn validate_name(value: &str, limits: &ApiLimits) -> Result<(), ApiCodecError> {
    bounded_len(
        CodecLimit::NameBytes,
        limits.maximum_name_bytes,
        value.len(),
    )
}

fn validate_text(value: &str, limits: &ApiLimits) -> Result<(), ApiCodecError> {
    bounded_len(
        CodecLimit::DiagnosticTextBytes,
        limits.maximum_diagnostic_text_bytes,
        value.len(),
    )
}

fn validate_payload(value: &PayloadDto, limits: &ApiLimits) -> Result<(), ApiCodecError> {
    let encoded = value.as_str().as_bytes();
    let padding = encoded
        .iter()
        .rev()
        .take_while(|&&byte| byte == b'=')
        .count();
    let decoded = encoded
        .len()
        .saturating_div(4)
        .saturating_mul(3)
        .saturating_sub(padding);
    bounded_len(
        CodecLimit::PayloadBytes,
        limits.maximum_payload_bytes,
        decoded,
    )
}

macro_rules! canonical_simple {
    ($($type:ty),+ $(,)?) => {$(
        impl CanonicalDto for $type {
            fn validate_boundary(&self, _limits: &ApiLimits) -> Result<(), ApiCodecError> {
                Ok(())
            }
        }
    )+};
}

canonical_simple!(
    ApiErrorCategoryDto,
    DecimalU64Dto,
    CandidateIdDto,
    NodeIdDto,
    RelationIdDto,
    EdgeIdDto,
    RequestIdDto,
    Binary32Dto,
    Binary64Dto,
    SearchBudgetDto,
    TiePolicyDto,
    NumericPolicyDto,
    CompletionReasonDto,
    ExecutorDto,
    ExecutorSelectionReasonDto,
    CudaIneligibilityDto,
    CpuSearchDiagnosticsDto,
    EdgeHandleDto,
    CudaExecutorPolicyDto,
    StartupBundlePolicyDto,
    CancellationOutcomeDto,
    RoutingUnavailableReasonDto,
    CudaAvailabilityDto,
    ApiLimitsDto,
    EngineLifecycleStateDto,
    ActiveOperationCountsDto,
    StoreHealthDto,
    CudaHealthDto,
    CatalogSummaryDto,
    ShutdownFailureStageDto,
    CudaWorkerShutdownReportDto,
);

impl CanonicalDto for ApiErrorDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        validate_text(&self.code, limits)?;
        validate_text(&self.message, limits)
    }
}

fn validate_base_weight(value: &Binary32Dto) -> Result<(), ApiCodecError> {
    pathhydra_core::BaseWeight::from_bits(
        value
            .bits()
            .map_err(|_| ApiCodecError::contextual_validation())?,
    )
    .map(|_| ())
    .map_err(|_| ApiCodecError::contextual_validation())
}

fn validate_multiplier(value: &Binary32Dto) -> Result<(), ApiCodecError> {
    pathhydra_routing::RelationMultiplier::from_bits(
        value
            .bits()
            .map_err(|_| ApiCodecError::contextual_validation())?,
    )
    .map(|_| ())
    .map_err(|_| ApiCodecError::contextual_validation())
}

fn validate_nonnegative_finite(value: &Binary64Dto) -> Result<(), ApiCodecError> {
    let value = f64::from_bits(
        value
            .bits()
            .map_err(|_| ApiCodecError::contextual_validation())?,
    );
    if value.is_finite() && value >= 0.0 && !(value == 0.0 && value.is_sign_negative()) {
        Ok(())
    } else {
        Err(ApiCodecError::contextual_validation())
    }
}

impl CanonicalDto for DurationDto {
    fn validate_boundary(&self, _limits: &ApiLimits) -> Result<(), ApiCodecError> {
        dto_validation(self.to_duration().map(|_| ()))
    }
}

impl CanonicalDto for EdgeRecordDto {
    fn validate_boundary(&self, _limits: &ApiLimits) -> Result<(), ApiCodecError> {
        validate_base_weight(&self.base_weight)
    }
}

impl CanonicalDto for RelationUseDto {
    fn validate_boundary(&self, _limits: &ApiLimits) -> Result<(), ApiCodecError> {
        match self {
            Self::Disabled => Ok(()),
            Self::Enabled { multiplier } => validate_multiplier(multiplier),
        }
    }
}

impl CanonicalDto for RelationProfileEntryDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        self.relation_use.validate_boundary(limits)
    }
}

impl CanonicalDto for PathStepDto {
    fn validate_boundary(&self, _limits: &ApiLimits) -> Result<(), ApiCodecError> {
        validate_base_weight(&self.base_weight)?;
        validate_multiplier(&self.multiplier)?;
        validate_nonnegative_finite(&self.effective_weight)
    }
}

impl CanonicalDto for PayloadDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        validate_payload(self, limits)
    }
}

impl CanonicalDto for CandidateDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        match self {
            Self::Node { name, payload, .. } => {
                validate_name(name, limits)?;
                validate_payload(payload, limits)
            }
            Self::RelationKind { name, .. } => validate_name(name, limits),
            Self::Edge { base_weight, .. } => validate_base_weight(base_weight),
        }
    }
}

impl CanonicalDto for NodeRecordDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        validate_name(&self.name, limits)?;
        validate_payload(&self.payload, limits)
    }
}

impl CanonicalDto for RelationKindRecordDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        validate_name(&self.name, limits)
    }
}

impl CanonicalDto for ConfirmedRecordDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        match self {
            Self::Node(record) => record.validate_boundary(limits),
            Self::RelationKind(record) => record.validate_boundary(limits),
            Self::Edge(record) => record.validate_boundary(limits),
        }
    }
}

impl CanonicalDto for RelationProfileDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        bounded_len(
            CodecLimit::ProfileEntries,
            limits.maximum_profile_entries,
            self.entries.len(),
        )?;
        for entry in &self.entries {
            entry.validate_boundary(limits)?;
        }
        dto_validation(self.validate())
    }
}

impl CanonicalDto for RoutingRequestDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        bounded_len(
            CodecLimit::Destinations,
            limits.maximum_destinations,
            self.destinations.len(),
        )?;
        self.profile.validate_boundary(limits)?;
        dto_validation(self.validate())
    }
}

impl CanonicalDto for RoutePathDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        bounded_len(
            CodecLimit::PathSteps,
            limits.maximum_path_steps,
            self.steps.len(),
        )?;
        dto_validation(self.validate())
    }
}

impl CanonicalDto for DestinationStateDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        if let Self::Exact {
            logical_distance,
            path,
        } = self
        {
            validate_nonnegative_finite(logical_distance)?;
            if let Some(path) = path {
                path.validate_boundary(limits)?;
            }
        }
        Ok(())
    }
}

impl CanonicalDto for DestinationResultDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        self.state.validate_boundary(limits)
    }
}

impl CanonicalDto for RoutingResponseDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        bounded_len(
            CodecLimit::Destinations,
            limits.maximum_destinations,
            self.results.len(),
        )?;
        self.profile.validate_boundary(limits)?;
        for result in &self.results {
            result.validate_boundary(limits)?;
        }
        dto_validation(self.validate())
    }
}

impl CanonicalDto for CudaRequestDiagnosticsDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        for value in [
            &self.algorithm,
            &self.parallel_strategy,
            &self.reset_mode,
            &self.target_mode,
            &self.profile_mode,
            &self.path_evidence_mode,
        ] {
            validate_text(value, limits)?;
        }
        if let Some(delta) = &self.delta {
            let value = f64::from_bits(
                delta
                    .bits()
                    .map_err(|_| ApiCodecError::contextual_validation())?,
            );
            if !value.is_finite() || value <= 0.0 {
                return Err(ApiCodecError::contextual_validation());
            }
        }
        for duration in [
            &self.queue_duration,
            &self.batch_collection_duration,
            &self.synchronized_execution_duration,
            &self.state_initialization_duration,
            &self.partition_scheduling_duration,
            &self.relation_relaxation_duration,
            &self.response_transfer_duration,
            &self.frontier_compaction_duration,
            &self.destination_completion_duration,
        ] {
            duration.validate_boundary(limits)?;
        }
        Ok(())
    }
}

impl CanonicalDto for PartitionedCpuDiagnosticsDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        self.io_wait.validate_boundary(limits)
    }
}

impl CanonicalDto for RuntimeDiagnosticsDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        self.admission_duration.validate_boundary(limits)?;
        self.execution_duration.validate_boundary(limits)?;
        if let Some(partitioned) = &self.partitioned_cpu {
            partitioned.io_wait.validate_boundary(limits)?;
        }
        if let Some(cuda) = &self.cuda {
            cuda.validate_boundary(limits)?;
        }
        Ok(())
    }
}

impl CanonicalDto for EngineRoutingResponseDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        self.response.validate_boundary(limits)?;
        self.diagnostics.validate_boundary(limits)
    }
}

impl CanonicalDto for HydrationRequestDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        let handles = self.node_ids.len().saturating_add(self.edge_ids.len());
        bounded_len(
            CodecLimit::HydrationHandles,
            limits.maximum_hydration_handles,
            handles,
        )?;
        if let Some(profile) = &self.profile {
            profile.validate_boundary(limits)?;
        }
        dto_validation(self.validate())
    }
}

impl CanonicalDto for HydratedNodeStateDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        match self {
            Self::Found { node } => node.validate_boundary(limits),
            Self::Missing => Ok(()),
        }
    }
}

impl CanonicalDto for HydratedNodeResultDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        self.state.validate_boundary(limits)
    }
}

impl CanonicalDto for EdgeEvaluationDto {
    fn validate_boundary(&self, _limits: &ApiLimits) -> Result<(), ApiCodecError> {
        match self {
            Self::Unprofiled | Self::Disabled => Ok(()),
            Self::Enabled {
                multiplier,
                effective_weight,
            } => {
                validate_multiplier(multiplier)?;
                validate_nonnegative_finite(effective_weight)
            }
        }
    }
}

impl CanonicalDto for HydratedEdgeDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        self.edge.validate_boundary(limits)?;
        self.relation_kind.validate_boundary(limits)?;
        self.evaluation.validate_boundary(limits)
    }
}

impl CanonicalDto for HydratedEdgeStateDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        match self {
            Self::Found { edge } => edge.validate_boundary(limits),
            Self::Missing => Ok(()),
        }
    }
}

impl CanonicalDto for HydratedEdgeResultDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        self.state.validate_boundary(limits)
    }
}

impl CanonicalDto for HydrationResponseDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        let handles = self.nodes.len().saturating_add(self.edges.len());
        bounded_len(
            CodecLimit::HydrationHandles,
            limits.maximum_hydration_handles,
            handles,
        )?;
        for node in &self.nodes {
            node.validate_boundary(limits)?;
        }
        for edge in &self.edges {
            edge.validate_boundary(limits)?;
        }
        if let Some(profile) = &self.profile {
            profile.validate_boundary(limits)?;
        }
        Ok(())
    }
}

impl CanonicalDto for HydratedPathDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        bounded_len(
            CodecLimit::PathSteps,
            limits.maximum_path_steps,
            self.edges.len(),
        )?;
        for node in &self.nodes {
            node.validate_boundary(limits)?;
        }
        for edge in &self.edges {
            edge.validate_boundary(limits)?;
        }
        if self.nodes.len() != self.edges.len().saturating_add(1) {
            return Err(ApiCodecError::contextual_validation());
        }
        for (index, edge) in self.edges.iter().enumerate() {
            if edge.edge.source != self.nodes[index].id
                || edge.edge.destination != self.nodes[index + 1].id
            {
                return Err(ApiCodecError::contextual_validation());
            }
        }
        validate_nonnegative_finite(&self.logical_distance)?;
        self.profile.validate_boundary(limits)
    }
}

impl CanonicalDto for SubgraphHandlesDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        bounded_len(
            CodecLimit::SubgraphNodes,
            limits.maximum_subgraph_nodes,
            self.nodes.len(),
        )?;
        bounded_len(
            CodecLimit::SubgraphEdges,
            limits.maximum_subgraph_edges,
            self.edges.len(),
        )?;
        dto_validation(self.validate())
    }
}

impl CanonicalDto for HydratedSubgraphDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        bounded_len(
            CodecLimit::SubgraphNodes,
            limits.maximum_subgraph_nodes,
            self.nodes.len(),
        )?;
        bounded_len(
            CodecLimit::SubgraphEdges,
            limits.maximum_subgraph_edges,
            self.edges.len(),
        )?;
        for node in &self.nodes {
            node.validate_boundary(limits)?;
        }
        for edge in &self.edges {
            edge.validate_boundary(limits)?;
        }
        if self.nodes.windows(2).any(|pair| pair[0].id >= pair[1].id)
            || self
                .edges
                .windows(2)
                .any(|pair| pair[0].edge.id >= pair[1].edge.id)
            || self
                .missing_node_ids
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
            || self
                .missing_edge_ids
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
        {
            return Err(ApiCodecError::contextual_validation());
        }
        if let Some(profile) = &self.profile {
            profile.validate_boundary(limits)?;
        }
        Ok(())
    }
}

impl CanonicalDto for CudaAlgorithmDto {
    fn validate_boundary(&self, _limits: &ApiLimits) -> Result<(), ApiCodecError> {
        if let Self::DeltaStepping { delta } = self {
            let value = f64::from_bits(
                delta
                    .bits()
                    .map_err(|_| ApiCodecError::contextual_validation())?,
            );
            if !value.is_finite() || value <= 0.0 {
                return Err(ApiCodecError::contextual_validation());
            }
        }
        Ok(())
    }
}

impl CanonicalDto for CudaConfigDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        self.batch_collection_delay.validate_boundary(limits)?;
        self.algorithm.validate_boundary(limits)
    }
}

impl CanonicalDto for PathHydraConfigDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        self.maximum_verification_duration
            .validate_boundary(limits)?;
        self.shutdown_drain_timeout.validate_boundary(limits)?;
        self.cuda.validate_boundary(limits)
    }
}

impl CanonicalDto for VerificationLimitsDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        if let Some(duration) = &self.maximum_duration {
            duration.validate_boundary(limits)?;
        }
        Ok(())
    }
}

impl CanonicalDto for ImageBuildOutcomeDto {
    fn validate_boundary(&self, _limits: &ApiLimits) -> Result<(), ApiCodecError> {
        Ok(())
    }
}

impl CanonicalDto for ImageBuildReportDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        self.duration.validate_boundary(limits)?;
        self.outcome.validate_boundary(limits)
    }
}

impl CanonicalDto for PublicationOutcomeDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        match self {
            Self::Published { report } | Self::RoutingUnavailable { report, .. } => {
                report.validate_boundary(limits)
            }
        }
    }
}

impl CanonicalDto for MutationDurableResultDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        match self {
            Self::Confirmed(record) => record.validate_boundary(limits),
            Self::EdgeRemoved(_) | Self::NodeRemoved(_) => Ok(()),
        }
    }
}

impl CanonicalDto for MutationOutcomeDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        self.durable_result.validate_boundary(limits)?;
        self.publication.validate_boundary(limits)
    }
}

impl CanonicalDto for ResourceLimitsDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        self.maximum_verification_duration
            .validate_boundary(limits)?;
        self.shutdown_drain_timeout.validate_boundary(limits)
    }
}

impl CanonicalDto for CudaDeviceCapabilitiesDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        validate_text(&self.kernel_ptx_target, limits)
    }
}

impl CanonicalDto for CapabilitiesDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        for value in self.cuda_algorithms.iter().chain([
            &self.cuda_parallel_strategy,
            &self.cuda_reset_mode,
            &self.cuda_target_mode,
            &self.cuda_profile_mode,
            &self.cuda_path_evidence_mode,
            &self.numeric_policy_id,
            &self.tie_policy_id,
        ]) {
            validate_text(value, limits)?;
        }
        if let Some(device) = &self.cuda_device {
            device.validate_boundary(limits)?;
        }
        self.resource_limits.validate_boundary(limits)?;
        self.api_limits.validate_boundary(limits)?;
        Ok(())
    }
}

impl CanonicalDto for LifecycleSnapshotDto {
    fn validate_boundary(&self, _limits: &ApiLimits) -> Result<(), ApiCodecError> {
        Ok(())
    }
}

impl CanonicalDto for RoutingHealthDto {
    fn validate_boundary(&self, _limits: &ApiLimits) -> Result<(), ApiCodecError> {
        Ok(())
    }
}

impl CanonicalDto for RetirementHealthDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        self.cumulative_backpressure_duration
            .validate_boundary(limits)
    }
}

impl CanonicalDto for HealthDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        if let Some(age) = &self.current_image_age {
            age.validate_boundary(limits)?;
        }
        self.startup_image_duration.validate_boundary(limits)?;
        self.last_image_build.validate_boundary(limits)?;
        self.retired_bundles.validate_boundary(limits)
    }
}

impl CanonicalDto for VerificationReportDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        self.duration.validate_boundary(limits)
    }
}

impl CanonicalDto for CheckpointReportDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        self.catalog.validate_boundary(limits)?;
        self.duration.validate_boundary(limits)
    }
}

impl CanonicalDto for RestoreReportDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        self.restored_catalog.validate_boundary(limits)?;
        self.duration.validate_boundary(limits)
    }
}

impl CanonicalDto for EngineRestoreReportDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        self.store.validate_boundary(limits)?;
        self.routing_image.validate_boundary(limits)?;
        self.shutdown.validate_boundary(limits)
    }
}

impl CanonicalDto for CompactionFamilyReportDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        validate_text(&self.name, limits)
    }
}

impl CanonicalDto for CompactionReportDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        for family in &self.families {
            family.validate_boundary(limits)?;
        }
        self.duration.validate_boundary(limits)
    }
}

impl CanonicalDto for ShutdownFailureDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        validate_text(&self.category, limits)
    }
}

impl CanonicalDto for DrainOutcomeDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        self.duration.validate_boundary(limits)
    }
}

impl CanonicalDto for ShutdownReportDto {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        for failure in &self.failures {
            failure.validate_boundary(limits)?;
        }
        self.drain.validate_boundary(limits)?;
        if let Some(duration) = &self.store_flush_duration {
            duration.validate_boundary(limits)?;
        }
        self.retired_bundles.validate_boundary(limits)?;
        self.duration.validate_boundary(limits)
    }
}
