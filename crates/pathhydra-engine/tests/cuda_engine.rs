#![cfg(feature = "cuda")]

use pathhydra_core::ConfirmedRecord;
use pathhydra_cuda::CudaFaultStage;
use pathhydra_engine::{
    CudaAlgorithmSelection, CudaAvailability, CudaConfig, CudaExecutorPolicy, CudaIneligibility,
    EngineConfig, EngineError, Executor, GraphEngine, RequestId,
};
use pathhydra_routing::{
    DestinationState, RelationMultiplier, RelationProfile, RelationUse, RoutingRequest,
    SearchBudget, TiePolicy,
};

fn cuda_config(policy: CudaExecutorPolicy) -> EngineConfig {
    EngineConfig {
        cuda: CudaConfig {
            enabled: true,
            executor_policy: policy,
            minimum_free_memory_headroom: 0,
            batch_collection_delay: std::time::Duration::ZERO,
            ..CudaConfig::default()
        },
        ..EngineConfig::default()
    }
}

#[test]
fn forced_partitioned_frontier_and_delta_route_on_bounded_device_cache() {
    for (index, algorithm) in [
        CudaAlgorithmSelection::Frontier,
        CudaAlgorithmSelection::DeltaStepping { delta: 0.1 },
    ]
    .into_iter()
    .enumerate()
    {
        let directory = tempfile::tempdir().unwrap();
        let config = EngineConfig {
            max_active_image_bytes: 8,
            target_partition_topology_bytes: 48,
            hard_maximum_partition_topology_bytes: 48,
            host_partition_cache_bytes: 112,
            host_partition_cache_entries: 1,
            cuda: CudaConfig {
                enabled: true,
                executor_policy: CudaExecutorPolicy::RequireCuda,
                minimum_free_memory_headroom: 0,
                maximum_reserved_search_bytes: 64 * 1024 * 1024,
                maximum_partitioned_topology_cache_bytes: 64,
                maximum_partitioned_topology_cache_slots: 1,
                maximum_partitioned_host_staging_bytes: 64,
                batch_collection_delay: std::time::Duration::ZERO,
                algorithm,
                ..CudaConfig::default()
            },
            ..EngineConfig::default()
        };
        let engine = GraphEngine::open(directory.path(), config).unwrap();
        let (source, destination, relation) = populate(&engine);
        let output = engine
            .route(
                RequestId::new(500 + index as u64),
                &request(
                    source,
                    destination,
                    relation,
                    false,
                    SearchBudget::Unlimited,
                ),
            )
            .unwrap();
        assert_eq!(output.diagnostics.executor, Executor::NvidiaCuda);
        assert!(matches!(
            output.response.results()[0].state(),
            DestinationState::Exact(exact) if exact.logical_distance().to_bits() == 0.25_f64.to_bits()
        ));
        let cuda = output.diagnostics.cuda.unwrap();
        assert!(cuda.partitions_required > 0);
        assert!(cuda.transfer_bytes > 0);
        let health = engine.health().unwrap();
        assert!(health.cuda.partitioned_topology);
        let cache = health.cuda.device_topology_cache.unwrap();
        assert!(cache.current_bytes <= cache.capacity_bytes);
        assert!(cache.entries <= cache.capacity_slots);
    }
}

#[test]
fn partitioned_cuda_requests_do_not_mix_bundles_during_node_deletion() {
    let directory = tempfile::tempdir().unwrap();
    let engine = std::sync::Arc::new(
        GraphEngine::open(
            directory.path(),
            EngineConfig {
                max_active_image_bytes: 8,
                target_partition_topology_bytes: 48,
                hard_maximum_partition_topology_bytes: 48,
                host_partition_cache_bytes: 112,
                host_partition_cache_entries: 1,
                routing_io_staging_bytes: 56,
                cuda: CudaConfig {
                    enabled: true,
                    executor_policy: CudaExecutorPolicy::RequireCuda,
                    minimum_free_memory_headroom: 0,
                    maximum_reserved_search_bytes: 64 * 1024 * 1024,
                    maximum_partitioned_topology_cache_bytes: 64,
                    maximum_partitioned_topology_cache_slots: 1,
                    maximum_partitioned_host_staging_bytes: 64,
                    batch_collection_delay: std::time::Duration::ZERO,
                    algorithm: CudaAlgorithmSelection::Frontier,
                    ..CudaConfig::default()
                },
                ..EngineConfig::default()
            },
        )
        .unwrap(),
    );
    let (source, destination, relation) = populate(&engine);
    let route = std::sync::Arc::new(request(
        source,
        destination,
        relation,
        false,
        SearchBudget::Unlimited,
    ));
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
    let workers: Vec<_> = (0..2_u64)
        .map(|worker| {
            let engine = std::sync::Arc::clone(&engine);
            let route = std::sync::Arc::clone(&route);
            let barrier = std::sync::Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                (0..12_u64)
                    .map(|iteration| {
                        engine
                            .route(RequestId::new(30_000 + worker * 100 + iteration), &route)
                            .unwrap()
                            .response
                    })
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    barrier.wait();
    std::thread::sleep(std::time::Duration::from_millis(5));
    engine.remove_node(destination).unwrap();
    let outputs: Vec<_> = workers
        .into_iter()
        .flat_map(|worker| worker.join().unwrap())
        .collect();
    assert!(outputs.iter().all(|response| matches!(
        response.results()[0].state(),
        DestinationState::Exact(_) | DestinationState::MissingNode
    )));
    assert!(matches!(
        engine
            .route(RequestId::new(31_000), &route)
            .unwrap()
            .response
            .results()[0]
            .state(),
        DestinationState::MissingNode
    ));
}

fn populate(
    engine: &GraphEngine,
) -> (
    pathhydra_core::NodeId,
    pathhydra_core::NodeId,
    pathhydra_core::RelationId,
) {
    let source = engine.insert_node_candidate("source").unwrap();
    let ConfirmedRecord::Node(source) = engine
        .confirm_validated_candidate(source)
        .unwrap()
        .into_parts()
        .0
    else {
        panic!()
    };
    let destination = engine.insert_node_candidate("destination").unwrap();
    let ConfirmedRecord::Node(destination) = engine
        .confirm_validated_candidate(destination)
        .unwrap()
        .into_parts()
        .0
    else {
        panic!()
    };
    let relation = engine.insert_relation_candidate("road").unwrap();
    let ConfirmedRecord::Relation(relation) = engine
        .confirm_validated_candidate(relation)
        .unwrap()
        .into_parts()
        .0
    else {
        panic!()
    };
    let edge = engine
        .insert_edge_candidate(source.id(), destination.id(), relation.id(), 0.25)
        .unwrap();
    engine.confirm_validated_candidate(edge).unwrap();
    (source.id(), destination.id(), relation.id())
}

fn request(
    source: pathhydra_core::NodeId,
    destination: pathhydra_core::NodeId,
    relation: pathhydra_core::RelationId,
    paths: bool,
    budget: SearchBudget,
) -> RoutingRequest {
    RoutingRequest::new(
        source,
        [destination],
        RelationProfile::new([(
            relation,
            RelationUse::Enabled(RelationMultiplier::new(1.0).unwrap()),
        )]),
        paths,
        budget,
        TiePolicy::StablePredecessor,
    )
}

#[test]
fn prefer_cuda_publishes_matching_residency_and_paths_fall_back_to_cpu() {
    let directory = tempfile::tempdir().unwrap();
    let engine = GraphEngine::open(
        directory.path(),
        cuda_config(CudaExecutorPolicy::PreferCuda),
    )
    .unwrap();
    let (source, destination, relation) = populate(&engine);
    let distance = engine
        .route(
            RequestId::new(1),
            &request(
                source,
                destination,
                relation,
                false,
                SearchBudget::Unlimited,
            ),
        )
        .unwrap();
    assert_eq!(distance.diagnostics.executor, Executor::NvidiaCuda);
    assert!(matches!(
        distance.response.results()[0].state(),
        DestinationState::Exact(exact) if exact.logical_distance().to_bits() == 0.25_f64.to_bits()
    ));
    let paths = engine
        .route(
            RequestId::new(2),
            &request(source, destination, relation, true, SearchBudget::Unlimited),
        )
        .unwrap();
    assert_eq!(paths.diagnostics.executor, Executor::CpuReference);
    assert_eq!(
        paths.diagnostics.cuda_fallback_reason,
        Some(CudaIneligibility::PathsUnsupportedByCuda)
    );
    assert!(matches!(
        engine.health().unwrap().cuda.availability,
        CudaAvailability::Available
    ));
    assert!(engine.health().unwrap().cuda.cumulative_uploads >= 5);
    engine.rebuild_cuda_residency().unwrap();
    engine.reinitialize_cuda().unwrap();
    assert!(
        engine
            .health()
            .unwrap()
            .cuda
            .cumulative_context_reinitializations
            >= 1
    );
}

#[test]
fn partitioned_context_loss_falls_back_to_cpu_until_explicit_reinitialization() {
    let directory = tempfile::tempdir().unwrap();
    let engine = GraphEngine::open(
        directory.path(),
        EngineConfig {
            max_active_image_bytes: 8,
            target_partition_topology_bytes: 48,
            hard_maximum_partition_topology_bytes: 48,
            host_partition_cache_bytes: 112,
            host_partition_cache_entries: 1,
            routing_io_staging_bytes: 56,
            cuda: CudaConfig {
                enabled: true,
                executor_policy: CudaExecutorPolicy::PreferCuda,
                minimum_free_memory_headroom: 0,
                maximum_reserved_search_bytes: 64 * 1024 * 1024,
                maximum_partitioned_topology_cache_bytes: 64,
                maximum_partitioned_topology_cache_slots: 1,
                maximum_partitioned_host_staging_bytes: 64,
                batch_collection_delay: std::time::Duration::ZERO,
                ..CudaConfig::default()
            },
            ..EngineConfig::default()
        },
    )
    .unwrap();
    let (source, destination, relation) = populate(&engine);
    let route = request(
        source,
        destination,
        relation,
        false,
        SearchBudget::Unlimited,
    );
    engine
        .cuda_fault_injection()
        .unwrap()
        .fail_at(CudaFaultStage::ContextLoss);
    let fallback = engine.route(RequestId::new(40_000), &route).unwrap();
    assert_eq!(fallback.diagnostics.executor, Executor::CpuReference);
    assert!(matches!(
        fallback.response.results()[0].state(),
        DestinationState::Exact(exact) if exact.logical_distance().to_bits() == 0.25_f64.to_bits()
    ));
    let degraded = engine.health().unwrap();
    assert!(matches!(
        degraded.cuda.availability,
        CudaAvailability::Degraded(_) | CudaAvailability::Unavailable(_)
    ));
    assert!(degraded.last_cuda_degradation.is_some());
    let still_cpu = engine.route(RequestId::new(40_001), &route).unwrap();
    assert_eq!(still_cpu.diagnostics.executor, Executor::CpuReference);

    engine.reinitialize_cuda().unwrap();
    assert!(engine.health().unwrap().last_cuda_recovery.is_some());
    let recovered = engine.route(RequestId::new(40_002), &route).unwrap();
    assert_eq!(recovered.diagnostics.executor, Executor::NvidiaCuda);
    assert!(matches!(
        recovered.response.results()[0].state(),
        DestinationState::Exact(exact) if exact.logical_distance().to_bits() == 0.25_f64.to_bits()
    ));
}

#[test]
fn require_cuda_refuses_unsupported_request_shapes_before_allocation() {
    let directory = tempfile::tempdir().unwrap();
    let engine = GraphEngine::open(
        directory.path(),
        cuda_config(CudaExecutorPolicy::RequireCuda),
    )
    .unwrap();
    let (source, destination, relation) = populate(&engine);
    assert!(matches!(
        engine.route(
            RequestId::new(10),
            &request(source, destination, relation, true, SearchBudget::Unlimited),
        ),
        Err(EngineError::CudaIneligible(
            CudaIneligibility::PathsUnsupportedByCuda
        ))
    ));
    assert!(matches!(
        engine.route(
            RequestId::new(11),
            &request(
                source,
                destination,
                relation,
                false,
                SearchBudget::ExaminedEdges(1)
            ),
        ),
        Err(EngineError::CudaIneligible(
            CudaIneligibility::FiniteEdgeBudgetUnsupportedByCuda
        ))
    ));
}

#[test]
fn concurrent_origins_collect_as_independent_cuda_lanes() {
    let directory = tempfile::tempdir().unwrap();
    let config = EngineConfig {
        cuda: CudaConfig {
            enabled: true,
            executor_policy: CudaExecutorPolicy::PreferCuda,
            minimum_free_memory_headroom: 0,
            batch_collection_delay: std::time::Duration::from_millis(20),
            ..CudaConfig::default()
        },
        ..EngineConfig::default()
    };
    let engine = std::sync::Arc::new(GraphEngine::open(directory.path(), config).unwrap());
    let (source, destination, relation) = populate(&engine);
    let route = request(
        source,
        destination,
        relation,
        false,
        SearchBudget::Unlimited,
    );
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
    let workers: Vec<_> = (0..2)
        .map(|lane| {
            let engine = engine.clone();
            let route = route.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                engine.route(RequestId::new(100 + lane), &route).unwrap()
            })
        })
        .collect();
    barrier.wait();
    let outputs: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    for (lane, output) in outputs.iter().enumerate() {
        assert_eq!(output.diagnostics.executor, Executor::NvidiaCuda);
        let cuda = output.diagnostics.cuda.as_ref().unwrap();
        assert_eq!(cuda.batch_width, 2);
        assert!(cuda.lane_index < 2);
        assert!(matches!(
            output.response.results()[0].state(),
            DestinationState::Exact(exact) if exact.logical_distance().to_bits() == 0.25_f64.to_bits()
        ));
        assert_eq!(output.response.origin(), route.origin(), "lane {lane}");
    }
}
