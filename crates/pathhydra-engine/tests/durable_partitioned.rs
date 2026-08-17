use std::fs;

use pathhydra_core::ConfirmedRecord;
use pathhydra_engine::{
    EngineConfig, EngineRestoreRequest, GraphEngine, PublicationOutcome, PublicationStage,
    RequestId, StartupBundlePolicy, StartupImageOutcome,
};
use pathhydra_routing::{
    DestinationState, RelationMultiplier, RelationProfile, RelationUse, RoutingRequest,
    SearchBudget, TiePolicy,
};
use pathhydra_store::{CheckpointRequest, RestoreRequest, VerificationLimits};

fn directory_contents(root: &std::path::Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    fn visit(
        root: &std::path::Path,
        directory: &std::path::Path,
        contents: &mut Vec<(std::path::PathBuf, Vec<u8>)>,
    ) {
        let mut entries = fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap())
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                visit(root, &path, contents);
            } else {
                contents.push((
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    fs::read(path).unwrap(),
                ));
            }
        }
    }

    let mut contents = Vec::new();
    visit(root, root, &mut contents);
    contents
}

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

#[test]
fn every_publication_crash_boundary_reopens_complete_confirmed_data() {
    let stages = [
        PublicationStage::TemporaryDirectoryCreated,
        PublicationStage::BundleFilesSynchronized,
        PublicationStage::TemporaryBundleValidated,
        PublicationStage::FinalDirectoryRenamed,
        PublicationStage::DurablePointerCommitted,
        PublicationStage::RuntimeRepresentationConstructed,
        PublicationStage::BeforeEngineSwap,
    ];
    for (index, stage) in stages.into_iter().enumerate() {
        let temporary = tempfile::tempdir().unwrap();
        let database = temporary.path().join(format!("graph-{index}"));
        let engine = GraphEngine::open(&database, EngineConfig::default()).unwrap();
        let candidate = engine
            .insert_node_candidate(format!("node-{index}"))
            .unwrap();
        engine.publication_fault_injection().fail_at(stage);
        let mutation = engine.confirm_validated_candidate(candidate).unwrap();
        assert!(matches!(
            mutation.publication(),
            PublicationOutcome::RoutingUnavailable(_, _)
        ));
        drop(engine);

        let reopened = GraphEngine::open(&database, EngineConfig::default()).unwrap();
        let health = reopened.health().unwrap();
        assert_eq!(health.current_image_manifest.unwrap().node_count(), 1);
        let expected = if matches!(
            stage,
            PublicationStage::DurablePointerCommitted
                | PublicationStage::RuntimeRepresentationConstructed
                | PublicationStage::BeforeEngineSwap
        ) {
            StartupImageOutcome::ValidatedBundle
        } else {
            StartupImageOutcome::RebuiltFromCatalog
        };
        assert_eq!(health.startup_image_outcome, expected, "stage {stage:?}");
    }
}

#[test]
fn startup_policy_can_refuse_rebuild_until_manual_repair() {
    let temporary = tempfile::tempdir().unwrap();
    let database = temporary.path().join("graph");
    let engine = GraphEngine::open(
        &database,
        EngineConfig {
            startup_bundle_policy: StartupBundlePolicy::RequireValidBundle,
            ..EngineConfig::default()
        },
    )
    .unwrap();
    let health = engine.health().unwrap();
    assert_eq!(
        health.startup_image_outcome,
        StartupImageOutcome::RebuildFailed
    );
    assert!(health.current_image_manifest.is_none());
    engine.rebuild_routing_image().unwrap();
    assert!(engine.health().unwrap().current_image_manifest.is_some());
}

#[test]
fn publication_process_crash_child() {
    let Some(database) = std::env::var_os("PATHHYDRA_CRASH_HARNESS_DATABASE") else {
        return;
    };
    let engine = GraphEngine::open(database, EngineConfig::default()).unwrap();
    let candidate = engine
        .insert_node_candidate("survives-process-crash")
        .unwrap();
    let stage = match std::env::var("PATHHYDRA_CRASH_HARNESS_STAGE")
        .ok()
        .as_deref()
    {
        Some("0") => PublicationStage::TemporaryDirectoryCreated,
        Some("1") => PublicationStage::BundleFilesSynchronized,
        Some("2") => PublicationStage::TemporaryBundleValidated,
        Some("3") => PublicationStage::FinalDirectoryRenamed,
        Some("4") => PublicationStage::DurablePointerCommitted,
        Some("5") => PublicationStage::RuntimeRepresentationConstructed,
        Some("6") | None => PublicationStage::BeforeEngineSwap,
        Some(other) => panic!("unknown replayable crash stage {other}"),
    };
    engine.publication_fault_injection().fail_at(stage);
    let mutation = engine.confirm_validated_candidate(candidate).unwrap();
    assert!(matches!(
        mutation.publication(),
        PublicationOutcome::RoutingUnavailable(_, _)
    ));
    std::process::abort();
}

#[test]
fn abrupt_process_exit_after_pointer_commit_reopens_the_complete_bundle() {
    let temporary = tempfile::tempdir().unwrap();
    let database = temporary.path().join("graph");
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "publication_process_crash_child", "--nocapture"])
        .env("PATHHYDRA_CRASH_HARNESS_DATABASE", &database)
        .env("PATHHYDRA_CRASH_HARNESS_STAGE", "6")
        .status()
        .unwrap();
    assert!(!status.success());
    let reopened = GraphEngine::open(&database, EngineConfig::default()).unwrap();
    let health = reopened.health().unwrap();
    assert_eq!(
        health.startup_image_outcome,
        StartupImageOutcome::ValidatedBundle
    );
    assert_eq!(health.current_image_manifest.unwrap().node_count(), 1);
}

#[test]
fn replayable_subprocess_crash_matrix_combines_checkpoint_restore_failures() {
    let replay_seed = 0xd1b5_4a32_d192_ed03_u64;
    eprintln!("subprocess crash-matrix replay seed={replay_seed:#018x}");
    let mut order = [0_usize, 1, 2, 3, 4, 5, 6];
    let mut random = replay_seed;
    for index in (1..order.len()).rev() {
        random ^= random << 13;
        random ^= random >> 7;
        random ^= random << 17;
        order.swap(index, (random as usize) % (index + 1));
    }

    for stage in order {
        let temporary = tempfile::tempdir().unwrap();
        let database = temporary.path().join(format!("graph-stage-{stage}"));
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "publication_process_crash_child", "--nocapture"])
            .env("PATHHYDRA_CRASH_HARNESS_DATABASE", &database)
            .env("PATHHYDRA_CRASH_HARNESS_STAGE", stage.to_string())
            .status()
            .unwrap();
        assert!(
            !status.success(),
            "stage {stage} did not terminate abruptly"
        );

        let reopened = GraphEngine::open(&database, EngineConfig::default()).unwrap();
        assert_eq!(
            reopened
                .health()
                .unwrap()
                .current_image_manifest
                .unwrap()
                .node_count(),
            1,
            "stage {stage} lost the committed graph"
        );
        let checkpoint = temporary.path().join(format!("checkpoint-{stage}"));
        let checkpoint_report = reopened
            .create_checkpoint(&CheckpointRequest {
                destination_root: temporary.path().to_path_buf(),
                destination: checkpoint.clone(),
                routing_image_root: None,
                scratch_path: None,
                available_destination_bytes: u64::MAX,
                minimum_headroom_bytes: 0,
            })
            .unwrap();
        assert!(checkpoint_report.file_count > 0);
        assert!(reopened.shutdown().unwrap().complete());
        let checkpoint_contents = directory_contents(&checkpoint);

        let refused = temporary.path().join(format!("refused-restore-{stage}"));
        fs::create_dir(&refused).unwrap();
        fs::write(refused.join("operator-marker"), b"preserve").unwrap();
        let request = |destination| EngineRestoreRequest {
            store: RestoreRequest {
                source_root: temporary.path().to_path_buf(),
                source_checkpoint: checkpoint.clone(),
                destination_root: temporary.path().to_path_buf(),
                destination,
                routing_image_root: None,
                scratch_path: None,
                available_destination_bytes: u64::MAX,
                minimum_headroom_bytes: 0,
                verification_limits: VerificationLimits {
                    maximum_records: Some(10_000),
                    maximum_duration: Some(std::time::Duration::from_secs(30)),
                },
            },
            engine_config: EngineConfig::default(),
        };
        assert!(GraphEngine::restore_checkpoint(&request(refused.clone())).is_err());
        assert_eq!(
            fs::read(refused.join("operator-marker")).unwrap(),
            b"preserve"
        );
        assert_eq!(directory_contents(&checkpoint), checkpoint_contents);

        let restored = temporary.path().join(format!("restored-{stage}"));
        let report = GraphEngine::restore_checkpoint(&request(restored.clone())).unwrap();
        assert!(report.smoke_catalog_verified && report.smoke_route_verified);
        let restored_engine = GraphEngine::open(restored, EngineConfig::default()).unwrap();
        assert_eq!(
            restored_engine
                .health()
                .unwrap()
                .current_image_manifest
                .unwrap()
                .node_count(),
            1
        );
        assert!(restored_engine.shutdown().unwrap().complete());
        assert_eq!(directory_contents(&checkpoint), checkpoint_contents);
    }
}

#[test]
fn concurrent_partitioned_routes_observe_only_complete_old_or_new_publications() {
    let temporary = tempfile::tempdir().unwrap();
    let engine = std::sync::Arc::new(
        GraphEngine::open(
            temporary.path().join("graph"),
            EngineConfig {
                max_active_image_bytes: 8,
                target_partition_topology_bytes: 48,
                hard_maximum_partition_topology_bytes: 48,
                host_partition_cache_bytes: 112,
                host_partition_cache_entries: 1,
                routing_io_staging_bytes: 56,
                ..EngineConfig::default()
            },
        )
        .unwrap(),
    );
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
        .insert_edge_candidate(source.id(), destination.id(), relation.id(), 1.0)
        .unwrap();
    engine.confirm_validated_candidate(edge).unwrap();
    let request = std::sync::Arc::new(RoutingRequest::new(
        source.id(),
        [destination.id()],
        RelationProfile::new([(
            relation.id(),
            RelationUse::Enabled(RelationMultiplier::new(1.0).unwrap()),
        )]),
        false,
        SearchBudget::Unlimited,
        TiePolicy::StablePredecessor,
    ));
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(5));
    let workers: Vec<_> = (0..4_u64)
        .map(|worker| {
            let engine = std::sync::Arc::clone(&engine);
            let request = std::sync::Arc::clone(&request);
            let barrier = std::sync::Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                (0..40_u64)
                    .map(|iteration| {
                        engine
                            .route(RequestId::new(10_000 + worker * 100 + iteration), &request)
                            .unwrap()
                            .response
                    })
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    barrier.wait();
    std::thread::sleep(std::time::Duration::from_millis(5));
    engine.remove_node(destination.id()).unwrap();
    let responses: Vec<_> = workers
        .into_iter()
        .flat_map(|worker| worker.join().unwrap())
        .collect();
    assert!(responses.iter().all(|response| matches!(
        response.results()[0].state(),
        DestinationState::Exact(_) | DestinationState::MissingNode
    )));
    assert!(matches!(
        engine
            .route(RequestId::new(99_999), &request)
            .unwrap()
            .response
            .results()[0]
            .state(),
        DestinationState::MissingNode
    ));
}

#[test]
fn runtime_partition_failure_is_not_an_unreachable_result_and_triggers_rebuild() {
    let temporary = tempfile::tempdir().unwrap();
    let engine = GraphEngine::open(
        temporary.path().join("graph"),
        EngineConfig {
            max_active_image_bytes: 8,
            target_partition_topology_bytes: 48,
            hard_maximum_partition_topology_bytes: 48,
            host_partition_cache_bytes: 112,
            host_partition_cache_entries: 1,
            routing_io_staging_bytes: 56,
            ..EngineConfig::default()
        },
    )
    .unwrap();
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
        .insert_edge_candidate(source.id(), destination.id(), relation.id(), 1.0)
        .unwrap();
    engine.confirm_validated_candidate(edge).unwrap();
    let request = RoutingRequest::new(
        source.id(),
        [destination.id()],
        RelationProfile::new([(
            relation.id(),
            RelationUse::Enabled(RelationMultiplier::new(1.0).unwrap()),
        )]),
        false,
        SearchBudget::Unlimited,
        TiePolicy::StablePredecessor,
    );
    let before = engine
        .health()
        .unwrap()
        .current_bundle
        .unwrap()
        .relative_name;
    engine
        .routing_bundle_fault_injection()
        .unwrap()
        .read_error_at(0);
    assert!(matches!(
        engine.route(RequestId::new(40_000), &request),
        Err(pathhydra_engine::EngineError::RoutingUnavailable(_))
    ));
    let after = engine
        .health()
        .unwrap()
        .current_bundle
        .unwrap()
        .relative_name;
    assert_ne!(before, after);
    assert!(engine.health().unwrap().last_image_corruption.is_some());
    assert!(matches!(
        engine
            .route(RequestId::new(40_001), &request)
            .unwrap()
            .response
            .results()[0]
            .state(),
        DestinationState::Exact(_)
    ));
}
