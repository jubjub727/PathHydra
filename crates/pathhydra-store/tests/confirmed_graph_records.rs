use pathhydra_store::{Catalog, ConfirmedRecord, NodePayload, RelationRecord};
use tempfile::TempDir;

#[test]
fn confirmed_graph_read_is_sorted_complete_and_excludes_every_candidate_kind() {
    let directory = TempDir::new().unwrap();
    let catalog = Catalog::open(directory.path()).unwrap();
    let first = confirm_node(&catalog, "first");
    let removed = confirm_node(&catalog, "removed");
    let third = confirm_node(&catalog, "third");
    catalog.remove_node(removed.id()).unwrap();
    let relation = confirm_relation(&catalog, "confirmed");
    let edge = confirm_edge(&catalog, first.id(), third.id(), relation.id(), 0.25);

    let provisional_node = catalog.insert_node_candidate("provisional node").unwrap();
    let provisional_relation = catalog
        .insert_relation_candidate("provisional relation")
        .unwrap();
    let provisional_edge = catalog
        .insert_edge_candidate(third.id(), first.id(), relation.id(), 0.75)
        .unwrap();

    let records = catalog.confirmed_graph_records().unwrap();
    assert_eq!(records.nodes(), [first.clone(), third.clone()]);
    assert_eq!(
        records.relation_kinds(),
        [RelationRecord::with_usage(
            relation.id(),
            relation.name().clone(),
            1,
            1,
        )]
    );
    assert_eq!(records.edges(), std::slice::from_ref(&edge));
    for candidate in [provisional_node, provisional_relation, provisional_edge] {
        assert_eq!(catalog.get_candidate(candidate).unwrap().id(), candidate);
    }

    let later = confirm_node(&catalog, "later");
    assert_eq!(records.nodes(), [first, third]);
    assert_eq!(
        catalog.confirmed_graph_records().unwrap().nodes(),
        [
            records.nodes()[0].clone(),
            records.nodes()[1].clone(),
            later,
        ]
    );
}

#[test]
fn confirmed_graph_read_survives_reopen_with_identical_records() {
    let directory = TempDir::new().unwrap();
    let catalog = Catalog::open(directory.path()).unwrap();
    let source = confirm_node(&catalog, "source");
    let destination = confirm_node(&catalog, "destination");
    let relation = confirm_relation(&catalog, "relation");
    confirm_edge(&catalog, source.id(), destination.id(), relation.id(), 0.5);
    let before = catalog.confirmed_graph_records().unwrap();
    drop(catalog);

    let reopened = Catalog::open(directory.path()).unwrap();
    assert_eq!(reopened.confirmed_graph_records().unwrap(), before);
}

fn confirm_node(catalog: &Catalog, name: &str) -> pathhydra_store::NodeRecord {
    let candidate = catalog
        .insert_node_candidate_with_payload(name, NodePayload::default())
        .unwrap();
    let ConfirmedRecord::Node(record) = catalog.confirm_validated_candidate(candidate).unwrap()
    else {
        panic!("node candidate changed kind")
    };
    record
}

fn confirm_relation(catalog: &Catalog, name: &str) -> pathhydra_store::RelationRecord {
    let candidate = catalog.insert_relation_candidate(name).unwrap();
    let ConfirmedRecord::Relation(record) = catalog.confirm_validated_candidate(candidate).unwrap()
    else {
        panic!("relation candidate changed kind")
    };
    record
}

fn confirm_edge(
    catalog: &Catalog,
    source: pathhydra_store::NodeId,
    destination: pathhydra_store::NodeId,
    relation: pathhydra_store::RelationId,
    weight: f32,
) -> pathhydra_store::EdgeRecord {
    let candidate = catalog
        .insert_edge_candidate(source, destination, relation, weight)
        .unwrap();
    let ConfirmedRecord::Edge(record) = catalog.confirm_validated_candidate(candidate).unwrap()
    else {
        panic!("edge candidate changed kind")
    };
    record
}
