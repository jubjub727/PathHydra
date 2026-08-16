use std::fs;

use pathhydra_core::ConfirmedRecord;
use pathhydra_engine::{
    EngineConfig, GraphEngine, PublicationOutcome, PublicationStage, RequestId,
    StartupBundlePolicy, StartupImageOutcome,
};
use pathhydra_routing::{
    DestinationState, RelationMultiplier, RelationProfile, RelationUse, RoutingRequest,
    SearchBudget, TiePolicy,
};

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
    engine
        .publication_fault_injection()
        .fail_at(PublicationStage::BeforeEngineSwap);
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
