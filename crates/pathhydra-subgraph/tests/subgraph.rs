use pathhydra_core::{BaseWeight, EdgeRecord, NodeRecord, RelationId, RelationRecord};
use pathhydra_core::{EdgeId, NodeId};
use pathhydra_routing::{
    DestinationState, RelationMultiplier, RelationProfile, RelationUse, RoutingImage,
    RoutingRequest, SearchBudget, TiePolicy, route,
};
use pathhydra_store::ConfirmedGraphRecords;
use pathhydra_subgraph::{Subgraph, SubgraphError};

#[test]
fn edges_add_endpoints_and_removals_are_local_and_deterministic() {
    let mut graph = Subgraph::new();
    assert!(
        graph
            .add_edge(
                EdgeId::from_u64(2),
                NodeId::from_u64(2),
                NodeId::from_u64(3)
            )
            .unwrap()
    );
    assert!(
        graph
            .add_edge(
                EdgeId::from_u64(1),
                NodeId::from_u64(1),
                NodeId::from_u64(2)
            )
            .unwrap()
    );
    assert!(
        !graph
            .add_edge(
                EdgeId::from_u64(1),
                NodeId::from_u64(1),
                NodeId::from_u64(2)
            )
            .unwrap()
    );
    assert_eq!(
        graph.nodes().map(NodeId::as_u64).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert_eq!(
        graph
            .edges()
            .map(|edge| edge.edge_id().as_u64())
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert!(graph.remove_edge(EdgeId::from_u64(1)));
    assert!(graph.contains_node(NodeId::from_u64(1)));
    assert!(graph.remove_node(NodeId::from_u64(2)));
    assert_eq!(graph.edge_count(), 0);
}

#[test]
fn conflicts_and_union_fail_atomically() {
    let mut left = Subgraph::new();
    left.add_edge(
        EdgeId::from_u64(1),
        NodeId::from_u64(1),
        NodeId::from_u64(2),
    )
    .unwrap();
    let before = left.clone();
    let error = left
        .add_edge(
            EdgeId::from_u64(1),
            NodeId::from_u64(2),
            NodeId::from_u64(1),
        )
        .unwrap_err();
    assert!(matches!(error, SubgraphError::EdgeIdentityConflict { .. }));
    assert_eq!(left, before);

    let mut right = Subgraph::new();
    right.add_node(NodeId::from_u64(99));
    right
        .add_edge(
            EdgeId::from_u64(1),
            NodeId::from_u64(3),
            NodeId::from_u64(4),
        )
        .unwrap();
    assert!(left.union(&right).is_err());
    assert_eq!(left, before);
}

#[test]
fn parallel_and_self_edges_remain_distinct() {
    let mut graph = Subgraph::new();
    graph
        .add_edge(
            EdgeId::from_u64(1),
            NodeId::from_u64(7),
            NodeId::from_u64(7),
        )
        .unwrap();
    graph
        .add_edge(
            EdgeId::from_u64(2),
            NodeId::from_u64(7),
            NodeId::from_u64(7),
        )
        .unwrap();
    assert_eq!(graph.node_count(), 1);
    assert_eq!(graph.edge_count(), 2);
    graph.remove_node(NodeId::from_u64(7));
    assert_eq!(graph.edge_count(), 0);
}

#[test]
fn a_valid_returned_path_adds_all_evidence_idempotently() {
    let nodes = (1..=3)
        .map(|id| {
            NodeRecord::new(
                NodeId::from_u64(id),
                format!("n{id}").into(),
                Vec::<u8>::new().into(),
            )
        })
        .collect();
    let relation = RelationRecord::new(RelationId::from_u64(1), "kind".into());
    let edges = vec![
        EdgeRecord::new(
            EdgeId::from_u64(1),
            NodeId::from_u64(1),
            NodeId::from_u64(2),
            relation.id(),
            BaseWeight::new(0.2).unwrap(),
        ),
        EdgeRecord::new(
            EdgeId::from_u64(2),
            NodeId::from_u64(2),
            NodeId::from_u64(3),
            relation.id(),
            BaseWeight::new(0.3).unwrap(),
        ),
    ];
    let image = RoutingImage::compile(&ConfirmedGraphRecords::new(
        nodes,
        vec![relation.clone()],
        edges,
    ))
    .unwrap();
    let response = route(
        &image,
        &RoutingRequest::new(
            NodeId::from_u64(1),
            [NodeId::from_u64(3)],
            RelationProfile::new([(
                relation.id(),
                RelationUse::Enabled(RelationMultiplier::new(1.0).unwrap()),
            )]),
            true,
            SearchBudget::Unlimited,
            TiePolicy::StablePredecessor,
        ),
    )
    .unwrap();
    let DestinationState::Exact(exact) = response.results()[0].state() else {
        panic!()
    };
    let path = exact.path().unwrap();
    let mut graph = Subgraph::new();
    graph.add_path(path).unwrap();
    let once = graph.clone();
    graph.add_path(path).unwrap();
    assert_eq!(graph, once);
    assert_eq!(graph.node_count(), 3);
    assert_eq!(graph.edge_count(), 2);
}
