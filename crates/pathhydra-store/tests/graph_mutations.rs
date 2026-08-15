use std::{
    sync::{Arc, Barrier},
    thread,
};

use pathhydra_store::{
    BaseWeight, Candidate, Catalog, CatalogError, ConfirmedRecord, EdgeEndpoint, EdgeId,
    EdgeRecord, MAX_NODE_PAYLOAD_BYTES, NodeId, NodePayload, RecordKind, RelationId,
};
use rocksdb::{DB, Options};
use tempfile::TempDir;

const COLUMN_FAMILIES: [&str; 8] = [
    "candidates",
    "nodes",
    "node_names",
    "relation_kinds",
    "relation_names",
    "edges",
    "outgoing_edges",
    "incoming_edges",
];

#[test]
fn metadata_and_records_contain_only_current_fields() {
    let directory = TempDir::new().unwrap();
    let catalog = Catalog::open(directory.path()).unwrap();
    let node = confirm_node(&catalog, "node", NodePayload::default());
    drop(catalog);

    let db = open_raw(directory.path());
    let metadata_keys: Vec<_> = db
        .iterator(rocksdb::IteratorMode::Start)
        .map(|entry| entry.unwrap().0.into_vec())
        .collect();
    assert_eq!(
        metadata_keys,
        [
            b"next-candidate-id".to_vec(),
            b"next-edge-id".to_vec(),
            b"next-node-id".to_vec(),
            b"next-relation-id".to_vec(),
        ]
    );
    assert_eq!(db.get(b"next-node-id").unwrap().unwrap().len(), 8);
    let node_record = db
        .get_cf(
            db.cf_handle("nodes").unwrap(),
            node.id().as_u64().to_be_bytes(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(&node_record[..8], &node.id().as_u64().to_be_bytes());
}

#[test]
fn node_payloads_preserve_empty_non_utf8_and_large_bytes_across_restart() {
    let directory = TempDir::new().unwrap();
    let catalog = Catalog::open(directory.path()).unwrap();
    let empty = confirm_node(&catalog, "empty", NodePayload::default());
    let binary = confirm_node(&catalog, "binary", NodePayload::from([0, 0xff, 0x80]));
    let large_payload = vec![0xa5; MAX_NODE_PAYLOAD_BYTES];
    let large = confirm_node(&catalog, "large", NodePayload::from(large_payload.clone()));

    assert!(empty.payload().as_bytes().is_empty());
    assert_eq!(binary.payload().as_bytes(), [0, 0xff, 0x80]);
    assert_eq!(large.payload().as_bytes(), large_payload);
    drop(catalog);

    let reopened = Catalog::open(directory.path()).unwrap();
    assert!(
        reopened
            .get_node(empty.id())
            .unwrap()
            .payload()
            .as_bytes()
            .is_empty()
    );
    assert_eq!(
        reopened.get_node(binary.id()).unwrap().payload().as_bytes(),
        [0, 0xff, 0x80]
    );
    assert_eq!(
        reopened.get_node(large.id()).unwrap().payload().as_bytes(),
        large_payload
    );
}

#[test]
fn payload_larger_than_the_documented_limit_is_rejected_before_a_write() {
    let directory = TempDir::new().unwrap();
    let catalog = Catalog::open(directory.path()).unwrap();
    let oversized = NodePayload::from(vec![0; MAX_NODE_PAYLOAD_BYTES + 1]);
    assert!(matches!(
        catalog.insert_node_candidate_with_payload("too-large", oversized),
        Err(CatalogError::PayloadTooLong { byte_length })
            if byte_length == MAX_NODE_PAYLOAD_BYTES + 1
    ));
    let first = catalog.insert_node_candidate("first").unwrap();
    assert_eq!(first.as_u64(), 1);
}

#[test]
fn provisional_edges_are_invisible_and_confirmed_edges_are_directed_and_distinct() {
    let directory = TempDir::new().unwrap();
    let catalog = Catalog::open(directory.path()).unwrap();
    let source = confirm_node(&catalog, "source", NodePayload::default());
    let destination = confirm_node(&catalog, "destination", NodePayload::default());
    let kind = confirm_relation(&catalog, "kind");
    let first_candidate = catalog
        .insert_edge_candidate(source.id(), destination.id(), kind.id(), 0.0)
        .unwrap();
    assert!(catalog.outgoing_edges(source.id()).unwrap().is_empty());
    assert!(catalog.incoming_edges(destination.id()).unwrap().is_empty());

    let first = confirm_edge(&catalog, first_candidate);
    let duplicate = confirm_edge(
        &catalog,
        catalog
            .insert_edge_candidate(source.id(), destination.id(), kind.id(), 0.0)
            .unwrap(),
    );
    assert_ne!(first.id(), duplicate.id());
    assert_eq!(
        catalog.outgoing_edges(source.id()).unwrap(),
        vec![first.clone(), duplicate.clone()]
    );
    assert!(catalog.outgoing_edges(destination.id()).unwrap().is_empty());
    assert_eq!(
        catalog.incoming_edges(destination.id()).unwrap(),
        vec![first, duplicate]
    );
    assert!(catalog.incoming_edges(source.id()).unwrap().is_empty());
}

#[test]
fn self_edges_are_distinct_and_removed_once_in_a_node_cascade() {
    let directory = TempDir::new().unwrap();
    let catalog = Catalog::open(directory.path()).unwrap();
    let node = confirm_node(&catalog, "self", NodePayload::default());
    let unrelated = confirm_node(&catalog, "unrelated", NodePayload::default());
    let kind = confirm_relation(&catalog, "kind");
    let first = insert_and_confirm_edge(&catalog, node.id(), node.id(), kind.id(), 0.5);
    let second = insert_and_confirm_edge(&catalog, node.id(), node.id(), kind.id(), 1.0);
    catalog.remove_node(node.id()).unwrap();
    for edge in [first, second] {
        assert!(matches!(
            catalog.get_edge(edge.id()),
            Err(CatalogError::NotFound {
                kind: RecordKind::Edge,
                ..
            })
        ));
    }
    assert_eq!(catalog.get_node(unrelated.id()).unwrap(), unrelated);
    assert_eq!(catalog.lookup_node_exact("self").unwrap(), None);
}

#[test]
fn base_weight_boundaries_are_canonical_and_invalid_values_write_nothing() {
    assert_eq!(BaseWeight::new(-0.0).unwrap(), BaseWeight::MIN);
    assert_eq!(BaseWeight::new(1.0).unwrap(), BaseWeight::MAX);

    let directory = TempDir::new().unwrap();
    let catalog = Catalog::open(directory.path()).unwrap();
    for invalid in [-0.01, 1.01, f32::NAN, f32::INFINITY] {
        assert!(matches!(
            catalog.insert_edge_candidate(
                NodeId::from_u64(1),
                NodeId::from_u64(2),
                RelationId::from_u64(1),
                invalid,
            ),
            Err(CatalogError::InvalidBaseWeight(_))
        ));
    }
    let candidate = catalog
        .insert_edge_candidate(
            NodeId::from_u64(1),
            NodeId::from_u64(2),
            RelationId::from_u64(1),
            -0.0,
        )
        .unwrap();
    assert_eq!(candidate.as_u64(), 1);
    let Candidate::Edge { base_weight, .. } = catalog.get_candidate(candidate).unwrap() else {
        panic!("expected an edge candidate");
    };
    assert_eq!(base_weight.to_bits(), 0);
}

#[test]
fn failed_edge_promotion_preserves_candidate_counters_and_indexes() {
    let directory = TempDir::new().unwrap();
    let catalog = Catalog::open(directory.path()).unwrap();
    let source = confirm_node(&catalog, "source", NodePayload::default());
    let destination = confirm_node(&catalog, "destination", NodePayload::default());
    let kind = confirm_relation(&catalog, "kind");

    let cases = [
        (
            NodeId::from_u64(99),
            destination.id(),
            kind.id(),
            EdgeEndpoint::Source,
        ),
        (
            source.id(),
            NodeId::from_u64(99),
            kind.id(),
            EdgeEndpoint::Destination,
        ),
    ];
    for (candidate_source, candidate_destination, candidate_kind, endpoint) in cases {
        let candidate = catalog
            .insert_edge_candidate(candidate_source, candidate_destination, candidate_kind, 0.5)
            .unwrap();
        assert!(matches!(
            catalog.confirm_validated_candidate(candidate),
            Err(CatalogError::MissingEdgeEndpoint { endpoint: found, .. }) if found == endpoint
        ));
        assert_eq!(catalog.get_candidate(candidate).unwrap().id(), candidate);
    }

    let candidate = catalog
        .insert_edge_candidate(source.id(), destination.id(), RelationId::from_u64(99), 0.5)
        .unwrap();
    assert!(matches!(
        catalog.confirm_validated_candidate(candidate),
        Err(CatalogError::MissingEdgeRelationKind { relation_kind_id })
            if relation_kind_id == RelationId::from_u64(99)
    ));
    assert_eq!(catalog.get_candidate(candidate).unwrap().id(), candidate);

    let valid = insert_and_confirm_edge(&catalog, source.id(), destination.id(), kind.id(), 0.5);
    assert_eq!(valid.id(), EdgeId::from_u64(1));
}

#[test]
fn edge_and_node_deletion_remove_all_durable_representations() {
    let directory = TempDir::new().unwrap();
    let catalog = Catalog::open(directory.path()).unwrap();
    let a = confirm_node(&catalog, "a", NodePayload::default());
    let b = confirm_node(&catalog, "b", NodePayload::default());
    let c = confirm_node(&catalog, "c", NodePayload::default());
    let kind = confirm_relation(&catalog, "kind");
    let ab = insert_and_confirm_edge(&catalog, a.id(), b.id(), kind.id(), 0.25);
    let ba = insert_and_confirm_edge(&catalog, b.id(), a.id(), kind.id(), 0.5);
    let bc = insert_and_confirm_edge(&catalog, b.id(), c.id(), kind.id(), 0.75);
    catalog.remove_edge(ab.id()).unwrap();
    assert!(catalog.outgoing_edges(a.id()).unwrap().is_empty());
    assert!(catalog.incoming_edges(b.id()).unwrap().is_empty());
    assert!(matches!(
        catalog.remove_edge(ab.id()),
        Err(CatalogError::NotFound {
            kind: RecordKind::Edge,
            ..
        })
    ));
    catalog.remove_node(b.id()).unwrap();
    for edge in [ba, bc] {
        assert!(matches!(
            catalog.get_edge(edge.id()),
            Err(CatalogError::NotFound { .. })
        ));
    }
    assert_eq!(catalog.get_node(a.id()).unwrap(), a);
    assert_eq!(catalog.get_node(c.id()).unwrap(), c);
    assert_eq!(catalog.lookup_node_exact("b").unwrap(), None);
    drop(catalog);

    let db = open_raw(directory.path());
    for name in ["edges", "outgoing_edges", "incoming_edges"] {
        assert_eq!(
            db.iterator_cf(db.cf_handle(name).unwrap(), rocksdb::IteratorMode::Start)
                .count(),
            0
        );
    }
}

#[test]
fn high_degree_cascade_preserves_unrelated_graph_material() {
    let directory = TempDir::new().unwrap();
    let catalog = Catalog::open(directory.path()).unwrap();
    let hub = confirm_node(&catalog, "hub", NodePayload::default());
    let kind = confirm_relation(&catalog, "kind");
    let mut leaves = Vec::new();
    for index in 0..128 {
        let leaf = confirm_node(&catalog, &format!("leaf-{index}"), NodePayload::default());
        insert_and_confirm_edge(&catalog, hub.id(), leaf.id(), kind.id(), 0.1);
        insert_and_confirm_edge(&catalog, leaf.id(), hub.id(), kind.id(), 0.2);
        leaves.push(leaf);
    }
    let unrelated =
        insert_and_confirm_edge(&catalog, leaves[0].id(), leaves[1].id(), kind.id(), 0.3);

    catalog.remove_node(hub.id()).unwrap();
    assert_eq!(catalog.get_edge(unrelated.id()).unwrap(), unrelated);
    assert_eq!(
        catalog.outgoing_edges(leaves[0].id()).unwrap(),
        vec![unrelated]
    );
}

#[test]
fn concurrent_promotions_and_delete_promotion_races_leave_a_valid_graph() {
    let directory = TempDir::new().unwrap();
    let catalog = Arc::new(Catalog::open(directory.path()).unwrap());
    let source = confirm_node(&catalog, "source", NodePayload::default());
    let destination = confirm_node(&catalog, "destination", NodePayload::default());
    let kind = confirm_relation(&catalog, "kind");
    let candidates = [0.2, 0.4].map(|weight| {
        catalog
            .insert_edge_candidate(source.id(), destination.id(), kind.id(), weight)
            .unwrap()
    });
    let barrier = Arc::new(Barrier::new(3));
    let handles: Vec<_> = candidates
        .into_iter()
        .map(|candidate| {
            let catalog = Arc::clone(&catalog);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                catalog.confirm_validated_candidate(candidate)
            })
        })
        .collect();
    barrier.wait();
    for handle in handles {
        assert!(handle.join().unwrap().is_ok());
    }
    assert_eq!(catalog.outgoing_edges(source.id()).unwrap().len(), 2);

    let race_candidate = catalog
        .insert_edge_candidate(source.id(), destination.id(), kind.id(), 0.6)
        .unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let promote_catalog = Arc::clone(&catalog);
    let promote_barrier = Arc::clone(&barrier);
    let promote = thread::spawn(move || {
        promote_barrier.wait();
        promote_catalog.confirm_validated_candidate(race_candidate)
    });
    let delete_catalog = Arc::clone(&catalog);
    let delete_barrier = Arc::clone(&barrier);
    let delete = thread::spawn(move || {
        delete_barrier.wait();
        delete_catalog.remove_node(source.id())
    });
    barrier.wait();
    let promotion = promote.join().unwrap();
    assert!(delete.join().unwrap().is_ok());
    assert!(
        promotion.is_ok() || matches!(promotion, Err(CatalogError::MissingEdgeEndpoint { .. }))
    );
    drop(catalog);

    let reopened = Catalog::open(directory.path()).unwrap();
    assert!(
        reopened
            .incoming_edges(destination.id())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn provisional_edge_survives_node_removal_but_cannot_later_promote() {
    let directory = TempDir::new().unwrap();
    let catalog = Catalog::open(directory.path()).unwrap();
    let source = confirm_node(&catalog, "source", NodePayload::default());
    let destination = confirm_node(&catalog, "destination", NodePayload::default());
    let kind = confirm_relation(&catalog, "kind");
    let candidate = catalog
        .insert_edge_candidate(source.id(), destination.id(), kind.id(), 0.5)
        .unwrap();
    catalog.remove_node(source.id()).unwrap();

    assert_eq!(catalog.get_candidate(candidate).unwrap().id(), candidate);
    assert!(matches!(
        catalog.confirm_validated_candidate(candidate),
        Err(CatalogError::MissingEdgeEndpoint {
            endpoint: EdgeEndpoint::Source,
            ..
        })
    ));
}

#[test]
fn open_rejects_missing_malformed_and_mismatched_adjacency() {
    for corruption in [
        Corruption::Missing,
        Corruption::Malformed,
        Corruption::Mismatched,
    ] {
        let directory = TempDir::new().unwrap();
        let catalog = Catalog::open(directory.path()).unwrap();
        let source = confirm_node(&catalog, "source", NodePayload::default());
        let destination = confirm_node(&catalog, "destination", NodePayload::default());
        let kind = confirm_relation(&catalog, "kind");
        let edge = insert_and_confirm_edge(&catalog, source.id(), destination.id(), kind.id(), 0.5);
        drop(catalog);

        let db = open_raw(directory.path());
        let outgoing = db.cf_handle("outgoing_edges").unwrap();
        let key = adjacency_key(source.id(), edge.id());
        match corruption {
            Corruption::Missing => db.delete_cf(outgoing, key).unwrap(),
            Corruption::Malformed => db.put_cf(outgoing, key, [0]).unwrap(),
            Corruption::Mismatched => db
                .put_cf(
                    outgoing,
                    adjacency_key(destination.id(), edge.id()),
                    index_value(edge.id()),
                )
                .unwrap(),
        }
        drop(db);

        assert!(matches!(
            Catalog::open(directory.path()),
            Err(CatalogError::CorruptRecord { .. })
        ));
    }
}

#[test]
fn open_rejects_invalid_edge_weight_and_missing_canonical_edge() {
    for invalid_weight in [true, false] {
        let directory = TempDir::new().unwrap();
        let catalog = Catalog::open(directory.path()).unwrap();
        let source = confirm_node(&catalog, "source", NodePayload::default());
        let destination = confirm_node(&catalog, "destination", NodePayload::default());
        let kind = confirm_relation(&catalog, "kind");
        let edge = insert_and_confirm_edge(&catalog, source.id(), destination.id(), kind.id(), 0.5);
        drop(catalog);

        let db = open_raw(directory.path());
        let edges = db.cf_handle("edges").unwrap();
        if invalid_weight {
            let mut value = db
                .get_cf(edges, edge.id().as_u64().to_be_bytes())
                .unwrap()
                .unwrap();
            value[32..36].copy_from_slice(&1.5_f32.to_bits().to_be_bytes());
            db.put_cf(edges, edge.id().as_u64().to_be_bytes(), value)
                .unwrap();
        } else {
            db.delete_cf(edges, edge.id().as_u64().to_be_bytes())
                .unwrap();
        }
        drop(db);

        assert!(matches!(
            Catalog::open(directory.path()),
            Err(CatalogError::CorruptRecord { .. })
        ));
    }
}

enum Corruption {
    Missing,
    Malformed,
    Mismatched,
}

fn confirm_node(
    catalog: &Catalog,
    name: &str,
    payload: NodePayload,
) -> pathhydra_store::NodeRecord {
    let candidate = catalog
        .insert_node_candidate_with_payload(name, payload)
        .unwrap();
    let ConfirmedRecord::Node(node) = catalog.confirm_validated_candidate(candidate).unwrap()
    else {
        panic!("expected node confirmation");
    };
    node
}

fn confirm_relation(catalog: &Catalog, name: &str) -> pathhydra_store::RelationRecord {
    let candidate = catalog.insert_relation_candidate(name).unwrap();
    let ConfirmedRecord::Relation(relation) =
        catalog.confirm_validated_candidate(candidate).unwrap()
    else {
        panic!("expected relation-kind confirmation");
    };
    relation
}

fn confirm_edge(catalog: &Catalog, candidate: pathhydra_store::CandidateId) -> EdgeRecord {
    let ConfirmedRecord::Edge(edge) = catalog.confirm_validated_candidate(candidate).unwrap()
    else {
        panic!("expected edge confirmation");
    };
    edge
}

fn insert_and_confirm_edge(
    catalog: &Catalog,
    source: NodeId,
    destination: NodeId,
    relation_kind: RelationId,
    base_weight: f32,
) -> EdgeRecord {
    let candidate = catalog
        .insert_edge_candidate(source, destination, relation_kind, base_weight)
        .unwrap();
    confirm_edge(catalog, candidate)
}

fn open_raw(path: &std::path::Path) -> DB {
    DB::open_cf(&Options::default(), path, COLUMN_FAMILIES).unwrap()
}

fn adjacency_key(node: NodeId, edge: EdgeId) -> [u8; 16] {
    let mut key = [0; 16];
    key[..8].copy_from_slice(&node.as_u64().to_be_bytes());
    key[8..].copy_from_slice(&edge.as_u64().to_be_bytes());
    key
}

fn index_value(edge: EdgeId) -> [u8; 8] {
    edge.as_u64().to_be_bytes()
}
