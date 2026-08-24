use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Barrier, Mutex},
    time::Duration,
};

use pathhydra_store::{
    ActiveRoutingImage, Catalog, CatalogConfig, CatalogError, CheckpointRequest, ConfirmedRecord,
    FlushMode, MetricValue, OperationError, OperationalPathRequest, PathTarget, ReadOnlyCatalog,
    ResourceKind, RestoreRequest, VerificationLimits, WriteOperationClass, restore_checkpoint,
    standalone_restore_metrics, validate_operational_paths,
};
use rocksdb::{DB, IteratorMode, Options};
use tempfile::TempDir;

const COLUMN_FAMILIES: [&str; 9] = [
    "candidates",
    "nodes",
    "node_names",
    "relation_kinds",
    "relation_names",
    "edges",
    "outgoing_edges",
    "incoming_edges",
    "relation_popularity",
];

#[test]
fn strict_and_read_only_open_never_create_or_initialize_and_verify_every_kind() {
    let directory = TempDir::new().unwrap();
    let missing = directory.path().join("missing");
    assert!(Catalog::open_existing(&missing).is_err());
    assert!(ReadOnlyCatalog::open(&missing).is_err());
    assert!(!missing.exists());

    let empty = directory.path().join("empty");
    fs::create_dir(&empty).unwrap();
    assert!(Catalog::open_existing(&empty).is_err());
    assert!(ReadOnlyCatalog::open(&empty).is_err());
    assert_eq!(fs::read_dir(&empty).unwrap().count(), 0);

    let database = directory.path().join("catalog");
    let catalog = Catalog::open(&database).unwrap();
    let source = confirm_node(&catalog, "source");
    let destination = confirm_node(&catalog, "destination");
    let relation = confirm_relation(&catalog, "relation");
    let edge_candidate = catalog
        .insert_edge_candidate(source.id(), destination.id(), relation.id(), 0.5)
        .unwrap();
    let ConfirmedRecord::Edge(_) = catalog.confirm_validated_candidate(edge_candidate).unwrap()
    else {
        panic!("expected edge")
    };
    catalog.insert_node_candidate("candidate-node").unwrap();
    catalog
        .insert_relation_candidate("candidate-relation")
        .unwrap();
    catalog
        .insert_edge_candidate(source.id(), destination.id(), relation.id(), 0.25)
        .unwrap();
    let expected = catalog.verify(VerificationLimits::default()).unwrap();
    assert_eq!(expected.summary.candidates.nodes, 1);
    assert_eq!(expected.summary.candidates.relation_kinds, 1);
    assert_eq!(expected.summary.candidates.edges, 1);
    assert_eq!(expected.summary.confirmed_nodes, 2);
    assert_eq!(expected.summary.confirmed_edges, 1);
    drop(catalog);

    let before = directory_listing(&database);
    let read_only = ReadOnlyCatalog::open(&database).unwrap();
    let report = read_only.verify(VerificationLimits::default()).unwrap();
    assert_eq!(report.summary, expected.summary);
    assert!(read_only.path().ends_with("catalog"));
    let read_only_metrics = read_only.metrics_snapshot().unwrap();
    assert!(read_only_metrics.writes.is_empty());
    assert_eq!(read_only_metrics.scans.completed_scans, 0);
    assert_eq!(read_only_metrics.maintenance.checkpoint_attempts, 0);
    drop(read_only);
    assert_eq!(directory_listing(&database), before);
}

#[test]
fn exact_path_validation_rejects_roots_aliases_equality_and_nesting() {
    let directory = TempDir::new().unwrap();
    let database = directory.path().join("database");
    let image = directory.path().join("database").join("images");
    fs::create_dir(&database).unwrap();
    let request = OperationalPathRequest {
        database: Some(target(directory.path(), &database, true, false)),
        routing_images: Some(target(directory.path(), &image, false, false)),
        ..OperationalPathRequest::default()
    };
    assert!(matches!(
        validate_operational_paths(&request),
        Err(OperationError::UnsafePathRelationship { .. })
    ));

    let alias = directory.path().join("child").join("..").join("database");
    let equal = OperationalPathRequest {
        database: Some(target(directory.path(), &database, true, false)),
        checkpoint: Some(target(directory.path(), &alias, true, false)),
        ..OperationalPathRequest::default()
    };
    assert!(matches!(
        validate_operational_paths(&equal),
        Err(OperationError::UnsafePathRelationship { .. })
    ));

    let root = filesystem_root(directory.path());
    let invalid_root = OperationalPathRequest {
        database: Some(target(&root, &database, true, false)),
        ..OperationalPathRequest::default()
    };
    assert!(matches!(
        validate_operational_paths(&invalid_root),
        Err(OperationError::InvalidPath { .. })
    ));

    let nonempty = directory.path().join("nonempty");
    fs::create_dir(&nonempty).unwrap();
    fs::write(nonempty.join("unknown"), b"preserve").unwrap();
    let fresh = OperationalPathRequest {
        restore: Some(target(directory.path(), &nonempty, true, true)),
        ..OperationalPathRequest::default()
    };
    assert!(matches!(
        validate_operational_paths(&fresh),
        Err(OperationError::DestinationNotEmpty {
            resource: ResourceKind::Restore
        })
    ));
    assert_eq!(fs::read(nonempty.join("unknown")).unwrap(), b"preserve");
}

#[test]
fn checkpoint_and_restore_preserve_candidates_confirmed_state_and_deletion_history() {
    let directory = TempDir::new().unwrap();
    let database = directory.path().join("database");
    let images = directory.path().join("images");
    fs::create_dir(&images).unwrap();
    fs::write(images.join("bundle-bytes"), b"not a backup").unwrap();
    let catalog = Catalog::open(&database).unwrap();
    let restored_node = confirm_node(&catalog, "restored-node");
    let destination = confirm_node(&catalog, "destination");
    let relation = confirm_relation(&catalog, "relation");
    let pending_node = catalog.insert_node_candidate("pending-node").unwrap();
    let pending_relation = catalog
        .insert_relation_candidate("pending-relation")
        .unwrap();
    let pending_edge = catalog
        .insert_edge_candidate(restored_node.id(), destination.id(), relation.id(), 0.5)
        .unwrap();
    catalog
        .confirmed_graph_scan()
        .unwrap()
        .set_active_routing_image(&ActiveRoutingImage::new("bundle-1", [7; 32]))
        .unwrap();

    let checkpoint = directory.path().join("checkpoint");
    let checkpoint_report = catalog
        .create_checkpoint(&CheckpointRequest {
            destination_root: directory.path().to_path_buf(),
            destination: checkpoint.clone(),
            routing_image_root: Some(images.clone()),
            scratch_path: None,
            available_destination_bytes: u64::MAX,
            minimum_headroom_bytes: 0,
        })
        .unwrap();
    assert!(checkpoint_report.file_count > 0);
    assert!(checkpoint_report.bytes > 0);
    assert!(!checkpoint.join("bundle-bytes").exists());
    assert_eq!(
        fs::read(images.join("bundle-bytes")).unwrap(),
        b"not a backup"
    );

    let removed_after_checkpoint = confirm_node(&catalog, "removed-after-checkpoint");
    catalog.remove_node(removed_after_checkpoint.id()).unwrap();
    catalog.insert_node_candidate("after-checkpoint").unwrap();
    let restore = directory.path().join("restore");
    let restored = catalog
        .restore_checkpoint(&RestoreRequest {
            source_root: directory.path().to_path_buf(),
            source_checkpoint: checkpoint.clone(),
            destination_root: directory.path().to_path_buf(),
            destination: restore.clone(),
            routing_image_root: Some(images),
            scratch_path: None,
            available_destination_bytes: u64::MAX,
            minimum_headroom_bytes: 0,
            verification_limits: VerificationLimits::default(),
        })
        .unwrap();
    assert!(restored.cleared_routing_pointer);
    assert_eq!(restored.restored_catalog.summary.candidates.total(), 3);
    assert_eq!(restored.restored_catalog.summary.confirmed_nodes, 2);
    assert!(!restored.restored_catalog.summary.routing_pointer_present);

    let reopened = Catalog::open_existing(&restore).unwrap();
    assert_eq!(
        reopened
            .get_node(restored_node.id())
            .unwrap()
            .name()
            .as_str(),
        "restored-node"
    );
    assert_eq!(
        reopened.get_candidate(pending_node).unwrap().id(),
        pending_node
    );
    assert_eq!(
        reopened.get_candidate(pending_relation).unwrap().id(),
        pending_relation
    );
    assert_eq!(
        reopened.get_candidate(pending_edge).unwrap().id(),
        pending_edge
    );
    assert!(reopened.active_routing_image().unwrap().is_none());
}

#[test]
fn checkpoint_and_restore_refuse_disk_and_destinations_without_touching_sources() {
    let directory = TempDir::new().unwrap();
    let database = directory.path().join("database");
    let catalog = Catalog::open(&database).unwrap();
    confirm_node(&catalog, "node");
    let refused = directory.path().join("refused-checkpoint");
    assert!(matches!(
        catalog.create_checkpoint(&CheckpointRequest {
            destination_root: directory.path().to_path_buf(),
            destination: refused.clone(),
            routing_image_root: None,
            scratch_path: None,
            available_destination_bytes: 0,
            minimum_headroom_bytes: 0,
        }),
        Err(OperationError::ResourceRefused { .. })
    ));
    assert!(!refused.exists());

    let checkpoint = directory.path().join("checkpoint");
    catalog
        .create_checkpoint(&CheckpointRequest {
            destination_root: directory.path().to_path_buf(),
            destination: checkpoint.clone(),
            routing_image_root: None,
            scratch_path: None,
            available_destination_bytes: u64::MAX,
            minimum_headroom_bytes: 0,
        })
        .unwrap();
    let source_before = directory_listing(&checkpoint);
    let standalone_before = standalone_restore_metrics();
    let offline_refused = directory.path().join("offline-refused");
    assert!(matches!(
        restore_checkpoint(&RestoreRequest {
            source_root: directory.path().to_path_buf(),
            source_checkpoint: checkpoint.clone(),
            destination_root: directory.path().to_path_buf(),
            destination: offline_refused.clone(),
            routing_image_root: None,
            scratch_path: None,
            available_destination_bytes: 0,
            minimum_headroom_bytes: 0,
            verification_limits: VerificationLimits::default(),
        }),
        Err(OperationError::ResourceRefused { .. })
    ));
    assert!(!offline_refused.exists());
    let standalone_after = standalone_restore_metrics();
    assert!(standalone_after.attempts > standalone_before.attempts);
    assert!(standalone_after.failures > standalone_before.failures);
    let nonempty = directory.path().join("nonempty-restore");
    fs::create_dir(&nonempty).unwrap();
    fs::write(nonempty.join("keep"), b"keep").unwrap();
    assert!(
        catalog
            .restore_checkpoint(&RestoreRequest {
                source_root: directory.path().to_path_buf(),
                source_checkpoint: checkpoint.clone(),
                destination_root: directory.path().to_path_buf(),
                destination: nonempty.clone(),
                routing_image_root: None,
                scratch_path: None,
                available_destination_bytes: u64::MAX,
                minimum_headroom_bytes: 0,
                verification_limits: VerificationLimits::default(),
            })
            .is_err()
    );
    assert_eq!(directory_listing(&checkpoint), source_before);
    assert_eq!(fs::read(nonempty.join("keep")).unwrap(), b"keep");
}

#[test]
fn checkpoint_after_confirmed_deletion_restores_only_current_state() {
    let directory = TempDir::new().unwrap();
    let database = directory.path().join("database");
    let catalog = Catalog::open(&database).unwrap();
    let deleted = confirm_node(&catalog, "deleted-before-checkpoint");
    let retained = confirm_node(&catalog, "retained");
    catalog.remove_node(deleted.id()).unwrap();
    let checkpoint = directory.path().join("checkpoint-after-deletion");
    catalog
        .create_checkpoint(&CheckpointRequest {
            destination_root: directory.path().to_path_buf(),
            destination: checkpoint.clone(),
            routing_image_root: None,
            scratch_path: None,
            available_destination_bytes: u64::MAX,
            minimum_headroom_bytes: 0,
        })
        .unwrap();
    let restore = directory.path().join("restore-after-deletion");
    let report = catalog
        .restore_checkpoint(&restore_request(directory.path(), &checkpoint, &restore))
        .unwrap();
    assert_eq!(report.restored_catalog.summary.confirmed_nodes, 1);
    let restored = Catalog::open_existing(restore).unwrap();
    assert!(restored.get_node(deleted.id()).is_err());
    assert_eq!(
        restored.get_node(retained.id()).unwrap().id(),
        retained.id()
    );
}

#[test]
fn restore_rejects_missing_adjacency_and_truncated_checkpoint_and_preserves_source() {
    let directory = TempDir::new().unwrap();
    let database = directory.path().join("database");
    let catalog = Catalog::open(&database).unwrap();
    let source = confirm_node(&catalog, "source");
    let destination = confirm_node(&catalog, "destination");
    let relation = confirm_relation(&catalog, "relation");
    let candidate = catalog
        .insert_edge_candidate(source.id(), destination.id(), relation.id(), 0.5)
        .unwrap();
    let ConfirmedRecord::Edge(edge) = catalog.confirm_validated_candidate(candidate).unwrap()
    else {
        panic!("expected edge")
    };
    let checkpoint = directory.path().join("checkpoint");
    catalog
        .create_checkpoint(&CheckpointRequest {
            destination_root: directory.path().to_path_buf(),
            destination: checkpoint.clone(),
            routing_image_root: None,
            scratch_path: None,
            available_destination_bytes: u64::MAX,
            minimum_headroom_bytes: 0,
        })
        .unwrap();

    let clean_checkpoint_before = directory_listing(&checkpoint);
    let manifest_name = fs::read_to_string(checkpoint.join("CURRENT"))
        .unwrap()
        .trim()
        .to_owned();
    let mut required_components = vec![PathBuf::from("CURRENT"), PathBuf::from(manifest_name)];
    required_components.extend(
        fs::read_dir(&checkpoint)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "sst"))
            .map(|path| PathBuf::from(path.file_name().unwrap())),
    );
    assert!(required_components.len() >= 3);
    for (index, component) in required_components.iter().enumerate() {
        let corrupted = directory
            .path()
            .join(format!("missing-checkpoint-component-{index}"));
        copy_flat_directory(&checkpoint, &corrupted);
        fs::remove_file(corrupted.join(component)).unwrap();
        assert!(
            catalog
                .restore_checkpoint(&restore_request(
                    directory.path(),
                    &corrupted,
                    &directory
                        .path()
                        .join(format!("missing-component-restore-{index}"))
                ))
                .is_err(),
            "checkpoint without {} was accepted",
            component.display()
        );
        assert_eq!(directory_listing(&checkpoint), clean_checkpoint_before);
    }

    let raw = open_raw(&checkpoint);
    let outgoing = raw.cf_handle("outgoing_edges").unwrap();
    raw.delete_cf(
        outgoing,
        adjacency_key(source.id().as_u64(), edge.id().as_u64()),
    )
    .unwrap();
    drop(raw);
    let corrupted_before = directory_listing(&checkpoint);
    assert!(
        catalog
            .restore_checkpoint(&restore_request(
                directory.path(),
                &checkpoint,
                &directory.path().join("corrupt-restore")
            ))
            .is_err()
    );
    assert_eq!(directory_listing(&checkpoint), corrupted_before);
}

#[test]
fn restore_rejects_every_malformed_record_and_index_class_and_preserves_checkpoints() {
    let directory = TempDir::new().unwrap();
    let database = directory.path().join("database");
    let catalog = Catalog::open(&database).unwrap();
    let source = confirm_node(&catalog, "source");
    let destination = confirm_node(&catalog, "destination");
    let relation = confirm_relation(&catalog, "relation");
    let edge_candidate = catalog
        .insert_edge_candidate(source.id(), destination.id(), relation.id(), 0.5)
        .unwrap();
    let ConfirmedRecord::Edge(_) = catalog.confirm_validated_candidate(edge_candidate).unwrap()
    else {
        panic!("expected edge")
    };
    catalog.insert_node_candidate("candidate").unwrap();
    let checkpoint = directory.path().join("malformed-checkpoint");
    catalog
        .create_checkpoint(&CheckpointRequest {
            destination_root: directory.path().to_path_buf(),
            destination: checkpoint.clone(),
            routing_image_root: None,
            scratch_path: None,
            available_destination_bytes: u64::MAX,
            minimum_headroom_bytes: 0,
        })
        .unwrap();
    for family in ["default"].into_iter().chain(COLUMN_FAMILIES) {
        let corrupted = directory.path().join(format!("malformed-{family}"));
        copy_flat_directory(&checkpoint, &corrupted);
        let raw = open_raw(&corrupted);
        let handle = raw.cf_handle(family).unwrap();
        let first = raw
            .iterator_cf(handle, IteratorMode::Start)
            .next()
            .expect("record class is populated")
            .unwrap();
        raw.put_cf(handle, &first.0, [0xff]).unwrap();
        drop(raw);
        let source_before = directory_listing(&corrupted);
        assert!(
            catalog
                .restore_checkpoint(&restore_request(
                    directory.path(),
                    &corrupted,
                    &directory.path().join(format!("malformed-restore-{family}"))
                ))
                .is_err(),
            "malformed {family} record was accepted"
        );
        assert_eq!(directory_listing(&corrupted), source_before);
    }
}

#[test]
fn metrics_report_writes_scans_flush_properties_and_unavailable_counters() {
    let directory = TempDir::new().unwrap();
    let catalog = Catalog::open(directory.path().join("database")).unwrap();
    let node = confirm_node(&catalog, "node");
    let other = confirm_node(&catalog, "other");
    let relation = confirm_relation(&catalog, "relation");
    let edge = catalog
        .insert_edge_candidate(node.id(), other.id(), relation.id(), 0.5)
        .unwrap();
    let ConfirmedRecord::Edge(edge) = catalog.confirm_validated_candidate(edge).unwrap() else {
        panic!("expected edge")
    };
    catalog.remove_edge(edge.id()).unwrap();
    catalog.verify(VerificationLimits::default()).unwrap();
    catalog.flush(FlushMode::WalAndMemtables).unwrap();
    let compaction = catalog.compact_all().unwrap();
    assert_eq!(compaction.families.len(), COLUMN_FAMILIES.len() + 1);
    assert_eq!(compaction.families[0].name, "default");
    let metrics = catalog.metrics_snapshot().unwrap();
    for class in [
        WriteOperationClass::CandidateInsertion,
        WriteOperationClass::ConfirmedPromotion,
        WriteOperationClass::EdgeDeletion,
    ] {
        let value = metrics.writes.get(&class).unwrap();
        assert!(value.attempts > 0);
        assert_eq!(value.failures, 0);
        assert!(value.committed_bytes > 0);
    }
    assert_eq!(metrics.maintenance.last_verification_succeeded, Some(true));
    assert_eq!(metrics.maintenance.flush_attempts, 1);
    assert_eq!(metrics.maintenance.wal_sync_attempts, 3);
    assert_eq!(metrics.maintenance.wal_sync_failures, 0);
    assert_eq!(metrics.maintenance.compaction_attempts, 1);
    assert_eq!(metrics.maintenance.compaction_failures, 0);
    assert!(metrics.wal_enabled);
    assert!(!metrics.ordinary_writes_sync);
    assert_eq!(metrics.column_families.len(), COLUMN_FAMILIES.len() + 1);
    assert!(matches!(metrics.block_cache_hits, MetricValue::Unavailable));
    assert!(matches!(
        metrics.block_cache_misses,
        MetricValue::Unavailable
    ));
}

#[test]
fn verification_limits_and_invalid_checkpoint_configuration_are_typed() {
    let directory = TempDir::new().unwrap();
    assert!(matches!(
        Catalog::open_with_config(
            directory.path().join("invalid"),
            CatalogConfig {
                maximum_concurrent_checkpoints: 0
            }
        ),
        Err(CatalogError::InvalidConfiguration { .. })
    ));
    let catalog = Catalog::open(directory.path().join("database")).unwrap();
    confirm_node(&catalog, "node");
    assert!(matches!(
        catalog.verify(VerificationLimits {
            maximum_records: Some(1),
            maximum_duration: None,
        }),
        Err(OperationError::VerificationLimit {
            resource: "record count",
            ..
        })
    ));
    assert!(matches!(
        catalog.verify(VerificationLimits {
            maximum_records: None,
            maximum_duration: Some(Duration::ZERO),
        }),
        Err(OperationError::VerificationLimit {
            resource: "duration (nanoseconds)",
            ..
        })
    ));
}

fn target<'a>(root: &'a Path, path: &'a Path, exists: bool, fresh: bool) -> PathTarget<'a> {
    PathTarget {
        root,
        path,
        must_exist: exists,
        must_be_fresh: fresh,
    }
}

fn confirm_node(catalog: &Catalog, name: &str) -> pathhydra_store::NodeRecord {
    let candidate = catalog.insert_node_candidate(name).unwrap();
    let ConfirmedRecord::Node(node) = catalog.confirm_validated_candidate(candidate).unwrap()
    else {
        panic!("expected node")
    };
    node
}

fn confirm_relation(catalog: &Catalog, name: &str) -> pathhydra_store::RelationRecord {
    let candidate = catalog.insert_relation_candidate(name).unwrap();
    let ConfirmedRecord::Relation(relation) =
        catalog.confirm_validated_candidate(candidate).unwrap()
    else {
        panic!("expected relation")
    };
    relation
}

fn restore_request(root: &Path, source: &Path, destination: &Path) -> RestoreRequest {
    RestoreRequest {
        source_root: root.to_path_buf(),
        source_checkpoint: source.to_path_buf(),
        destination_root: root.to_path_buf(),
        destination: destination.to_path_buf(),
        routing_image_root: None,
        scratch_path: None,
        available_destination_bytes: u64::MAX,
        minimum_headroom_bytes: 0,
        verification_limits: VerificationLimits::default(),
    }
}

fn open_raw(path: &Path) -> DB {
    DB::open_cf(&Options::default(), path, COLUMN_FAMILIES).unwrap()
}

fn adjacency_key(node: u64, edge: u64) -> [u8; 16] {
    let mut output = [0_u8; 16];
    output[..8].copy_from_slice(&node.to_be_bytes());
    output[8..].copy_from_slice(&edge.to_be_bytes());
    output
}

fn directory_listing(path: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut entries: Vec<_> = fs::read_dir(path)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (entry.file_name().into(), fs::read(entry.path()).unwrap())
        })
        .collect();
    entries.sort();
    entries
}

fn copy_flat_directory(source: &Path, destination: &Path) {
    fs::create_dir(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        assert!(entry.file_type().unwrap().is_file());
        fs::copy(entry.path(), destination.join(entry.file_name())).unwrap();
    }
}

fn filesystem_root(path: &Path) -> PathBuf {
    let mut root = PathBuf::new();
    for component in path.components() {
        root.push(component.as_os_str());
        if root.parent().is_none() {
            break;
        }
    }
    root
}

#[test]
fn concurrent_checkpoint_limit_refuses_excess_callers_before_copying() {
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("database");
    let catalog = Arc::new(
        Catalog::open_with_config(
            &database,
            CatalogConfig {
                maximum_concurrent_checkpoints: 1,
            },
        )
        .unwrap(),
    );
    for index in 0..2_000 {
        catalog
            .insert_node_candidate(format!("candidate-{index}"))
            .unwrap();
    }
    let callers = 8;
    let destination_root = root.path().to_path_buf();
    let barrier = Arc::new(Barrier::new(callers));
    let results = Arc::new(Mutex::new(Vec::new()));
    std::thread::scope(|scope| {
        for index in 0..callers {
            let catalog = Arc::clone(&catalog);
            let barrier = Arc::clone(&barrier);
            let results = Arc::clone(&results);
            let destination_root = destination_root.clone();
            let destination = destination_root.join(format!("checkpoint-{index}"));
            scope.spawn(move || {
                barrier.wait();
                let result = catalog.create_checkpoint(&CheckpointRequest {
                    destination_root,
                    destination,
                    routing_image_root: None,
                    scratch_path: None,
                    available_destination_bytes: u64::MAX,
                    minimum_headroom_bytes: 0,
                });
                results.lock().unwrap().push(result);
            });
        }
    });
    let results = results.lock().unwrap();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(
                result,
                Err(OperationError::ConcurrentCheckpoint { maximum: 1 })
            ))
            .count(),
        callers - 1
    );
    let checkpoint_metrics = catalog.metrics_snapshot().unwrap().maintenance;
    assert_eq!(checkpoint_metrics.checkpoint_attempts, callers as u64);
    assert_eq!(checkpoint_metrics.checkpoint_failures, (callers - 1) as u64);
}
