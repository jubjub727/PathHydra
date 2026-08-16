use std::{path::Path, sync::Arc, time::Instant};

use pathhydra_core::{NodeId, RelationId};
use pathhydra_routing::{
    AnalyticParallelBundleConfig, BundleConfig, ChunkedRoutingImage, DestinationState,
    HostCacheConfig, RelationMultiplier, RelationProfile, RelationUse, RoutingRequest,
    SearchBudget, TiePolicy, generate_analytic_parallel_bundle, open_bundle, route_partitioned,
};

pub fn run(directory: &Path, target_gib: u64) {
    let target_bytes = target_gib
        .checked_mul(1024 * 1024 * 1024)
        .expect("scale target overflow");
    let adjacency_count = target_bytes.div_ceil(20).saturating_add(1);
    let (build, generated_metrics) = if directory.join("manifest.bin").exists() {
        (std::time::Duration::ZERO, None)
    } else {
        let build_started = Instant::now();
        let (_, metrics) = generate_analytic_parallel_bundle(
            directory,
            AnalyticParallelBundleConfig {
                adjacency_count,
                bundle: BundleConfig {
                    target_partition_topology_bytes: 8 * 1024 * 1024,
                    hard_maximum_partition_topology_bytes: 8 * 1024 * 1024,
                    maximum_total_bundle_bytes: target_bytes.saturating_add(1024 * 1024 * 1024),
                },
            },
        )
        .expect("analytic scale bundle generation and validation");
        (build_started.elapsed(), Some(metrics))
    };
    let reopen_started = Instant::now();
    let bundle = open_bundle(directory).expect("scale bundle reopen");
    let reopen = reopen_started.elapsed();
    let metrics = generated_metrics.unwrap_or_else(|| {
        let snapshot = bundle.snapshot();
        pathhydra_routing::BundleBuildMetrics {
            node_count: 2,
            relation_kind_count: 1,
            adjacency_count,
            segment_count: snapshot.segment_count as u64,
            partition_count: snapshot.partition_count as u64,
            split_source_count: snapshot.split_source_count as u64,
            peak_partition_buffer_bytes: snapshot.largest_partition_bytes,
            decoded_record_bytes: 0,
            bundle_bytes: snapshot.total_bytes,
            scan_duration: std::time::Duration::ZERO,
            write_duration: std::time::Duration::ZERO,
            validation_duration: reopen,
            total_duration: reopen,
        }
    });
    assert!(metrics.bundle_bytes > target_bytes);
    let image = Arc::new(
        ChunkedRoutingImage::open(
            bundle,
            HostCacheConfig {
                maximum_bytes: 64 * 1024 * 1024,
                maximum_staging_bytes: 32 * 1024 * 1024,
                maximum_entries: 2,
                io_worker_count: 2,
                maximum_queued_reads: 4,
            },
        )
        .expect("bounded scale host cache"),
    );
    let request = RoutingRequest::new(
        NodeId::from_u64(1),
        [NodeId::from_u64(2)],
        RelationProfile::new([(
            RelationId::from_u64(1),
            RelationUse::Enabled(RelationMultiplier::new(1.0).unwrap()),
        )]),
        false,
        SearchBudget::Unlimited,
        TiePolicy::StablePredecessor,
    );
    let cpu_started = Instant::now();
    let cpu = route_partitioned(&image, &request).expect("scale partitioned CPU route");
    let cpu_elapsed = cpu_started.elapsed();
    assert!(matches!(
        cpu.results()[0].state(),
        DestinationState::Exact(exact) if exact.logical_distance().to_bits() == 1.0_f64.to_bits()
    ));
    let host = image.cache_snapshot();
    println!(
        "scale-build,writer,analytic,2,{},{},{},{},{},{},{},{},{},true,{},{},true,,,,1.95.0,nightly-2024-02-17,sm_86",
        adjacency_count,
        metrics.bundle_bytes,
        metrics.bundle_bytes,
        metrics.partition_count,
        host.capacity_bytes,
        host.hits,
        host.misses,
        host.read_bytes,
        0,
        build.as_micros(),
        reopen.as_micros(),
    );
    println!(
        "scale-route,cpu-partitioned,dijkstra,2,{},{},{},{},{},{},{},{},{},true,0,{},true,,,,1.95.0,nightly-2024-02-17,sm_86",
        adjacency_count,
        metrics.bundle_bytes,
        metrics.bundle_bytes,
        metrics.partition_count,
        host.current_bytes,
        host.hits,
        host.misses,
        host.read_bytes,
        0,
        cpu_elapsed.as_micros(),
    );

    #[cfg(feature = "cuda")]
    run_cuda(image, &request, adjacency_count, &metrics);
}

#[cfg(feature = "cuda")]
fn run_cuda(
    image: Arc<ChunkedRoutingImage>,
    request: &RoutingRequest,
    adjacency_count: u64,
    metrics: &pathhydra_routing::BundleBuildMetrics,
) {
    let context = pathhydra_cuda::CudaContextOwner::initialize(0).expect("CUDA context");
    let cuda = pathhydra_cuda::CudaPartitionedImage::upload(
        Arc::clone(&context),
        Arc::clone(&image),
        pathhydra_cuda::CudaPartitionedConfig {
            maximum_topology_cache_bytes: 32 * 1024 * 1024,
            maximum_topology_cache_slots: 2,
            maximum_host_staging_bytes: 16 * 1024 * 1024,
            minimum_free_memory_headroom: 512 * 1024 * 1024,
            reserved_concurrent_search_bytes: 1024 * 1024,
            reverse_partition_order: false,
        },
    )
    .expect("partitioned CUDA scale image");
    let reserved = pathhydra_cuda::estimate_search_bytes(
        image.node_count(),
        image.relation_kind_count(),
        image.adjacency_count(),
        request.destinations().len(),
        pathhydra_cuda::CudaAlgorithm::Frontier,
    )
    .unwrap();
    let started = Instant::now();
    let output = cuda
        .route(
            request,
            pathhydra_cuda::CudaAlgorithm::Frontier,
            &std::sync::atomic::AtomicBool::new(false),
            reserved,
        )
        .expect("partitioned CUDA scale route");
    let elapsed = started.elapsed();
    assert!(matches!(
        output.response.results()[0].state(),
        DestinationState::Exact(exact) if exact.logical_distance().to_bits() == 1.0_f64.to_bits()
    ));
    let cache = cuda.topology_cache_snapshot();
    let capabilities = context.capabilities();
    println!(
        "scale-route,cuda-partitioned,frontier,2,{},{},{},{},{},{},{},{},{},true,0,{},true,{},{}.{},{},1.95.0,nightly-2024-02-17,sm_86",
        adjacency_count,
        metrics.bundle_bytes,
        metrics.bundle_bytes,
        metrics.partition_count,
        cache.current_bytes,
        cache.hits,
        cache.misses,
        output.diagnostics.file_bytes,
        output.diagnostics.transfer_bytes,
        elapsed.as_micros(),
        capabilities.device_name,
        capabilities.compute_capability.0,
        capabilities.compute_capability.1,
        capabilities.driver_version,
    );
}
