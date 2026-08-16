use std::fs;

use pathhydra_core::ConfirmedRecord;
use pathhydra_engine::{EngineConfig, GraphEngine, RequestId};
use pathhydra_routing::{
    DestinationState, RelationMultiplier, RelationProfile, RelationUse, RoutingRequest,
    SearchBudget, TiePolicy,
};

#[test]
fn partitioned_cpu_routes_exactly_and_valid_bundle_reopens_without_rebuild() {
    let temporary = tempfile::tempdir().unwrap();
    let database = temporary.path().join("graph");
    let config = EngineConfig {
        max_active_image_bytes: 8,
        target_partition_topology_bytes: 48,
        hard_maximum_partition_topology_bytes: 48,
        host_partition_cache_bytes: 112,
        host_partition_cache_entries: 1,
        ..EngineConfig::default()
    };
    let engine = GraphEngine::open(&database, config.clone()).unwrap();
    let source_candidate = engine.insert_node_candidate("source").unwrap();
    let ConfirmedRecord::Node(source) = engine
        .confirm_validated_candidate(source_candidate)
        .unwrap()
        .into_parts()
        .0
    else {
        panic!()
    };
    let destination_candidate = engine.insert_node_candidate("destination").unwrap();
    let ConfirmedRecord::Node(destination) = engine
        .confirm_validated_candidate(destination_candidate)
        .unwrap()
        .into_parts()
        .0
    else {
        panic!()
    };
    let relation_candidate = engine.insert_relation_candidate("kind").unwrap();
    let ConfirmedRecord::Relation(relation) = engine
        .confirm_validated_candidate(relation_candidate)
        .unwrap()
        .into_parts()
        .0
    else {
        panic!()
    };
    let edge = engine
        .insert_edge_candidate(source.id(), destination.id(), relation.id(), 0.5)
        .unwrap();
    engine.confirm_validated_candidate(edge).unwrap();
    let request = RoutingRequest::new(
        source.id(),
        [destination.id()],
        RelationProfile::new([(
            relation.id(),
            RelationUse::Enabled(RelationMultiplier::new(1.0).unwrap()),
        )]),
        true,
        SearchBudget::Unlimited,
        TiePolicy::StablePredecessor,
    );
    let routed = engine.route(RequestId::new(1), &request).unwrap();
    assert!(
        matches!(routed.response.results()[0].state(), DestinationState::Exact(exact) if exact.logical_distance().to_bits()==0.5_f64.to_bits() && exact.path().unwrap().steps().len()==1)
    );
    let candidate_only = engine.insert_node_candidate("still provisional").unwrap();
    let root = temporary.path().join("graph.routing-images");
    let before: Vec<_> = fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    drop(engine);
    let reopened = GraphEngine::open(&database, config).unwrap();
    let after: Vec<_> = fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(before, after);
    assert!(reopened.get_candidate(candidate_only).is_ok());
    assert!(matches!(
        reopened
            .route(RequestId::new(2), &request)
            .unwrap()
            .response
            .results()[0]
            .state(),
        DestinationState::Exact(_)
    ));
}
