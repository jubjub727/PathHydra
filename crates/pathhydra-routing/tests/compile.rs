use pathhydra_core::{
    BaseWeight, EdgeId, EdgeRecord, NodeId, NodeName, NodePayload, NodeRecord, RelationId,
    RelationName, RelationRecord,
};
use pathhydra_routing::{CompileError, RoutingImage};
use pathhydra_store::ConfirmedGraphRecords;

#[test]
fn empty_and_isolated_sparse_nodes_compile_to_deterministic_dense_ids() {
    let empty = RoutingImage::compile(&ConfirmedGraphRecords::default()).unwrap();
    assert_eq!(empty.node_count(), 0);
    assert_eq!(empty.manifest().adjacency_count(), 0);
    assert!(empty.manifest().predecessor_edge_ids_present());

    let records = ConfirmedGraphRecords::new(vec![node(90), node(3), node(40)], vec![], vec![]);
    let image = RoutingImage::compile(&records).unwrap();
    for (external, dense) in [(3, 0), (40, 1), (90, 2)] {
        let dense_id = image.dense_node_id(NodeId::from_u64(external)).unwrap();
        assert_eq!(dense_id.as_u32(), dense);
        assert_eq!(
            image.external_node_id(dense_id),
            Some(NodeId::from_u64(external))
        );
        assert_eq!(image.outgoing_edges(dense_id).unwrap().count(), 0);
    }
    assert!(image.manifest().byte_counts().total() > 0);
}

#[test]
fn compiler_rejects_duplicate_and_dangling_stable_ids() {
    let relation = relation(7);
    let cases = [
        (
            ConfirmedGraphRecords::new(vec![node(1), node(1)], vec![], vec![]),
            "node",
        ),
        (
            ConfirmedGraphRecords::new(vec![], vec![relation.clone(), relation.clone()], vec![]),
            "relation",
        ),
        (
            ConfirmedGraphRecords::new(
                vec![node(1)],
                vec![relation.clone()],
                vec![edge(2, 1, 1, 7, 0.5), edge(2, 1, 1, 7, 0.5)],
            ),
            "edge",
        ),
    ];
    assert!(matches!(
        RoutingImage::compile(&cases[0].0),
        Err(CompileError::DuplicateNodeId(_))
    ));
    assert!(matches!(
        RoutingImage::compile(&cases[1].0),
        Err(CompileError::DuplicateRelationId(_))
    ));
    assert!(matches!(
        RoutingImage::compile(&cases[2].0),
        Err(CompileError::DuplicateEdgeId(_))
    ));

    let missing_node = ConfirmedGraphRecords::new(
        vec![node(1)],
        vec![relation.clone()],
        vec![edge(1, 1, 2, 7, 0.5)],
    );
    assert!(matches!(
        RoutingImage::compile(&missing_node),
        Err(CompileError::MissingEndpoint { .. })
    ));
    let missing_relation =
        ConfirmedGraphRecords::new(vec![node(1)], vec![], vec![edge(1, 1, 1, 7, 0.5)]);
    assert!(matches!(
        RoutingImage::compile(&missing_relation),
        Err(CompileError::MissingRelationKind { .. })
    ));
}

#[test]
fn directed_parallel_and_self_edges_are_kept_and_sorted_by_edge_id() {
    let records = ConfirmedGraphRecords::new(
        vec![node(2), node(1)],
        vec![relation(7)],
        vec![
            edge(30, 1, 2, 7, 0.3),
            edge(10, 1, 2, 7, 0.1),
            edge(20, 1, 1, 7, 0.2),
        ],
    );
    let image = RoutingImage::compile(&records).unwrap();
    let source = image.dense_node_id(NodeId::from_u64(1)).unwrap();
    let ids: Vec<_> = image
        .outgoing_edges(source)
        .unwrap()
        .map(|edge| edge.edge_id().as_u64())
        .collect();
    assert_eq!(ids, [10, 20, 30]);
    let destination = image.dense_node_id(NodeId::from_u64(2)).unwrap();
    assert_eq!(image.outgoing_edges(destination).unwrap().count(), 0);
}

fn node(id: u64) -> NodeRecord {
    NodeRecord::new(
        NodeId::from_u64(id),
        NodeName::from(format!("node-{id}")),
        NodePayload::default(),
    )
}

fn relation(id: u64) -> RelationRecord {
    RelationRecord::new(
        RelationId::from_u64(id),
        RelationName::from(format!("relation-{id}")),
    )
}

fn edge(id: u64, source: u64, destination: u64, relation: u64, weight: f32) -> EdgeRecord {
    EdgeRecord::new(
        EdgeId::from_u64(id),
        NodeId::from_u64(source),
        NodeId::from_u64(destination),
        RelationId::from_u64(relation),
        BaseWeight::new(weight).unwrap(),
    )
}
