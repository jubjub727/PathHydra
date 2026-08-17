#![cfg(feature = "cuda")]

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use pathhydra_core::{ConfirmedRecord, NodeId, RelationId};
use pathhydra_cuda::{
    CudaAlgorithm, CudaContextOwner, CudaFailureKind, CudaFaultStage, CudaPartitionedConfig,
    CudaPartitionedImage, CudaResidentImage, DeltaConfiguration, estimate_search_bytes,
    estimate_search_bytes_for_request,
};
use pathhydra_routing::{
    BundleConfig, ChunkedRoutingImage, CompletionReason, DestinationState, HostCacheConfig,
    RelationMultiplier, RelationProfile, RelationUse, RoutingImage, RoutingRequest, SearchBudget,
    TiePolicy, compile_bundle, estimate_cpu_working_set, open_bundle, responses_agree, route,
    route_partitioned,
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

fn assert_route_semantics_equal(
    cpu: &pathhydra_routing::RoutingResponse,
    cuda: &pathhydra_routing::RoutingResponse,
) {
    assert_eq!(cpu.origin(), cuda.origin());
    assert_eq!(cpu.profile(), cuda.profile());
    assert_eq!(cpu.numeric_policy(), cuda.numeric_policy());
    assert_eq!(cpu.tie_policy(), cuda.tie_policy());
    assert_eq!(cpu.paths_requested(), cuda.paths_requested());
    assert_eq!(cpu.completion_reason(), cuda.completion_reason());
    assert_eq!(cpu.finalized_nodes(), cuda.finalized_nodes());
    assert_states_equal(cpu.results(), cuda.results());
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
            image.adjacency_count(),
            request.destinations().len(),
            algorithm,
        )
        .unwrap();
        let undersized = resident
            .route(
                &request,
                algorithm,
                &AtomicBool::new(false),
                reserved.saturating_sub(1),
            )
            .expect_err("undersized resident search evidence must be refused before allocation");
        assert_eq!(undersized.kind(), CudaFailureKind::Admission);
        let cuda = resident
            .route(&request, algorithm, &AtomicBool::new(false), reserved)
            .unwrap();
        assert_route_semantics_equal(&cpu, &cuda.response);
        assert!(cuda.diagnostics.kernel_launches > 0);
        assert!(cuda.diagnostics.compacted_task_count > 0);
        assert_eq!(
            cuda.diagnostics.destination_count_checked,
            request.destinations().len()
        );
        assert!(!cuda.diagnostics.frontier_compaction_duration.is_zero());
        assert!(!cuda.diagnostics.destination_completion_duration.is_zero());
    }
}

#[test]
fn unrepresentable_delta_bucket_is_a_typed_failure_in_resident_and_partitioned_modes() {
    let directory = tempfile::tempdir().unwrap();
    let catalog = Catalog::open(directory.path().join("catalog")).unwrap();
    let nodes: Vec<_> = ["origin", "destination"]
        .into_iter()
        .map(|name| {
            let candidate = catalog.insert_node_candidate(name).unwrap();
            let ConfirmedRecord::Node(node) =
                catalog.confirm_validated_candidate(candidate).unwrap()
            else {
                panic!()
            };
            node.id()
        })
        .collect();
    let relation = catalog.insert_relation_candidate("maximum").unwrap();
    let ConfirmedRecord::Relation(relation) =
        catalog.confirm_validated_candidate(relation).unwrap()
    else {
        panic!()
    };
    let edge = catalog
        .insert_edge_candidate(nodes[0], nodes[1], relation.id(), 1.0)
        .unwrap();
    catalog.confirm_validated_candidate(edge).unwrap();
    let image =
        Arc::new(RoutingImage::compile(&catalog.confirmed_graph_records().unwrap()).unwrap());
    let request = RoutingRequest::new(
        nodes[0],
        [nodes[1]],
        RelationProfile::new([(
            relation.id(),
            RelationUse::Enabled(RelationMultiplier::new(f32::MAX).unwrap()),
        )]),
        false,
        SearchBudget::Unlimited,
        TiePolicy::StablePredecessor,
    );
    let algorithm =
        CudaAlgorithm::DeltaStepping(DeltaConfiguration::new(f64::MIN_POSITIVE).unwrap());
    let context = CudaContextOwner::initialize(0).unwrap();
    let resident =
        CudaResidentImage::upload(Arc::clone(&context), Arc::clone(&image), usize::MAX, 0).unwrap();
    let reserved = estimate_search_bytes(2, 1, 1, 1, algorithm).unwrap();
    let resident_error = resident
        .route(&request, algorithm, &AtomicBool::new(false), reserved)
        .unwrap_err();
    assert_eq!(resident_error.kind(), CudaFailureKind::KernelInvariant);

    let bundle_path = directory.path().join("unrepresentable-bucket");
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
    let partitioned = CudaPartitionedImage::upload(
        context,
        chunked,
        CudaPartitionedConfig {
            maximum_topology_cache_bytes: 48,
            maximum_topology_cache_slots: 1,
            maximum_host_staging_bytes: 48,
            minimum_free_memory_headroom: 0,
            reserved_concurrent_search_bytes: 1024 * 1024,
            reverse_partition_order: false,
        },
    )
    .unwrap();
    let partitioned_error = partitioned
        .route(&request, algorithm, &AtomicBool::new(false), reserved)
        .unwrap_err();
    assert_eq!(partitioned_error.kind(), CudaFailureKind::KernelInvariant);
}

#[test]
fn resident_cuda_paths_use_verified_same_image_cpu_evidence() {
    let (_directory, _catalog, image, request) = fixture();
    let request = RoutingRequest::new(
        request.origin(),
        request.destinations().to_vec(),
        request.profile().clone(),
        true,
        request.budget(),
        request.tie_policy(),
    );
    let cpu = route(&image, &request).unwrap();
    let resident = CudaResidentImage::upload(
        CudaContextOwner::initialize(0).unwrap(),
        image.clone(),
        usize::MAX,
        0,
    )
    .unwrap();
    for algorithm in [
        CudaAlgorithm::Frontier,
        CudaAlgorithm::DeltaStepping(DeltaConfiguration::new(0.1).unwrap()),
    ] {
        let reserved = estimate_search_bytes_for_request(
            image.node_count(),
            image.relation_kind_count(),
            image.adjacency_count(),
            request.destinations().len(),
            algorithm,
            Some(estimate_cpu_working_set(&image, &request).unwrap().bytes()),
        )
        .unwrap();
        let cuda = resident
            .route(&request, algorithm, &AtomicBool::new(false), reserved)
            .unwrap();
        assert!(responses_agree(&cpu, &cuda.response));
        assert_eq!(
            cuda.diagnostics.path_evidence_mode,
            pathhydra_cuda::CudaPathEvidenceMode::CpuPassSameImage
        );
    }
}

#[test]
fn equal_cost_cuda_paths_preserve_stable_predecessor_evidence() {
    let directory = tempfile::tempdir().unwrap();
    let catalog = Catalog::open(directory.path()).unwrap();
    let nodes: Vec<_> = (0..4)
        .map(|index| {
            let candidate = catalog
                .insert_node_candidate(format!("equal-cost-node-{index}"))
                .unwrap();
            let ConfirmedRecord::Node(node) =
                catalog.confirm_validated_candidate(candidate).unwrap()
            else {
                panic!()
            };
            node.id()
        })
        .collect();
    let candidate = catalog.insert_relation_candidate("equal-cost").unwrap();
    let ConfirmedRecord::Relation(relation) =
        catalog.confirm_validated_candidate(candidate).unwrap()
    else {
        panic!()
    };
    for (source, destination) in [(0, 1), (1, 3), (0, 2), (2, 3)] {
        let candidate = catalog
            .insert_edge_candidate(nodes[source], nodes[destination], relation.id(), 0.5)
            .unwrap();
        catalog.confirm_validated_candidate(candidate).unwrap();
    }
    let image =
        Arc::new(RoutingImage::compile(&catalog.confirmed_graph_records().unwrap()).unwrap());
    let request = RoutingRequest::new(
        nodes[0],
        [nodes[3]],
        RelationProfile::new([(
            relation.id(),
            RelationUse::Enabled(RelationMultiplier::new(1.0).unwrap()),
        )]),
        true,
        SearchBudget::Unlimited,
        TiePolicy::StablePredecessor,
    );
    let cpu = route(&image, &request).unwrap();
    let DestinationState::Exact(cpu_route) = cpu.results()[0].state() else {
        panic!("equal-cost CPU route must be exact")
    };
    assert!(cpu_route.path().is_some());
    let resident = CudaResidentImage::upload(
        CudaContextOwner::initialize(0).unwrap(),
        image.clone(),
        usize::MAX,
        0,
    )
    .unwrap();
    for algorithm in [
        CudaAlgorithm::Frontier,
        CudaAlgorithm::DeltaStepping(DeltaConfiguration::new(0.5).unwrap()),
    ] {
        let reserved = estimate_search_bytes_for_request(
            image.node_count(),
            image.relation_kind_count(),
            image.adjacency_count(),
            request.destinations().len(),
            algorithm,
            Some(estimate_cpu_working_set(&image, &request).unwrap().bytes()),
        )
        .unwrap();
        let cuda = resident
            .route(&request, algorithm, &AtomicBool::new(false), reserved)
            .unwrap();
        assert!(responses_agree(&cpu, &cuda.response));
        let DestinationState::Exact(cuda_route) = cuda.response.results()[0].state() else {
            panic!("equal-cost resident CUDA route must be exact")
        };
        assert_eq!(cpu_route.path(), cuda_route.path());
    }

    let bundle_path = directory.path().join("equal-cost-partitions");
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
    assert!(chunked.partition_count() > 1);
    let context = CudaContextOwner::initialize(0).unwrap();
    for reverse in [false, true] {
        for algorithm in [
            CudaAlgorithm::Frontier,
            CudaAlgorithm::DeltaStepping(DeltaConfiguration::new(0.5).unwrap()),
        ] {
            let partitioned = CudaPartitionedImage::upload(
                Arc::clone(&context),
                Arc::clone(&chunked),
                CudaPartitionedConfig {
                    maximum_topology_cache_bytes: 48,
                    maximum_topology_cache_slots: 1,
                    maximum_host_staging_bytes: 56,
                    minimum_free_memory_headroom: 0,
                    reserved_concurrent_search_bytes: 1024 * 1024,
                    reverse_partition_order: reverse,
                },
            )
            .unwrap();
            let reserved = estimate_search_bytes_for_request(
                chunked.node_count(),
                chunked.relation_kind_count(),
                chunked.adjacency_count(),
                request.destinations().len(),
                algorithm,
                Some(chunked.estimate_working_set(&request).unwrap().bytes()),
            )
            .unwrap();
            let cuda = partitioned
                .route(&request, algorithm, &AtomicBool::new(false), reserved)
                .unwrap();
            assert!(responses_agree(&cpu, &cuda.response));
            let DestinationState::Exact(cuda_route) = cuda.response.results()[0].state() else {
                panic!("equal-cost partitioned CUDA route must be exact")
            };
            assert_eq!(cpu_route.path(), cuda_route.path());
            assert!(cuda.diagnostics.partitions_required > 1);
            assert!(cuda.diagnostics.compacted_task_count > 0);
        }
    }
}

#[test]
fn cancellation_at_resident_path_evidence_boundary_returns_controlled_incomplete_results() {
    let (_directory, _catalog, image, request) = fixture();
    let request = RoutingRequest::new(
        request.origin(),
        request.destinations().to_vec(),
        request.profile().clone(),
        true,
        request.budget(),
        request.tie_policy(),
    );
    let resident = CudaResidentImage::upload(
        CudaContextOwner::initialize(0).unwrap(),
        image.clone(),
        usize::MAX,
        0,
    )
    .unwrap();
    let reserved = estimate_search_bytes_for_request(
        image.node_count(),
        image.relation_kind_count(),
        image.adjacency_count(),
        request.destinations().len(),
        CudaAlgorithm::Frontier,
        Some(estimate_cpu_working_set(&image, &request).unwrap().bytes()),
    )
    .unwrap();
    let faults = resident.fault_injection();
    faults.hold_path_evidence(true);
    let cancellation = AtomicBool::new(false);
    let output = std::thread::scope(|scope| {
        let route = scope
            .spawn(|| resident.route(&request, CudaAlgorithm::Frontier, &cancellation, reserved));
        assert!(faults.wait_for_path_evidence(Duration::from_secs(5)));
        cancellation.store(true, Ordering::Release);
        faults.hold_path_evidence(false);
        route.join().unwrap().unwrap()
    });
    assert_eq!(
        output.response.completion_reason(),
        CompletionReason::Cancelled
    );
    assert!(output.response.paths_requested());
    assert_eq!(output.diagnostics.first_destination_duration, None);
    for result in output.response.results() {
        match result.state() {
            DestinationState::Exact(route) => {
                assert_eq!(result.destination(), request.origin());
                assert_eq!(route.logical_distance().to_bits(), 0.0_f64.to_bits());
            }
            DestinationState::MissingNode | DestinationState::Incomplete => {}
            state => panic!("cancelled path evidence returned unsafe state {state:?}"),
        }
    }
}

#[test]
fn cancellation_at_resident_task_compaction_boundary_releases_without_allocating_tasks() {
    let (directory, catalog, image, request) = fixture();
    let resident = CudaResidentImage::upload(
        CudaContextOwner::initialize(0).unwrap(),
        image.clone(),
        usize::MAX,
        0,
    )
    .unwrap();
    let reserved = estimate_search_bytes(
        image.node_count(),
        image.relation_kind_count(),
        image.adjacency_count(),
        request.destinations().len(),
        CudaAlgorithm::Frontier,
    )
    .unwrap();
    let faults = resident.fault_injection();
    faults.hold_task_compaction(true);
    let cancellation = AtomicBool::new(false);
    let output = std::thread::scope(|scope| {
        let route = scope
            .spawn(|| resident.route(&request, CudaAlgorithm::Frontier, &cancellation, reserved));
        assert!(faults.wait_for_task_compaction(Duration::from_secs(5)));
        cancellation.store(true, Ordering::Release);
        faults.hold_task_compaction(false);
        route.join().unwrap().unwrap()
    });
    assert_eq!(
        output.response.completion_reason(),
        CompletionReason::Cancelled
    );
    assert_eq!(output.response.finalized_nodes(), 0);
    assert_eq!(output.diagnostics.compacted_task_count, 0);

    let bundle_path = directory.path().join("compaction-cancellation-partitions");
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
    let partitioned = CudaPartitionedImage::upload(
        CudaContextOwner::initialize(0).unwrap(),
        Arc::clone(&chunked),
        CudaPartitionedConfig {
            maximum_topology_cache_bytes: 48,
            maximum_topology_cache_slots: 1,
            maximum_host_staging_bytes: 56,
            minimum_free_memory_headroom: 0,
            reserved_concurrent_search_bytes: 1024 * 1024,
            reverse_partition_order: false,
        },
    )
    .unwrap();
    let reserved = estimate_search_bytes(
        chunked.node_count(),
        chunked.relation_kind_count(),
        chunked.adjacency_count(),
        request.destinations().len(),
        CudaAlgorithm::Frontier,
    )
    .unwrap();
    let faults = partitioned.fault_injection();
    faults.hold_task_compaction(true);
    let cancellation = AtomicBool::new(false);
    let output = std::thread::scope(|scope| {
        let route = scope.spawn(|| {
            partitioned.route(&request, CudaAlgorithm::Frontier, &cancellation, reserved)
        });
        assert!(faults.wait_for_task_compaction(Duration::from_secs(5)));
        cancellation.store(true, Ordering::Release);
        faults.hold_task_compaction(false);
        route.join().unwrap().unwrap()
    });
    assert_eq!(
        output.response.completion_reason(),
        CompletionReason::Cancelled
    );
    assert_eq!(output.response.finalized_nodes(), 0);
    assert_eq!(output.diagnostics.compacted_task_count, 0);
    assert_eq!(partitioned.topology_cache_snapshot().in_use_slots, 0);
}

#[test]
fn resident_path_evidence_fault_is_classified_and_the_image_recovers() {
    let (_directory, _catalog, image, request) = fixture();
    let request = RoutingRequest::new(
        request.origin(),
        request.destinations().to_vec(),
        request.profile().clone(),
        true,
        request.budget(),
        request.tie_policy(),
    );
    let resident = CudaResidentImage::upload(
        CudaContextOwner::initialize(0).unwrap(),
        image.clone(),
        usize::MAX,
        0,
    )
    .unwrap();
    let reserved = estimate_search_bytes_for_request(
        image.node_count(),
        image.relation_kind_count(),
        image.adjacency_count(),
        request.destinations().len(),
        CudaAlgorithm::Frontier,
        Some(estimate_cpu_working_set(&image, &request).unwrap().bytes()),
    )
    .unwrap();
    let faults = resident.fault_injection();
    faults.fail_at(CudaFaultStage::PathEvidence);
    let error = resident
        .route(
            &request,
            CudaAlgorithm::Frontier,
            &AtomicBool::new(false),
            reserved,
        )
        .unwrap_err();
    assert_eq!(error.kind(), CudaFailureKind::KernelInvariant);
    let recovered = resident
        .route(
            &request,
            CudaAlgorithm::Frontier,
            &AtomicBool::new(false),
            reserved,
        )
        .unwrap();
    assert!(responses_agree(
        &route(&image, &request).unwrap(),
        &recovered.response
    ));
}

#[test]
fn cancelled_cuda_search_never_reports_discovered_tentative_values_as_exact() {
    let (_directory, _catalog, image, request) = fixture();
    let reserved = estimate_search_bytes(
        image.node_count(),
        image.relation_kind_count(),
        image.adjacency_count(),
        request.destinations().len(),
        CudaAlgorithm::Frontier,
    )
    .unwrap();
    let context = CudaContextOwner::initialize(0).unwrap();
    let resident = CudaResidentImage::upload(context, image, usize::MAX, 0).unwrap();
    let cuda = resident
        .route(
            &request,
            CudaAlgorithm::Frontier,
            &AtomicBool::new(true),
            reserved,
        )
        .unwrap();
    assert_eq!(cuda.diagnostics.kernel_launches, 0);
    assert_eq!(cuda.response.finalized_nodes(), 0);
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
    let reserved = estimate_search_bytes(
        image.node_count(),
        image.relation_kind_count(),
        image.adjacency_count(),
        request.destinations().len(),
        CudaAlgorithm::Frontier,
    )
    .unwrap();
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
            reserved,
        )
        .unwrap();
    let disabled = resident
        .route(
            &disabled,
            CudaAlgorithm::Frontier,
            &AtomicBool::new(false),
            reserved,
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
    let partitioned_cpu = route_partitioned(&chunked, &request).unwrap();
    assert!(responses_agree(&cpu, &partitioned_cpu));
    let path_request = RoutingRequest::new(
        request.origin(),
        request.destinations().to_vec(),
        request.profile().clone(),
        true,
        request.budget(),
        request.tie_policy(),
    );
    let path_cpu = route(&resident, &path_request).unwrap();
    let context = CudaContextOwner::initialize(0).unwrap();
    for reverse in [false, true] {
        for algorithm in [
            CudaAlgorithm::Frontier,
            CudaAlgorithm::DeltaStepping(DeltaConfiguration::new(0.1).unwrap()),
        ] {
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
            let reserved = estimate_search_bytes(
                chunked.node_count(),
                chunked.relation_kind_count(),
                chunked.adjacency_count(),
                request.destinations().len(),
                algorithm,
            )
            .unwrap();
            let minimum_reserved = estimate_search_bytes(
                chunked.node_count(),
                chunked.relation_kind_count(),
                partitioned.maximum_partition_adjacency_count(),
                request.destinations().len(),
                algorithm,
            )
            .unwrap();
            let undersized = partitioned
                .route(
                    &request,
                    algorithm,
                    &AtomicBool::new(false),
                    minimum_reserved.saturating_sub(1),
                )
                .expect_err(
                    "undersized partitioned search evidence must be refused before allocation",
                );
            assert_eq!(undersized.kind(), CudaFailureKind::Admission);
            let cancellation = AtomicBool::new(false);
            let cuda = if !reverse && matches!(algorithm, CudaAlgorithm::Frontier) {
                partitioned.fault_injection().hold_completion_events(true);
                std::thread::scope(|scope| {
                    let route = scope
                        .spawn(|| partitioned.route(&request, algorithm, &cancellation, reserved));
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
                    let observed_wait = loop {
                        if partitioned.topology_cache_snapshot().completion_waits > 0 {
                            break true;
                        }
                        if std::time::Instant::now() >= deadline {
                            break false;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(2));
                    };
                    partitioned.fault_injection().hold_completion_events(false);
                    let result = route.join().expect("partitioned route thread").unwrap();
                    assert!(
                        observed_wait,
                        "one-slot cache must wait while its only slot owns a completion event"
                    );
                    result
                })
            } else {
                partitioned
                    .route(&request, algorithm, &cancellation, reserved)
                    .unwrap()
            };
            assert_route_semantics_equal(&cpu, &cuda.response);
            assert!(cuda.diagnostics.partitions_required > 1);
            let reported_phases = cuda
                .diagnostics
                .state_initialization_duration
                .saturating_add(cuda.diagnostics.partition_scheduling_duration)
                .saturating_add(cuda.diagnostics.relation_relaxation_duration)
                .saturating_add(cuda.diagnostics.response_transfer_duration)
                .saturating_add(cuda.diagnostics.frontier_compaction_duration)
                .saturating_add(cuda.diagnostics.destination_completion_duration);
            assert!(reported_phases <= cuda.diagnostics.synchronized_execution_duration);
            assert!(!cuda.diagnostics.partition_scheduling_duration.is_zero());
            assert!(!cuda.diagnostics.relation_relaxation_duration.is_zero());
            assert!(!cuda.diagnostics.response_transfer_duration.is_zero());
            assert!(!cuda.diagnostics.frontier_compaction_duration.is_zero());
            assert!(!cuda.diagnostics.destination_completion_duration.is_zero());
            assert!(cuda.diagnostics.compacted_task_count > 0);
            assert_eq!(
                cuda.diagnostics.destination_count_checked,
                request.destinations().len()
            );
            assert!(partitioned.topology_cache_snapshot().evictions > 0);
            assert_eq!(partitioned.topology_cache_snapshot().in_use_slots, 0);
            if !reverse {
                let path_reserved = estimate_search_bytes_for_request(
                    chunked.node_count(),
                    chunked.relation_kind_count(),
                    chunked.adjacency_count(),
                    path_request.destinations().len(),
                    algorithm,
                    Some(chunked.estimate_working_set(&path_request).unwrap().bytes()),
                )
                .unwrap();
                let path_cuda = partitioned
                    .route(
                        &path_request,
                        algorithm,
                        &AtomicBool::new(false),
                        path_reserved,
                    )
                    .unwrap();
                assert!(responses_agree(&path_cpu, &path_cuda.response));
            }
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
        chunked.adjacency_count(),
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
        (CudaFaultStage::ParallelScratch, CudaFailureKind::Allocation),
        (CudaFaultStage::TaskCompaction, CudaFailureKind::Allocation),
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
    let path_request = RoutingRequest::new(
        request.origin(),
        request.destinations().to_vec(),
        request.profile().clone(),
        true,
        request.budget(),
        request.tie_policy(),
    );
    let path_reserved = estimate_search_bytes_for_request(
        chunked.node_count(),
        chunked.relation_kind_count(),
        chunked.adjacency_count(),
        path_request.destinations().len(),
        CudaAlgorithm::Frontier,
        Some(chunked.estimate_working_set(&path_request).unwrap().bytes()),
    )
    .unwrap();
    let path_fault =
        CudaPartitionedImage::upload(Arc::clone(&context), Arc::clone(&chunked), config).unwrap();
    path_fault
        .fault_injection()
        .fail_at(CudaFaultStage::PathEvidence);
    let error = path_fault
        .route(
            &path_request,
            CudaAlgorithm::Frontier,
            &AtomicBool::new(false),
            path_reserved,
        )
        .unwrap_err();
    assert_eq!(error.kind(), CudaFailureKind::KernelInvariant);
    assert_eq!(path_fault.topology_cache_snapshot().in_use_slots, 0);
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
        chunked.adjacency_count(),
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

#[test]
fn concurrent_partition_requests_coalesce_loading_and_leave_no_cache_state_in_use() {
    let (directory, catalog, resident, request) = fixture();
    let bundle_path = directory.path().join("coalesced-device-cache");
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
    chunked
        .bundle()
        .fault_injection()
        .delay_reads(std::time::Duration::from_millis(50));
    let partitioned = CudaPartitionedImage::upload(
        CudaContextOwner::initialize(0).unwrap(),
        Arc::clone(&chunked),
        CudaPartitionedConfig {
            maximum_topology_cache_bytes: 48,
            maximum_topology_cache_slots: 1,
            maximum_host_staging_bytes: 48,
            minimum_free_memory_headroom: 0,
            reserved_concurrent_search_bytes: 2 * 1024 * 1024,
            reverse_partition_order: false,
        },
    )
    .unwrap();
    let reserved = estimate_search_bytes(
        chunked.node_count(),
        chunked.relation_kind_count(),
        chunked.adjacency_count(),
        request.destinations().len(),
        CudaAlgorithm::Frontier,
    )
    .unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(3));
    let workers: Vec<_> = (0..2)
        .map(|_| {
            let image = Arc::clone(&partitioned);
            let request = request.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                image
                    .route(
                        &request,
                        CudaAlgorithm::Frontier,
                        &AtomicBool::new(false),
                        reserved,
                    )
                    .unwrap()
            })
        })
        .collect();
    barrier.wait();
    let cpu = route(&resident, &request).unwrap();
    for worker in workers {
        let output = worker.join().unwrap();
        assert_states_equal(cpu.results(), output.response.results());
    }
    let cache = partitioned.topology_cache_snapshot();
    assert!(cache.coalesced_waits > 0);
    assert_eq!(cache.host_loading_entries, 0);
    assert_eq!(cache.copying_entries, 0);
    assert_eq!(cache.evicting_entries, 0);
    assert_eq!(cache.failed_entries, 0);
    assert_eq!(cache.in_use_slots, 0);
    assert!(cache.current_bytes <= cache.capacity_bytes);
    assert!(cache.entries <= cache.capacity_slots);
}

#[test]
fn delta_groups_only_current_bucket_source_partitions() {
    let directory = tempfile::tempdir().unwrap();
    let catalog = Catalog::open(directory.path().join("catalog")).unwrap();
    let nodes: Vec<_> = (0..64)
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
    let relation = catalog.insert_relation_candidate("road").unwrap();
    let ConfirmedRecord::Relation(relation) =
        catalog.confirm_validated_candidate(relation).unwrap()
    else {
        panic!()
    };
    for source in std::iter::once(0).chain(2..63) {
        let destination = if source == 0 { 1 } else { source + 1 };
        let edge = catalog
            .insert_edge_candidate(nodes[source], nodes[destination], relation.id(), 1.0)
            .unwrap();
        catalog.confirm_validated_candidate(edge).unwrap();
    }
    let resident =
        Arc::new(RoutingImage::compile(&catalog.confirmed_graph_records().unwrap()).unwrap());
    let request = RoutingRequest::new(
        nodes[0],
        [nodes[1]],
        RelationProfile::new([(
            relation.id(),
            RelationUse::Enabled(RelationMultiplier::new(1.0).unwrap()),
        )]),
        false,
        SearchBudget::Unlimited,
        TiePolicy::StablePredecessor,
    );
    let bundle_path = directory.path().join("delta-grouped");
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
    assert!(chunked.partition_count() > 32);
    let partitioned = CudaPartitionedImage::upload(
        CudaContextOwner::initialize(0).unwrap(),
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
    .unwrap();
    let algorithm = CudaAlgorithm::DeltaStepping(DeltaConfiguration::new(1.0e-9).unwrap());
    let output = partitioned
        .route(
            &request,
            algorithm,
            &AtomicBool::new(false),
            estimate_search_bytes(
                chunked.node_count(),
                chunked.relation_kind_count(),
                chunked.adjacency_count(),
                request.destinations().len(),
                algorithm,
            )
            .unwrap(),
        )
        .unwrap();
    assert_states_equal(
        route(&resident, &request).unwrap().results(),
        output.response.results(),
    );
    assert_eq!(partitioned.topology_cache_snapshot().misses, 1);
    assert_eq!(partitioned.topology_cache_snapshot().copies, 1);
    assert_eq!(output.diagnostics.partitions_required, 2);
}

#[test]
fn extreme_block_boundary_and_high_degree_split_source_remain_exact() {
    let directory = tempfile::tempdir().unwrap();
    let catalog = Catalog::open(directory.path().join("catalog")).unwrap();
    let nodes: Vec<_> = (0..2)
        .map(|index| {
            let candidate = catalog
                .insert_node_candidate(format!("parallel-node-{index}"))
                .unwrap();
            let ConfirmedRecord::Node(node) =
                catalog.confirm_validated_candidate(candidate).unwrap()
            else {
                panic!()
            };
            node.id()
        })
        .collect();
    let relation = catalog.insert_relation_candidate("parallel").unwrap();
    let ConfirmedRecord::Relation(relation) =
        catalog.confirm_validated_candidate(relation).unwrap()
    else {
        panic!()
    };
    for index in 0..1_025 {
        let weight = 0.001 + (index % 100) as f32 * 0.001;
        let candidate = catalog
            .insert_edge_candidate(nodes[0], nodes[1], relation.id(), weight)
            .unwrap();
        catalog.confirm_validated_candidate(candidate).unwrap();
    }
    let resident =
        Arc::new(RoutingImage::compile(&catalog.confirmed_graph_records().unwrap()).unwrap());
    let request = RoutingRequest::new(
        nodes[0],
        [nodes[1]],
        RelationProfile::new([(
            relation.id(),
            RelationUse::Enabled(RelationMultiplier::new(1.0).unwrap()),
        )]),
        false,
        SearchBudget::Unlimited,
        TiePolicy::StablePredecessor,
    );
    let expected = route(&resident, &request).unwrap();
    let context = CudaContextOwner::initialize(0).unwrap();
    let cuda_resident =
        CudaResidentImage::upload(Arc::clone(&context), Arc::clone(&resident), usize::MAX, 0)
            .unwrap();
    for algorithm in [
        CudaAlgorithm::Frontier,
        CudaAlgorithm::DeltaStepping(DeltaConfiguration::new(0.01).unwrap()),
    ] {
        let reserved =
            estimate_search_bytes(2, 1, resident.adjacency_count(), 1, algorithm).unwrap();
        let output = cuda_resident
            .route(&request, algorithm, &AtomicBool::new(false), reserved)
            .unwrap();
        assert_states_equal(expected.results(), output.response.results());
        assert!(output.diagnostics.kernel_launches > 0);
    }

    let bundle_path = directory.path().join("split-source");
    let scan = catalog.confirmed_graph_scan().unwrap();
    compile_bundle(
        &scan,
        &bundle_path,
        BundleConfig {
            target_partition_topology_bytes: 4 * 1024,
            hard_maximum_partition_topology_bytes: 4 * 1024,
            maximum_total_bundle_bytes: 4 * 1024 * 1024,
        },
    )
    .unwrap();
    drop(scan);
    let chunked = Arc::new(
        ChunkedRoutingImage::open(
            open_bundle(&bundle_path).unwrap(),
            HostCacheConfig {
                maximum_bytes: 16 * 1024,
                maximum_staging_bytes: 8 * 1024,
                maximum_entries: 2,
                io_worker_count: 2,
                maximum_queued_reads: 4,
            },
        )
        .unwrap(),
    );
    assert!(chunked.source_partition_ids(0).count() > 1);
    for reverse in [false, true] {
        for algorithm in [
            CudaAlgorithm::Frontier,
            CudaAlgorithm::DeltaStepping(DeltaConfiguration::new(0.01).unwrap()),
        ] {
            let partitioned = CudaPartitionedImage::upload(
                Arc::clone(&context),
                Arc::clone(&chunked),
                CudaPartitionedConfig {
                    maximum_topology_cache_bytes: 8 * 1024,
                    maximum_topology_cache_slots: 2,
                    maximum_host_staging_bytes: 4 * 1024,
                    minimum_free_memory_headroom: 0,
                    reserved_concurrent_search_bytes: 1024 * 1024,
                    reverse_partition_order: reverse,
                },
            )
            .unwrap();
            let reserved = estimate_search_bytes(
                2,
                1,
                partitioned.maximum_partition_adjacency_count(),
                1,
                algorithm,
            )
            .unwrap();
            let output = partitioned
                .route(&request, algorithm, &AtomicBool::new(false), reserved)
                .unwrap();
            assert_states_equal(expected.results(), output.response.results());
            assert!(output.diagnostics.partitions_required > 1);
        }
    }
}
