use pathhydra_core::ConfirmedRecord;
use pathhydra_engine::{
    EdgeEvaluation, EngineConfig, GraphEngine, HydratedEdgeState, HydratedNodeState,
    HydrationRequest, PublicationOutcome, RequestId, RoutingHealth, Subgraph,
};
use pathhydra_routing::{
    DestinationState, RelationMultiplier, RelationProfile, RelationUse, RoutingRequest,
    SearchBudget, TiePolicy,
};

fn confirm_node(engine: &GraphEngine, name: &str) -> pathhydra_core::NodeRecord {
    let candidate = engine.insert_node_candidate(name).unwrap();
    let ConfirmedRecord::Node(record) = engine
        .confirm_validated_candidate(candidate)
        .unwrap()
        .into_parts()
        .0
    else {
        panic!()
    };
    record
}

#[test]
fn publication_routing_hydration_and_subgraphs_share_one_engine_boundary() {
    let directory = tempfile::tempdir().unwrap();
    let engine = GraphEngine::open(directory.path(), EngineConfig::default()).unwrap();
    assert!(matches!(
        engine.health().unwrap().routing,
        RoutingHealth::Available
    ));
    let source = confirm_node(&engine, "Source");
    let destination = confirm_node(&engine, "Destination");
    let relation_candidate = engine.insert_relation_candidate("EXACT Kind").unwrap();
    let ConfirmedRecord::Relation(relation) = engine
        .confirm_validated_candidate(relation_candidate)
        .unwrap()
        .into_parts()
        .0
    else {
        panic!()
    };
    let edge_candidate = engine
        .insert_edge_candidate(source.id(), destination.id(), relation.id(), 0.25)
        .unwrap();
    let edge_mutation = engine.confirm_validated_candidate(edge_candidate).unwrap();
    assert!(matches!(
        edge_mutation.publication(),
        PublicationOutcome::Published(_)
    ));
    let ConfirmedRecord::Edge(edge) = edge_mutation.into_parts().0 else {
        panic!()
    };
    let profile = RelationProfile::new([(
        relation.id(),
        RelationUse::Enabled(RelationMultiplier::new(2.0).unwrap()),
    )]);
    let routed = engine
        .route(
            RequestId::new(7),
            &RoutingRequest::new(
                source.id(),
                [destination.id()],
                profile.clone(),
                true,
                SearchBudget::Unlimited,
                TiePolicy::StablePredecessor,
            ),
        )
        .unwrap();
    assert!(matches!(
        routed.response.results()[0].state(),
        DestinationState::Exact(_)
    ));
    let hydrated_path = engine.hydrate_path(&routed.response, 0).unwrap();
    assert_eq!(hydrated_path.nodes.len(), 2);
    assert_eq!(
        hydrated_path.edges[0].relation_kind.name().as_str(),
        "EXACT Kind"
    );

    let handles = HydrationRequest::new(
        [source.id(), source.id()],
        [edge.id(), edge.id()],
        Some(profile),
    );
    let hydrated = engine.hydrate(&handles).unwrap();
    assert!(
        hydrated
            .nodes
            .iter()
            .all(|result| matches!(result.state, HydratedNodeState::Found(_)))
    );
    assert!(
        hydrated
            .edges
            .iter()
            .all(|result| matches!(result.state, HydratedEdgeState::Found(_)))
    );
    let HydratedEdgeState::Found(found) = &hydrated.edges[0].state else {
        panic!()
    };
    assert!(matches!(
        found.evaluation,
        EdgeEvaluation::Enabled {
            effective_weight: 0.5,
            ..
        }
    ));

    let mut subgraph = Subgraph::new();
    subgraph.add_edge_record(&edge).unwrap();
    let hydrated = engine.hydrate_subgraph(&subgraph, None).unwrap();
    assert!(hydrated.complete);
    assert_eq!(hydrated.nodes.len(), 2);
    assert_eq!(hydrated.edges.len(), 1);
}

#[test]
fn postcommit_topology_failure_is_unambiguous_and_repair_recovers() {
    let directory = tempfile::tempdir().unwrap();
    let config = EngineConfig {
        max_active_image_bytes: 8,
        ..EngineConfig::default()
    };
    let engine = GraphEngine::open(directory.path(), config).unwrap();
    let candidate = engine.insert_node_candidate("too large for image").unwrap();
    let mutation = engine.confirm_validated_candidate(candidate).unwrap();
    let ConfirmedRecord::Node(node) = mutation.durable_result() else {
        panic!()
    };
    assert!(matches!(
        mutation.publication(),
        PublicationOutcome::RoutingUnavailable(_, _)
    ));
    assert_eq!(
        engine.get_node(node.id()).unwrap().name().as_str(),
        "too large for image"
    );
    assert!(matches!(
        engine.health().unwrap().routing,
        RoutingHealth::Unavailable(_)
    ));
    let repair = engine.remove_node(node.id()).unwrap();
    assert!(matches!(
        repair.publication(),
        PublicationOutcome::Published(_)
    ));
    assert!(matches!(
        engine.health().unwrap().routing,
        RoutingHealth::Available
    ));
}

#[test]
fn invalid_limits_and_destination_admission_are_typed() {
    let directory = tempfile::tempdir().unwrap();
    let invalid = EngineConfig {
        max_concurrent_routes: 0,
        ..EngineConfig::default()
    };
    assert!(GraphEngine::open(directory.path(), invalid).is_err());

    let directory = tempfile::tempdir().unwrap();
    let engine = GraphEngine::open(
        directory.path(),
        EngineConfig {
            max_destinations_per_request: 1,
            ..EngineConfig::default()
        },
    )
    .unwrap();
    let origin = confirm_node(&engine, "origin");
    let request = RoutingRequest::new(
        origin.id(),
        [origin.id(), origin.id()],
        RelationProfile::default(),
        false,
        SearchBudget::Unlimited,
        TiePolicy::StablePredecessor,
    );
    assert!(engine.route(RequestId::new(1), &request).is_err());
    assert_eq!(engine.health().unwrap().active_routes, 0);
}
