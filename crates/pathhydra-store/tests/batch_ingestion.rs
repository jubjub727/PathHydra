use pathhydra_store::{
    BaseWeight, BatchNodeReference, BatchRelationReference, Candidate, CandidateBatchEntry,
    CandidateNodeReference, CandidateRelationReference, Catalog, CatalogError, ConfirmedRecord,
    NodePayload,
};
use std::sync::Arc;
use tempfile::TempDir;

fn node(name: &str) -> CandidateBatchEntry {
    CandidateBatchEntry::Node {
        name: name.into(),
        payload: NodePayload::default(),
    }
}

#[test]
fn mixed_forward_references_promote_atomically_with_deterministic_ids_and_usage() {
    let directory = TempDir::new().unwrap();
    let catalog = Catalog::open(directory.path()).unwrap();
    let inserted = catalog
        .insert_candidate_batch(&[
            CandidateBatchEntry::Edge {
                source: BatchNodeReference::BatchNode(2),
                destination: BatchNodeReference::BatchNode(1),
                relation_kind: BatchRelationReference::BatchRelationKind(3),
                base_weight: BaseWeight::new(0.25).unwrap(),
            },
            node("B"),
            node("A"),
            CandidateBatchEntry::RelationKind {
                name: "kind".into(),
            },
            CandidateBatchEntry::Edge {
                source: BatchNodeReference::BatchNode(2),
                destination: BatchNodeReference::BatchNode(2),
                relation_kind: BatchRelationReference::BatchRelationKind(3),
                base_weight: BaseWeight::new(0.5).unwrap(),
            },
        ])
        .unwrap();
    assert_eq!(
        inserted
            .candidate_ids
            .iter()
            .map(|id| id.as_u64())
            .collect::<Vec<_>>(),
        [1, 2, 3, 4, 5]
    );
    assert!(
        catalog
            .confirmed_graph_records()
            .unwrap()
            .nodes()
            .is_empty()
    );
    assert!(catalog.most_used_relation_kinds(10).unwrap().is_empty());

    let Candidate::Node {
        incoming_reference_count,
        ..
    } = catalog.get_candidate(inserted.candidate_ids[2]).unwrap()
    else {
        panic!("expected node candidate")
    };
    assert_eq!(incoming_reference_count, 3);
    let Candidate::Edge {
        source,
        destination,
        relation_kind,
        ..
    } = catalog.get_candidate(inserted.candidate_ids[0]).unwrap()
    else {
        panic!("expected edge candidate")
    };
    assert_eq!(
        source,
        CandidateNodeReference::Candidate(inserted.candidate_ids[2])
    );
    assert_eq!(
        destination,
        CandidateNodeReference::Candidate(inserted.candidate_ids[1])
    );
    assert_eq!(
        relation_kind,
        CandidateRelationReference::Candidate(inserted.candidate_ids[3])
    );

    let confirmed = catalog
        .confirm_candidate_batch(&inserted.candidate_ids)
        .unwrap();
    assert!(confirmed.graph_changed);
    assert_eq!(confirmed.created_nodes, 2);
    assert_eq!(confirmed.created_relation_kinds, 1);
    let ConfirmedRecord::Edge(first_edge) = &confirmed.records[0] else {
        panic!("aligned edge result")
    };
    let ConfirmedRecord::Node(node_b) = &confirmed.records[1] else {
        panic!("aligned node result")
    };
    let ConfirmedRecord::Node(node_a) = &confirmed.records[2] else {
        panic!("aligned node result")
    };
    let ConfirmedRecord::Relation(relation) = &confirmed.records[3] else {
        panic!("aligned relation result")
    };
    let ConfirmedRecord::Edge(self_edge) = &confirmed.records[4] else {
        panic!("aligned edge result")
    };
    assert_eq!((node_b.id().as_u64(), node_a.id().as_u64()), (1, 2));
    assert_eq!((first_edge.id().as_u64(), self_edge.id().as_u64()), (1, 2));
    assert_eq!(first_edge.source(), node_a.id());
    assert_eq!(first_edge.destination(), node_b.id());
    assert_eq!(self_edge.source(), node_a.id());
    assert_eq!(self_edge.destination(), node_a.id());

    let usage = catalog.most_used_relation_kinds(10).unwrap();
    assert_eq!(usage.len(), 1);
    assert_eq!(usage[0].relation_kind.id(), relation.id());
    assert_eq!(usage[0].relation_kind.provisional_reference_count(), 0);
    assert_eq!(usage[0].relation_kind.confirmed_edge_count(), 2);

    assert!(
        catalog
            .remove_edge(first_edge.id())
            .unwrap()
            .removed_relation_kinds
            .is_empty()
    );
    assert_eq!(
        catalog
            .remove_edge(self_edge.id())
            .unwrap()
            .removed_relation_kinds,
        [relation.id()]
    );
    assert!(catalog.most_used_relation_kinds(10).unwrap().is_empty());
    drop(catalog);
    Catalog::open(directory.path()).unwrap();
}

#[test]
fn incomplete_dependency_confirmation_and_invalid_local_reference_write_nothing() {
    let directory = TempDir::new().unwrap();
    let catalog = Catalog::open(directory.path()).unwrap();
    assert!(matches!(
        catalog.insert_candidate_batch(&[CandidateBatchEntry::Edge {
            source: BatchNodeReference::BatchNode(1),
            destination: BatchNodeReference::BatchNode(1),
            relation_kind: BatchRelationReference::BatchRelationKind(1),
            base_weight: BaseWeight::MIN,
        }]),
        Err(CatalogError::InvalidBatch { .. })
    ));
    let inserted = catalog
        .insert_candidate_batch(&[
            node("node"),
            CandidateBatchEntry::RelationKind {
                name: "kind".into(),
            },
            CandidateBatchEntry::Edge {
                source: BatchNodeReference::BatchNode(0),
                destination: BatchNodeReference::BatchNode(0),
                relation_kind: BatchRelationReference::BatchRelationKind(1),
                base_weight: BaseWeight::MIN,
            },
        ])
        .unwrap();
    assert_eq!(inserted.candidate_ids[0].as_u64(), 1);
    assert!(matches!(
        catalog.confirm_candidate_batch(&inserted.candidate_ids[..2]),
        Err(CatalogError::InvalidCandidateDependency { .. })
    ));
    for id in inserted.candidate_ids {
        assert_eq!(catalog.get_candidate(id).unwrap().id(), id);
    }
    assert!(
        catalog
            .confirmed_graph_records()
            .unwrap()
            .nodes()
            .is_empty()
    );
}

#[test]
fn concurrent_same_kind_usage_updates_have_no_lost_counts_or_cleanup() {
    let directory = TempDir::new().unwrap();
    let catalog = Arc::new(Catalog::open(directory.path()).unwrap());
    let base = catalog
        .insert_candidate_batch(&[
            node("source"),
            node("destination"),
            CandidateBatchEntry::RelationKind {
                name: "kind".into(),
            },
        ])
        .unwrap();
    let confirmed = catalog
        .confirm_candidate_batch(&base.candidate_ids)
        .unwrap();
    let ConfirmedRecord::Node(source) = &confirmed.records[0] else {
        panic!()
    };
    let ConfirmedRecord::Node(destination) = &confirmed.records[1] else {
        panic!()
    };
    let ConfirmedRecord::Relation(relation) = &confirmed.records[2] else {
        panic!()
    };
    let handles = (0..16)
        .map(|_| {
            let catalog = Arc::clone(&catalog);
            let (source, destination, relation) = (source.id(), destination.id(), relation.id());
            std::thread::spawn(move || {
                catalog
                    .insert_edge_candidate(source, destination, relation, 0.5)
                    .unwrap()
            })
        })
        .collect::<Vec<_>>();
    let mut candidates = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect::<Vec<_>>();
    candidates.sort_unstable();
    let usage = catalog.most_used_relation_kinds(1).unwrap();
    assert_eq!(usage[0].relation_kind.provisional_reference_count(), 16);
    assert_eq!(usage[0].relation_kind.confirmed_edge_count(), 0);

    catalog.confirm_candidate_batch(&candidates).unwrap();
    let usage = catalog.most_used_relation_kinds(1).unwrap();
    assert_eq!(usage[0].relation_kind.provisional_reference_count(), 0);
    assert_eq!(usage[0].relation_kind.confirmed_edge_count(), 16);
    let removals = (1..=16)
        .map(|id| {
            let catalog = Arc::clone(&catalog);
            std::thread::spawn(move || {
                catalog
                    .remove_edge(pathhydra_store::EdgeId::from_u64(id))
                    .unwrap()
            })
        })
        .collect::<Vec<_>>();
    let removed = removals
        .into_iter()
        .flat_map(|handle| handle.join().unwrap().removed_relation_kinds)
        .collect::<Vec<_>>();
    assert_eq!(removed, [relation.id()]);
    assert!(catalog.most_used_relation_kinds(1).unwrap().is_empty());
    drop(catalog);
    Catalog::open(directory.path()).unwrap();
}
