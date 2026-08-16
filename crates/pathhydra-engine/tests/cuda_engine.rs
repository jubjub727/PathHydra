#![cfg(feature = "cuda")]

use pathhydra_core::ConfirmedRecord;
use pathhydra_engine::{
    CudaAvailability, CudaConfig, CudaExecutorPolicy, CudaIneligibility, EngineConfig, EngineError,
    Executor, GraphEngine, RequestId,
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
