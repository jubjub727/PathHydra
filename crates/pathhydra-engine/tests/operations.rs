use std::path::PathBuf;

use pathhydra_core::ConfirmedRecord;
#[cfg(feature = "cuda")]
use pathhydra_engine::{CudaConfig, CudaExecutorPolicy};
use pathhydra_engine::{
    EngineConfig, EngineError, EngineLifecycleState, EngineRestoreRequest, GraphEngine,
    RoutingHealth,
};
use pathhydra_store::{
    Catalog, CheckpointRequest, OperationError, ReadOnlyCatalog, RestoreRequest,
    VerificationLimits, standalone_restore_metrics,
};

fn engine_config(routing_image_root: PathBuf) -> EngineConfig {
    EngineConfig {
        routing_image_root: Some(routing_image_root),
        ..EngineConfig::default()
    }
}

#[test]
fn engine_verification_configuration_caps_unbounded_and_oversized_requests() {
    let root = tempfile::tempdir().unwrap();
    let engine = GraphEngine::open(
        root.path().join("catalog"),
        EngineConfig {
            routing_image_root: Some(root.path().join("routing")),
            maximum_verification_records: 1,
            maximum_verification_duration: std::time::Duration::from_secs(30),
            ..EngineConfig::default()
        },
    )
    .unwrap();
    for limits in [
        VerificationLimits::default(),
        VerificationLimits {
            maximum_records: Some(u64::MAX),
            maximum_duration: Some(std::time::Duration::from_secs(60)),
        },
    ] {
        assert!(matches!(
            engine.verify_catalog(limits),
            Err(EngineError::Operation(OperationError::VerificationLimit {
                maximum: 1,
                ..
            }))
        ));
    }
    assert!(engine.shutdown().unwrap().complete());
}

#[test]
fn checkpoint_metrics_and_shutdown_are_coordinated_and_idempotent() {
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("catalog");
    let routing = root.path().join("routing");
    let checkpoint = root.path().join("checkpoint");
    let engine = GraphEngine::open(&database, engine_config(routing.clone())).unwrap();

    let confirmed = engine.insert_node_candidate("confirmed").unwrap();
    assert!(matches!(
        engine
            .confirm_validated_candidate(confirmed)
            .unwrap()
            .durable_result(),
        ConfirmedRecord::Node(_)
    ));
    engine.insert_node_candidate("still candidate").unwrap();
    let before = engine.catalog_summary().unwrap();
    assert_eq!(before.confirmed_nodes, 1);
    assert_eq!(before.candidates.nodes, 1);
    assert!(engine.store_metrics().unwrap().wal_enabled);

    let report = engine
        .create_checkpoint(&CheckpointRequest {
            destination_root: root.path().to_path_buf(),
            destination: checkpoint.clone(),
            routing_image_root: Some(routing),
            scratch_path: None,
            available_destination_bytes: u64::MAX,
            minimum_headroom_bytes: 0,
        })
        .unwrap();
    assert!(report.bytes > 0);
    assert_eq!(report.catalog.summary, before);
    let checkpoint_catalog = ReadOnlyCatalog::open(&checkpoint).unwrap();
    assert_eq!(checkpoint_catalog.summary().unwrap(), before);

    let shutdown = engine.shutdown().unwrap();
    assert!(shutdown.complete(), "{:#?}", shutdown.failures);
    assert_eq!(shutdown.state_after, EngineLifecycleState::ShutDown);
    assert_eq!(shutdown.active_before, Default::default());
    assert_eq!(shutdown.drained, Default::default());
    assert!(shutdown.store_flush_duration.is_some());
    let repeated = engine.shutdown().unwrap();
    assert!(repeated.complete());
    assert!(repeated.already_shut_down);
    assert!(engine.insert_node_candidate("rejected").is_err());
    assert!(matches!(
        engine.health().unwrap().routing,
        RoutingHealth::ShutDown
    ));

    // Explicit shutdown released RocksDB's exclusive path lock before the
    // engine value itself is dropped.
    drop(Catalog::open_existing(&database).unwrap());
}

#[test]
fn offline_restore_rebuilds_routing_smokes_and_preserves_candidates() {
    let metrics_before = standalone_restore_metrics();
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("source-catalog");
    let source_routing = root.path().join("source-routing");
    let checkpoint = root.path().join("checkpoint");
    let restored = root.path().join("restored-catalog");
    let restored_routing = root.path().join("restored-routing");
    let engine = GraphEngine::open(&database, engine_config(source_routing.clone())).unwrap();
    let source_candidate = engine.insert_node_candidate("source").unwrap();
    let ConfirmedRecord::Node(source) = engine
        .confirm_validated_candidate(source_candidate)
        .unwrap()
        .into_parts()
        .0
    else {
        panic!("expected node")
    };
    let destination_candidate = engine.insert_node_candidate("destination").unwrap();
    let ConfirmedRecord::Node(destination) = engine
        .confirm_validated_candidate(destination_candidate)
        .unwrap()
        .into_parts()
        .0
    else {
        panic!("expected node")
    };
    let relation_candidate = engine.insert_relation_candidate("relation").unwrap();
    let ConfirmedRecord::Relation(relation) = engine
        .confirm_validated_candidate(relation_candidate)
        .unwrap()
        .into_parts()
        .0
    else {
        panic!("expected relation")
    };
    let edge_candidate = engine
        .insert_edge_candidate(source.id(), destination.id(), relation.id(), 1.0)
        .unwrap();
    engine.confirm_validated_candidate(edge_candidate).unwrap();
    let candidate = engine.insert_node_candidate("candidate survives").unwrap();
    assert_eq!(engine.get_candidate(candidate).unwrap().id(), candidate);
    engine
        .create_checkpoint(&CheckpointRequest {
            destination_root: root.path().to_path_buf(),
            destination: checkpoint.clone(),
            routing_image_root: Some(source_routing),
            scratch_path: None,
            available_destination_bytes: u64::MAX,
            minimum_headroom_bytes: 0,
        })
        .unwrap();
    assert!(engine.shutdown().unwrap().complete());
    std::fs::create_dir_all(restored_routing.join("bundle-corrupt")).unwrap();
    std::fs::write(
        restored_routing.join("bundle-corrupt").join("manifest"),
        b"deliberately corrupt",
    )
    .unwrap();

    let report = GraphEngine::restore_checkpoint(&EngineRestoreRequest {
        store: RestoreRequest {
            source_root: root.path().to_path_buf(),
            source_checkpoint: checkpoint,
            destination_root: root.path().to_path_buf(),
            destination: restored.clone(),
            routing_image_root: Some(restored_routing.clone()),
            scratch_path: None,
            available_destination_bytes: u64::MAX,
            minimum_headroom_bytes: 0,
            verification_limits: VerificationLimits {
                maximum_records: Some(1_000),
                maximum_duration: Some(std::time::Duration::from_secs(30)),
            },
        },
        engine_config: engine_config(restored_routing),
    })
    .unwrap();
    assert!(report.smoke_catalog_verified);
    assert!(report.smoke_route_verified);
    assert!(report.smoke_hydration_verified);
    assert!(report.shutdown.complete());
    let metrics_after = standalone_restore_metrics();
    assert!(metrics_after.attempts > metrics_before.attempts);
    assert!(
        metrics_after.restored_bytes >= metrics_before.restored_bytes + report.store.source_bytes
    );
    assert!(metrics_after.total_duration >= metrics_before.total_duration);
    let restored_catalog = ReadOnlyCatalog::open(restored).unwrap();
    assert!(
        restored_catalog
            .metrics_snapshot()
            .unwrap()
            .standalone_restore
            .attempts
            > metrics_before.attempts
    );
    assert_eq!(restored_catalog.summary().unwrap().candidates.nodes, 1);
    assert_eq!(restored_catalog.summary().unwrap().confirmed_nodes, 2);
    assert_eq!(restored_catalog.summary().unwrap().relation_kinds, 1);
}

#[test]
fn drop_fallback_releases_the_database_owner() {
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("catalog");
    let routing = root.path().join("routing");
    drop(GraphEngine::open(&database, engine_config(routing)).unwrap());
    drop(Catalog::open_existing(database).unwrap());
}

#[test]
#[cfg(feature = "cuda")]
fn cuda_restore_reinitializes_and_routes_exactly() {
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("cuda-source");
    let catalog = Catalog::open(&database).unwrap();
    let ConfirmedRecord::Node(source) = catalog
        .confirm_validated_candidate(catalog.insert_node_candidate("source").unwrap())
        .unwrap()
    else {
        panic!("expected node")
    };
    let ConfirmedRecord::Node(destination) = catalog
        .confirm_validated_candidate(catalog.insert_node_candidate("destination").unwrap())
        .unwrap()
    else {
        panic!("expected node")
    };
    let ConfirmedRecord::Relation(relation) = catalog
        .confirm_validated_candidate(catalog.insert_relation_candidate("relation").unwrap())
        .unwrap()
    else {
        panic!("expected relation")
    };
    let edge = catalog
        .insert_edge_candidate(source.id(), destination.id(), relation.id(), 1.0)
        .unwrap();
    catalog.confirm_validated_candidate(edge).unwrap();
    let checkpoint = root.path().join("cuda-checkpoint");
    catalog
        .create_checkpoint(&CheckpointRequest {
            destination_root: root.path().to_path_buf(),
            destination: checkpoint.clone(),
            routing_image_root: None,
            scratch_path: None,
            available_destination_bytes: u64::MAX,
            minimum_headroom_bytes: 0,
        })
        .unwrap();
    drop(catalog);

    let restored = root.path().join("cuda-restored");
    let routing = root.path().join("cuda-restored-routing");
    let report = GraphEngine::restore_checkpoint(&EngineRestoreRequest {
        store: RestoreRequest {
            source_root: root.path().to_path_buf(),
            source_checkpoint: checkpoint,
            destination_root: root.path().to_path_buf(),
            destination: restored,
            routing_image_root: Some(routing.clone()),
            scratch_path: None,
            available_destination_bytes: u64::MAX,
            minimum_headroom_bytes: 0,
            verification_limits: VerificationLimits::default(),
        },
        engine_config: EngineConfig {
            routing_image_root: Some(routing),
            cuda: CudaConfig {
                enabled: true,
                executor_policy: CudaExecutorPolicy::PreferCuda,
                ..CudaConfig::default()
            },
            ..EngineConfig::default()
        },
    })
    .unwrap();
    assert!(report.cuda_initialized);
    assert!(report.smoke_route_verified);
    assert!(report.smoke_hydration_verified);
    assert!(report.shutdown.complete());
}
