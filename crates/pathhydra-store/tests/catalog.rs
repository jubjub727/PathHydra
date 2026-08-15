use std::{
    collections::HashSet,
    sync::{Arc, Barrier},
    thread,
};

use pathhydra_store::{
    Candidate, Catalog, CatalogError, ConfirmedId, ConfirmedRecord, RecordKind, RelationId,
};
use rocksdb::{DB, Options};
use tempfile::TempDir;

const COLUMN_FAMILIES: [&str; 5] = [
    "candidates",
    "nodes",
    "node_names",
    "relation_kinds",
    "relation_names",
];
const EXACT_NAMES: [&str; 7] = [
    "token",
    "Token",
    "TOKEN",
    "token!",
    "token ",
    "tokén",
    "toke\u{301}n",
];

#[test]
fn node_candidate_lifecycle_preserves_exact_identity_across_restart() {
    let directory = TempDir::new().unwrap();
    let catalog = Catalog::open(directory.path()).unwrap();
    let candidate_id = catalog.insert_node_candidate("token ").unwrap();

    assert_eq!(catalog.graph_version().unwrap(), 0);
    assert_eq!(catalog.lookup_node_exact("token ").unwrap(), None);
    assert_eq!(
        catalog.get_candidate(candidate_id).unwrap(),
        Candidate::Node {
            id: candidate_id,
            name: "token ".into(),
        }
    );

    let node = match catalog.confirm_validated_candidate(candidate_id).unwrap() {
        ConfirmedRecord::Node(node) => node,
        ConfirmedRecord::Relation(_) => panic!("node candidate became a relation"),
    };
    assert_eq!(
        catalog.lookup_node_exact("token ").unwrap(),
        Some(node.id())
    );
    assert_eq!(catalog.get_node(node.id()).unwrap(), node);
    assert_eq!(catalog.graph_version().unwrap(), 1);
    drop(catalog);

    let reopened = Catalog::open(directory.path()).unwrap();
    assert_eq!(
        reopened.lookup_node_exact("token ").unwrap(),
        Some(node.id())
    );
    assert_eq!(
        reopened.get_node(node.id()).unwrap().name().as_str(),
        "token "
    );
}

#[test]
fn relation_candidate_lifecycle_preserves_exact_identity_across_restart() {
    let directory = TempDir::new().unwrap();
    let catalog = Catalog::open(directory.path()).unwrap();
    let candidate_id = catalog.insert_relation_candidate("tokén").unwrap();

    assert_eq!(catalog.lookup_relation_exact("tokén").unwrap(), None);
    assert_eq!(
        catalog.get_candidate(candidate_id).unwrap(),
        Candidate::Relation {
            id: candidate_id,
            name: "tokén".into(),
        }
    );

    let relation = match catalog.confirm_validated_candidate(candidate_id).unwrap() {
        ConfirmedRecord::Relation(relation) => relation,
        ConfirmedRecord::Node(_) => panic!("relation candidate became a node"),
    };
    drop(catalog);

    let reopened = Catalog::open(directory.path()).unwrap();
    assert_eq!(
        reopened.lookup_relation_exact("tokén").unwrap(),
        Some(relation.id())
    );
    assert_eq!(
        reopened
            .get_relation(relation.id())
            .unwrap()
            .name()
            .as_str(),
        "tokén"
    );
}

#[test]
fn all_case_spelling_punctuation_whitespace_and_unicode_forms_are_distinct() {
    let directory = TempDir::new().unwrap();
    let catalog = Catalog::open(directory.path()).unwrap();
    let mut node_ids = HashSet::new();
    let mut relation_ids = HashSet::new();

    for name in EXACT_NAMES {
        let node_candidate = catalog.insert_node_candidate(name).unwrap();
        let relation_candidate = catalog.insert_relation_candidate(name).unwrap();
        assert_eq!(catalog.lookup_node_exact(name).unwrap(), None);
        assert_eq!(catalog.lookup_relation_exact(name).unwrap(), None);

        let ConfirmedRecord::Node(node) =
            catalog.confirm_validated_candidate(node_candidate).unwrap()
        else {
            panic!("expected node");
        };
        let ConfirmedRecord::Relation(relation) = catalog
            .confirm_validated_candidate(relation_candidate)
            .unwrap()
        else {
            panic!("expected relation");
        };
        assert!(node_ids.insert(node.id()));
        assert!(relation_ids.insert(relation.id()));
        assert_eq!(node.name().as_str(), name);
        assert_eq!(relation.name().as_str(), name);
    }

    for name in EXACT_NAMES {
        assert!(catalog.lookup_node_exact(name).unwrap().is_some());
        assert!(catalog.lookup_relation_exact(name).unwrap().is_some());
    }
    assert_eq!(node_ids.len(), EXACT_NAMES.len());
    assert_eq!(relation_ids.len(), EXACT_NAMES.len());
}

#[test]
fn node_and_relation_namespaces_are_independent() {
    let directory = TempDir::new().unwrap();
    let catalog = Catalog::open(directory.path()).unwrap();
    let node_candidate = catalog.insert_node_candidate("same").unwrap();
    let relation_candidate = catalog.insert_relation_candidate("same").unwrap();

    let ConfirmedRecord::Node(node) = catalog.confirm_validated_candidate(node_candidate).unwrap()
    else {
        panic!("expected node");
    };
    let ConfirmedRecord::Relation(relation) = catalog
        .confirm_validated_candidate(relation_candidate)
        .unwrap()
    else {
        panic!("expected relation");
    };

    assert_eq!(catalog.lookup_node_exact("same").unwrap(), Some(node.id()));
    assert_eq!(
        catalog.lookup_relation_exact("same").unwrap(),
        Some(relation.id())
    );
}

#[test]
fn duplicate_confirmation_returns_existing_id_without_mutation() {
    let directory = TempDir::new().unwrap();
    let catalog = Catalog::open(directory.path()).unwrap();
    let first = catalog.insert_node_candidate("duplicate").unwrap();
    let duplicate = catalog.insert_node_candidate("duplicate").unwrap();
    let ConfirmedRecord::Node(node) = catalog.confirm_validated_candidate(first).unwrap() else {
        panic!("expected node");
    };
    let version = catalog.graph_version().unwrap();

    let error = catalog.confirm_validated_candidate(duplicate).unwrap_err();
    assert!(matches!(
        error,
        CatalogError::NameAlreadyConfirmed {
            existing_id: ConfirmedId::Node(id),
            ..
        } if id == node.id()
    ));
    assert_eq!(catalog.graph_version().unwrap(), version);
    assert_eq!(catalog.get_candidate(duplicate).unwrap().id(), duplicate);
    assert_eq!(
        catalog.get_node(node.id()).unwrap().name().as_str(),
        "duplicate"
    );
}

#[test]
fn confirmed_candidate_is_removed() {
    let directory = TempDir::new().unwrap();
    let catalog = Catalog::open(directory.path()).unwrap();
    let candidate = catalog.insert_node_candidate("confirmed").unwrap();
    catalog.confirm_validated_candidate(candidate).unwrap();
    assert!(matches!(
        catalog.get_candidate(candidate),
        Err(CatalogError::NotFound {
            kind: RecordKind::Candidate,
            ..
        })
    ));
}

#[test]
fn cache_rebuild_rejects_a_missing_confirmed_record() {
    let directory = TempDir::new().unwrap();
    let catalog = Catalog::open(directory.path()).unwrap();
    let candidate = catalog.insert_node_candidate("orphaned").unwrap();
    let ConfirmedRecord::Node(node) = catalog.confirm_validated_candidate(candidate).unwrap()
    else {
        panic!("expected node");
    };
    drop(catalog);

    let db = open_raw(directory.path());
    db.delete_cf(
        db.cf_handle("nodes").unwrap(),
        node.id().as_u64().to_be_bytes(),
    )
    .unwrap();
    drop(db);

    assert!(matches!(
        Catalog::open(directory.path()),
        Err(CatalogError::CorruptRecord {
            key_space: "node_names",
            ..
        })
    ));
}

#[test]
fn malformed_candidate_makes_open_fail_without_panicking() {
    let directory = TempDir::new().unwrap();
    let catalog = Catalog::open(directory.path()).unwrap();
    let candidate = catalog.insert_relation_candidate("broken").unwrap();
    drop(catalog);

    let db = open_raw(directory.path());
    db.put_cf(
        db.cf_handle("candidates").unwrap(),
        candidate.as_u64().to_be_bytes(),
        [1, 2, 0],
    )
    .unwrap();
    drop(db);

    assert!(matches!(
        Catalog::open(directory.path()),
        Err(CatalogError::CorruptRecord {
            key_space: "candidates",
            ..
        })
    ));
}

#[test]
fn incompatible_record_version_is_reported_explicitly() {
    let directory = TempDir::new().unwrap();
    let catalog = Catalog::open(directory.path()).unwrap();
    drop(catalog);
    let db = open_raw(directory.path());
    db.put(b"graph-version", [99, 0, 0, 0, 0, 0, 0, 0, 0])
        .unwrap();
    drop(db);

    assert!(matches!(
        Catalog::open(directory.path()),
        Err(CatalogError::IncompatibleFormat {
            key_space: "default",
            found: 99,
            ..
        })
    ));
}

#[test]
fn counter_overflow_does_not_remove_candidate_or_change_graph_version() {
    let directory = TempDir::new().unwrap();
    let catalog = Catalog::open(directory.path()).unwrap();
    let candidate = catalog.insert_node_candidate("last").unwrap();
    drop(catalog);

    let db = open_raw(directory.path());
    let mut maximum = vec![1];
    maximum.extend_from_slice(&u64::MAX.to_be_bytes());
    db.put(b"next-node-id", maximum).unwrap();
    drop(db);

    let catalog = Catalog::open(directory.path()).unwrap();
    assert!(matches!(
        catalog.confirm_validated_candidate(candidate),
        Err(CatalogError::CounterOverflow { counter: "node ID" })
    ));
    assert_eq!(catalog.graph_version().unwrap(), 0);
    assert_eq!(catalog.get_candidate(candidate).unwrap().id(), candidate);
    assert_eq!(catalog.lookup_node_exact("last").unwrap(), None);
}

#[test]
fn simultaneous_duplicate_confirmation_creates_at_most_one_record() {
    let directory = TempDir::new().unwrap();
    let catalog = Arc::new(Catalog::open(directory.path()).unwrap());
    let first = catalog.insert_node_candidate("racing").unwrap();
    let second = catalog.insert_node_candidate("racing").unwrap();
    let barrier = Arc::new(Barrier::new(3));

    let handles: Vec<_> = [first, second]
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
    let results: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();

    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(CatalogError::NameAlreadyConfirmed { .. })))
            .count(),
        1
    );
    assert_eq!(catalog.graph_version().unwrap(), 1);
    assert!(catalog.lookup_node_exact("racing").unwrap().is_some());
}

#[test]
fn concurrent_readers_never_observe_a_provisional_name_as_confirmed() {
    let directory = TempDir::new().unwrap();
    let catalog = Arc::new(Catalog::open(directory.path()).unwrap());
    let candidate = catalog
        .insert_relation_candidate("visible-after-commit")
        .unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let reader_catalog = Arc::clone(&catalog);
    let reader_barrier = Arc::clone(&barrier);
    let reader = thread::spawn(move || {
        assert_eq!(
            reader_catalog
                .lookup_relation_exact("visible-after-commit")
                .unwrap(),
            None
        );
        reader_barrier.wait();
        for _ in 0..1_000 {
            let observed = reader_catalog
                .lookup_relation_exact("visible-after-commit")
                .unwrap();
            assert!(observed.is_none() || observed == Some(RelationId::from_u64(1)));
        }
    });

    barrier.wait();
    catalog.confirm_validated_candidate(candidate).unwrap();
    reader.join().unwrap();
    assert_eq!(
        catalog
            .lookup_relation_exact("visible-after-commit")
            .unwrap(),
        Some(RelationId::from_u64(1))
    );
}

fn open_raw(path: &std::path::Path) -> DB {
    DB::open_cf(&Options::default(), path, COLUMN_FAMILIES).unwrap()
}
