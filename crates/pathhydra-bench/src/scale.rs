use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use pathhydra_core::{NodeId, RelationId};
use pathhydra_routing::{
    AnalyticParallelBundleConfig, BundleConfig, ChunkedRoutingImage, CompletionReason,
    DestinationState, HostCacheConfig, HostCacheSnapshot, RelationMultiplier, RelationProfile,
    RelationUse, RoutingRequest, SearchBudget, TiePolicy, generate_analytic_parallel_bundle,
    open_bundle, route_partitioned_controlled,
};

fn wait_for_host_cache_idle(image: &ChunkedRoutingImage) -> HostCacheSnapshot {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let snapshot = image.cache_snapshot();
        if snapshot.pinned_entries == 0
            && snapshot.queue_depth == 0
            && snapshot.staging_current_bytes == 0
        {
            return snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "partition cache did not drain after cancellation: {snapshot:?}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[derive(Default)]
struct ScaleRow<'a> {
    event: &'a str,
    executor: &'a str,
    algorithm: &'a str,
    cold: bool,
    nodes: u64,
    adjacencies: u64,
    bundle_bytes: u64,
    topology_bytes: u64,
    target_topology_bytes: u64,
    disk_bytes: u64,
    partitions: u64,
    generation_us: u128,
    open_us: u128,
    restart_open_us: u128,
    route_us: u128,
    total_us: u128,
    host_cache_current: u64,
    host_cache_high_water: u64,
    host_staging_high_water: u64,
    host_cache_hits: u64,
    host_cache_misses: u64,
    host_cache_evictions: u64,
    partitions_read: u64,
    file_bytes: u64,
    device_cache_current: u64,
    device_cache_high_water: u64,
    device_cache_hits: u64,
    device_cache_misses: u64,
    device_cache_evictions: u64,
    device_cache_transfer_bytes: u64,
    route_transfer_bytes: u64,
    reserved_search_bytes: u64,
    peak_host_bytes: u64,
    device_total_bytes: u64,
    observed_device_used_bytes: u64,
    state_initialization_us: u128,
    partition_scheduling_us: u128,
    frontier_compaction_us: u128,
    relation_relaxation_us: u128,
    response_transfer_us: u128,
    completion: &'a str,
    correct: bool,
    rebuilt: bool,
    gpu: String,
    compute_capability: String,
    driver: String,
}

impl ScaleRow<'_> {
    fn write(&self) {
        let fields = vec![
            self.event.to_owned(),
            self.executor.to_owned(),
            self.algorithm.to_owned(),
            self.cold.to_string(),
            self.nodes.to_string(),
            self.adjacencies.to_string(),
            self.bundle_bytes.to_string(),
            self.topology_bytes.to_string(),
            self.target_topology_bytes.to_string(),
            self.disk_bytes.to_string(),
            self.partitions.to_string(),
            self.generation_us.to_string(),
            self.open_us.to_string(),
            self.restart_open_us.to_string(),
            self.route_us.to_string(),
            self.total_us.to_string(),
            self.host_cache_current.to_string(),
            self.host_cache_high_water.to_string(),
            self.host_staging_high_water.to_string(),
            self.host_cache_hits.to_string(),
            self.host_cache_misses.to_string(),
            self.host_cache_evictions.to_string(),
            self.partitions_read.to_string(),
            self.file_bytes.to_string(),
            self.device_cache_current.to_string(),
            self.device_cache_high_water.to_string(),
            self.device_cache_hits.to_string(),
            self.device_cache_misses.to_string(),
            self.device_cache_evictions.to_string(),
            self.device_cache_transfer_bytes.to_string(),
            self.route_transfer_bytes.to_string(),
            self.reserved_search_bytes.to_string(),
            self.peak_host_bytes.to_string(),
            self.device_total_bytes.to_string(),
            self.observed_device_used_bytes.to_string(),
            self.state_initialization_us.to_string(),
            self.partition_scheduling_us.to_string(),
            self.frontier_compaction_us.to_string(),
            self.relation_relaxation_us.to_string(),
            self.response_transfer_us.to_string(),
            self.completion.to_owned(),
            self.correct.to_string(),
            self.rebuilt.to_string(),
            self.gpu.clone(),
            self.compute_capability.clone(),
            self.driver.clone(),
            "analytic-parallel-fanout".to_owned(),
            "closed-form-no-rng".to_owned(),
            "partition-target-bytes=8388608;partition-hard-max-bytes=8388608;host-cache-bytes=67108864;host-staging-bytes=33554432;host-cache-entries=2;io-workers=2;queued-reads=4;device-cache-bytes=33554432;device-cache-slots=2;device-staging-bytes=16777216;device-headroom-bytes=536870912;concurrent-search-reserve-bytes=268435456"
                .to_owned(),
            std::env::consts::OS.to_owned(),
            std::env::consts::ARCH.to_owned(),
            std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_else(|_| "unknown".to_owned()),
            "local-filesystem-type-undiscovered".to_owned(),
            crate::system::rustc_version(),
            "nightly-2024-02-17;ptx-sm_86".to_owned(),
        ];
        println!(
            "{}",
            fields
                .into_iter()
                .map(csv_escape)
                .collect::<Vec<_>>()
                .join(",")
        );
    }
}

fn csv_escape(value: String) -> String {
    if value
        .chars()
        .any(|character| matches!(character, ',' | '"' | '\n' | '\r'))
    {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value
    }
}

pub fn header() {
    println!(
        "event,executor,algorithm,cold,nodes,adjacencies,bundle_bytes,topology_bytes,target_topology_bytes,disk_bytes,partitions,generation_us,open_us,restart_open_us,route_us,total_us,host_cache_current,host_cache_high_water,host_staging_high_water,host_cache_hits,host_cache_misses,host_cache_evictions,partitions_read,file_bytes,device_cache_current,device_cache_high_water,device_cache_hits,device_cache_misses,device_cache_evictions,device_cache_transfer_bytes,route_transfer_bytes,reserved_search_bytes,peak_host_bytes,device_total_bytes,observed_device_used_bytes,state_initialization_us,partition_scheduling_us,frontier_compaction_us,relation_relaxation_us,response_transfer_us,completion,correct,rebuilt,gpu,compute_capability,driver,dataset,seed,configuration,os,architecture,cpu,storage,rust_toolchain,kernel_toolchain"
    );
}

pub fn run(directory: &Path, target_gib: u64) {
    let total_started = Instant::now();
    let target_bytes = target_gib
        .checked_mul(1024 * 1024 * 1024)
        .expect("scale target overflow");
    // The target applies to the CUDA-consumed topology arrays (12 bytes per
    // adjacency), not to stable-edge evidence that stays on the CPU. This
    // makes `12` a literal topology-larger-than-10-GiB-device proof.
    let adjacency_count = target_bytes.div_ceil(12).saturating_add(1);
    let destination_count =
        u32::try_from(adjacency_count.min(1_048_576)).expect("bounded analytic destination count");
    let generated = !directory.join("manifest.bin").exists();
    let (generation, generated_metrics) = if generated {
        let started = Instant::now();
        let (_, metrics) = generate_analytic_parallel_bundle(
            directory,
            AnalyticParallelBundleConfig {
                adjacency_count,
                destination_count,
                bundle: BundleConfig {
                    target_partition_topology_bytes: 8 * 1024 * 1024,
                    hard_maximum_partition_topology_bytes: 8 * 1024 * 1024,
                    maximum_total_bundle_bytes: target_bytes
                        .saturating_mul(2)
                        .saturating_add(1024 * 1024 * 1024),
                },
            },
        )
        .expect("analytic scale bundle generation and validation");
        (started.elapsed(), Some(metrics))
    } else {
        (Duration::ZERO, None)
    };

    let open_started = Instant::now();
    let bundle = open_bundle(directory).expect("scale bundle open and validation");
    let open = open_started.elapsed();
    assert_eq!(
        bundle.manifest().adjacency_count,
        adjacency_count,
        "an existing scale bundle does not match the requested target; use a fresh directory"
    );
    let topology_bytes = bundle.snapshot().topology_bytes;
    let metrics =
        generated_metrics.unwrap_or_else(|| metrics_from_bundle(&bundle, adjacency_count, open));
    assert!(metrics.bundle_bytes > target_bytes);
    assert!(topology_bytes > target_bytes);
    let disk_bytes = directory_bytes(directory);
    assert!(disk_bytes >= metrics.bundle_bytes);
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
    let request = analytic_request();

    ScaleRow {
        event: "scale-build-open",
        executor: "bundle",
        algorithm: "analytic-generate-validate",
        nodes: metrics.node_count,
        adjacencies: adjacency_count,
        bundle_bytes: metrics.bundle_bytes,
        topology_bytes,
        target_topology_bytes: target_bytes,
        disk_bytes,
        partitions: metrics.partition_count,
        generation_us: generation.as_micros(),
        open_us: open.as_micros(),
        total_us: total_started.elapsed().as_micros(),
        peak_host_bytes: crate::system::peak_memory_bytes().unwrap_or(0),
        completion: "bundle-valid",
        correct: true,
        rebuilt: generated,
        ..ScaleRow::default()
    }
    .write();

    let cold_before = image.cache_snapshot();
    let cold_started = Instant::now();
    let (cold, _, cold_partitioned) =
        route_partitioned_controlled(&image, &request, &AtomicBool::new(false))
            .expect("cold scale CPU route");
    let cold_elapsed = cold_started.elapsed();
    assert_exact(&cold);
    let cold_after = image.cache_snapshot();
    cpu_row(
        "scale-route-cold",
        true,
        &metrics,
        topology_bytes,
        target_bytes,
        disk_bytes,
        generation,
        open,
        cold_elapsed,
        total_started.elapsed(),
        cold_before,
        cold_after,
        cold_partitioned,
    )
    .write();

    let warm_before = image.cache_snapshot();
    let warm_started = Instant::now();
    let (warm, _, warm_partitioned) =
        route_partitioned_controlled(&image, &request, &AtomicBool::new(false))
            .expect("warm scale CPU route");
    let warm_elapsed = warm_started.elapsed();
    assert_exact(&warm);
    let warm_after = image.cache_snapshot();
    cpu_row(
        "scale-route-warm",
        false,
        &metrics,
        topology_bytes,
        target_bytes,
        disk_bytes,
        generation,
        open,
        warm_elapsed,
        total_started.elapsed(),
        warm_before,
        warm_after,
        warm_partitioned,
    )
    .write();

    let cancel_before = image.cache_snapshot();
    let cancellation = AtomicBool::new(target_gib == 0);
    let cancel_started = Instant::now();
    let (cancelled, _, cancel_diagnostics) = if target_gib == 0 {
        route_partitioned_controlled(&image, &request, &cancellation)
            .expect("pre-cancelled scale CPU route")
    } else {
        std::thread::scope(|scope| {
            let route = scope.spawn(|| {
                route_partitioned_controlled(&image, &request, &cancellation)
                    .expect("active-cancelled scale CPU route")
            });
            let deadline = Instant::now() + Duration::from_secs(30);
            let observed_active_work = loop {
                let snapshot = image.cache_snapshot();
                if snapshot.pinned_entries > 0 || snapshot.misses > cancel_before.misses {
                    break true;
                }
                if route.is_finished() || Instant::now() >= deadline {
                    break false;
                }
                std::thread::yield_now();
            };
            cancellation.store(true, Ordering::Release);
            assert!(
                observed_active_work,
                "scale CPU route did not expose active partition work before cancellation"
            );
            route.join().expect("active scale CPU cancellation worker")
        })
    };
    let cancel_elapsed = cancel_started.elapsed();
    assert_eq!(cancelled.completion_reason(), CompletionReason::Cancelled);
    let cancel_after = wait_for_host_cache_idle(&image);
    assert_eq!(cancel_after.pinned_entries, 0);
    assert_eq!(cancel_after.queue_depth, 0);
    assert_eq!(cancel_after.staging_current_bytes, 0);
    ScaleRow {
        event: "scale-cancel",
        executor: "cpu-partitioned",
        algorithm: if target_gib == 0 {
            "pre-cancelled-dijkstra"
        } else {
            "active-cancelled-dijkstra"
        },
        nodes: metrics.node_count,
        adjacencies: adjacency_count,
        bundle_bytes: metrics.bundle_bytes,
        topology_bytes,
        target_topology_bytes: target_bytes,
        disk_bytes,
        partitions: metrics.partition_count,
        generation_us: generation.as_micros(),
        open_us: open.as_micros(),
        route_us: cancel_elapsed.as_micros(),
        total_us: total_started.elapsed().as_micros(),
        host_cache_current: cancel_after.current_bytes,
        host_cache_high_water: cancel_after.high_water_bytes,
        host_staging_high_water: cancel_after.staging_high_water_bytes,
        host_cache_hits: cancel_after.hits.saturating_sub(cancel_before.hits),
        host_cache_misses: cancel_after.misses.saturating_sub(cancel_before.misses),
        host_cache_evictions: cancel_after
            .evictions
            .saturating_sub(cancel_before.evictions),
        partitions_read: cancel_diagnostics.partitions,
        file_bytes: cancel_diagnostics.file_bytes,
        peak_host_bytes: crate::system::peak_memory_bytes().unwrap_or(0),
        completion: "cancelled",
        correct: true,
        rebuilt: false,
        ..ScaleRow::default()
    }
    .write();

    #[cfg(feature = "cuda")]
    run_cuda(
        Arc::clone(&image),
        &request,
        adjacency_count,
        &metrics,
        topology_bytes,
        target_bytes,
        disk_bytes,
        generation,
        open,
        total_started,
    );

    drop(image);
    let restart_started = Instant::now();
    let restarted = open_bundle(directory).expect("restart validates existing scale bundle");
    let restart_open = restart_started.elapsed();
    let snapshot = restarted.snapshot();
    assert_eq!(snapshot.total_bytes, metrics.bundle_bytes);
    assert_eq!(snapshot.partition_count as u64, metrics.partition_count);
    let peak_host_bytes = crate::system::peak_memory_bytes();
    if target_gib > 0 {
        let peak = peak_host_bytes.expect("scale host peak must be observable for the proof run");
        assert!(
            peak < topology_bytes,
            "the complete CUDA topology became resident in the benchmark process"
        );
    }
    ScaleRow {
        event: "scale-restart-open",
        executor: "bundle",
        algorithm: "validate-existing-no-rebuild",
        nodes: metrics.node_count,
        adjacencies: adjacency_count,
        bundle_bytes: metrics.bundle_bytes,
        topology_bytes,
        target_topology_bytes: target_bytes,
        disk_bytes,
        partitions: metrics.partition_count,
        generation_us: generation.as_micros(),
        open_us: open.as_micros(),
        restart_open_us: restart_open.as_micros(),
        total_us: total_started.elapsed().as_micros(),
        peak_host_bytes: peak_host_bytes.unwrap_or(0),
        completion: "bundle-valid",
        correct: true,
        rebuilt: false,
        ..ScaleRow::default()
    }
    .write();
}

#[allow(clippy::too_many_arguments)]
fn cpu_row(
    event: &'static str,
    cold: bool,
    metrics: &pathhydra_routing::BundleBuildMetrics,
    topology_bytes: u64,
    target_bytes: u64,
    disk_bytes: u64,
    generation: Duration,
    open: Duration,
    elapsed: Duration,
    total: Duration,
    before: pathhydra_routing::HostCacheSnapshot,
    after: pathhydra_routing::HostCacheSnapshot,
    diagnostics: pathhydra_routing::PartitionedCpuDiagnostics,
) -> ScaleRow<'static> {
    ScaleRow {
        event,
        executor: "cpu-partitioned",
        algorithm: "dijkstra",
        cold,
        nodes: metrics.node_count,
        adjacencies: metrics.adjacency_count,
        bundle_bytes: metrics.bundle_bytes,
        topology_bytes,
        target_topology_bytes: target_bytes,
        disk_bytes,
        partitions: metrics.partition_count,
        generation_us: generation.as_micros(),
        open_us: open.as_micros(),
        route_us: elapsed.as_micros(),
        total_us: total.as_micros(),
        host_cache_current: after.current_bytes,
        host_cache_high_water: after.high_water_bytes,
        host_staging_high_water: after.staging_high_water_bytes,
        host_cache_hits: after.hits.saturating_sub(before.hits),
        host_cache_misses: after.misses.saturating_sub(before.misses),
        host_cache_evictions: after.evictions.saturating_sub(before.evictions),
        partitions_read: diagnostics.partitions,
        file_bytes: diagnostics.file_bytes,
        peak_host_bytes: crate::system::peak_memory_bytes().unwrap_or(0),
        completion: "all-destinations-finalized",
        correct: true,
        ..ScaleRow::default()
    }
}

#[cfg(feature = "cuda")]
#[allow(clippy::too_many_arguments)]
fn run_cuda(
    image: Arc<ChunkedRoutingImage>,
    request: &RoutingRequest,
    adjacency_count: u64,
    metrics: &pathhydra_routing::BundleBuildMetrics,
    topology_bytes: u64,
    target_bytes: u64,
    disk_bytes: u64,
    generation: Duration,
    open: Duration,
    total_started: Instant,
) {
    const CONCURRENT_SEARCH_RESERVE_BYTES: usize = 256 * 1024 * 1024;
    let maximum_partition_adjacencies = image
        .bundle()
        .manifest()
        .partitions
        .iter()
        .map(|partition| usize::try_from(partition.edge_count).unwrap_or(usize::MAX))
        .max()
        .unwrap_or(0);
    for algorithm in [
        pathhydra_cuda::CudaAlgorithm::Frontier,
        pathhydra_cuda::CudaAlgorithm::DeltaStepping(
            pathhydra_cuda::DeltaConfiguration::new(0.1).unwrap(),
        ),
    ] {
        let required = pathhydra_cuda::estimate_search_bytes(
            image.node_count(),
            image.relation_kind_count(),
            maximum_partition_adjacencies,
            request.destinations().len(),
            algorithm,
        )
        .expect("scale upload-time search reserve estimate");
        assert!(
            required <= CONCURRENT_SEARCH_RESERVE_BYTES,
            "the scale search-state ceiling no longer covers every selected algorithm"
        );
    }
    let context = pathhydra_cuda::CudaContextOwner::initialize(0).expect("CUDA context");
    let (baseline_free_device_bytes, _) = context
        .current_memory()
        .expect("CUDA memory before scale upload");
    let cuda = pathhydra_cuda::CudaPartitionedImage::upload(
        Arc::clone(&context),
        Arc::clone(&image),
        pathhydra_cuda::CudaPartitionedConfig {
            maximum_topology_cache_bytes: 32 * 1024 * 1024,
            maximum_topology_cache_slots: 2,
            maximum_host_staging_bytes: 16 * 1024 * 1024,
            minimum_free_memory_headroom: 512 * 1024 * 1024,
            // Search state is independently estimated per route below. This
            // upload-time reserve is a conservative ceiling for the fixed
            // one-million-node/fixed-partition scale corpus, so topology-cache
            // admission cannot consume memory needed by either algorithm.
            reserved_concurrent_search_bytes: CONCURRENT_SEARCH_RESERVE_BYTES,
            reverse_partition_order: false,
        },
    )
    .expect("partitioned CUDA scale image");
    let mut minimum_free = context
        .current_memory()
        .expect("CUDA memory after upload")
        .0;
    let capabilities = context.capabilities();
    let device_total_bytes = capabilities.total_memory_bytes as u64;
    if target_bytes > device_total_bytes {
        assert!(topology_bytes > device_total_bytes);
    }
    let gpu = capabilities.device_name.clone();
    let compute = format!(
        "{}.{}",
        capabilities.compute_capability.0, capabilities.compute_capability.1
    );
    let driver = capabilities.driver_version.to_string();

    for algorithm in [
        pathhydra_cuda::CudaAlgorithm::Frontier,
        pathhydra_cuda::CudaAlgorithm::DeltaStepping(
            pathhydra_cuda::DeltaConfiguration::new(0.1).unwrap(),
        ),
    ] {
        let reserved = pathhydra_cuda::estimate_search_bytes(
            image.node_count(),
            image.relation_kind_count(),
            cuda.maximum_partition_adjacency_count(),
            request.destinations().len(),
            algorithm,
        )
        .expect("scale CUDA memory estimate");
        let before = cuda.topology_cache_snapshot();
        let host_before = image.cache_snapshot();
        let started = Instant::now();
        let output = cuda
            .route(request, algorithm, &AtomicBool::new(false), reserved)
            .expect("partitioned CUDA scale route");
        let elapsed = started.elapsed();
        assert_exact(&output.response);
        let after = cuda.topology_cache_snapshot();
        let host_after = image.cache_snapshot();
        minimum_free =
            minimum_free.min(context.current_memory().expect("CUDA memory after route").0);
        ScaleRow {
            event: "scale-route",
            executor: "cuda-partitioned",
            algorithm: match algorithm {
                pathhydra_cuda::CudaAlgorithm::Frontier => "frontier",
                pathhydra_cuda::CudaAlgorithm::DeltaStepping(_) => "delta-stepping-0.1",
            },
            cold: matches!(algorithm, pathhydra_cuda::CudaAlgorithm::Frontier),
            nodes: metrics.node_count,
            adjacencies: adjacency_count,
            bundle_bytes: metrics.bundle_bytes,
            topology_bytes,
            target_topology_bytes: target_bytes,
            disk_bytes,
            partitions: metrics.partition_count,
            generation_us: generation.as_micros(),
            open_us: open.as_micros(),
            restart_open_us: 0,
            route_us: elapsed.as_micros(),
            total_us: total_started.elapsed().as_micros(),
            host_cache_current: host_after.current_bytes,
            host_cache_high_water: host_after.high_water_bytes,
            host_staging_high_water: host_after.staging_high_water_bytes,
            host_cache_hits: host_after.hits.saturating_sub(host_before.hits),
            host_cache_misses: host_after.misses.saturating_sub(host_before.misses),
            host_cache_evictions: host_after.evictions.saturating_sub(host_before.evictions),
            partitions_read: output.diagnostics.partitions_required,
            file_bytes: output.diagnostics.file_bytes,
            device_cache_current: after.current_bytes as u64,
            device_cache_high_water: after.high_water_bytes as u64,
            device_cache_hits: after.hits.saturating_sub(before.hits),
            device_cache_misses: after.misses.saturating_sub(before.misses),
            device_cache_evictions: after.evictions.saturating_sub(before.evictions),
            device_cache_transfer_bytes: after.transfer_bytes.saturating_sub(before.transfer_bytes),
            route_transfer_bytes: output.diagnostics.transfer_bytes,
            reserved_search_bytes: output.diagnostics.reserved_search_bytes as u64,
            peak_host_bytes: crate::system::peak_memory_bytes().unwrap_or(0),
            device_total_bytes,
            observed_device_used_bytes: baseline_free_device_bytes.saturating_sub(minimum_free)
                as u64,
            state_initialization_us: output.diagnostics.state_initialization_duration.as_micros(),
            partition_scheduling_us: output.diagnostics.partition_scheduling_duration.as_micros(),
            frontier_compaction_us: output.diagnostics.frontier_compaction_duration.as_micros(),
            relation_relaxation_us: output.diagnostics.relation_relaxation_duration.as_micros(),
            response_transfer_us: output.diagnostics.response_transfer_duration.as_micros(),
            completion: "all-destinations-finalized",
            correct: true,
            rebuilt: false,
            gpu: gpu.clone(),
            compute_capability: compute.clone(),
            driver: driver.clone(),
        }
        .write();

        let cancel_before = cuda.topology_cache_snapshot();
        let host_cancel_before = image.cache_snapshot();
        let cancellation = AtomicBool::new(target_bytes == 0);
        let cancel_started = Instant::now();
        let cancelled = if target_bytes == 0 {
            cuda.route(request, algorithm, &cancellation, reserved)
                .expect("pre-cancelled scale CUDA route")
        } else {
            let faults = cuda.fault_injection();
            faults.hold_task_compaction(true);
            std::thread::scope(|scope| {
                let route = scope.spawn(|| {
                    cuda.route(request, algorithm, &cancellation, reserved)
                        .expect("active-cancelled scale CUDA route")
                });
                let observed_active_work = faults.wait_for_task_compaction(Duration::from_secs(30));
                cancellation.store(true, Ordering::Release);
                faults.hold_task_compaction(false);
                let result = route.join().expect("active scale CUDA cancellation worker");
                assert!(
                    observed_active_work,
                    "scale CUDA route did not reach task compaction before cancellation"
                );
                result
            })
        };
        let cancel_elapsed = cancel_started.elapsed();
        assert_eq!(
            cancelled.response.completion_reason(),
            CompletionReason::Cancelled
        );
        let cancel_after = cuda.topology_cache_snapshot();
        let host_cancel_after = wait_for_host_cache_idle(&image);
        assert_eq!(cancel_after.in_use_slots, 0);
        assert_eq!(host_cancel_after.pinned_entries, 0);
        assert_eq!(host_cancel_after.queue_depth, 0);
        assert_eq!(host_cancel_after.staging_current_bytes, 0);
        ScaleRow {
            event: "scale-cancel",
            executor: "cuda-partitioned",
            algorithm: match (target_bytes == 0, algorithm) {
                (true, pathhydra_cuda::CudaAlgorithm::Frontier) => "pre-cancelled-frontier",
                (true, pathhydra_cuda::CudaAlgorithm::DeltaStepping(_)) => {
                    "pre-cancelled-delta-0.1"
                }
                (false, pathhydra_cuda::CudaAlgorithm::Frontier) => "active-cancelled-frontier",
                (false, pathhydra_cuda::CudaAlgorithm::DeltaStepping(_)) => {
                    "active-cancelled-delta-0.1"
                }
            },
            nodes: metrics.node_count,
            adjacencies: adjacency_count,
            bundle_bytes: metrics.bundle_bytes,
            topology_bytes,
            target_topology_bytes: target_bytes,
            disk_bytes,
            partitions: metrics.partition_count,
            route_us: cancel_elapsed.as_micros(),
            total_us: total_started.elapsed().as_micros(),
            host_cache_current: host_cancel_after.current_bytes,
            host_cache_high_water: host_cancel_after.high_water_bytes,
            host_staging_high_water: host_cancel_after.staging_high_water_bytes,
            host_cache_hits: host_cancel_after
                .hits
                .saturating_sub(host_cancel_before.hits),
            host_cache_misses: host_cancel_after
                .misses
                .saturating_sub(host_cancel_before.misses),
            host_cache_evictions: host_cancel_after
                .evictions
                .saturating_sub(host_cancel_before.evictions),
            partitions_read: cancelled.diagnostics.partitions_required,
            file_bytes: cancelled.diagnostics.file_bytes,
            device_cache_current: cancel_after.current_bytes as u64,
            device_cache_high_water: cancel_after.high_water_bytes as u64,
            device_cache_hits: cancel_after.hits.saturating_sub(cancel_before.hits),
            device_cache_misses: cancel_after.misses.saturating_sub(cancel_before.misses),
            device_cache_evictions: cancel_after
                .evictions
                .saturating_sub(cancel_before.evictions),
            device_cache_transfer_bytes: cancel_after
                .transfer_bytes
                .saturating_sub(cancel_before.transfer_bytes),
            route_transfer_bytes: cancelled.diagnostics.transfer_bytes,
            reserved_search_bytes: cancelled.diagnostics.reserved_search_bytes as u64,
            peak_host_bytes: crate::system::peak_memory_bytes().unwrap_or(0),
            device_total_bytes,
            observed_device_used_bytes: baseline_free_device_bytes.saturating_sub(minimum_free)
                as u64,
            completion: "cancelled",
            correct: true,
            rebuilt: false,
            gpu: gpu.clone(),
            compute_capability: compute.clone(),
            driver: driver.clone(),
            ..ScaleRow::default()
        }
        .write();
    }

    let fallback_algorithm = pathhydra_cuda::CudaAlgorithm::Frontier;
    let fallback_reserved = pathhydra_cuda::estimate_search_bytes(
        image.node_count(),
        image.relation_kind_count(),
        cuda.maximum_partition_adjacency_count(),
        request.destinations().len(),
        fallback_algorithm,
    )
    .expect("scale fallback CUDA memory estimate");
    cuda.fault_injection()
        .fail_at(pathhydra_cuda::CudaFaultStage::ContextLoss);
    let fallback_started = Instant::now();
    let failed = cuda
        .route(
            request,
            fallback_algorithm,
            &AtomicBool::new(false),
            fallback_reserved,
        )
        .expect_err("injected scale device loss must be visible");
    assert_eq!(failed.kind(), pathhydra_cuda::CudaFailureKind::DeviceLost);
    let (fallback, _, fallback_partitioned) =
        route_partitioned_controlled(&image, request, &AtomicBool::new(false))
            .expect("scale CPU fallback route");
    assert_exact(&fallback);
    let fallback_elapsed = fallback_started.elapsed();
    assert_eq!(cuda.topology_cache_snapshot().in_use_slots, 0);
    ScaleRow {
        event: "scale-device-fallback",
        executor: "cpu-partitioned",
        algorithm: "injected-device-loss-then-dijkstra",
        nodes: metrics.node_count,
        adjacencies: adjacency_count,
        bundle_bytes: metrics.bundle_bytes,
        topology_bytes,
        target_topology_bytes: target_bytes,
        disk_bytes,
        partitions: metrics.partition_count,
        route_us: fallback_elapsed.as_micros(),
        total_us: total_started.elapsed().as_micros(),
        partitions_read: fallback_partitioned.partitions,
        file_bytes: fallback_partitioned.file_bytes,
        peak_host_bytes: crate::system::peak_memory_bytes().unwrap_or(0),
        device_total_bytes,
        observed_device_used_bytes: baseline_free_device_bytes.saturating_sub(minimum_free) as u64,
        completion: "all-destinations-finalized",
        correct: true,
        rebuilt: false,
        gpu,
        compute_capability: compute,
        driver,
        ..ScaleRow::default()
    }
    .write();
}

fn analytic_request() -> RoutingRequest {
    RoutingRequest::new(
        NodeId::from_u64(1),
        [NodeId::from_u64(2)],
        RelationProfile::new([(
            RelationId::from_u64(1),
            RelationUse::Enabled(RelationMultiplier::new(1.0).unwrap()),
        )]),
        false,
        SearchBudget::Unlimited,
        TiePolicy::StablePredecessor,
    )
}

fn assert_exact(response: &pathhydra_routing::RoutingResponse) {
    assert!(matches!(
        response.results()[0].state(),
        DestinationState::Exact(exact) if exact.logical_distance().to_bits() == 1.0_f64.to_bits()
    ));
    assert_eq!(
        response.completion_reason(),
        CompletionReason::AllDestinationsFinalized
    );
}

fn metrics_from_bundle(
    bundle: &pathhydra_routing::RoutingBundle,
    adjacency_count: u64,
    validation: Duration,
) -> pathhydra_routing::BundleBuildMetrics {
    let snapshot = bundle.snapshot();
    pathhydra_routing::BundleBuildMetrics {
        node_count: bundle.manifest().node_count,
        relation_kind_count: 1,
        adjacency_count,
        segment_count: snapshot.segment_count as u64,
        partition_count: snapshot.partition_count as u64,
        split_source_count: snapshot.split_source_count as u64,
        peak_partition_buffer_bytes: snapshot.largest_partition_bytes,
        decoded_record_bytes: 0,
        bundle_bytes: snapshot.total_bytes,
        scan_duration: Duration::ZERO,
        write_duration: Duration::ZERO,
        validation_duration: validation,
        total_duration: validation,
    }
}

fn directory_bytes(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| {
            entry.metadata().map_or(0, |metadata| {
                if metadata.is_dir() {
                    directory_bytes(&entry.path())
                } else {
                    metadata.len()
                }
            })
        })
        .fold(0, u64::saturating_add)
}
