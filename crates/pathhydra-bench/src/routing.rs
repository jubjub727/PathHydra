use std::time::{Duration, Instant};

use pathhydra_routing::{DestinationState, RoutingResponse, route, route_partitioned};

use crate::{fixtures::Fixture, report::Measurement};

pub fn cpu<'a>(name: &'a str, fixture: &Fixture) -> (RoutingResponse, Measurement<'a>) {
    let expected = route(&fixture.image, &fixture.request).expect("CPU oracle");
    for _ in 0..3 {
        let _ = route(&fixture.image, &fixture.request).expect("CPU warmup");
    }
    let started = Instant::now();
    let actual = route(&fixture.image, &fixture.request).expect("CPU timed route");
    let elapsed = started.elapsed();
    (
        expected.clone(),
        Measurement {
            workload: name,
            executor: "cpu",
            algorithm: "dijkstra",
            nodes: fixture.image.node_count(),
            adjacencies: fixture.image.adjacency_count(),
            topology_bytes: fixture.image.manifest().byte_counts().total(),
            upload: Duration::ZERO,
            elapsed,
            correctness: same_distances(&expected, &actual),
            gpu_name: String::new(),
            compute_capability: String::new(),
            driver: 0,
            bundle_bytes: fixture.build_metrics.bundle_bytes,
            partition_count: fixture.chunked.partition_count(),
            cache_bytes: 0,
            cache_hits: 0,
            cache_misses: 0,
            file_bytes: 0,
            transfer_bytes: 0,
            cold: false,
        },
    )
}

pub fn partitioned_cpu_thrash<'a>(
    name: &'a str,
    fixture: &Fixture,
    expected: &RoutingResponse,
) -> Measurement<'a> {
    let image = pathhydra_routing::ChunkedRoutingImage::open(
        pathhydra_routing::open_bundle(&fixture.bundle_path).expect("thrash bundle reopen"),
        pathhydra_routing::HostCacheConfig {
            maximum_bytes: 16 * 1024,
            maximum_staging_bytes: 8 * 1024,
            maximum_entries: 1,
            io_worker_count: 1,
            maximum_queued_reads: 1,
        },
    )
    .expect("one-entry host cache");
    let before = image.cache_snapshot();
    let started = Instant::now();
    let actual = route_partitioned(&image, &fixture.request).expect("thrashing CPU route");
    let elapsed = started.elapsed();
    let after = image.cache_snapshot();
    Measurement {
        workload: name,
        executor: "cpu-partitioned-thrash",
        algorithm: "dijkstra",
        nodes: image.node_count(),
        adjacencies: image.adjacency_count(),
        topology_bytes: fixture.image.manifest().byte_counts().total(),
        upload: Duration::ZERO,
        elapsed,
        correctness: same_distances(expected, &actual),
        gpu_name: String::new(),
        compute_capability: String::new(),
        driver: 0,
        bundle_bytes: fixture.build_metrics.bundle_bytes,
        partition_count: image.partition_count(),
        cache_bytes: after.current_bytes,
        cache_hits: after.hits.saturating_sub(before.hits),
        cache_misses: after.misses.saturating_sub(before.misses),
        file_bytes: after.read_bytes.saturating_sub(before.read_bytes),
        transfer_bytes: 0,
        cold: true,
    }
}

pub fn partitioned_cpu<'a>(
    name: &'a str,
    fixture: &Fixture,
    expected: &RoutingResponse,
    cold: bool,
) -> Measurement<'a> {
    if !cold {
        let _ = route_partitioned(&fixture.chunked, &fixture.request).expect("CPU cache warmup");
    }
    let before = fixture.chunked.cache_snapshot();
    let started = Instant::now();
    let actual = route_partitioned(&fixture.chunked, &fixture.request).expect("partitioned CPU");
    let elapsed = started.elapsed();
    let after = fixture.chunked.cache_snapshot();
    Measurement {
        workload: name,
        executor: "cpu-partitioned",
        algorithm: "dijkstra",
        nodes: fixture.chunked.node_count(),
        adjacencies: fixture.chunked.adjacency_count(),
        topology_bytes: fixture.image.manifest().byte_counts().total(),
        upload: Duration::ZERO,
        elapsed,
        correctness: same_distances(expected, &actual),
        gpu_name: String::new(),
        compute_capability: String::new(),
        driver: 0,
        bundle_bytes: fixture.build_metrics.bundle_bytes,
        partition_count: fixture.chunked.partition_count(),
        cache_bytes: after.current_bytes,
        cache_hits: after.hits.saturating_sub(before.hits),
        cache_misses: after.misses.saturating_sub(before.misses),
        file_bytes: after.read_bytes.saturating_sub(before.read_bytes),
        transfer_bytes: 0,
        cold,
    }
}

#[cfg(feature = "cuda")]
pub fn cuda<'a>(
    name: &'a str,
    fixture: &Fixture,
    expected: &RoutingResponse,
    context: std::sync::Arc<pathhydra_cuda::CudaContextOwner>,
    algorithm: pathhydra_cuda::CudaAlgorithm,
) -> Measurement<'a> {
    let upload_started = Instant::now();
    let resident = pathhydra_cuda::CudaResidentImage::upload(
        context.clone(),
        fixture.image.clone(),
        usize::MAX,
        0,
    )
    .expect("CUDA residency");
    let upload = upload_started.elapsed();
    let reserved = pathhydra_cuda::estimate_search_bytes(
        fixture.image.node_count(),
        fixture.image.relation_kind_count(),
        fixture.image.adjacency_count(),
        fixture.request.destinations().len(),
        algorithm,
    )
    .expect("CUDA memory estimate");
    for _ in 0..3 {
        resident
            .route(
                &fixture.request,
                algorithm,
                &std::sync::atomic::AtomicBool::new(false),
                reserved,
            )
            .expect("CUDA warmup");
    }
    let started = Instant::now();
    let output = resident
        .route(
            &fixture.request,
            algorithm,
            &std::sync::atomic::AtomicBool::new(false),
            reserved,
        )
        .expect("CUDA timed route");
    let elapsed = started.elapsed();
    let capabilities = context.capabilities();
    let compute = format!(
        "{}.{}",
        capabilities.compute_capability.0, capabilities.compute_capability.1
    );
    Measurement {
        workload: name,
        executor: "cuda",
        algorithm: match algorithm {
            pathhydra_cuda::CudaAlgorithm::Frontier => "frontier",
            pathhydra_cuda::CudaAlgorithm::DeltaStepping(_) => "delta-stepping",
        },
        nodes: fixture.image.node_count(),
        adjacencies: fixture.image.adjacency_count(),
        topology_bytes: resident.allocated_bytes(),
        upload,
        elapsed,
        correctness: same_distances(expected, &output.response),
        gpu_name: capabilities.device_name.clone(),
        compute_capability: compute,
        driver: capabilities.driver_version,
        bundle_bytes: fixture.build_metrics.bundle_bytes,
        partition_count: fixture.chunked.partition_count(),
        cache_bytes: 0,
        cache_hits: 0,
        cache_misses: 0,
        file_bytes: 0,
        transfer_bytes: resident.allocated_bytes() as u64,
        cold: false,
    }
}

#[cfg(feature = "cuda")]
pub fn partitioned_cuda<'a>(
    name: &'a str,
    fixture: &Fixture,
    expected: &RoutingResponse,
    context: std::sync::Arc<pathhydra_cuda::CudaContextOwner>,
    algorithm: pathhydra_cuda::CudaAlgorithm,
) -> Measurement<'a> {
    let chunked = std::sync::Arc::new(
        pathhydra_routing::ChunkedRoutingImage::open(
            pathhydra_routing::open_bundle(&fixture.bundle_path)
                .expect("partitioned CUDA bundle reopen"),
            pathhydra_routing::HostCacheConfig {
                maximum_bytes: 128 * 1024,
                maximum_staging_bytes: 8 * 1024,
                maximum_entries: 8,
                io_worker_count: 2,
                maximum_queued_reads: 8,
            },
        )
        .expect("partitioned CUDA host cache"),
    );
    let upload_started = Instant::now();
    let image = pathhydra_cuda::CudaPartitionedImage::upload(
        context.clone(),
        chunked.clone(),
        pathhydra_cuda::CudaPartitionedConfig {
            maximum_topology_cache_bytes: 16 * 1024,
            maximum_topology_cache_slots: 2,
            maximum_host_staging_bytes: 8 * 1024,
            minimum_free_memory_headroom: 0,
            reserved_concurrent_search_bytes: 64 * 1024 * 1024,
            reverse_partition_order: false,
        },
    )
    .expect("partitioned CUDA image");
    let upload = upload_started.elapsed();
    let reserved = pathhydra_cuda::estimate_search_bytes(
        chunked.node_count(),
        chunked.relation_kind_count(),
        chunked.adjacency_count(),
        fixture.request.destinations().len(),
        algorithm,
    )
    .expect("partitioned CUDA memory estimate");
    let started = Instant::now();
    let output = image
        .route(
            &fixture.request,
            algorithm,
            &std::sync::atomic::AtomicBool::new(false),
            reserved,
        )
        .expect("partitioned CUDA timed route");
    let elapsed = started.elapsed();
    let cache = image.topology_cache_snapshot();
    let capabilities = context.capabilities();
    Measurement {
        workload: name,
        executor: "cuda-partitioned",
        algorithm: match algorithm {
            pathhydra_cuda::CudaAlgorithm::Frontier => "frontier",
            pathhydra_cuda::CudaAlgorithm::DeltaStepping(_) => "delta-stepping",
        },
        nodes: chunked.node_count(),
        adjacencies: chunked.adjacency_count(),
        topology_bytes: fixture.image.manifest().byte_counts().total(),
        upload,
        elapsed,
        correctness: same_distances(expected, &output.response),
        gpu_name: capabilities.device_name.clone(),
        compute_capability: format!(
            "{}.{}",
            capabilities.compute_capability.0, capabilities.compute_capability.1
        ),
        driver: capabilities.driver_version,
        bundle_bytes: fixture.build_metrics.bundle_bytes,
        partition_count: fixture.chunked.partition_count(),
        cache_bytes: cache.current_bytes as u64,
        cache_hits: cache.hits,
        cache_misses: cache.misses,
        file_bytes: output.diagnostics.file_bytes,
        transfer_bytes: output.diagnostics.transfer_bytes,
        cold: true,
    }
}

pub(crate) fn same_distances(expected: &RoutingResponse, actual: &RoutingResponse) -> bool {
    expected
        .results()
        .iter()
        .zip(actual.results())
        .all(|(left, right)| {
            left.destination() == right.destination()
                && match (left.state(), right.state()) {
                    (DestinationState::Exact(left), DestinationState::Exact(right)) => {
                        left.logical_distance().to_bits() == right.logical_distance().to_bits()
                    }
                    (left, right) => left == right,
                }
        })
}
