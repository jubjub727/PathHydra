use std::fs;

use pathhydra_core::ConfirmedRecord;
use pathhydra_routing::{
    BundleConfig, ChunkedRoutingImage, HostCacheConfig, RelationMultiplier, RelationProfile,
    RelationUse, RoutingImage, RoutingRequest, SearchBudget, TiePolicy, compile_bundle,
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
    let bundle = open_bundle(&temporary.path().join("bundle-a")).unwrap();
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
