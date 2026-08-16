#![cfg(feature = "cuda")]

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use pathhydra_core::{ConfirmedRecord, NodeId, RelationId};
use pathhydra_cuda::{
    CudaAlgorithm, CudaContextOwner, CudaFailureKind, CudaFaultStage, CudaPartitionedConfig,
    CudaPartitionedImage, CudaResidentImage, DeltaConfiguration, estimate_search_bytes,
};
use pathhydra_routing::{
    BundleConfig, ChunkedRoutingImage, CompletionReason, DestinationState, HostCacheConfig,
    RelationMultiplier, RelationProfile, RelationUse, RoutingImage, RoutingRequest, SearchBudget,
    TiePolicy, compile_bundle, open_bundle, route,
};
use pathhydra_store::Catalog;

fn fixture() -> (
    tempfile::TempDir,
    Catalog,
    Arc<RoutingImage>,
    RoutingRequest,
) {
    let directory = tempfile::tempdir().unwrap();
    let catalog = Catalog::open(directory.path()).unwrap();
    let nodes: Vec<_> = (0..6)
        .map(|index| {
            let candidate = catalog
                .insert_node_candidate(format!("node-{index}"))
                .unwrap();
            let ConfirmedRecord::Node(node) =
                catalog.confirm_validated_candidate(candidate).unwrap()
            else {
                panic!()
            };
            node.id()
        })
        .collect();
    let relations: Vec<_> = ["road", "rail"]
        .into_iter()
        .map(|name| {
            let candidate = catalog.insert_relation_candidate(name).unwrap();
            let ConfirmedRecord::Relation(relation) =
                catalog.confirm_validated_candidate(candidate).unwrap()
            else {
                panic!()
            };
            relation.id()
        })
        .collect();
    for (source, destination, relation, weight) in [
        (0, 1, 0, 1.0),
        (1, 2, 0, 0.0),
        (2, 1, 0, 0.0),
        (2, 3, 0, 0.2),
        (0, 3, 1, 0.8),
        (3, 4, 1, 0.05),
        (0, 4, 0, 1.0),
        (0, 4, 1, 0.6),
    ] {
        let candidate = catalog
            .insert_edge_candidate(
                nodes[source],
                nodes[destination],
                relations[relation],
                weight,
            )
            .unwrap();
        catalog.confirm_validated_candidate(candidate).unwrap();
    }
    let image =
        Arc::new(RoutingImage::compile(&catalog.confirmed_graph_records().unwrap()).unwrap());
    let request = RoutingRequest::new(
        nodes[0],
        [
            nodes[4],
            nodes[3],
            NodeId::from_u64(u64::MAX),
            nodes[4],
            nodes[5],
            nodes[0],
        ],
        RelationProfile::new([
            (
                relations[0],
                RelationUse::Enabled(RelationMultiplier::new(1.0).unwrap()),
            ),
            (
                relations[1],
                RelationUse::Enabled(RelationMultiplier::new(0.25).unwrap()),
            ),
        ]),
        false,
        SearchBudget::Unlimited,
        TiePolicy::StablePredecessor,
    );
    (directory, catalog, image, request)
}

fn assert_states_equal(
    cpu: &[pathhydra_routing::DestinationResult],
    cuda: &[pathhydra_routing::DestinationResult],
) {
    assert_eq!(cpu.len(), cuda.len());
    for (cpu, cuda) in cpu.iter().zip(cuda) {
        assert_eq!(cpu.destination(), cuda.destination());
        match (cpu.state(), cuda.state()) {
            (DestinationState::Exact(cpu), DestinationState::Exact(cuda)) => {
                assert_eq!(
                    cpu.logical_distance().to_bits(),
                    cuda.logical_distance().to_bits()
                );
                assert!(cuda.path().is_none());
            }
            (left, right) => assert_eq!(left, right),
        }
    }
}

#[test]
fn frontier_and_delta_match_the_cpu_oracle_bit_for_bit() {
    let (_directory, _catalog, image, request) = fixture();
    let cpu = route(&image, &request).unwrap();
    let context = CudaContextOwner::initialize(0).unwrap();
    let resident = CudaResidentImage::upload(context, image.clone(), usize::MAX, 0).unwrap();
    assert_eq!(
        resident.device_array_lengths(),
        (
            image.node_count() + 1,
            image.adjacency_count(),
            image.adjacency_count(),
            image.adjacency_count()
        )
    );
    for algorithm in [
        CudaAlgorithm::Frontier,
        CudaAlgorithm::DeltaStepping(DeltaConfiguration::new(0.1).unwrap()),
    ] {
        let reserved = estimate_search_bytes(
            image.node_count(),
            image.relation_kind_count(),
            request.destinations().len(),
            algorithm,
        )
        .unwrap();
        let cuda = resident
            .route(&request, algorithm, &AtomicBool::new(false), reserved)
            .unwrap();
        assert_states_equal(cpu.results(), cuda.response.results());
        assert!(cuda.diagnostics.kernel_launches > 0);
    }
}

#[test]
fn cancelled_cuda_search_never_reports_discovered_tentative_values_as_exact() {
    let (_directory, _catalog, image, request) = fixture();
    let context = CudaContextOwner::initialize(0).unwrap();
    let resident = CudaResidentImage::upload(context, image, usize::MAX, 0).unwrap();
    let cuda = resident
        .route(&request, CudaAlgorithm::Frontier, &AtomicBool::new(true), 1)
        .unwrap();
    assert_eq!(cuda.diagnostics.kernel_launches, 0);
    assert!(matches!(
        cuda.response.results()[0].state(),
        DestinationState::Incomplete
    ));
    assert!(matches!(
        cuda.response.results()[2].state(),
        DestinationState::MissingNode
    ));
    assert!(
        matches!(cuda.response.results()[5].state(), DestinationState::Exact(exact) if exact.logical_distance() == 0.0)
    );
}

#[test]
fn complete_profiles_remain_distinct_between_independent_searches() {
    let (_directory, _catalog, image, request) = fixture();
    let context = CudaContextOwner::initialize(0).unwrap();
    let resident = CudaResidentImage::upload(context, image, usize::MAX, 0).unwrap();
    let disabled = RoutingRequest::new(
        request.origin(),
        request.destinations().iter().copied(),
        RelationProfile::new([
            (RelationId::from_u64(1), RelationUse::Disabled),
            (RelationId::from_u64(2), RelationUse::Disabled),
        ]),
        false,
        SearchBudget::Unlimited,
        TiePolicy::StablePredecessor,
    );
    let enabled = resident
        .route(
            &request,
            CudaAlgorithm::Frontier,
            &AtomicBool::new(false),
            1,
        )
        .unwrap();
    let disabled = resident
        .route(
            &disabled,
            CudaAlgorithm::Frontier,
            &AtomicBool::new(false),
            1,
        )
        .unwrap();
    assert_ne!(
        enabled.response.results()[0].state(),
        disabled.response.results()[0].state()
    );
}

#[test]
fn one_edge_partitions_cache_thrash_and_reverse_group_order_match_cpu() {
    let (directory, catalog, resident, request) = fixture();
    let bundle_path = directory.path().join("forced-partitions");
    let scan = catalog.confirmed_graph_scan().unwrap();
    compile_bundle(
        &scan,
        &bundle_path,
        BundleConfig {
            target_partition_topology_bytes: 48,
            hard_maximum_partition_topology_bytes: 48,
            maximum_total_bundle_bytes: 1024 * 1024,
        },
    )
    .unwrap();
    drop(scan);
    let bundle = open_bundle(&bundle_path).unwrap();
    let chunked = Arc::new(
        ChunkedRoutingImage::open(
            bundle,
            HostCacheConfig {
                maximum_bytes: 112,
                maximum_staging_bytes: 56,
                maximum_entries: 1,
                io_worker_count: 2,
                maximum_queued_reads: 2,
            },
        )
        .unwrap(),
    );
    assert!(chunked.partition_count() > 1);
    let cpu = route(&resident, &request).unwrap();
    let context = CudaContextOwner::initialize(0).unwrap();
    for reverse in [false, true] {
        let partitioned = CudaPartitionedImage::upload(
            Arc::clone(&context),
            Arc::clone(&chunked),
            CudaPartitionedConfig {
                maximum_topology_cache_bytes: 48,
                maximum_topology_cache_slots: 1,
                maximum_host_staging_bytes: 48,
                minimum_free_memory_headroom: 0,
                reserved_concurrent_search_bytes: 1024 * 1024,
                reverse_partition_order: reverse,
            },
        )
        .unwrap();
        for algorithm in [
            CudaAlgorithm::Frontier,
            CudaAlgorithm::DeltaStepping(DeltaConfiguration::new(0.1).unwrap()),
        ] {
            let reserved = estimate_search_bytes(
                chunked.node_count(),
                chunked.relation_kind_count(),
                request.destinations().len(),
                algorithm,
            )
            .unwrap();
            let cuda = partitioned
                .route(&request, algorithm, &AtomicBool::new(false), reserved)
                .unwrap();
            assert_states_equal(cpu.results(), cuda.response.results());
            assert!(cuda.diagnostics.partitions_required > 1);
            assert!(partitioned.topology_cache_snapshot().evictions > 0);
        }
    }
}

#[test]
fn partitioned_fault_matrix_releases_state_and_preserves_bundle_cpu_oracle() {
    let (directory, catalog, resident, request) = fixture();
    let bundle_path = directory.path().join("fault-partitions");
    let scan = catalog.confirmed_graph_scan().unwrap();
    compile_bundle(
        &scan,
        &bundle_path,
        BundleConfig {
            target_partition_topology_bytes: 48,
            hard_maximum_partition_topology_bytes: 48,
            maximum_total_bundle_bytes: 1024 * 1024,
        },
    )
    .unwrap();
    drop(scan);
    let chunked = Arc::new(
        ChunkedRoutingImage::open(
            open_bundle(&bundle_path).unwrap(),
            HostCacheConfig {
                maximum_bytes: 112,
                maximum_staging_bytes: 56,
                maximum_entries: 1,
                io_worker_count: 2,
                maximum_queued_reads: 2,
            },
        )
        .unwrap(),
    );
    let context = CudaContextOwner::initialize(0).unwrap();
    let config = CudaPartitionedConfig {
        maximum_topology_cache_bytes: 48,
        maximum_topology_cache_slots: 1,
        maximum_host_staging_bytes: 48,
        minimum_free_memory_headroom: 0,
        reserved_concurrent_search_bytes: 1024 * 1024,
        reverse_partition_order: false,
    };
    let reserved = estimate_search_bytes(
        chunked.node_count(),
        chunked.relation_kind_count(),
        request.destinations().len(),
        CudaAlgorithm::Frontier,
    )
    .unwrap();
    for (stage, expected) in [
        (CudaFaultStage::HostAllocation, CudaFailureKind::Allocation),
        (
            CudaFaultStage::PinnedAllocation,
            CudaFailureKind::Allocation,
        ),
        (
            CudaFaultStage::DeviceAllocation,
            CudaFailureKind::Allocation,
        ),
        (CudaFaultStage::Copy, CudaFailureKind::Upload),
        (CudaFaultStage::Launch, CudaFailureKind::Launch),
        (CudaFaultStage::Event, CudaFailureKind::Launch),
        (
            CudaFaultStage::Synchronization,
            CudaFailureKind::Synchronization,
        ),
        (CudaFaultStage::ContextLoss, CudaFailureKind::DeviceLost),
    ] {
        let partitioned =
            CudaPartitionedImage::upload(Arc::clone(&context), Arc::clone(&chunked), config)
                .unwrap();
        partitioned.fault_injection().fail_at(stage);
        let error = partitioned
            .route(
                &request,
                CudaAlgorithm::Frontier,
                &AtomicBool::new(false),
                reserved,
            )
            .unwrap_err();
        assert_eq!(error.kind(), expected, "stage {stage:?}");
        assert_states_equal(
            route(&resident, &request).unwrap().results(),
            route(&resident, &request).unwrap().results(),
        );
    }
    let recovered = CudaPartitionedImage::upload(context, chunked, config).unwrap();
    let output = recovered
        .route(
            &request,
            CudaAlgorithm::Frontier,
            &AtomicBool::new(false),
            reserved,
        )
        .unwrap();
    assert_states_equal(
        route(&resident, &request).unwrap().results(),
        output.response.results(),
    );
}

#[test]
fn cancellation_during_partition_read_releases_cuda_and_host_cache_state() {
    let (directory, catalog, _resident, request) = fixture();
    let bundle_path = directory.path().join("cancelled-partitions");
    let scan = catalog.confirmed_graph_scan().unwrap();
    compile_bundle(
        &scan,
        &bundle_path,
        BundleConfig {
            target_partition_topology_bytes: 48,
            hard_maximum_partition_topology_bytes: 48,
            maximum_total_bundle_bytes: 1024 * 1024,
        },
    )
    .unwrap();
    drop(scan);
    let chunked = Arc::new(
        ChunkedRoutingImage::open(
            open_bundle(&bundle_path).unwrap(),
            HostCacheConfig {
                maximum_bytes: 112,
                maximum_staging_bytes: 56,
                maximum_entries: 1,
                io_worker_count: 1,
                maximum_queued_reads: 1,
            },
        )
        .unwrap(),
    );
    chunked
        .bundle()
        .fault_injection()
        .delay_reads(std::time::Duration::from_millis(100));
    let context = CudaContextOwner::initialize(0).unwrap();
    let partitioned = Arc::new(
        CudaPartitionedImage::upload(
            context,
            Arc::clone(&chunked),
            CudaPartitionedConfig {
                maximum_topology_cache_bytes: 48,
                maximum_topology_cache_slots: 1,
                maximum_host_staging_bytes: 48,
                minimum_free_memory_headroom: 0,
                reserved_concurrent_search_bytes: 1024 * 1024,
                reverse_partition_order: false,
            },
        )
        .unwrap(),
    );
    let reserved = estimate_search_bytes(
        chunked.node_count(),
        chunked.relation_kind_count(),
        request.destinations().len(),
        CudaAlgorithm::Frontier,
    )
    .unwrap();
    let cancellation = Arc::new(AtomicBool::new(false));
    let worker = {
        let partitioned = Arc::clone(&partitioned);
        let cancellation = Arc::clone(&cancellation);
        std::thread::spawn(move || {
            partitioned
                .route(&request, CudaAlgorithm::Frontier, &cancellation, reserved)
                .unwrap()
        })
    };
    std::thread::sleep(std::time::Duration::from_millis(10));
    cancellation.store(true, Ordering::Release);
    let output = worker.join().unwrap();
    assert_eq!(
        output.response.completion_reason(),
        CompletionReason::Cancelled
    );
    assert!(
        output
            .response
            .results()
            .iter()
            .any(|result| matches!(result.state(), DestinationState::Incomplete))
    );
    assert_eq!(partitioned.topology_cache_snapshot().in_use_slots, 0);
    for _ in 0..100 {
        if chunked.cache_snapshot().pinned_entries == 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(chunked.cache_snapshot().pinned_entries, 0);
}
