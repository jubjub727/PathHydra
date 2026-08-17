use std::fs;
use std::sync::{Arc, Barrier, atomic::AtomicBool};
use std::time::Duration;

use pathhydra_core::ConfirmedRecord;
use pathhydra_routing::{
    AnalyticParallelBundleConfig, BundleConfig, ChunkedRoutingImage, DestinationState,
    HostCacheConfig, RelationMultiplier, RelationProfile, RelationUse, RoutingImage,
    RoutingRequest, SearchBudget, TiePolicy, compile_bundle, generate_analytic_parallel_bundle,
    open_bundle, route, route_partitioned,
};
use pathhydra_store::Catalog;

fn confirm_node(catalog: &Catalog, name: &str) -> pathhydra_core::NodeRecord {
    let id = catalog.insert_node_candidate(name).unwrap();
    let ConfirmedRecord::Node(record) = catalog.confirm_validated_candidate(id).unwrap() else {
        panic!()
    };
    record
}

#[test]
fn deterministic_chunked_bundle_reconstructs_and_routes_exactly() {
    let temporary = tempfile::tempdir().unwrap();
    let catalog = Catalog::open(temporary.path().join("db")).unwrap();
    let source = confirm_node(&catalog, "source");
    let destinations: Vec<_> = (0..9)
        .map(|i| confirm_node(&catalog, &format!("node-{i}")))
        .collect();
    let candidate = catalog.insert_relation_candidate("kind").unwrap();
    let ConfirmedRecord::Relation(kind) = catalog.confirm_validated_candidate(candidate).unwrap()
    else {
        panic!()
    };
    for (index, destination) in destinations.iter().enumerate() {
        let edge = catalog
            .insert_edge_candidate(
                source.id(),
                destination.id(),
                kind.id(),
                (index as f32 + 1.0) / 10.0,
            )
            .unwrap();
        catalog.confirm_validated_candidate(edge).unwrap();
    }
    let config = BundleConfig {
        target_partition_topology_bytes: 48,
        hard_maximum_partition_topology_bytes: 48,
        maximum_total_bundle_bytes: 1_000_000,
    };
    let scan = catalog.confirmed_graph_scan().unwrap();
    let (first, metrics) =
        compile_bundle(&scan, &temporary.path().join("bundle-a"), config).unwrap();
    assert_eq!(metrics.partition_count, 9);
    assert_eq!(metrics.split_source_count, 1);
    let (second, _) = compile_bundle(&scan, &temporary.path().join("bundle-b"), config).unwrap();
    assert_eq!(first, second);
    for name in [
        "manifest.bin",
        "identities.bin",
        "source-directory.bin",
        "topology.bin",
        "evidence.bin",
    ] {
        assert_eq!(
            fs::read(temporary.path().join("bundle-a").join(name)).unwrap(),
            fs::read(temporary.path().join("bundle-b").join(name)).unwrap()
        );
    }
    drop(scan);
    let bundle_path = temporary.path().join("bundle-a");
    for configure in [
        (|config: &mut HostCacheConfig| config.maximum_bytes = 0) as fn(&mut HostCacheConfig),
        (|config: &mut HostCacheConfig| config.maximum_staging_bytes = 0)
            as fn(&mut HostCacheConfig),
        (|config: &mut HostCacheConfig| config.maximum_entries = 0) as fn(&mut HostCacheConfig),
        (|config: &mut HostCacheConfig| config.io_worker_count = 0) as fn(&mut HostCacheConfig),
        (|config: &mut HostCacheConfig| config.maximum_queued_reads = 0)
            as fn(&mut HostCacheConfig),
    ] {
        let mut invalid = HostCacheConfig::default();
        configure(&mut invalid);
        assert!(
            ChunkedRoutingImage::open(open_bundle(&bundle_path).unwrap(), invalid).is_err(),
            "zero host-cache resource limit was accepted"
        );
    }
    let bundle = open_bundle(&bundle_path).unwrap();
    let resident = bundle.to_resident_image().unwrap();
    let ordinary = RoutingImage::compile(&catalog.confirmed_graph_records().unwrap()).unwrap();
    assert_eq!(resident.arrays().offsets, ordinary.arrays().offsets);
    assert_eq!(
        resident.arrays().destinations,
        ordinary.arrays().destinations
    );
    assert_eq!(
        resident.arrays().relation_indexes,
        ordinary.arrays().relation_indexes
    );
    assert_eq!(
        resident.arrays().base_weight_bits,
        ordinary.arrays().base_weight_bits
    );
    let chunked = ChunkedRoutingImage::open(
        bundle,
        HostCacheConfig {
            maximum_bytes: 112,
            maximum_staging_bytes: 56,
            maximum_entries: 1,
            ..HostCacheConfig::default()
        },
    )
    .unwrap();
    let request = RoutingRequest::new(
        source.id(),
        destinations
            .iter()
            .rev()
            .map(|n| n.id())
            .chain([source.id()]),
        RelationProfile::new([(
            kind.id(),
            RelationUse::Enabled(RelationMultiplier::new(1.0).unwrap()),
        )]),
        true,
        SearchBudget::Unlimited,
        TiePolicy::StablePredecessor,
    );
    assert_eq!(
        route(&ordinary, &request).unwrap(),
        route_partitioned(&chunked, &request).unwrap()
    );
    assert!(chunked.cache_snapshot().evictions > 0);
    let cancellation = AtomicBool::new(false);
    let pinned = chunked
        .acquire_partition(0, &cancellation)
        .unwrap()
        .unwrap();
    assert!(matches!(
        chunked.acquire_partition(1, &cancellation),
        Err(pathhydra_routing::RoutingError::ImageAccess(reason))
            if reason.contains("pinned or loading")
    ));
    drop(pinned);
}

#[test]
fn io_loads_coalesce_and_faults_cancellation_and_shutdown_are_typed() {
    let temporary = tempfile::tempdir().unwrap();
    let catalog = Catalog::open(temporary.path().join("db")).unwrap();
    let source = confirm_node(&catalog, "source");
    let destination = confirm_node(&catalog, "destination");
    let relation_candidate = catalog.insert_relation_candidate("kind").unwrap();
    let ConfirmedRecord::Relation(relation) = catalog
        .confirm_validated_candidate(relation_candidate)
        .unwrap()
    else {
        panic!()
    };
    let edge = catalog
        .insert_edge_candidate(source.id(), destination.id(), relation.id(), 0.5)
        .unwrap();
    catalog.confirm_validated_candidate(edge).unwrap();
    let scan = catalog.confirmed_graph_scan().unwrap();
    compile_bundle(
        &scan,
        &temporary.path().join("bundle"),
        BundleConfig {
            target_partition_topology_bytes: 48,
            hard_maximum_partition_topology_bytes: 48,
            maximum_total_bundle_bytes: 1_000_000,
        },
    )
    .unwrap();
    drop(scan);
    let request = Arc::new(RoutingRequest::new(
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

    let bundle = open_bundle(&temporary.path().join("bundle")).unwrap();
    bundle
        .fault_injection()
        .delay_reads(Duration::from_millis(100));
    let image = Arc::new(
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
    let barrier = Arc::new(Barrier::new(3));
    let workers: Vec<_> = (0..2)
        .map(|_| {
            let image = Arc::clone(&image);
            let request = Arc::clone(&request);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                route_partitioned(&image, &request).unwrap()
            })
        })
        .collect();
    barrier.wait();
    for worker in workers {
        assert!(matches!(
            worker.join().unwrap().results()[0].state(),
            DestinationState::Exact(_)
        ));
    }
    let snapshot = image.cache_snapshot();
    assert_eq!(snapshot.misses, 1);
    assert!(snapshot.coalesced_waits > 0);
    assert_eq!(snapshot.queue_high_water, 1);

    let bundle = open_bundle(&temporary.path().join("bundle")).unwrap();
    bundle
        .fault_injection()
        .delay_reads(Duration::from_millis(100));
    let image = Arc::new(
        ChunkedRoutingImage::open(
            bundle,
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
    let first = {
        let image = Arc::clone(&image);
        let request = Arc::clone(&request);
        std::thread::spawn(move || route_partitioned(&image, &request).unwrap())
    };
    std::thread::sleep(Duration::from_millis(10));
    let cancelled = AtomicBool::new(true);
    let (response, _, _) =
        pathhydra_routing::route_partitioned_controlled(&image, &request, &cancelled).unwrap();
    assert_eq!(
        response.completion_reason(),
        pathhydra_routing::CompletionReason::Cancelled
    );
    first.join().unwrap();

    for fault in ["short", "error"] {
        let bundle = open_bundle(&temporary.path().join("bundle")).unwrap();
        let hooks = bundle.fault_injection();
        if fault == "short" {
            hooks.short_read_at(0)
        } else {
            hooks.read_error_at(0)
        }
        let image = ChunkedRoutingImage::open(
            bundle,
            HostCacheConfig {
                maximum_bytes: 112,
                maximum_staging_bytes: 56,
                maximum_entries: 1,
                io_worker_count: 1,
                maximum_queued_reads: 1,
            },
        )
        .unwrap();
        assert!(matches!(
            route_partitioned(&image, &request),
            Err(pathhydra_routing::RoutingError::ImageAccess(_))
        ));
    }
    let bundle = open_bundle(&temporary.path().join("bundle")).unwrap();
    let image = ChunkedRoutingImage::open(
        bundle,
        HostCacheConfig {
            maximum_bytes: 112,
            maximum_staging_bytes: 56,
            maximum_entries: 1,
            io_worker_count: 1,
            maximum_queued_reads: 1,
        },
    )
    .unwrap();
    image.shutdown_io_workers().unwrap();
    assert!(matches!(
        route_partitioned(&image, &request),
        Err(pathhydra_routing::RoutingError::ImageAccess(_))
    ));
}

#[test]
fn empty_graph_has_canonical_empty_topology_and_corruption_is_rejected() {
    let temporary = tempfile::tempdir().unwrap();
    let catalog = Catalog::open(temporary.path().join("db")).unwrap();
    let scan = catalog.confirmed_graph_scan().unwrap();
    compile_bundle(
        &scan,
        &temporary.path().join("bundle"),
        BundleConfig::default(),
    )
    .unwrap();
    drop(scan);
    let bundle = open_bundle(&temporary.path().join("bundle")).unwrap();
    assert_eq!(bundle.manifest().adjacency_count, 0);
    assert!(
        fs::read(temporary.path().join("bundle/topology.bin"))
            .unwrap()
            .is_empty()
    );
    let mut manifest = fs::read(temporary.path().join("bundle/manifest.bin")).unwrap();
    manifest[0] ^= 1;
    fs::write(temporary.path().join("bundle/manifest.bin"), manifest).unwrap();
    assert!(open_bundle(&temporary.path().join("bundle")).is_err());
}

#[test]
fn corruption_in_every_bundle_file_and_partition_region_is_rejected() {
    let temporary = tempfile::tempdir().unwrap();
    let catalog = Catalog::open(temporary.path().join("db")).unwrap();
    let source = confirm_node(&catalog, "source");
    let destination = confirm_node(&catalog, "destination");
    let relation_candidate = catalog.insert_relation_candidate("kind").unwrap();
    let ConfirmedRecord::Relation(relation) = catalog
        .confirm_validated_candidate(relation_candidate)
        .unwrap()
    else {
        panic!()
    };
    let edge = catalog
        .insert_edge_candidate(source.id(), destination.id(), relation.id(), 1.0)
        .unwrap();
    catalog.confirm_validated_candidate(edge).unwrap();
    let scan = catalog.confirmed_graph_scan().unwrap();
    let original = temporary.path().join("original");
    compile_bundle(
        &scan,
        &original,
        BundleConfig {
            target_partition_topology_bytes: 48,
            hard_maximum_partition_topology_bytes: 48,
            maximum_total_bundle_bytes: 1_000_000,
        },
    )
    .unwrap();
    drop(scan);
    for (index, name) in [
        "manifest.bin",
        "identities.bin",
        "source-directory.bin",
        "topology.bin",
        "evidence.bin",
    ]
    .into_iter()
    .enumerate()
    {
        let corrupted = temporary.path().join(format!("corrupted-{index}"));
        fs::create_dir(&corrupted).unwrap();
        for file in [
            "manifest.bin",
            "identities.bin",
            "source-directory.bin",
            "topology.bin",
            "evidence.bin",
        ] {
            fs::copy(original.join(file), corrupted.join(file)).unwrap();
        }
        let path = corrupted.join(name);
        let mut bytes = fs::read(&path).unwrap();
        let middle = bytes.len() / 2;
        bytes[middle] ^= 0x80;
        fs::write(path, bytes).unwrap();
        assert!(open_bundle(&corrupted).is_err(), "corruption in {name}");
    }
}

#[test]
fn analytic_scale_generator_uses_production_reader_and_exact_route() {
    let temporary = tempfile::tempdir().unwrap();
    let bundle_path = temporary.path().join("analytic");
    let (_, metrics) = generate_analytic_parallel_bundle(
        &bundle_path,
        AnalyticParallelBundleConfig {
            adjacency_count: 1_000,
            destination_count: 64,
            bundle: BundleConfig {
                target_partition_topology_bytes: 48,
                hard_maximum_partition_topology_bytes: 48,
                maximum_total_bundle_bytes: 300_000,
            },
        },
    )
    .unwrap();
    assert_eq!(metrics.adjacency_count, 1_000);
    assert_eq!(metrics.peak_partition_buffer_bytes, 56);
    let image = ChunkedRoutingImage::open(
        open_bundle(&bundle_path).unwrap(),
        HostCacheConfig {
            maximum_bytes: 112,
            maximum_staging_bytes: 56,
            maximum_entries: 1,
            io_worker_count: 2,
            maximum_queued_reads: 2,
        },
    )
    .unwrap();
    let request = RoutingRequest::new(
        pathhydra_core::NodeId::from_u64(1),
        [pathhydra_core::NodeId::from_u64(2)],
        RelationProfile::new([(
            pathhydra_core::RelationId::from_u64(1),
            RelationUse::Enabled(RelationMultiplier::new(1.0).unwrap()),
        )]),
        false,
        SearchBudget::Unlimited,
        TiePolicy::StablePredecessor,
    );
    assert!(matches!(
        route_partitioned(&image, &request).unwrap().results()[0].state(),
        DestinationState::Exact(exact) if exact.logical_distance().to_bits() == 1.0_f64.to_bits()
    ));
}
