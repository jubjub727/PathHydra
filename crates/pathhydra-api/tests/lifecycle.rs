use std::{
    hint::black_box,
    sync::{Arc, Barrier},
    thread,
    time::Instant,
};

use pathhydra_api::{
    ApiErrorCategory, ApiLimits, Binary32Dto, Binary64Dto, CancellationOutcomeDto, CandidateIdDto,
    CompletionReasonDto, ConfirmedRecordDto, DecimalU64Dto, DestinationResultDto,
    DestinationStateDto, EdgeHandleDto, EdgeIdDto, EngineLifecycleStateDto, HydratedEdgeResultDto,
    HydratedEdgeStateDto, HydratedNodeResultDto, HydratedNodeStateDto, HydrationRequestDto,
    HydrationResponseDto, MutationDurableResultDto, NodeIdDto, NodeRecordDto, NumericPolicyDto,
    PathHydra, PathStepDto, PayloadDto, PublicationOutcomeDto, RelationIdDto, RelationProfileDto,
    RelationProfileEntryDto, RelationUseDto, RequestIdAllocation, RoutePathDto, RoutingRequestDto,
    RoutingResponseDto, SearchBudgetDto, SubgraphHandlesDto, TiePolicyDto, VerificationLimitsDto,
    decode, encode,
};

fn route_workload() -> RoutingResponseDto {
    let origin = NodeIdDto::from_u64(1);
    let steps = (0_u64..16)
        .map(|index| PathStepDto {
            edge: EdgeIdDto::from_u64(index + 1),
            source: NodeIdDto::from_u64(index + 1),
            destination: NodeIdDto::from_u64(index + 2),
            relation_kind: RelationIdDto::from_u64(1),
            base_weight: Binary32Dto::from_bits(1.0_f32.to_bits()),
            multiplier: Binary32Dto::from_bits(1.0_f32.to_bits()),
            effective_weight: Binary64Dto::from_bits(1.0_f64.to_bits()),
        })
        .collect::<Vec<_>>();
    let distance = Binary64Dto::from_bits(16.0_f64.to_bits());
    let path = RoutePathDto {
        origin: origin.clone(),
        destination: NodeIdDto::from_u64(17),
        logical_distance: distance.clone(),
        steps,
    };
    let results = (0_u64..64)
        .map(|index| DestinationResultDto {
            destination: NodeIdDto::from_u64(index + 2),
            state: if index == 15 {
                DestinationStateDto::Exact {
                    logical_distance: distance.clone(),
                    path: Some(path.clone()),
                }
            } else {
                DestinationStateDto::Unreachable
            },
        })
        .collect();
    RoutingResponseDto {
        origin,
        results,
        profile: RelationProfileDto::default(),
        numeric_policy: NumericPolicyDto::Binary32OperandsSeparateBinary64,
        tie_policy: TiePolicyDto::StablePredecessor,
        paths_requested: true,
        examined_edges: DecimalU64Dto::from_u64(1_024),
        finalized_nodes: DecimalU64Dto::from_u64(64),
        completion_reason: CompletionReasonDto::AllDestinationsFinalized,
    }
}

fn hydration_workload() -> HydrationResponseDto {
    HydrationResponseDto {
        nodes: (0_u64..128)
            .map(|index| HydratedNodeResultDto {
                requested_node_id: NodeIdDto::from_u64(index + 1),
                state: HydratedNodeStateDto::Found {
                    node: NodeRecordDto {
                        id: NodeIdDto::from_u64(index + 1),
                        name: format!("node-{index:03}"),
                        payload: PayloadDto::from_bytes(&[u8::try_from(index).unwrap_or(0); 16]),
                    },
                },
            })
            .collect(),
        edges: (0_u64..128)
            .map(|index| HydratedEdgeResultDto {
                requested_edge_id: EdgeIdDto::from_u64(index + 1),
                state: HydratedEdgeStateDto::Missing,
            })
            .collect(),
        profile: None,
    }
}

fn subgraph_workload() -> SubgraphHandlesDto {
    SubgraphHandlesDto {
        nodes: (1_u64..=4_096).map(NodeIdDto::from_u64).collect(),
        edges: (1_u64..=8_192)
            .map(|edge| EdgeHandleDto {
                edge: EdgeIdDto::from_u64(edge),
                source: NodeIdDto::from_u64(((edge - 1) % 4_096) + 1),
                destination: NodeIdDto::from_u64((edge % 4_096) + 1),
            })
            .collect(),
    }
}

fn measure_json_lower_bound<T>(name: &str, value: &T, iterations: u32)
where
    T: Clone + serde::Serialize + serde::de::DeserializeOwned,
{
    let direct_started = Instant::now();
    for _ in 0..iterations {
        black_box(value.clone());
    }
    let direct = direct_started.elapsed();

    let bytes = serde_json::to_vec(value).expect("representative DTO encodes");
    let json_started = Instant::now();
    for _ in 0..iterations {
        let encoded = serde_json::to_vec(black_box(value)).expect("encode representative DTO");
        black_box(serde_json::from_slice::<T>(&encoded).expect("decode representative DTO"));
    }
    let json = json_started.elapsed();
    println!(
        "boundary-evidence name={name} iterations={iterations} bytes={} direct_clone_ns_per_iter={} json_round_trip_ns_per_iter={}",
        bytes.len(),
        direct.as_nanos() / u128::from(iterations),
        json.as_nanos() / u128::from(iterations),
    );
}

/// Named representative evidence for Decision 0011. This deliberately measures
/// only DTO JSON cost; a hosted transport would add framing, IPC, scheduling,
/// authorization, and server-side dispatch.
#[test]
fn boundary_workload_evidence() {
    measure_json_lower_bound("route-64-path-16", &route_workload(), 200);
    measure_json_lower_bound("hydrate-128-128", &hydration_workload(), 100);
    measure_json_lower_bound("subgraph-4096-8192", &subgraph_workload(), 10);
}

// Keep the full lifecycle imports honest while the facade and conversion
// owner finish their shared boundary in the same Plan 09 wave.
#[test]
fn maximum_candidate_id_wrapper_is_lossless() {
    assert_eq!(
        CandidateIdDto::from_u64(u64::MAX)
            .as_u64()
            .expect("canonical maximum ID"),
        u64::MAX
    );
}

#[test]
fn capabilities_report_the_facade_configured_api_limits() {
    let directory = tempfile::tempdir().expect("temporary root");
    let limits = ApiLimits {
        maximum_destinations: 7,
        maximum_payload_bytes: 123,
        ..ApiLimits::default()
    };
    let api =
        PathHydra::open_with_limits(directory.path().join("catalog"), limits).expect("open facade");
    let capabilities = api.capabilities();
    assert_eq!(
        capabilities
            .api_limits
            .maximum_destinations
            .as_u64()
            .expect("canonical limit"),
        7
    );
    assert_eq!(
        capabilities
            .api_limits
            .maximum_payload_bytes
            .as_u64()
            .expect("canonical limit"),
        123
    );
}

fn confirmed_node(value: pathhydra_api::MutationOutcomeDto) -> NodeRecordDto {
    let MutationDurableResultDto::Confirmed(ConfirmedRecordDto::Node(node)) = value.durable_result
    else {
        panic!("expected confirmed node")
    };
    node
}

fn confirmed_relation(
    value: pathhydra_api::MutationOutcomeDto,
) -> pathhydra_api::RelationKindRecordDto {
    let MutationDurableResultDto::Confirmed(ConfirmedRecordDto::RelationKind(relation)) =
        value.durable_result
    else {
        panic!("expected confirmed relation kind")
    };
    relation
}

fn confirmed_edge(value: pathhydra_api::MutationOutcomeDto) -> pathhydra_api::EdgeRecordDto {
    let MutationDurableResultDto::Confirmed(ConfirmedRecordDto::Edge(edge)) = value.durable_result
    else {
        panic!("expected confirmed edge")
    };
    edge
}

#[test]
fn full_encoded_facade_lifecycle_and_current_state_hydration() {
    let directory = tempfile::tempdir().expect("temporary root");
    let api = PathHydra::open(directory.path().join("catalog")).expect("open facade");

    let source_candidate = api
        .insert_node_candidate(
            "Source".to_owned(),
            PayloadDto::from_bytes(b"source-payload"),
        )
        .expect("insert source candidate");
    let source = confirmed_node(
        api.confirm_candidate(&source_candidate)
            .expect("confirm source"),
    );
    let duplicate_source_candidate = api
        .insert_node_candidate(
            "Source".to_owned(),
            PayloadDto::from_bytes(b"discarded duplicate payload"),
        )
        .expect("insert duplicate exact-name candidate");
    let duplicate_outcome = api
        .confirm_candidate(&duplicate_source_candidate)
        .expect("consume duplicate exact-name candidate");
    let MutationDurableResultDto::Confirmed(ConfirmedRecordDto::Node(duplicate_source)) =
        duplicate_outcome.durable_result
    else {
        panic!("expected existing confirmed node");
    };
    assert_eq!(duplicate_source.id, source.id);
    assert!(matches!(
        duplicate_outcome.publication,
        PublicationOutcomeDto::Published { .. } | PublicationOutcomeDto::RoutingUnavailable { .. }
    ));
    assert_eq!(
        api.get_candidate(&duplicate_source_candidate)
            .expect_err("duplicate candidate is consumed")
            .category(),
        ApiErrorCategory::MissingCandidate
    );
    let destination_candidate = api
        .insert_node_candidate(
            "Destination".to_owned(),
            PayloadDto::from_bytes(b"destination-payload"),
        )
        .expect("insert destination candidate");
    let destination = confirmed_node(
        api.confirm_candidate(&destination_candidate)
            .expect("confirm destination"),
    );
    let relation_candidate = api
        .insert_relation_kind_candidate("connects exactly".to_owned())
        .expect("insert relation-kind candidate");
    let relation = confirmed_relation(
        api.confirm_candidate(&relation_candidate)
            .expect("confirm relation kind"),
    );
    let edge_candidate = api
        .insert_edge_candidate(
            &source.id,
            &destination.id,
            &relation.id,
            &Binary32Dto::from_bits(0.25_f32.to_bits()),
        )
        .expect("insert edge candidate");
    let edge = confirmed_edge(
        api.confirm_candidate(&edge_candidate)
            .expect("confirm edge"),
    );

    assert_eq!(
        api.lookup_node_exact("Source").expect("exact lookup"),
        Some(source.id.clone())
    );
    assert_eq!(
        api.lookup_node_exact("source")
            .expect("case-sensitive miss"),
        None
    );
    assert_eq!(
        api.lookup_relation_kind_exact("connects exactly")
            .expect("relation exact lookup"),
        Some(relation.id.clone())
    );

    let profile = RelationProfileDto {
        entries: vec![RelationProfileEntryDto {
            relation_kind: relation.id.clone(),
            relation_use: RelationUseDto::Enabled {
                multiplier: Binary32Dto::from_bits(2.0_f32.to_bits()),
            },
        }],
    };
    let request = RoutingRequestDto {
        origin: source.id.clone(),
        destinations: vec![destination.id.clone(), NodeIdDto::from_u64(u64::MAX)],
        profile: profile.clone(),
        return_paths: true,
        budget: SearchBudgetDto::Unlimited,
        tie_policy: TiePolicyDto::StablePredecessor,
    };
    let handle = api
        .request_handle(RequestIdAllocation::Automatic)
        .expect("allocate route");
    let routed = handle.route(&request).expect("route");
    assert!(matches!(
        routed.response.results[0].state,
        DestinationStateDto::Exact { .. }
    ));
    assert!(matches!(
        routed.response.results[1].state,
        DestinationStateDto::MissingNode
    ));
    assert_eq!(
        handle.cancel().expect("completed cancellation race"),
        CancellationOutcomeDto::AlreadyCompleted
    );
    assert_eq!(
        handle
            .route(&request)
            .expect_err("a request handle executes only once")
            .code(),
        "request_already_executed"
    );

    let hydrated_path = handle
        .hydrate_path(&DecimalU64Dto::from_u64(0))
        .expect("hydrate current path");
    assert_eq!(hydrated_path.nodes.len(), 2);
    assert_eq!(hydrated_path.edges.len(), 1);

    let mut subgraph = api.new_subgraph();
    handle
        .add_path_to_subgraph(&DecimalU64Dto::from_u64(0), &mut subgraph)
        .expect("add selected path");
    let encoded = encode(&subgraph, &api.limits()).expect("encode subgraph");
    let decoded: SubgraphHandlesDto = decode(&encoded, &api.limits()).expect("decode subgraph");
    assert_eq!(decoded, subgraph);
    assert!(
        api.hydrate_subgraph(&decoded, Some(&profile))
            .expect("hydrate subgraph")
            .complete
    );

    let verification = VerificationLimitsDto {
        maximum_records: Some(DecimalU64Dto::from_u64(1_000)),
        maximum_duration: None,
    };
    assert!(
        api.verify_catalog(&verification)
            .expect("verify live catalog")
            .records_examined
            .as_u64()
            .expect("verification count")
            > 0
    );
    api.create_checkpoint(
        "checkpoint-a",
        &DecimalU64Dto::from_u64(u64::MAX),
        &DecimalU64Dto::from_u64(0),
    )
    .expect("create checkpoint");
    api.validate_checkpoint("checkpoint-a", &verification)
        .expect("validate checkpoint");
    let restore = api
        .restore_checkpoint(
            "checkpoint-a",
            "restore-a",
            &DecimalU64Dto::from_u64(u64::MAX),
            &DecimalU64Dto::from_u64(0),
            &verification,
        )
        .expect("restore and smoke-check checkpoint");
    assert!(restore.smoke_catalog_verified);
    assert!(restore.smoke_route_verified);
    assert!(restore.smoke_hydration_verified);
    let restore_bytes = encode(&restore, &api.limits()).expect("encode path-free restore report");
    let local_root = directory.path().to_string_lossy();
    assert!(!String::from_utf8_lossy(&restore_bytes).contains(local_root.as_ref()));
    let invalid_path = api
        .create_checkpoint(
            "../private-checkpoint",
            &DecimalU64Dto::from_u64(u64::MAX),
            &DecimalU64Dto::from_u64(0),
        )
        .expect_err("encoded names cannot escape configured roots");
    assert_eq!(invalid_path.code(), "invalid_resource_name");
    assert!(!invalid_path.to_string().contains("private-checkpoint"));

    api.remove_confirmed_edge(&edge.id)
        .expect("remove confirmed edge");
    let unavailable = handle
        .hydrate_path(&DecimalU64Dto::from_u64(0))
        .expect_err("hydration intentionally resolves current records");
    assert_eq!(
        unavailable.category(),
        ApiErrorCategory::HydrationUnavailable
    );
    let current = api
        .hydrate(&HydrationRequestDto {
            node_ids: vec![source.id.clone(), destination.id.clone()],
            edge_ids: vec![edge.id.clone()],
            profile: Some(profile),
        })
        .expect("arbitrary hydration reports missing state");
    assert!(matches!(
        current.edges[0].state,
        HydratedEdgeStateDto::Missing
    ));

    api.compact_store().expect("compact fixed store scope");
    let health = api.health().expect("health");
    assert!(health.durable_catalog_available);
    let health_bytes = encode(&health, &api.limits()).expect("encode path-free health");
    assert!(!String::from_utf8_lossy(&health_bytes).contains(local_root.as_ref()));
    let never_admitted = api
        .request_handle(RequestIdAllocation::Automatic)
        .expect("allocate before shutdown");
    let shutdown = api.shutdown().expect("explicit shutdown");
    assert_eq!(shutdown.state_after, EngineLifecycleStateDto::ShutDown);
    assert_eq!(
        never_admitted.cancel().expect("shutdown cancellation race"),
        CancellationOutcomeDto::ShuttingDown
    );
    assert_eq!(
        api.request_handle(RequestIdAllocation::Automatic)
            .expect_err("post-shutdown allocation is rejected")
            .category(),
        ApiErrorCategory::Shutdown
    );
}

#[test]
fn shutdown_and_maintenance_race_has_only_typed_outcomes() {
    let directory = tempfile::tempdir().expect("temporary root");
    let api = Arc::new(PathHydra::open(directory.path().join("catalog")).expect("open facade"));
    let candidate = api
        .insert_node_candidate("node".to_owned(), PayloadDto::from_bytes(b"payload"))
        .expect("insert candidate");
    api.confirm_candidate(&candidate).expect("confirm node");

    let barrier = Arc::new(Barrier::new(3));
    let verifier = {
        let api = Arc::clone(&api);
        let barrier = Arc::clone(&barrier);
        thread::spawn(move || {
            barrier.wait();
            (0..16)
                .map(|_| {
                    api.verify_catalog(&VerificationLimitsDto {
                        maximum_records: Some(DecimalU64Dto::from_u64(10_000)),
                        maximum_duration: None,
                    })
                })
                .collect::<Vec<_>>()
        })
    };
    let shutdown = {
        let api = Arc::clone(&api);
        let barrier = Arc::clone(&barrier);
        thread::spawn(move || {
            barrier.wait();
            api.shutdown()
        })
    };
    barrier.wait();

    let outcomes = verifier.join().expect("verification worker");
    let shutdown = shutdown
        .join()
        .expect("shutdown worker")
        .expect("typed shutdown report");
    assert_eq!(shutdown.state_after, EngineLifecycleStateDto::ShutDown);
    assert!(outcomes.iter().all(|outcome| match outcome {
        Ok(_) => true,
        Err(error) => matches!(
            error.category(),
            ApiErrorCategory::Shutdown | ApiErrorCategory::Maintenance
        ),
    }));
    assert_eq!(
        api.verify_catalog(&VerificationLimitsDto {
            maximum_records: None,
            maximum_duration: None,
        })
        .expect_err("maintenance after shutdown is rejected")
        .category(),
        ApiErrorCategory::Shutdown
    );
}
