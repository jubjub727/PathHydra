use std::{hint::black_box, time::Instant};

use pathhydra_api::{
    ActiveOperationCountsDto, ApiCodecError, ApiLimits, Binary32Dto, Binary64Dto, CandidateIdDto,
    CanonicalDto, CompletionReasonDto, CpuSearchDiagnosticsDto, CudaAvailabilityDto, CudaHealthDto,
    CudaIneligibilityDto, CudaRequestDiagnosticsDto, DecimalU64Dto, DestinationResultDto,
    DestinationStateDto, DurationDto, EdgeHandleDto, EdgeIdDto, EngineLifecycleStateDto,
    EngineRoutingResponseDto, ExecutorDto, ExecutorSelectionReasonDto, HealthDto,
    HydratedNodeResultDto, HydratedNodeStateDto, HydrationDiagnosticsDto, HydrationResponseDto,
    ImageBuildOutcomeDto, ImageBuildReportDto, LifecycleSnapshotDto, MalformedKind, NodeIdDto,
    NodeRecordDto, NumericPolicyDto, PartitionedCpuDiagnosticsDto, PathStepDto, PayloadDto,
    RelationIdDto, RelationProfileDto, RelationProfileEntryDto, RelationUseDto, RequestIdDto,
    RetirementHealthDto, RoutePathDto, RoutingHealthDto, RoutingRequestDto, RoutingResponseDto,
    RuntimeDiagnosticsDto, SearchBudgetDto, SubgraphHandlesDto, TiePolicyDto, decode,
    decode_and_reencode, decode_strict_canonical, encode,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExactFixture {
    id: String,
    bits32: String,
    bits64: String,
    name: String,
    payload: String,
}

impl CanonicalDto for ExactFixture {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        if self.name.len() > limits.maximum_name_bytes {
            return Err(ApiCodecError::limit(
                pathhydra_api::CodecLimit::NameBytes,
                limits.maximum_name_bytes,
                self.name.len(),
            ));
        }
        Ok(())
    }
}

fn exact_fixture() -> ExactFixture {
    ExactFixture {
        id: u64::MAX.to_string(),
        bits32: format!("{:08x}", u32::MAX),
        bits64: format!("{:016x}", u64::MAX),
        name: "Ångström/ångström/Ａngström\n'!?".to_owned(),
        payload: "/wAB/g==".to_owned(),
    }
}

#[test]
fn canonical_json_is_compact_stable_utf8_and_lossless() {
    let fixture = exact_fixture();
    let limits = ApiLimits::default();
    let encoded = encode(&fixture, &limits).unwrap();
    assert_eq!(
        encoded,
        "{\"id\":\"18446744073709551615\",\"bits32\":\"ffffffff\",\"bits64\":\"ffffffffffffffff\",\"name\":\"Ångström/ångström/Ａngström\\n'!?\",\"payload\":\"/wAB/g==\"}"
            .as_bytes()
    );
    assert!(!encoded.starts_with(&[0xef, 0xbb, 0xbf]));
    assert!(!encoded.windows(7).any(|window| window == b"version"));
    assert_eq!(decode::<ExactFixture>(&encoded, &limits).unwrap(), fixture);
}

#[test]
fn accepted_noncanonical_layout_reencodes_to_canonical_bytes() {
    let limits = ApiLimits::default();
    let input = r#" {
        "payload":"/wAB/g==", "name":"Ångström/ångström/Ａngström\n'!?",
        "bits64":"ffffffffffffffff", "bits32":"ffffffff",
        "id":"18446744073709551615"
    } "#
    .as_bytes();
    let decoded: ExactFixture = decode(input, &limits).unwrap();
    let canonical = encode(&exact_fixture(), &limits).unwrap();
    assert_eq!(encode(&decoded, &limits).unwrap(), canonical);
    let document = decode_and_reencode::<ExactFixture>(input, &limits).unwrap();
    assert!(!document.input_was_canonical);
    assert_eq!(document.canonical_bytes, canonical);
    assert!(decode_strict_canonical::<ExactFixture>(input, &limits).is_err());
    assert_eq!(
        decode_strict_canonical::<ExactFixture>(&canonical, &limits).unwrap(),
        exact_fixture()
    );
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WrapperFixture {
    node_id: NodeIdDto,
    candidate_id: CandidateIdDto,
    request_id: RequestIdDto,
    binary32: Binary32Dto,
    binary64: Binary64Dto,
    payload: PayloadDto,
}

impl CanonicalDto for WrapperFixture {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        self.payload.validate_boundary(limits)
    }
}

#[test]
fn real_dto_wrappers_preserve_max_ids_bits_and_arbitrary_payload_bytes() {
    let fixture = WrapperFixture {
        node_id: NodeIdDto::from_u64(u64::MAX),
        candidate_id: CandidateIdDto::from_u64(u64::MAX),
        request_id: RequestIdDto::from_u64(u64::MAX),
        binary32: Binary32Dto::from_bits(u32::MAX),
        binary64: Binary64Dto::from_bits(u64::MAX),
        payload: PayloadDto::from_bytes(&[0, 255, 1, 2, 128, 254]),
    };
    let encoded = encode(&fixture, &ApiLimits::default()).unwrap();
    let text = std::str::from_utf8(&encoded).unwrap();
    assert!(text.contains("\"18446744073709551615\""));
    assert!(text.contains("\"0xffffffff\""));
    assert!(text.contains("\"0xffffffffffffffff\""));
    assert!(text.contains("\"AP8BAoD+\""));
    assert_eq!(
        decode::<WrapperFixture>(&encoded, &ApiLimits::default()).unwrap(),
        fixture
    );
}

#[test]
fn handle_subgraphs_round_trip_self_and_parallel_edges_in_stable_order() {
    let subgraph = SubgraphHandlesDto {
        nodes: vec![NodeIdDto::from_u64(1), NodeIdDto::from_u64(2)],
        edges: vec![
            EdgeHandleDto {
                edge: EdgeIdDto::from_u64(10),
                source: NodeIdDto::from_u64(1),
                destination: NodeIdDto::from_u64(1),
            },
            EdgeHandleDto {
                edge: EdgeIdDto::from_u64(11),
                source: NodeIdDto::from_u64(1),
                destination: NodeIdDto::from_u64(2),
            },
            EdgeHandleDto {
                edge: EdgeIdDto::from_u64(12),
                source: NodeIdDto::from_u64(1),
                destination: NodeIdDto::from_u64(2),
            },
        ],
    };
    let encoded = encode(&subgraph, &ApiLimits::default()).unwrap();
    assert_eq!(
        decode::<SubgraphHandlesDto>(&encoded, &ApiLimits::default()).unwrap(),
        subgraph
    );
}

fn count(value: u64) -> DecimalU64Dto {
    DecimalU64Dto::from_u64(value)
}

fn search_diagnostics(
    completion: CompletionReasonDto,
    paths_requested: bool,
) -> CpuSearchDiagnosticsDto {
    let (unreachable, missing, incomplete, unique_present) = match completion {
        CompletionReasonDto::AllDestinationsFinalized => (0, 3, 0, 1),
        CompletionReasonDto::FrontierExhausted => (2, 1, 0, 3),
        CompletionReasonDto::BudgetExhausted | CompletionReasonDto::Cancelled => (0, 1, 2, 3),
    };
    CpuSearchDiagnosticsDto {
        examined_edges: count(13),
        relaxation_updates: count(8),
        finalized_nodes: count(5),
        frontier_high_water_mark: count(3),
        unique_present_destinations: count(unique_present),
        exact_destinations: count(2),
        unreachable_destinations: count(unreachable),
        missing_destinations: count(missing),
        incomplete_destinations: count(incomplete),
        path_reconstruction_steps: count(u64::from(paths_requested)),
        first_destination_duration: Some(DurationDto::default()),
        path_reconstruction_duration: DurationDto::default(),
    }
}

fn partitioned_diagnostics() -> PartitionedCpuDiagnosticsDto {
    PartitionedCpuDiagnosticsDto {
        partitions: count(2),
        file_bytes: count(1_024),
        io_wait: DurationDto::default(),
        cache_hits: count(3),
        cache_misses: count(2),
        cache_current_bytes: count(512),
        cache_high_water_bytes: count(768),
        cache_entries: count(1),
        cache_queue_depth: count(1),
    }
}

fn cuda_diagnostics(partitions: u64, paths_requested: bool) -> CudaRequestDiagnosticsDto {
    CudaRequestDiagnosticsDto {
        algorithm: "frontier".to_owned(),
        delta: None,
        queue_duration: DurationDto::default(),
        batch_collection_duration: DurationDto::default(),
        batch_width: count(1),
        lane_index: count(0),
        topology_bytes: count(2_048),
        search_bytes: count(4_096),
        host_to_device_bytes: count(512),
        device_to_host_bytes: count(256),
        kernel_launches: count(4),
        synchronized_execution_duration: DurationDto::default(),
        examined_edges: count(13),
        relaxation_attempts: count(13),
        relaxation_updates: count(8),
        phases: count(3),
        partitions_required: count(partitions),
        host_cache_hits: count(1),
        device_cache_hits: count(1),
        file_bytes: count(1_024),
        staged_bytes: count(512),
        transfer_bytes: count(768),
        parallel_strategy: "relation_threads_atomic_binary64".to_owned(),
        reset_mode: "explicit_clear".to_owned(),
        target_mode: "sorted_sparse_host".to_owned(),
        profile_mode: "inline_exact".to_owned(),
        path_evidence_mode: if paths_requested {
            "cpu_pass_same_image".to_owned()
        } else {
            "not_requested".to_owned()
        },
        state_initialization_duration: DurationDto::default(),
        partition_scheduling_duration: DurationDto::default(),
        relation_relaxation_duration: DurationDto::default(),
        response_transfer_duration: DurationDto::default(),
        frontier_compaction_duration: DurationDto::default(),
        compacted_task_count: count(13),
        destination_completion_duration: DurationDto::default(),
        destination_count_checked: count(5),
        atomic_cas_retries: count(2),
        first_destination_duration: Some(DurationDto::default()),
        path_reconstruction_duration: DurationDto::default(),
    }
}

fn routing_response(paths_requested: bool, completion: CompletionReasonDto) -> RoutingResponseDto {
    let origin = NodeIdDto::from_u64(1);
    let destination = NodeIdDto::from_u64(2);
    let distance = Binary64Dto::from_bits(0.5_f64.to_bits());
    let path = paths_requested.then(|| RoutePathDto {
        origin: origin.clone(),
        destination: destination.clone(),
        logical_distance: distance.clone(),
        steps: vec![PathStepDto {
            edge: EdgeIdDto::from_u64(1),
            source: origin.clone(),
            destination: destination.clone(),
            relation_kind: RelationIdDto::from_u64(1),
            base_weight: Binary32Dto::from_bits(0.25_f32.to_bits()),
            multiplier: Binary32Dto::from_bits(2.0_f32.to_bits()),
            effective_weight: distance.clone(),
        }],
    });
    let terminal_state = match completion {
        CompletionReasonDto::AllDestinationsFinalized => DestinationStateDto::MissingNode,
        CompletionReasonDto::FrontierExhausted => DestinationStateDto::Unreachable,
        CompletionReasonDto::BudgetExhausted | CompletionReasonDto::Cancelled => {
            DestinationStateDto::Incomplete
        }
    };
    RoutingResponseDto {
        origin,
        results: vec![
            DestinationResultDto {
                destination: destination.clone(),
                state: DestinationStateDto::Exact {
                    logical_distance: distance.clone(),
                    path: path.clone(),
                },
            },
            DestinationResultDto {
                destination: destination.clone(),
                state: DestinationStateDto::Exact {
                    logical_distance: distance,
                    path,
                },
            },
            DestinationResultDto {
                destination: NodeIdDto::from_u64(3),
                state: terminal_state.clone(),
            },
            DestinationResultDto {
                destination: NodeIdDto::from_u64(4),
                state: DestinationStateDto::MissingNode,
            },
            DestinationResultDto {
                destination: NodeIdDto::from_u64(5),
                state: terminal_state,
            },
        ],
        profile: RelationProfileDto::default(),
        numeric_policy: NumericPolicyDto::Binary32OperandsSeparateBinary64,
        tie_policy: TiePolicyDto::StablePredecessor,
        paths_requested,
        examined_edges: count(13),
        finalized_nodes: count(5),
        completion_reason: completion,
    }
}

fn runtime_diagnostics(
    executor: ExecutorDto,
    reason: ExecutorSelectionReasonDto,
    completion: CompletionReasonDto,
    partitioned: bool,
    cuda: bool,
    paths_requested: bool,
) -> RuntimeDiagnosticsDto {
    RuntimeDiagnosticsDto {
        request_id: RequestIdDto::from_u64(9),
        executor,
        selection_reason: reason,
        attempted_cuda: cuda,
        cuda_fallback_reason: None,
        numeric_policy: NumericPolicyDto::Binary32OperandsSeparateBinary64,
        tie_policy: TiePolicyDto::StablePredecessor,
        image_node_count: count(5),
        image_relation_kind_count: count(1),
        image_adjacency_count: count(13),
        reserved_working_bytes: count(8_192),
        admission_duration: DurationDto::default(),
        execution_duration: DurationDto::default(),
        first_destination_duration: Some(DurationDto::default()),
        reconstruction_duration: DurationDto::default(),
        completion_reason: completion,
        search: search_diagnostics(completion, paths_requested),
        partitioned_cpu: partitioned.then(partitioned_diagnostics),
        cuda: cuda.then(|| cuda_diagnostics(u64::from(partitioned) + 1, paths_requested)),
    }
}

#[test]
fn routing_states_policies_paths_and_runtime_diagnostics_round_trip() {
    let limits = ApiLimits::default();
    for budget in [
        SearchBudgetDto::Unlimited,
        SearchBudgetDto::ExaminedEdges { maximum: count(13) },
    ] {
        let request = RoutingRequestDto {
            origin: NodeIdDto::from_u64(1),
            destinations: vec![NodeIdDto::from_u64(2), NodeIdDto::from_u64(2)],
            profile: RelationProfileDto::default(),
            return_paths: true,
            budget,
            tie_policy: TiePolicyDto::StablePredecessor,
        };
        let encoded = encode(&request, &limits).unwrap();
        assert_eq!(
            decode::<RoutingRequestDto>(&encoded, &limits).unwrap(),
            request
        );
    }

    let cases = [
        (
            ExecutorDto::CpuReference,
            ExecutorSelectionReasonDto::CpuOnlyPolicy,
            CompletionReasonDto::AllDestinationsFinalized,
            false,
            false,
        ),
        (
            ExecutorDto::CpuReference,
            ExecutorSelectionReasonDto::AutomaticPolicySelectedCpu,
            CompletionReasonDto::FrontierExhausted,
            true,
            false,
        ),
        (
            ExecutorDto::NvidiaCuda,
            ExecutorSelectionReasonDto::PreferredCudaEligible,
            CompletionReasonDto::BudgetExhausted,
            false,
            true,
        ),
        (
            ExecutorDto::NvidiaCuda,
            ExecutorSelectionReasonDto::RequiredCudaEligible,
            CompletionReasonDto::Cancelled,
            false,
            true,
        ),
    ];
    for (index, (executor, reason, completion, partitioned, cuda)) in cases.into_iter().enumerate()
    {
        let paths_requested = index % 2 == 0;
        let value = EngineRoutingResponseDto {
            response: routing_response(paths_requested, completion),
            diagnostics: runtime_diagnostics(
                executor,
                reason,
                completion,
                partitioned,
                cuda,
                paths_requested,
            ),
        };
        let encoded = encode(&value, &limits).unwrap();
        assert_eq!(
            decode::<EngineRoutingResponseDto>(&encoded, &limits).unwrap(),
            value
        );
        assert_eq!(
            value.response.results[0].destination,
            value.response.results[1].destination
        );
    }

    for fallback in [
        CudaIneligibilityDto::Disabled,
        CudaIneligibilityDto::SupportNotCompiled,
        CudaIneligibilityDto::FiniteEdgeBudgetUnsupportedByCuda,
        CudaIneligibilityDto::NoResidentImage,
        CudaIneligibilityDto::ResourceRefusal,
        CudaIneligibilityDto::AutomaticPolicySelectedCpu,
        CudaIneligibilityDto::Unhealthy,
    ] {
        let mut diagnostics = runtime_diagnostics(
            ExecutorDto::CpuReference,
            ExecutorSelectionReasonDto::CpuFallback,
            CompletionReasonDto::FrontierExhausted,
            false,
            false,
            false,
        );
        diagnostics.attempted_cuda = true;
        diagnostics.cuda_fallback_reason = Some(fallback);
        let encoded = encode(&diagnostics, &limits).unwrap();
        assert_eq!(
            decode::<RuntimeDiagnosticsDto>(&encoded, &limits).unwrap(),
            diagnostics
        );
    }
}

#[test]
fn runtime_diagnostic_timing_and_response_reconciliation_are_canonical_invariants() {
    let limits = ApiLimits::default();
    let mut value = EngineRoutingResponseDto {
        response: routing_response(true, CompletionReasonDto::AllDestinationsFinalized),
        diagnostics: runtime_diagnostics(
            ExecutorDto::CpuReference,
            ExecutorSelectionReasonDto::CpuOnlyPolicy,
            CompletionReasonDto::AllDestinationsFinalized,
            false,
            false,
            true,
        ),
    };
    value.diagnostics.execution_duration = DurationDto {
        seconds: count(1),
        nanoseconds: 0,
    };
    value.diagnostics.reconstruction_duration = DurationDto {
        seconds: count(2),
        nanoseconds: 0,
    };
    value.diagnostics.search.path_reconstruction_duration =
        value.diagnostics.reconstruction_duration.clone();
    assert!(matches!(
        encode(&value, &limits),
        Err(ApiCodecError::Malformed {
            kind: MalformedKind::ContextualValidation,
            ..
        })
    ));

    value.diagnostics.reconstruction_duration = DurationDto::default();
    value.diagnostics.search.path_reconstruction_duration = DurationDto::default();
    value.diagnostics.search.exact_destinations = count(1);
    assert!(matches!(
        encode(&value, &limits),
        Err(ApiCodecError::Malformed {
            kind: MalformedKind::ContextualValidation,
            ..
        })
    ));

    let mut invalid_completion = EngineRoutingResponseDto {
        response: routing_response(true, CompletionReasonDto::AllDestinationsFinalized),
        diagnostics: runtime_diagnostics(
            ExecutorDto::CpuReference,
            ExecutorSelectionReasonDto::CpuOnlyPolicy,
            CompletionReasonDto::AllDestinationsFinalized,
            false,
            false,
            true,
        ),
    };
    invalid_completion.response.completion_reason = CompletionReasonDto::BudgetExhausted;
    invalid_completion.diagnostics.completion_reason = CompletionReasonDto::BudgetExhausted;
    assert!(matches!(
        encode(&invalid_completion, &limits),
        Err(ApiCodecError::Malformed {
            kind: MalformedKind::ContextualValidation,
            ..
        })
    ));

    let mut missing_first = EngineRoutingResponseDto {
        response: routing_response(true, CompletionReasonDto::AllDestinationsFinalized),
        diagnostics: runtime_diagnostics(
            ExecutorDto::CpuReference,
            ExecutorSelectionReasonDto::CpuOnlyPolicy,
            CompletionReasonDto::AllDestinationsFinalized,
            false,
            false,
            true,
        ),
    };
    missing_first.diagnostics.first_destination_duration = None;
    missing_first.diagnostics.search.first_destination_duration = None;
    assert!(matches!(
        encode(&missing_first, &limits),
        Err(ApiCodecError::Malformed {
            kind: MalformedKind::ContextualValidation,
            ..
        })
    ));

    let mut invalid_cuda_scope = EngineRoutingResponseDto {
        response: routing_response(false, CompletionReasonDto::AllDestinationsFinalized),
        diagnostics: runtime_diagnostics(
            ExecutorDto::NvidiaCuda,
            ExecutorSelectionReasonDto::PreferredCudaEligible,
            CompletionReasonDto::AllDestinationsFinalized,
            false,
            true,
            false,
        ),
    };
    invalid_cuda_scope.diagnostics.execution_duration = DurationDto {
        seconds: count(1),
        nanoseconds: 0,
    };
    invalid_cuda_scope
        .diagnostics
        .cuda
        .as_mut()
        .expect("CUDA diagnostics fixture")
        .queue_duration = DurationDto {
        seconds: count(2),
        nanoseconds: 0,
    };
    assert!(matches!(
        encode(&invalid_cuda_scope, &limits),
        Err(ApiCodecError::Malformed {
            kind: MalformedKind::ContextualValidation,
            ..
        })
    ));

    let invalid_hydration = HydrationDiagnosticsDto {
        requested_nodes: count(1),
        found_nodes: count(1),
        missing_nodes: count(1),
        ..HydrationDiagnosticsDto::default()
    };
    assert!(matches!(
        encode(&invalid_hydration, &limits),
        Err(ApiCodecError::Malformed {
            kind: MalformedKind::ContextualValidation,
            ..
        })
    ));
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
enum BenchmarkDocument {
    RoutingRequest(RoutingRequestDto),
    RoutePath(RoutePathDto),
    HydrationResponse(HydrationResponseDto),
    Health(Box<HealthDto>),
    LargeSubgraph(SubgraphHandlesDto),
}

impl CanonicalDto for BenchmarkDocument {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        match self {
            Self::RoutingRequest(value) => value.validate_boundary(limits),
            Self::RoutePath(value) => value.validate_boundary(limits),
            Self::HydrationResponse(value) => value.validate_boundary(limits),
            Self::Health(value) => value.validate_boundary(limits),
            Self::LargeSubgraph(value) => value.validate_boundary(limits),
        }
    }
}

fn benchmark_corpus() -> Vec<(&'static str, BenchmarkDocument)> {
    let request = RoutingRequestDto {
        origin: NodeIdDto::from_u64(u64::MAX),
        destinations: (2..66).map(NodeIdDto::from_u64).collect(),
        profile: RelationProfileDto {
            entries: (1..=16)
                .map(|id| RelationProfileEntryDto {
                    relation_kind: RelationIdDto::from_u64(id.into()),
                    relation_use: RelationUseDto::Enabled {
                        multiplier: Binary32Dto::from_bits(0x3f80_0000_u32 + id),
                    },
                })
                .collect(),
        },
        return_paths: true,
        budget: SearchBudgetDto::Unlimited,
        tie_policy: TiePolicyDto::StablePredecessor,
    };
    let path = RoutePathDto {
        origin: NodeIdDto::from_u64(1),
        destination: NodeIdDto::from_u64(256),
        logical_distance: Binary64Dto::from_bits(std::f64::consts::PI.to_bits()),
        steps: (1..256)
            .map(|id| PathStepDto {
                edge: EdgeIdDto::from_u64(10_000 + id),
                source: NodeIdDto::from_u64(id),
                destination: NodeIdDto::from_u64(id + 1),
                relation_kind: RelationIdDto::from_u64((id % 16) + 1),
                base_weight: Binary32Dto::from_bits(0.5_f32.to_bits()),
                multiplier: Binary32Dto::from_bits(1.0_f32.to_bits()),
                effective_weight: Binary64Dto::from_bits(0.5_f64.to_bits()),
            })
            .collect(),
    };
    let hydration = HydrationResponseDto {
        nodes: (1..=128)
            .map(|id| HydratedNodeResultDto {
                requested_node_id: NodeIdDto::from_u64(id),
                state: HydratedNodeStateDto::Found {
                    node: NodeRecordDto {
                        id: NodeIdDto::from_u64(id),
                        name: format!("Unicode exact node {id} — Å/å/Ａ"),
                        payload: PayloadDto::from_bytes(&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]),
                    },
                },
            })
            .collect(),
        edges: Vec::new(),
        profile: None,
        diagnostics: HydrationDiagnosticsDto {
            execution_duration: DurationDto::default(),
            requested_nodes: count(128),
            requested_edges: count(0),
            found_nodes: count(128),
            missing_nodes: count(0),
            found_edges: count(0),
            missing_edges: count(0),
        },
    };
    let health = benchmark_health();
    let subgraph = SubgraphHandlesDto {
        nodes: (100_000..110_000).map(NodeIdDto::from_u64).collect(),
        edges: (100_000..109_999)
            .map(|id| EdgeHandleDto {
                edge: EdgeIdDto::from_u64(200_000 + id),
                source: NodeIdDto::from_u64(id),
                destination: NodeIdDto::from_u64(id + 1),
            })
            .collect(),
    };
    vec![
        (
            "routing-request",
            BenchmarkDocument::RoutingRequest(request),
        ),
        ("route-path", BenchmarkDocument::RoutePath(path)),
        (
            "hydration-response",
            BenchmarkDocument::HydrationResponse(hydration),
        ),
        ("health", BenchmarkDocument::Health(Box::new(health))),
        ("large-subgraph", BenchmarkDocument::LargeSubgraph(subgraph)),
    ]
}

fn benchmark_health() -> HealthDto {
    let zero = || DecimalU64Dto::from_u64(0);
    let duration = || DurationDto::default();
    HealthDto {
        durable_catalog_available: true,
        lifecycle: LifecycleSnapshotDto {
            state: EngineLifecycleStateDto::Running,
            active: ActiveOperationCountsDto {
                routes: DecimalU64Dto::from_u64(17),
                ..ActiveOperationCountsDto::default()
            },
            shutdown_active_before: None,
            rejected_after_close: zero(),
        },
        store: None,
        routing: RoutingHealthDto::Available,
        current_image_age: Some(DurationDto {
            seconds: DecimalU64Dto::from_u64(5),
            nanoseconds: 123_456_789,
        }),
        active_image_references: DecimalU64Dto::from_u64(17),
        image_node_count: Some(DecimalU64Dto::from_u64(10_000)),
        image_relation_kind_count: Some(DecimalU64Dto::from_u64(16)),
        image_adjacency_count: Some(DecimalU64Dto::from_u64(9_999)),
        startup_image_duration: duration(),
        last_image_build: ImageBuildReportDto {
            duration: duration(),
            outcome: ImageBuildOutcomeDto::Published,
            node_count: DecimalU64Dto::from_u64(10_000),
            relation_kind_count: DecimalU64Dto::from_u64(16),
            adjacency_count: DecimalU64Dto::from_u64(9_999),
            bundle_bytes: Some(DecimalU64Dto::from_u64(646_668)),
            partition_count: Some(DecimalU64Dto::from_u64(1)),
            segment_count: Some(DecimalU64Dto::from_u64(1)),
        },
        image_corruption_present: false,
        cuda_degradation_present: false,
        cuda_recovery_present: false,
        active_routes: DecimalU64Dto::from_u64(17),
        peak_active_routes: DecimalU64Dto::from_u64(23),
        reserved_route_bytes: DecimalU64Dto::from_u64(4096),
        peak_reserved_route_bytes: DecimalU64Dto::from_u64(8192),
        cumulative_route_admissions: DecimalU64Dto::from_u64(1000),
        cumulative_admission_rejections: zero(),
        cumulative_cancellations: DecimalU64Dto::from_u64(3),
        cumulative_image_build_failures: zero(),
        retired_bundles: RetirementHealthDto {
            bundle_count: zero(),
            bundle_bytes: zero(),
            maximum_bundle_count: DecimalU64Dto::from_u64(8),
            maximum_bundle_bytes: DecimalU64Dto::from_u64(1 << 30),
            limit_exceeded: false,
            cleanup_failure_present: false,
            cumulative_backpressure_waits: zero(),
            cumulative_backpressure_duration: duration(),
        },
        cuda: CudaHealthDto {
            availability: CudaAvailabilityDto::Available,
            resident_node_count: DecimalU64Dto::from_u64(10_000),
            resident_adjacency_count: DecimalU64Dto::from_u64(9_999),
            resident_topology_bytes: DecimalU64Dto::from_u64(1 << 20),
            partitioned_topology: false,
            queued_lanes: zero(),
            active_lanes: DecimalU64Dto::from_u64(2),
            peak_active_lanes: DecimalU64Dto::from_u64(4),
            reserved_search_bytes: DecimalU64Dto::from_u64(1 << 16),
            peak_reserved_search_bytes: DecimalU64Dto::from_u64(1 << 17),
            cumulative_admission_rejections: zero(),
            worker_running: true,
            cumulative_uploads: DecimalU64Dto::from_u64(2),
            cumulative_upload_failures: zero(),
            cumulative_launches: DecimalU64Dto::from_u64(1000),
            cumulative_launch_failures: zero(),
            cumulative_fallbacks: zero(),
            cumulative_cancellations: DecimalU64Dto::from_u64(3),
            cumulative_context_reinitializations: zero(),
        },
    }
}

/// Release evidence command:
/// `cargo test -p pathhydra-api --release --test encoding encoding_candidate_measurements -- --ignored --nocapture`
#[test]
#[ignore = "release encoding candidate evidence"]
fn encoding_candidate_measurements() {
    const SAMPLES: usize = 7;
    const ITERATIONS: usize = 50;
    let limits = ApiLimits::default();
    println!(
        "workload,encoding,bytes,encode_min_ns,encode_median_ns,encode_max_ns,decode_min_ns,decode_median_ns,decode_max_ns,samples,iterations_per_sample,correct"
    );
    for (name, value) in benchmark_corpus() {
        let json = encode(&value, &limits).unwrap();
        let postcard = postcard::to_allocvec(&value).unwrap();

        let json_encode = distribution::<SAMPLES, ITERATIONS>(|| {
            black_box(encode(black_box(&value), &limits).unwrap());
        });
        let json_decode = distribution::<SAMPLES, ITERATIONS>(|| {
            black_box(decode::<BenchmarkDocument>(black_box(&json), &limits).unwrap());
        });
        let postcard_encode = distribution::<SAMPLES, ITERATIONS>(|| {
            black_box(postcard::to_allocvec(black_box(&value)).unwrap());
        });
        let json_correct = decode::<BenchmarkDocument>(&json, &limits).unwrap() == value;
        println!(
            "{name},json,{},{},{},{},{},{},{},{SAMPLES},{ITERATIONS},{json_correct}",
            json.len(),
            json_encode.0,
            json_encode.1,
            json_encode.2,
            json_decode.0,
            json_decode.1,
            json_decode.2,
        );
        match postcard::from_bytes::<BenchmarkDocument>(&postcard) {
            Ok(decoded) => {
                let postcard_decode = distribution::<SAMPLES, ITERATIONS>(|| {
                    black_box(
                        postcard::from_bytes::<BenchmarkDocument>(black_box(&postcard)).unwrap(),
                    );
                });
                let postcard_correct = decoded == value;
                println!(
                    "{name},postcard,{},{},{},{},{},{},{},{SAMPLES},{ITERATIONS},{postcard_correct}",
                    postcard.len(),
                    postcard_encode.0,
                    postcard_encode.1,
                    postcard_encode.2,
                    postcard_decode.0,
                    postcard_decode.1,
                    postcard_decode.2,
                );
            }
            Err(postcard::Error::WontImplement) => println!(
                "{name},postcard,{},{},{},{},unsupported,unsupported,unsupported,{SAMPLES},{ITERATIONS},false",
                postcard.len(),
                postcard_encode.0,
                postcard_encode.1,
                postcard_encode.2,
            ),
            Err(error) => panic!("unexpected Postcard decode failure for {name}: {error}"),
        }
    }
}

fn distribution<const SAMPLES: usize, const ITERATIONS: usize>(
    mut operation: impl FnMut(),
) -> (u128, u128, u128) {
    let mut samples = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let started = Instant::now();
        for _ in 0..ITERATIONS {
            operation();
        }
        samples.push(started.elapsed().as_nanos() / ITERATIONS as u128);
    }
    samples.sort_unstable();
    (samples[0], samples[SAMPLES / 2], samples[SAMPLES - 1])
}
