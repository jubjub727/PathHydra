use std::{hint::black_box, time::Instant};

#[cfg(feature = "cuda")]
use pathhydra_routing::{route, route_partitioned};

#[cfg(feature = "cuda")]
use crate::fixtures::{self, Shape, Workload};
use crate::report::{self, StrategyMeasurement};

#[cfg(feature = "cuda")]
const WORKLOADS: [Workload; 2] = [
    Workload {
        name: "broad-star",
        nodes: 512,
        shape: Shape::Star,
    },
    Workload {
        name: "dense-scc",
        nodes: 48,
        shape: Shape::Dense,
    },
];

pub fn run(repeats: usize) {
    isolated_comparators(repeats);
    #[cfg(feature = "cuda")]
    cuda_strategy(repeats);
    #[cfg(not(feature = "cuda"))]
    eprintln!("parallel-strategy CUDA rows require --features cuda");
}

fn isolated_comparators(repeats: usize) {
    const NODES: usize = 512;
    const RELATIONS: usize = 32_768;
    const INNER: usize = 256;
    const TARGET_INNER: usize = 16_384;
    for sample in 0..repeats {
        let mut distances = vec![0_u64; NODES];
        let mut frontier = vec![1_u32; NODES];
        let started = Instant::now();
        for _ in 0..INNER {
            distances.fill(f64::INFINITY.to_bits());
            frontier.fill(0);
            black_box((&distances, &frontier));
        }
        comparator(
            "reset-nodes-512",
            "explicit-clear",
            sample,
            NODES * INNER,
            started,
        );

        let mut stamps = vec![0_u32; NODES];
        let mut generation = 0_u32;
        let started = Instant::now();
        for _ in 0..INNER {
            generation = generation.wrapping_add(1);
            for (index, stamp) in stamps.iter_mut().enumerate() {
                if *stamp != generation {
                    *stamp = generation;
                    distances[index] = f64::INFINITY.to_bits();
                    frontier[index] = 0;
                }
            }
            black_box((&distances, &frontier));
        }
        comparator(
            "reset-nodes-512",
            "generation-first-touch-all-nodes",
            sample,
            NODES * INNER,
            started,
        );

        for &destination_count in &[1_usize, 3, 64, 512] {
            let ids: Vec<usize> = (0..destination_count)
                .map(|index| (index * 131) % NODES)
                .collect();
            let started = Instant::now();
            for _ in 0..TARGET_INNER {
                let mut sparse = ids.clone();
                sparse.sort_unstable();
                sparse.dedup();
                for &id in &ids {
                    black_box(sparse.binary_search(&id).is_ok());
                }
            }
            target_comparator(
                destination_count,
                "sorted-sparse",
                sample,
                destination_count * TARGET_INNER,
                started,
            );

            let started = Instant::now();
            let mut dense = vec![false; NODES];
            for _ in 0..TARGET_INNER {
                dense.fill(false);
                for &id in &ids {
                    dense[id] = true;
                }
                for &id in &ids {
                    black_box(dense[id]);
                }
            }
            target_comparator(
                destination_count,
                "dense-bitset",
                sample,
                destination_count * TARGET_INNER,
                started,
            );

            let started = Instant::now();
            let mut dense_stamps = vec![0_u32; NODES];
            for generation in 1..=TARGET_INNER as u32 {
                for &id in &ids {
                    dense_stamps[id] = generation;
                }
                for &id in &ids {
                    black_box(dense_stamps[id] == generation);
                }
            }
            target_comparator(
                destination_count,
                "generation-dense",
                sample,
                destination_count * TARGET_INNER,
                started,
            );
        }

        let bases: Vec<f32> = (0..RELATIONS)
            .map(|index| 0.001 + (index % 17) as f32 * 0.0001)
            .collect();
        let relation_indexes: Vec<usize> = (0..RELATIONS).map(|index| index % 16).collect();
        let multipliers: Vec<f32> = (0..16).map(|index| 0.5 + index as f32 * 0.1).collect();
        let started = Instant::now();
        let mut total = 0.0_f64;
        for (&base, &relation) in bases.iter().zip(&relation_indexes) {
            total += f64::from(base) * f64::from(multipliers[relation]);
        }
        black_box(total);
        comparator(
            "profile-relations-32768",
            "inline-one-use",
            sample,
            RELATIONS,
            started,
        );

        let started = Instant::now();
        let materialized: Vec<f64> = bases
            .iter()
            .zip(&relation_indexes)
            .map(|(&base, &relation)| f64::from(base) * f64::from(multipliers[relation]))
            .collect();
        black_box(materialized.iter().copied().sum::<f64>());
        comparator(
            "profile-relations-32768",
            "materialized-one-use",
            sample,
            RELATIONS,
            started,
        );

        let started = Instant::now();
        let mut inline_reuse_total = 0.0_f64;
        for _ in 0..16 {
            for (&base, &relation) in bases.iter().zip(&relation_indexes) {
                inline_reuse_total += f64::from(base) * f64::from(multipliers[relation]);
            }
        }
        black_box(inline_reuse_total);
        comparator(
            "profile-relations-32768",
            "inline-reuse-16",
            sample,
            RELATIONS * 16,
            started,
        );

        let started = Instant::now();
        for _ in 0..16 {
            black_box(materialized.iter().copied().sum::<f64>());
        }
        comparator(
            "profile-relations-32768",
            "materialized-reuse-16",
            sample,
            RELATIONS * 16,
            started,
        );
    }
}

fn comparator(
    workload: &'static str,
    strategy: &'static str,
    sample: usize,
    operations: usize,
    started: Instant,
) {
    report::write_strategy(&StrategyMeasurement {
        workload,
        executor: "isolated-host-comparator",
        algorithm: "none",
        strategy,
        sample,
        requested_lanes: 1,
        observed_batch_width: 1,
        lane_index: 0,
        operations,
        elapsed: started.elapsed(),
        queue: std::time::Duration::ZERO,
        correctness: true,
        gpu_name: "",
    });
}

fn target_comparator(
    destination_count: usize,
    strategy: &'static str,
    sample: usize,
    operations: usize,
    started: Instant,
) {
    let workload = match destination_count {
        1 => "targets-1-of-512",
        3 => "targets-3-of-512",
        64 => "targets-64-of-512",
        512 => "targets-512-of-512",
        _ => unreachable!(),
    };
    comparator(workload, strategy, sample, operations, started);
}

#[cfg(feature = "cuda")]
fn cuda_candidate_strategies(repeats: usize, context: &pathhydra_cuda::CudaContextOwner) {
    use pathhydra_cuda::{
        CudaBenchmarkProfileStrategy, CudaBenchmarkResetStrategy, CudaBenchmarkTargetStrategy,
    };

    let gpu = &context.capabilities().device_name;
    for execution_context in ["resident", "partitioned"] {
        for strategy in [
            CudaBenchmarkResetStrategy::ExplicitClear,
            CudaBenchmarkResetStrategy::GenerationStamp,
        ] {
            context
                .benchmark_reset_strategy(512, strategy)
                .expect("CUDA reset candidate warmup");
            for sample in 0..repeats {
                let started = Instant::now();
                context
                    .benchmark_reset_strategy(512, strategy)
                    .expect("CUDA reset candidate");
                candidate_measurement(
                    "reset-nodes-512",
                    execution_context,
                    match strategy {
                        CudaBenchmarkResetStrategy::ExplicitClear => "explicit-clear",
                        CudaBenchmarkResetStrategy::GenerationStamp => "generation-stamp",
                    },
                    sample,
                    512,
                    started,
                    gpu,
                );
            }
        }
        for &destination_count in &[1_usize, 3, 64, 512] {
            let destinations: Vec<u32> = (0..destination_count)
                .map(|index| ((index * 131) % 512) as u32)
                .collect();
            for strategy in [
                CudaBenchmarkTargetStrategy::SortedSparse,
                CudaBenchmarkTargetStrategy::DenseBitset,
                CudaBenchmarkTargetStrategy::GenerationDense,
            ] {
                context
                    .benchmark_target_strategy(512, &destinations, strategy)
                    .expect("CUDA target candidate warmup");
                for sample in 0..repeats {
                    let started = Instant::now();
                    context
                        .benchmark_target_strategy(512, &destinations, strategy)
                        .expect("CUDA target candidate");
                    candidate_measurement(
                        match destination_count {
                            1 => "targets-1-of-512",
                            3 => "targets-3-of-512",
                            64 => "targets-64-of-512",
                            512 => "targets-512-of-512",
                            _ => unreachable!(),
                        },
                        execution_context,
                        match strategy {
                            CudaBenchmarkTargetStrategy::SortedSparse => "sorted-sparse",
                            CudaBenchmarkTargetStrategy::DenseBitset => "dense-bitset",
                            CudaBenchmarkTargetStrategy::GenerationDense => "generation-dense",
                        },
                        sample,
                        destination_count,
                        started,
                        gpu,
                    );
                }
            }
        }

        for (workload, edge_count) in [
            ("profile-narrow", 127_usize),
            ("profile-broad", 511),
            ("profile-dense", 2_256),
        ] {
            for &relation_kind_count in &[2_usize, 64] {
                let bases: Vec<u32> = (0..edge_count)
                    .map(|index| (0.001 + (index % 17) as f32 * 0.0001).to_bits())
                    .collect();
                let relations: Vec<u32> = (0..edge_count)
                    .map(|index| (index % relation_kind_count) as u32)
                    .collect();
                let multipliers: Vec<u32> = (0..relation_kind_count)
                    .map(|index| (0.5 + index as f32 * 0.01).to_bits())
                    .collect();
                for &reuse_count in &[1_usize, 16] {
                    for strategy in [
                        CudaBenchmarkProfileStrategy::Inline,
                        CudaBenchmarkProfileStrategy::Materialized,
                    ] {
                        context
                            .benchmark_profile_strategy(
                                &bases,
                                &relations,
                                &multipliers,
                                reuse_count,
                                if execution_context == "resident" {
                                    usize::MAX
                                } else {
                                    128
                                },
                                strategy,
                            )
                            .expect("CUDA profile candidate warmup");
                        for sample in 0..repeats {
                            let started = Instant::now();
                            context
                                .benchmark_profile_strategy(
                                    &bases,
                                    &relations,
                                    &multipliers,
                                    reuse_count,
                                    if execution_context == "resident" {
                                        usize::MAX
                                    } else {
                                        128
                                    },
                                    strategy,
                                )
                                .expect("CUDA profile candidate");
                            candidate_measurement(
                                workload,
                                execution_context,
                                profile_strategy_name(strategy, reuse_count, relation_kind_count),
                                sample,
                                edge_count * reuse_count,
                                started,
                                gpu,
                            );
                        }
                    }
                }
            }
        }
    }
}

#[cfg(feature = "cuda")]
fn profile_strategy_name(
    strategy: pathhydra_cuda::CudaBenchmarkProfileStrategy,
    reuse_count: usize,
    relation_kind_count: usize,
) -> &'static str {
    use pathhydra_cuda::CudaBenchmarkProfileStrategy::{Inline, Materialized};
    match (strategy, reuse_count, relation_kind_count) {
        (Inline, 1, 2) => "inline-use1-kinds2",
        (Inline, 1, 64) => "inline-use1-kinds64",
        (Inline, 16, 2) => "inline-use16-kinds2",
        (Inline, 16, 64) => "inline-use16-kinds64",
        (Materialized, 1, 2) => "materialized-use1-kinds2",
        (Materialized, 1, 64) => "materialized-use1-kinds64",
        (Materialized, 16, 2) => "materialized-use16-kinds2",
        (Materialized, 16, 64) => "materialized-use16-kinds64",
        _ => unreachable!(),
    }
}

#[cfg(feature = "cuda")]
fn candidate_measurement(
    workload: &'static str,
    execution_context: &'static str,
    strategy: &'static str,
    sample: usize,
    operations: usize,
    started: Instant,
    gpu: &str,
) {
    report::write_strategy(&StrategyMeasurement {
        workload,
        executor: "cuda-candidate",
        algorithm: execution_context,
        strategy,
        sample,
        requested_lanes: 1,
        observed_batch_width: 1,
        lane_index: 0,
        operations,
        elapsed: started.elapsed(),
        queue: std::time::Duration::ZERO,
        correctness: true,
        gpu_name: gpu,
    });
}

#[cfg(feature = "cuda")]
fn cuda_strategy(repeats: usize) {
    use std::sync::atomic::AtomicBool;

    use pathhydra_cuda::{
        CudaAlgorithm, CudaContextOwner, CudaPartitionedConfig, CudaPartitionedImage,
        CudaResidentImage, DeltaConfiguration, estimate_search_bytes_for_request,
    };

    let context = CudaContextOwner::initialize(0).expect("CUDA device");
    cuda_candidate_strategies(repeats, &context);
    let gpu_name = context.capabilities().device_name.clone();
    for workload in WORKLOADS {
        let fixture = fixtures::build(workload);
        let expected = route(&fixture.image, &fixture.request).expect("resident CPU oracle");
        let _ =
            route_partitioned(&fixture.chunked, &fixture.request).expect("partitioned CPU warmup");
        for sample in 0..repeats {
            let started = Instant::now();
            let actual = route(&fixture.image, &fixture.request).expect("resident CPU route");
            route_measurement(
                workload.name,
                "cpu",
                "dijkstra",
                "resident",
                sample,
                fixture.image.adjacency_count(),
                started,
                super::routing::same_distances(&expected, &actual),
                "",
            );
            let started = Instant::now();
            let actual = route_partitioned(&fixture.chunked, &fixture.request)
                .expect("partitioned CPU route");
            route_measurement(
                workload.name,
                "cpu-partitioned",
                "dijkstra",
                "warm-cache",
                sample,
                fixture.image.adjacency_count(),
                started,
                super::routing::same_distances(&expected, &actual),
                "",
            );
        }

        let resident =
            CudaResidentImage::upload(context.clone(), fixture.image.clone(), usize::MAX, 0)
                .expect("CUDA residency");
        let partitioned = CudaPartitionedImage::upload(
            context.clone(),
            fixture.chunked.clone(),
            CudaPartitionedConfig {
                maximum_topology_cache_bytes: 16 * 1024,
                maximum_topology_cache_slots: 2,
                maximum_host_staging_bytes: 8 * 1024,
                minimum_free_memory_headroom: 0,
                reserved_concurrent_search_bytes: 64 * 1024 * 1024,
                reverse_partition_order: false,
            },
        )
        .expect("partitioned CUDA image");
        for algorithm in [
            CudaAlgorithm::Frontier,
            CudaAlgorithm::DeltaStepping(DeltaConfiguration::new(0.01).unwrap()),
            CudaAlgorithm::DeltaStepping(DeltaConfiguration::new(0.1).unwrap()),
            CudaAlgorithm::DeltaStepping(DeltaConfiguration::new(1.0).unwrap()),
            CudaAlgorithm::DeltaStepping(DeltaConfiguration::new(10.0).unwrap()),
        ] {
            let algorithm_name = match algorithm {
                CudaAlgorithm::Frontier => "frontier",
                CudaAlgorithm::DeltaStepping(configuration) if configuration.delta() == 0.01 => {
                    "delta-0.01"
                }
                CudaAlgorithm::DeltaStepping(configuration) if configuration.delta() == 0.1 => {
                    "delta-0.1"
                }
                CudaAlgorithm::DeltaStepping(configuration) if configuration.delta() == 1.0 => {
                    "delta-1.0"
                }
                CudaAlgorithm::DeltaStepping(_) => "delta-10.0",
            };
            let reserved = estimate_search_bytes_for_request(
                fixture.image.node_count(),
                fixture.image.relation_kind_count(),
                fixture.image.adjacency_count(),
                fixture.request.destinations().len(),
                algorithm,
                None,
            )
            .expect("search reservation");
            resident
                .route(
                    &fixture.request,
                    algorithm,
                    &AtomicBool::new(false),
                    reserved,
                )
                .expect("resident warmup");
            partitioned
                .route(
                    &fixture.request,
                    algorithm,
                    &AtomicBool::new(false),
                    reserved,
                )
                .expect("partitioned warmup");
            for sample in 0..repeats {
                let started = Instant::now();
                let output = resident
                    .route(
                        &fixture.request,
                        algorithm,
                        &AtomicBool::new(false),
                        reserved,
                    )
                    .expect("resident CUDA route");
                route_measurement(
                    workload.name,
                    "cuda",
                    algorithm_name,
                    "relation-threads-atomic-binary64",
                    sample,
                    fixture.image.adjacency_count(),
                    started,
                    super::routing::same_distances(&expected, &output.response),
                    &gpu_name,
                );
                let started = Instant::now();
                let output = partitioned
                    .route(
                        &fixture.request,
                        algorithm,
                        &AtomicBool::new(false),
                        reserved,
                    )
                    .expect("partitioned CUDA route");
                route_measurement(
                    workload.name,
                    "cuda-partitioned",
                    algorithm_name,
                    "relation-threads-atomic-binary64",
                    sample,
                    fixture.image.adjacency_count(),
                    started,
                    super::routing::same_distances(&expected, &output.response),
                    &gpu_name,
                );
            }

            if workload.name == "broad-star" && algorithm == CudaAlgorithm::Frontier {
                for (delay, strategy) in [
                    (std::time::Duration::ZERO, "resident-delay-0us"),
                    (std::time::Duration::from_micros(50), "resident-delay-50us"),
                    (std::time::Duration::from_millis(5), "resident-delay-5000us"),
                ] {
                    for &lanes in &[1_usize, 2, 4, 8] {
                        for sample in 0..repeats {
                            benchmark_worker_lanes(
                                lanes,
                                sample,
                                delay,
                                strategy,
                                false,
                                &resident,
                                &partitioned,
                                &fixture.request,
                                algorithm,
                                reserved,
                                &expected,
                                fixture.image.adjacency_count(),
                                &gpu_name,
                            );
                        }
                    }
                }
                for &lanes in &[1_usize, 2, 4, 8] {
                    for sample in 0..repeats {
                        benchmark_worker_lanes(
                            lanes,
                            sample,
                            std::time::Duration::from_micros(50),
                            "mixed-resident-partitioned-delay-50us",
                            true,
                            &resident,
                            &partitioned,
                            &fixture.request,
                            algorithm,
                            reserved,
                            &expected,
                            fixture.image.adjacency_count(),
                            &gpu_name,
                        );
                    }
                }
            }
        }
    }
}

#[cfg(feature = "cuda")]
#[allow(clippy::too_many_arguments)]
fn benchmark_worker_lanes(
    lanes: usize,
    sample: usize,
    delay: std::time::Duration,
    strategy: &'static str,
    mixed: bool,
    resident: &std::sync::Arc<pathhydra_cuda::CudaResidentImage>,
    partitioned: &std::sync::Arc<pathhydra_cuda::CudaPartitionedImage>,
    request: &pathhydra_routing::RoutingRequest,
    algorithm: pathhydra_cuda::CudaAlgorithm,
    reserved: usize,
    expected: &pathhydra_routing::RoutingResponse,
    operations: usize,
    gpu_name: &str,
) {
    use std::sync::{Arc, Barrier, atomic::AtomicBool};

    let worker =
        Arc::new(pathhydra_cuda::CudaWorker::start(lanes, delay).expect("CUDA benchmark worker"));
    let barrier = Arc::new(Barrier::new(lanes));
    let mut threads = Vec::with_capacity(lanes);
    for submit_index in 0..lanes {
        let worker = Arc::clone(&worker);
        let resident = Arc::clone(resident);
        let partitioned = Arc::clone(partitioned);
        let barrier = Arc::clone(&barrier);
        let request = request.clone();
        threads.push(std::thread::spawn(move || {
            barrier.wait();
            let started = Instant::now();
            let output = if mixed && submit_index % 2 == 1 {
                worker.submit_partitioned(
                    partitioned,
                    &request,
                    algorithm,
                    Arc::new(AtomicBool::new(false)),
                    reserved,
                )
            } else {
                worker.submit(
                    resident,
                    &request,
                    algorithm,
                    Arc::new(AtomicBool::new(false)),
                    reserved,
                )
            }
            .expect("batched CUDA route");
            (started.elapsed(), output)
        }));
    }
    for thread in threads {
        let (elapsed, output) = thread.join().expect("lane join");
        report::write_strategy(&StrategyMeasurement {
            workload: "broad-star",
            executor: "cuda-worker",
            algorithm: "frontier",
            strategy,
            sample,
            requested_lanes: lanes,
            observed_batch_width: output.diagnostics.batch_width,
            lane_index: output.diagnostics.lane_index,
            operations,
            elapsed,
            queue: output.diagnostics.queue_duration,
            correctness: super::routing::same_distances(expected, &output.response),
            gpu_name,
        });
    }
    let mut worker = Arc::try_unwrap(worker).ok().expect("worker lane owners");
    assert!(worker.shutdown().joined);
}

#[cfg(feature = "cuda")]
#[allow(clippy::too_many_arguments)]
fn route_measurement(
    workload: &'static str,
    executor: &'static str,
    algorithm: &'static str,
    strategy: &'static str,
    sample: usize,
    operations: usize,
    started: Instant,
    correctness: bool,
    gpu_name: &str,
) {
    report::write_strategy(&StrategyMeasurement {
        workload,
        executor,
        algorithm,
        strategy,
        sample,
        requested_lanes: 1,
        observed_batch_width: 1,
        lane_index: 0,
        operations,
        elapsed: started.elapsed(),
        queue: std::time::Duration::ZERO,
        correctness,
        gpu_name,
    });
}
