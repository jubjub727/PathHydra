use std::{
    fs,
    path::Path,
    sync::{Arc, Barrier},
    time::{Duration, Instant},
};

use crate::fixtures::{self, Shape, Workload};
use pathhydra_core::{ConfirmedRecord, EdgeId, NodeId, RelationId};
use pathhydra_engine::{EngineConfig, GraphEngine, HydrationRequest, RequestId};
use pathhydra_routing::{
    DestinationState, RelationMultiplier, RelationProfile, RelationUse, RoutingRequest,
    SearchBudget, TiePolicy, estimate_cpu_working_set, open_bundle, route, route_controlled,
    route_partitioned_controlled,
};
use pathhydra_store::{
    Catalog, CheckpointRequest, RestoreRequest, VerificationLimits, restore_checkpoint,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputFormat {
    Human,
    Csv,
    Json,
}

#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub repeats: usize,
    pub warmups: usize,
    pub format: OutputFormat,
}

#[derive(Clone, Debug, Default)]
struct Row {
    suite: String,
    case: String,
    executor: String,
    algorithm: String,
    configuration: String,
    row_kind: String,
    sample: Option<usize>,
    repeats: usize,
    warmups: usize,
    seed: u64,
    cold: bool,
    nodes: u64,
    relation_kinds: u64,
    edges: u64,
    partitions: u64,
    rocksdb_bytes: Option<u64>,
    bundle_bytes: Option<u64>,
    resident_bytes: Option<u64>,
    search_bytes: Option<u64>,
    peak_memory_bytes: Option<u64>,
    elapsed_ns: Option<u64>,
    median_ns: Option<u64>,
    minimum_ns: Option<u64>,
    maximum_ns: Option<u64>,
    spread_ns: Option<u64>,
    scan_ns: Option<u64>,
    build_ns: Option<u64>,
    validate_ns: Option<u64>,
    load_ns: Option<u64>,
    profile_pack_ns: Option<u64>,
    first_destination_ns: Option<u64>,
    full_completion_ns: Option<u64>,
    examined_edges: Option<u64>,
    relaxation_attempts: Option<u64>,
    relaxation_updates: Option<u64>,
    frontier_high_water: Option<u64>,
    phases_or_buckets: Option<u64>,
    cache_hits: Option<u64>,
    cache_misses: Option<u64>,
    cache_evictions: Option<u64>,
    file_bytes: Option<u64>,
    transfer_bytes: Option<u64>,
    reserved_bytes: Option<u64>,
    concurrent_routes: Option<u64>,
    throughput_per_second: Option<f64>,
    reconstruction_ns: Option<u64>,
    hydration_ns: Option<u64>,
    checkpoint_ns: Option<u64>,
    restore_ns: Option<u64>,
    correctness: bool,
    os: String,
    architecture: String,
    cpu: String,
    storage: String,
    rust_toolchain: String,
    kernel_toolchain: String,
    gpu: String,
    cuda_driver: String,
}

impl Row {
    fn base(suite: &str, case: &str, executor: &str, algorithm: &str, options: Options) -> Self {
        Self {
            suite: suite.to_owned(),
            case: case.to_owned(),
            executor: executor.to_owned(),
            algorithm: algorithm.to_owned(),
            repeats: options.repeats,
            warmups: options.warmups,
            seed: 0x5041_5448_4859_4452,
            configuration: format!(
                "repeats={};warmups={};seed=0x5041544848594452",
                options.repeats, options.warmups
            ),
            correctness: true,
            os: std::env::consts::OS.to_owned(),
            architecture: std::env::consts::ARCH.to_owned(),
            cpu: std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_else(|_| "unknown".to_owned()),
            storage: "local-filesystem-type-undiscovered".to_owned(),
            rust_toolchain: rustc_version(),
            kernel_toolchain: if cfg!(feature = "cuda") {
                "nightly-2024-02-17;ptx-sm_86".to_owned()
            } else {
                "not-compiled".to_owned()
            },
            ..Self::default()
        }
    }

    fn with_configuration(mut self, details: &str) -> Self {
        self.configuration.push(';');
        self.configuration.push_str(details);
        self
    }
}

#[derive(Default)]
struct Report {
    rows: Vec<Row>,
}

impl Report {
    fn distribution(&mut self, base: Row, mut samples: Vec<(Duration, Row)>) {
        assert!(
            !samples.is_empty(),
            "benchmark distribution requires samples"
        );
        assert!(
            samples.iter().all(|(_, row)| row.correctness),
            "{} / {} timed sample disagreed with its oracle",
            base.suite,
            base.case
        );
        let observed_peak = peak_memory_bytes();
        for (_, row) in &mut samples {
            row.peak_memory_bytes = observed_peak;
        }
        let mut values: Vec<_> = samples.iter().map(|(elapsed, _)| nanos(*elapsed)).collect();
        values.sort_unstable();
        let mut summary = merge(base.clone(), samples[0].1.clone());
        macro_rules! median_optional_u64 {
            ($($field:ident),+ $(,)?) => {$({
                let mut field_values = samples
                    .iter()
                    .filter_map(|(_, row)| row.$field)
                    .collect::<Vec<_>>();
                field_values.sort_unstable();
                summary.$field = field_values
                    .get(field_values.len() / 2)
                    .copied();
            })+};
        }
        median_optional_u64!(
            rocksdb_bytes,
            bundle_bytes,
            resident_bytes,
            search_bytes,
            peak_memory_bytes,
            scan_ns,
            build_ns,
            validate_ns,
            load_ns,
            profile_pack_ns,
            first_destination_ns,
            full_completion_ns,
            examined_edges,
            relaxation_attempts,
            relaxation_updates,
            frontier_high_water,
            phases_or_buckets,
            cache_hits,
            cache_misses,
            cache_evictions,
            file_bytes,
            transfer_bytes,
            reserved_bytes,
            concurrent_routes,
            reconstruction_ns,
            hydration_ns,
            checkpoint_ns,
            restore_ns,
        );
        let mut throughput_values = samples
            .iter()
            .filter_map(|(_, row)| row.throughput_per_second)
            .collect::<Vec<_>>();
        throughput_values.sort_unstable_by(f64::total_cmp);
        summary.throughput_per_second = throughput_values.get(throughput_values.len() / 2).copied();
        for (sample, (elapsed, dynamic)) in samples.into_iter().enumerate() {
            let mut row = merge(base.clone(), dynamic);
            row.row_kind = "sample".to_owned();
            row.sample = Some(sample);
            row.elapsed_ns = Some(nanos(elapsed));
            self.rows.push(row);
        }
        summary.row_kind = "summary".to_owned();
        summary.median_ns = Some(values[values.len() / 2]);
        summary.minimum_ns = values.first().copied();
        summary.maximum_ns = values.last().copied();
        summary.spread_ns = Some(values.last().unwrap().saturating_sub(values[0]));
        self.rows.push(summary);
    }

    fn finish(self, format: OutputFormat) {
        assert!(self.rows.iter().all(|row| row.correctness));
        match format {
            OutputFormat::Json => write_json(&self.rows),
            OutputFormat::Csv => write_csv(&self.rows),
            OutputFormat::Human => {
                println!("PathHydra bounded benchmark summary (all correctness oracles passed)");
                for row in self.rows.iter().filter(|row| row.row_kind == "summary") {
                    println!(
                        "{:24} {:24} {:18} {:20} median={:>10} ns spread={:>10} ns samples={} correct={}",
                        row.suite,
                        row.case,
                        row.executor,
                        row.algorithm,
                        row.median_ns.unwrap_or(0),
                        row.spread_ns.unwrap_or(0),
                        row.repeats,
                        row.correctness
                    );
                }
            }
        }
    }
}

fn merge(mut base: Row, dynamic: Row) -> Row {
    macro_rules! optional {
        ($($field:ident),+ $(,)?) => {$({ if dynamic.$field.is_some() { base.$field = dynamic.$field; } })+};
    }
    optional!(
        rocksdb_bytes,
        bundle_bytes,
        resident_bytes,
        search_bytes,
        peak_memory_bytes,
        scan_ns,
        build_ns,
        validate_ns,
        load_ns,
        profile_pack_ns,
        first_destination_ns,
        full_completion_ns,
        examined_edges,
        relaxation_attempts,
        relaxation_updates,
        frontier_high_water,
        phases_or_buckets,
        cache_hits,
        cache_misses,
        cache_evictions,
        file_bytes,
        transfer_bytes,
        reserved_bytes,
        concurrent_routes,
        throughput_per_second,
        reconstruction_ns,
        hydration_ns,
        checkpoint_ns,
        restore_ns
    );
    if dynamic.nodes != 0 {
        base.nodes = dynamic.nodes;
    }
    if dynamic.relation_kinds != 0 {
        base.relation_kinds = dynamic.relation_kinds;
    }
    if dynamic.edges != 0 {
        base.edges = dynamic.edges;
    }
    if dynamic.partitions != 0 {
        base.partitions = dynamic.partitions;
    }
    base.cold = dynamic.cold;
    base.correctness = dynamic.correctness;
    if !dynamic.gpu.is_empty() {
        base.gpu = dynamic.gpu;
    }
    if !dynamic.cuda_driver.is_empty() {
        base.cuda_driver = dynamic.cuda_driver;
    }
    base
}

pub fn run(suite: &str, options: Options) {
    assert!(options.repeats > 0);
    let mut report = Report::default();
    match suite {
        "store-ingest" => store_ingest(&mut report, options),
        "store-mutation" => store_mutation(&mut report, options),
        "snapshot-build-load" => snapshot_build_load(&mut report, options, false),
        "cpu-routing" => cpu_routing(&mut report, options),
        "cuda-resident" => cuda_resident(&mut report, options),
        "cuda-out-of-core" => cuda_out_of_core(&mut report, options),
        "concurrency" => concurrency(&mut report, options),
        "reconstruction-hydration" => reconstruction_hydration(&mut report, options),
        "backup-restore" => backup_restore(&mut report, options),
        "scale" => snapshot_build_load(&mut report, options, true),
        "all" => {
            store_ingest(&mut report, options);
            store_mutation(&mut report, options);
            snapshot_build_load(&mut report, options, false);
            cpu_routing(&mut report, options);
            cuda_resident(&mut report, options);
            cuda_out_of_core(&mut report, options);
            concurrency(&mut report, options);
            reconstruction_hydration(&mut report, options);
            backup_restore(&mut report, options);
            snapshot_build_load(&mut report, options, true);
        }
        _ => unreachable!("validated suite"),
    }
    report.finish(options.format);
}

fn store_ingest(report: &mut Report, options: Options) {
    let oracle = ingest_once(128);
    assert_eq!(oracle.1, (128, 1, 127));
    for _ in 0..options.warmups {
        let _ = ingest_once(128);
    }
    let samples = (0..options.repeats)
        .map(|_| {
            let (elapsed, (nodes, relations, edges), bytes) = ingest_once(128);
            (
                elapsed,
                Row {
                    nodes,
                    relation_kinds: relations,
                    edges,
                    rocksdb_bytes: Some(bytes),
                    correctness: true,
                    ..Row::default()
                },
            )
        })
        .collect();
    report.distribution(
        Row::base(
            "store-ingest",
            "chain-128",
            "rocksdb",
            "candidate-confirm",
            options,
        )
        .with_configuration(
            "catalog=default-current-layout;ordinary-writes=wal-enabled-nonsync;nodes=128",
        ),
        samples,
    );
}

fn ingest_once(count: usize) -> (Duration, (u64, u64, u64), u64) {
    let root = tempfile::tempdir().expect("ingest tempdir");
    let database = root.path().join("catalog");
    let started = Instant::now();
    let catalog = Catalog::open(&database).expect("open ingest catalog");
    let (nodes, relation, edges) = populate(&catalog, count, false);
    let elapsed = started.elapsed();
    let summary = catalog.summary().expect("ingest summary");
    assert_eq!(summary.confirmed_nodes, nodes.len() as u64);
    assert_eq!(summary.relation_kinds, 1);
    assert_eq!(summary.confirmed_edges, edges.len() as u64);
    drop(catalog);
    (
        elapsed,
        (
            nodes.len() as u64,
            u64::from(relation.as_u64() != 0),
            edges.len() as u64,
        ),
        directory_bytes(&database),
    )
}

fn store_mutation(report: &mut Report, options: Options) {
    let oracle = mutation_once(96);
    assert!(oracle.1);
    for _ in 0..options.warmups {
        let _ = mutation_once(96);
    }
    let samples = (0..options.repeats)
        .map(|_| {
            let (elapsed, correct, bytes) = mutation_once(96);
            (
                elapsed,
                Row {
                    nodes: 96,
                    relation_kinds: 1,
                    edges: 285,
                    rocksdb_bytes: Some(bytes),
                    correctness: correct,
                    ..Row::default()
                },
            )
        })
        .collect();
    report.distribution(
        Row::base(
            "store-mutation",
            "high-degree-delete-churn",
            "rocksdb",
            "atomic-node-cascade",
            options,
        )
        .with_configuration(
            "catalog=default-current-layout;ordinary-writes=wal-enabled-nonsync;nodes=96;high-degree-fanout=3",
        ),
        samples,
    );
}

fn mutation_once(count: usize) -> (Duration, bool, u64) {
    let root = tempfile::tempdir().expect("mutation tempdir");
    let database = root.path().join("catalog");
    let catalog = Catalog::open(&database).expect("open mutation catalog");
    let (nodes, _, _) = populate(&catalog, count, true);
    let started = Instant::now();
    catalog
        .remove_node(nodes[0])
        .expect("atomic high-degree node removal");
    let elapsed = started.elapsed();
    let summary = catalog.summary().expect("mutation summary");
    let correct = catalog.get_node(nodes[0]).is_err()
        && summary.confirmed_nodes == count as u64 - 1
        && summary.confirmed_edges == count as u64 - 2;
    drop(catalog);
    (elapsed, correct, directory_bytes(&database))
}

fn snapshot_build_load(report: &mut Report, options: Options, scale: bool) {
    let workload = if scale {
        Workload {
            name: "bounded-scale-2048",
            nodes: 2_048,
            shape: Shape::Mixed,
        }
    } else {
        Workload {
            name: "snapshot-mixed-512",
            nodes: 512,
            shape: Shape::Mixed,
        }
    };
    let oracle = fixtures::build(workload);
    assert_eq!(oracle.image.node_count(), workload.nodes);
    assert_eq!(
        open_bundle(&oracle.bundle_path)
            .unwrap()
            .snapshot()
            .total_bytes,
        oracle.build_metrics.bundle_bytes
    );
    for _ in 0..options.warmups {
        let _ = fixtures::build(workload);
    }
    let samples = (0..options.repeats)
        .map(|_| {
            let started = Instant::now();
            let fixture = fixtures::build(workload);
            let elapsed = started.elapsed();
            let load_started = Instant::now();
            let reopened = open_bundle(&fixture.bundle_path).expect("reopen benchmark bundle");
            let load = load_started.elapsed();
            let metrics = &fixture.build_metrics;
            let correct = reopened.snapshot().total_bytes == metrics.bundle_bytes;
            (
                elapsed,
                Row {
                    nodes: metrics.node_count,
                    relation_kinds: metrics.relation_kind_count,
                    edges: metrics.adjacency_count,
                    partitions: metrics.partition_count,
                    bundle_bytes: Some(metrics.bundle_bytes),
                    scan_ns: Some(nanos(metrics.scan_duration)),
                    build_ns: Some(nanos(metrics.write_duration)),
                    validate_ns: Some(nanos(metrics.validation_duration)),
                    load_ns: Some(nanos(load)),
                    correctness: correct,
                    ..Row::default()
                },
            )
        })
        .collect();
    let suite = if scale {
        "scale"
    } else {
        "snapshot-build-load"
    };
    report.distribution(
        Row::base(
            suite,
            workload.name,
            "bundle",
            "scan-build-validate-open",
            options,
        )
        .with_configuration(&format!(
            "dataset={};nodes={};partition-target-bytes=4096;partition-hard-max-bytes=4096;bundle-max-bytes=1073741824",
            workload.name, workload.nodes
        )),
        samples,
    );
}

fn cpu_routing(report: &mut Report, options: Options) {
    for workload in fixtures::BASELINE {
        let fixture = fixtures::build(*workload);
        let (oracle, _) = route_controlled(&fixture.image, &fixture.request, &NeverCancelled)
            .expect("CPU oracle");
        for _ in 0..options.warmups {
            let _ = route(&fixture.image, &fixture.request).unwrap();
        }
        let estimate = estimate_cpu_working_set(&fixture.image, &fixture.request).unwrap();
        let samples = (0..options.repeats)
            .map(|_| {
                let profile_started = Instant::now();
                let _ = fixture.request.profile().pack(&fixture.image).unwrap();
                let profile = profile_started.elapsed();
                let started = Instant::now();
                let (actual, diagnostics) =
                    route_controlled(&fixture.image, &fixture.request, &NeverCancelled).unwrap();
                let elapsed = started.elapsed();
                let correct = crate::routing::same_distances(&oracle, &actual);
                (
                    elapsed,
                    Row {
                        nodes: fixture.image.node_count() as u64,
                        relation_kinds: fixture.image.relation_kind_count() as u64,
                        edges: fixture.image.adjacency_count() as u64,
                        resident_bytes: Some(fixture.image.manifest().byte_counts().total() as u64),
                        search_bytes: Some(estimate.bytes() as u64),
                        profile_pack_ns: Some(nanos(profile)),
                        first_destination_ns: diagnostics.first_destination_duration.map(nanos),
                        full_completion_ns: Some(nanos(elapsed)),
                        examined_edges: Some(diagnostics.examined_edges),
                        relaxation_attempts: Some(diagnostics.examined_edges),
                        relaxation_updates: Some(diagnostics.relaxation_updates),
                        frontier_high_water: Some(diagnostics.frontier_high_water_mark as u64),
                        reconstruction_ns: fixture
                            .request
                            .return_paths()
                            .then(|| nanos(diagnostics.path_reconstruction_duration)),
                        correctness: correct,
                        ..Row::default()
                    },
                )
            })
            .collect();
        report.distribution(
            Row::base(
                "cpu-routing",
                case_name(workload.name),
                "cpu-resident",
                "dijkstra",
                options,
            )
            .with_configuration(&format!(
                "dataset={};nodes={};destinations=4;paths=false;budget=unlimited;tie=stable-predecessor",
                workload.name, workload.nodes
            )),
            samples,
        );

        let (partition_oracle, _, _) =
            route_partitioned_controlled(&fixture.chunked, &fixture.request, &NeverCancelled)
                .unwrap();
        assert!(crate::routing::same_distances(&oracle, &partition_oracle));
        let samples = (0..options.repeats)
            .map(|_| {
                let before = fixture.chunked.cache_snapshot();
                let started = Instant::now();
                let (actual, diagnostics, partitioned) = route_partitioned_controlled(
                    &fixture.chunked,
                    &fixture.request,
                    &NeverCancelled,
                )
                .unwrap();
                let elapsed = started.elapsed();
                let after = fixture.chunked.cache_snapshot();
                (
                    elapsed,
                    Row {
                        nodes: fixture.chunked.node_count() as u64,
                        relation_kinds: fixture.chunked.relation_kind_count() as u64,
                        edges: fixture.chunked.adjacency_count() as u64,
                        partitions: partitioned.partitions,
                        bundle_bytes: Some(fixture.build_metrics.bundle_bytes),
                        search_bytes: Some(estimate.bytes() as u64),
                        first_destination_ns: diagnostics.first_destination_duration.map(nanos),
                        full_completion_ns: Some(nanos(elapsed)),
                        examined_edges: Some(diagnostics.examined_edges),
                        relaxation_attempts: Some(diagnostics.examined_edges),
                        relaxation_updates: Some(diagnostics.relaxation_updates),
                        frontier_high_water: Some(diagnostics.frontier_high_water_mark as u64),
                        reconstruction_ns: fixture
                            .request
                            .return_paths()
                            .then(|| nanos(diagnostics.path_reconstruction_duration)),
                        cache_hits: Some(after.hits.saturating_sub(before.hits)),
                        cache_misses: Some(after.misses.saturating_sub(before.misses)),
                        cache_evictions: Some(after.evictions.saturating_sub(before.evictions)),
                        file_bytes: Some(partitioned.file_bytes),
                        cold: false,
                        correctness: crate::routing::same_distances(&oracle, &actual),
                        ..Row::default()
                    },
                )
            })
            .collect();
        report.distribution(
            Row::base(
                "cpu-routing",
                case_name(workload.name),
                "cpu-partitioned",
                "dijkstra",
                options,
            )
            .with_configuration(&format!(
                "dataset={};nodes={};destinations=4;paths=false;budget=unlimited;tie=stable-predecessor;host-cache-bytes=131072;host-staging-bytes=8192;host-cache-entries=8;io-workers=2;queued-reads=8",
                workload.name, workload.nodes
            )),
            samples,
        );
    }
}

#[cfg(feature = "cuda")]
fn cuda_resident(report: &mut Report, options: Options) {
    let context = pathhydra_cuda::CudaContextOwner::initialize(0).expect("CUDA device");
    for workload in fixtures::BASELINE {
        let fixture = fixtures::build(*workload);
        let oracle = route(&fixture.image, &fixture.request).unwrap();
        for algorithm in cuda_algorithms() {
            let resident = pathhydra_cuda::CudaResidentImage::upload(
                Arc::clone(&context),
                Arc::clone(&fixture.image),
                usize::MAX,
                0,
            )
            .unwrap();
            let reserved = pathhydra_cuda::estimate_search_bytes(
                fixture.image.node_count(),
                fixture.image.relation_kind_count(),
                fixture.image.adjacency_count(),
                fixture.request.destinations().len(),
                algorithm,
            )
            .unwrap();
            let untimed = resident
                .route(
                    &fixture.request,
                    algorithm,
                    &std::sync::atomic::AtomicBool::new(false),
                    reserved,
                )
                .unwrap();
            assert!(crate::routing::same_distances(&oracle, &untimed.response));
            for _ in 0..options.warmups {
                let _ = resident
                    .route(
                        &fixture.request,
                        algorithm,
                        &std::sync::atomic::AtomicBool::new(false),
                        reserved,
                    )
                    .unwrap();
            }
            let samples = (0..options.repeats)
                .map(|_| {
                    let started = Instant::now();
                    let output = resident
                        .route(
                            &fixture.request,
                            algorithm,
                            &std::sync::atomic::AtomicBool::new(false),
                            reserved,
                        )
                        .unwrap();
                    let elapsed = started.elapsed();
                    let d = output.diagnostics;
                    (
                        elapsed,
                        Row {
                            nodes: fixture.image.node_count() as u64,
                            relation_kinds: fixture.image.relation_kind_count() as u64,
                            edges: fixture.image.adjacency_count() as u64,
                            resident_bytes: Some(resident.allocated_bytes() as u64),
                            search_bytes: Some(reserved as u64),
                            first_destination_ns: d.first_destination_duration.map(nanos),
                            full_completion_ns: Some(nanos(elapsed)),
                            examined_edges: Some(d.examined_edges),
                            relaxation_attempts: Some(d.relaxation_attempts),
                            relaxation_updates: Some(d.relaxation_updates),
                            phases_or_buckets: Some(d.phases),
                            file_bytes: Some(d.file_bytes),
                            transfer_bytes: Some(d.transfer_bytes),
                            reserved_bytes: Some(d.reserved_search_bytes as u64),
                            reconstruction_ns: fixture
                                .request
                                .return_paths()
                                .then(|| nanos(d.path_reconstruction_duration)),
                            correctness: crate::routing::same_distances(&oracle, &output.response),
                            gpu: context.capabilities().device_name.clone(),
                            cuda_driver: context.capabilities().driver_version.to_string(),
                            ..Row::default()
                        },
                    )
                })
                .collect();
            report.distribution(
                Row::base(
                    "cuda-resident",
                    case_name(workload.name),
                    "cuda-resident",
                    algorithm_name(algorithm),
                    options,
                )
                .with_configuration(&format!(
                    "dataset={};nodes={};destinations=4;paths=false;budget=unlimited;device=0;topology-headroom-bytes=0",
                    workload.name, workload.nodes
                )),
                samples,
            );
        }
    }
}

#[cfg(not(feature = "cuda"))]
fn cuda_resident(_: &mut Report, _: Options) {
    eprintln!("cuda-resident skipped: rebuild pathhydra-bench with --features cuda");
}

#[cfg(feature = "cuda")]
fn cuda_out_of_core(report: &mut Report, options: Options) {
    let context = pathhydra_cuda::CudaContextOwner::initialize(0).expect("CUDA device");
    for workload in fixtures::BASELINE {
        let fixture = fixtures::build(*workload);
        let oracle = route(&fixture.image, &fixture.request).unwrap();
        for algorithm in cuda_algorithms() {
            let image = pathhydra_cuda::CudaPartitionedImage::upload(
                Arc::clone(&context),
                Arc::clone(&fixture.chunked),
                pathhydra_cuda::CudaPartitionedConfig {
                    maximum_topology_cache_bytes: 16 * 1024,
                    maximum_topology_cache_slots: 2,
                    maximum_host_staging_bytes: 8 * 1024,
                    minimum_free_memory_headroom: 0,
                    reserved_concurrent_search_bytes: 64 * 1024 * 1024,
                    reverse_partition_order: false,
                },
            )
            .unwrap();
            let reserved = pathhydra_cuda::estimate_search_bytes(
                fixture.chunked.node_count(),
                fixture.chunked.relation_kind_count(),
                fixture.chunked.adjacency_count(),
                fixture.request.destinations().len(),
                algorithm,
            )
            .unwrap();
            let untimed = image
                .route(
                    &fixture.request,
                    algorithm,
                    &std::sync::atomic::AtomicBool::new(false),
                    reserved,
                )
                .unwrap();
            assert!(crate::routing::same_distances(&oracle, &untimed.response));
            for _ in 0..options.warmups {
                let _ = image
                    .route(
                        &fixture.request,
                        algorithm,
                        &std::sync::atomic::AtomicBool::new(false),
                        reserved,
                    )
                    .unwrap();
            }
            let samples = (0..options.repeats)
                .map(|_| {
                    let before = image.topology_cache_snapshot();
                    let started = Instant::now();
                    let output = image
                        .route(
                            &fixture.request,
                            algorithm,
                            &std::sync::atomic::AtomicBool::new(false),
                            reserved,
                        )
                        .unwrap();
                    let elapsed = started.elapsed();
                    let after = image.topology_cache_snapshot();
                    let d = output.diagnostics;
                    (
                        elapsed,
                        Row {
                            nodes: fixture.chunked.node_count() as u64,
                            relation_kinds: fixture.chunked.relation_kind_count() as u64,
                            edges: fixture.chunked.adjacency_count() as u64,
                            partitions: d.partitions_required,
                            bundle_bytes: Some(fixture.build_metrics.bundle_bytes),
                            search_bytes: Some(reserved as u64),
                            first_destination_ns: d.first_destination_duration.map(nanos),
                            full_completion_ns: Some(nanos(elapsed)),
                            examined_edges: Some(d.examined_edges),
                            relaxation_attempts: Some(d.relaxation_attempts),
                            relaxation_updates: Some(d.relaxation_updates),
                            phases_or_buckets: Some(d.phases),
                            cache_hits: Some(after.hits.saturating_sub(before.hits)),
                            cache_misses: Some(after.misses.saturating_sub(before.misses)),
                            cache_evictions: Some(after.evictions.saturating_sub(before.evictions)),
                            file_bytes: Some(d.file_bytes),
                            transfer_bytes: Some(d.transfer_bytes),
                            reserved_bytes: Some(d.reserved_search_bytes as u64),
                            reconstruction_ns: fixture
                                .request
                                .return_paths()
                                .then(|| nanos(d.path_reconstruction_duration)),
                            correctness: crate::routing::same_distances(&oracle, &output.response),
                            gpu: context.capabilities().device_name.clone(),
                            cuda_driver: context.capabilities().driver_version.to_string(),
                            ..Row::default()
                        },
                    )
                })
                .collect();
            report.distribution(
                Row::base(
                    "cuda-out-of-core",
                    case_name(workload.name),
                    "cuda-partitioned",
                    algorithm_name(algorithm),
                    options,
                )
                .with_configuration(&format!(
                    "dataset={};nodes={};destinations=4;paths=false;budget=unlimited;device=0;device-cache-bytes=16384;device-cache-slots=2;host-staging-bytes=8192;headroom-bytes=0;concurrent-search-reserve-bytes=67108864;reverse-partitions=false",
                    workload.name, workload.nodes
                )),
                samples,
            );
        }
    }
}

#[cfg(not(feature = "cuda"))]
fn cuda_out_of_core(_: &mut Report, _: Options) {
    eprintln!("cuda-out-of-core skipped: rebuild pathhydra-bench with --features cuda");
}

fn concurrency(report: &mut Report, options: Options) {
    let fixture = fixtures::build(Workload {
        name: "concurrent-mixed",
        nodes: 512,
        shape: Shape::Mixed,
    });
    let oracle = route(&fixture.image, &fixture.request).unwrap();
    for lanes in [1_usize, 2, 4] {
        let run = || {
            let barrier = Arc::new(Barrier::new(lanes));
            let started = Instant::now();
            let workers: Vec<_> = (0..lanes)
                .map(|_| {
                    let image = Arc::clone(&fixture.image);
                    let request = fixture.request.clone();
                    let barrier = Arc::clone(&barrier);
                    std::thread::spawn(move || {
                        barrier.wait();
                        route(&image, &request).unwrap()
                    })
                })
                .collect();
            let responses: Vec<_> = workers
                .into_iter()
                .map(|worker| worker.join().unwrap())
                .collect();
            let elapsed = started.elapsed();
            let correct = responses
                .iter()
                .all(|response| crate::routing::same_distances(&oracle, response));
            (elapsed, correct)
        };
        let oracle_run = run();
        assert!(oracle_run.1);
        for _ in 0..options.warmups {
            let _ = run();
        }
        let samples = (0..options.repeats)
            .map(|_| {
                let (elapsed, correct) = run();
                (
                    elapsed,
                    Row {
                        nodes: fixture.image.node_count() as u64,
                        relation_kinds: 1,
                        edges: fixture.image.adjacency_count() as u64,
                        concurrent_routes: Some(lanes as u64),
                        throughput_per_second: Some(lanes as f64 / elapsed.as_secs_f64()),
                        correctness: correct,
                        ..Row::default()
                    },
                )
            })
            .collect();
        report.distribution(
            Row::base(
                "concurrency",
                &format!("mixed-{lanes}-routes"),
                "cpu-resident",
                "parallel-requests",
                options,
            )
            .with_configuration(&format!(
                "dataset=mixed-locality;nodes=256;lanes={lanes};paths=false;budget=unlimited"
            )),
            samples,
        );
    }
}

fn reconstruction_hydration(report: &mut Report, options: Options) {
    let run = |ordinal: u64| {
        let root = tempfile::tempdir().unwrap();
        let database = root.path().join("catalog");
        let catalog = Catalog::open(&database).unwrap();
        let (nodes, relation, edges) = populate(&catalog, 64, false);
        drop(catalog);
        let engine = GraphEngine::open(&database, EngineConfig::default()).unwrap();
        let request = RoutingRequest::new(
            nodes[0],
            [nodes[63]],
            RelationProfile::new([(
                relation,
                RelationUse::Enabled(RelationMultiplier::new(1.0).unwrap()),
            )]),
            true,
            SearchBudget::Unlimited,
            TiePolicy::StablePredecessor,
        );
        let route_started = Instant::now();
        let routed = engine.route(RequestId::new(ordinal), &request).unwrap();
        let route_elapsed = route_started.elapsed();
        let reconstruction = routed.diagnostics.reconstruction_duration;
        assert!(
            matches!(routed.response.results()[0].state(), DestinationState::Exact(exact) if exact.path().is_some())
        );
        let hydration_started = Instant::now();
        let hydrated = engine.hydrate_path(&routed.response, 0).unwrap();
        let hydration = hydration_started.elapsed();
        let arbitrary = engine
            .hydrate(&HydrationRequest::new(
                nodes.clone(),
                edges.clone(),
                Some(request.profile().clone()),
            ))
            .unwrap();
        let correct =
            hydrated.nodes.len() == 64 && hydrated.edges.len() == 63 && arbitrary.nodes.len() == 64;
        assert!(engine.shutdown().unwrap().complete());
        (
            route_elapsed + hydration,
            reconstruction,
            hydration,
            correct,
        )
    };
    assert!(run(1).3);
    for index in 0..options.warmups {
        let _ = run(10 + index as u64);
    }
    let samples = (0..options.repeats)
        .map(|index| {
            let (elapsed, reconstruction, hydration, correct) = run(100 + index as u64);
            (
                elapsed,
                Row {
                    nodes: 64,
                    relation_kinds: 1,
                    edges: 63,
                    reconstruction_ns: Some(nanos(reconstruction)),
                    hydration_ns: Some(nanos(hydration)),
                    correctness: correct,
                    ..Row::default()
                },
            )
        })
        .collect();
    report.distribution(
        Row::base(
            "reconstruction-hydration",
            "far-path-63-steps",
            "engine-cpu",
            "stable-predecessor-current-hydration",
            options,
        )
        .with_configuration(
            "catalog=default-current-layout;engine=default;nodes=64;path-steps=63;paths=true;hydration=current-state",
        ),
        samples,
    );
}

fn backup_restore(report: &mut Report, options: Options) {
    let oracle = backup_restore_once();
    assert!(oracle.3);
    for _ in 0..options.warmups {
        let _ = backup_restore_once();
    }
    let samples = (0..options.repeats)
        .map(|_| {
            let (checkpoint, restore, bytes, correct) = backup_restore_once();
            (
                checkpoint + restore,
                Row {
                    nodes: 64,
                    relation_kinds: 1,
                    edges: 63,
                    file_bytes: Some(bytes),
                    checkpoint_ns: Some(nanos(checkpoint)),
                    restore_ns: Some(nanos(restore)),
                    correctness: correct,
                    ..Row::default()
                },
            )
        })
        .collect();
    report.distribution(
        Row::base(
            "backup-restore",
            "idle-checkpoint-fresh-restore",
            "rocksdb",
            "checkpoint-verify-restore",
            options,
        )
        .with_configuration(
            "catalog=default-current-layout;nodes=64;checkpoint=rocksdb-native;restore=fresh-destination;verification=bounded",
        ),
        samples,
    );
}

fn backup_restore_once() -> (Duration, Duration, u64, bool) {
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("catalog");
    let checkpoint_root = root.path().join("checkpoints");
    let restore_root = root.path().join("restores");
    fs::create_dir_all(&checkpoint_root).unwrap();
    fs::create_dir_all(&restore_root).unwrap();
    let checkpoint = checkpoint_root.join("cp");
    let restored = restore_root.join("restored");
    let catalog = Catalog::open(&database).unwrap();
    let _ = populate(&catalog, 64, false);
    let checkpoint_started = Instant::now();
    let checkpoint_report = catalog
        .create_checkpoint(&CheckpointRequest {
            destination_root: checkpoint_root.clone(),
            destination: checkpoint.clone(),
            routing_image_root: None,
            scratch_path: None,
            available_destination_bytes: u64::MAX,
            minimum_headroom_bytes: 0,
        })
        .unwrap();
    let checkpoint_elapsed = checkpoint_started.elapsed();
    drop(catalog);
    let restore_started = Instant::now();
    let restore_report = restore_checkpoint(&RestoreRequest {
        source_root: checkpoint_root,
        source_checkpoint: checkpoint,
        destination_root: restore_root,
        destination: restored.clone(),
        routing_image_root: None,
        scratch_path: None,
        available_destination_bytes: u64::MAX,
        minimum_headroom_bytes: 0,
        verification_limits: VerificationLimits::default(),
    })
    .unwrap();
    let restore_elapsed = restore_started.elapsed();
    let restored_catalog = Catalog::open(restored).unwrap();
    let verified = restored_catalog
        .verify(VerificationLimits::default())
        .unwrap();
    let correct = checkpoint_report.catalog.catalog_checksum
        == restore_report.restored_catalog.catalog_checksum
        && verified.catalog_checksum == checkpoint_report.catalog.catalog_checksum;
    (
        checkpoint_elapsed,
        restore_elapsed,
        checkpoint_report.bytes,
        correct,
    )
}

fn populate(
    catalog: &Catalog,
    count: usize,
    high_degree: bool,
) -> (Vec<NodeId>, RelationId, Vec<EdgeId>) {
    let nodes: Vec<_> = (0..count)
        .map(|index| {
            let candidate = catalog
                .insert_node_candidate(format!("bench-node-{index}"))
                .unwrap();
            let ConfirmedRecord::Node(node) =
                catalog.confirm_validated_candidate(candidate).unwrap()
            else {
                unreachable!()
            };
            node.id()
        })
        .collect();
    let candidate = catalog.insert_relation_candidate("bench-relation").unwrap();
    let ConfirmedRecord::Relation(relation) =
        catalog.confirm_validated_candidate(candidate).unwrap()
    else {
        unreachable!()
    };
    let mut edges = Vec::new();
    for pair in nodes.windows(2) {
        edges.push(confirm_edge(catalog, pair[0], pair[1], relation.id()));
    }
    if high_degree {
        for &node in &nodes[1..] {
            edges.push(confirm_edge(catalog, nodes[0], node, relation.id()));
            edges.push(confirm_edge(catalog, node, nodes[0], relation.id()));
        }
    }
    (nodes, relation.id(), edges)
}

fn confirm_edge(
    catalog: &Catalog,
    source: NodeId,
    destination: NodeId,
    relation: RelationId,
) -> EdgeId {
    let candidate = catalog
        .insert_edge_candidate(source, destination, relation, 0.01)
        .unwrap();
    let ConfirmedRecord::Edge(edge) = catalog.confirm_validated_candidate(candidate).unwrap()
    else {
        unreachable!()
    };
    edge.id()
}

fn case_name(name: &str) -> &str {
    match name {
        "narrow-chain" => "narrow-near-far",
        "broad-star" => "broad-high-degree",
        "dense-scc" => "dense",
        "zero-closure" => "zero-closure",
        "disconnected-regions" => "unreachable",
        "mixed-locality" => "churn-mixed-locality",
        other => other,
    }
}

#[cfg(feature = "cuda")]
fn cuda_algorithms() -> [pathhydra_cuda::CudaAlgorithm; 2] {
    [
        pathhydra_cuda::CudaAlgorithm::Frontier,
        pathhydra_cuda::CudaAlgorithm::DeltaStepping(
            pathhydra_cuda::DeltaConfiguration::new(0.1).unwrap(),
        ),
    ]
}

#[cfg(feature = "cuda")]
fn algorithm_name(algorithm: pathhydra_cuda::CudaAlgorithm) -> &'static str {
    match algorithm {
        pathhydra_cuda::CudaAlgorithm::Frontier => "frontier",
        pathhydra_cuda::CudaAlgorithm::DeltaStepping(_) => "delta-stepping-0.1",
    }
}

struct NeverCancelled;
impl pathhydra_routing::CancellationSignal for NeverCancelled {
    fn is_cancelled(&self) -> bool {
        false
    }
}

fn nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

#[cfg(target_os = "windows")]
pub(crate) fn peak_memory_bytes() -> Option<u64> {
    let command = format!("(Get-Process -Id {}).PeakWorkingSet64", std::process::id());
    let output = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-Command", &command])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().parse().ok())
        .flatten()
}

#[cfg(target_os = "linux")]
pub(crate) fn peak_memory_bytes() -> Option<u64> {
    fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))?
        .split_whitespace()
        .next()?
        .parse::<u64>()
        .ok()?
        .checked_mul(1024)
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
pub(crate) fn peak_memory_bytes() -> Option<u64> {
    None
}

fn directory_bytes(path: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(path) else {
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

pub(crate) fn rustc_version() -> String {
    std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .unwrap_or_else(|| "unknown".to_owned())
}

fn csv(value: impl ToString) -> String {
    let value = value.to_string();
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value
    }
}

fn option<T: ToString>(value: Option<T>) -> String {
    value.map_or_else(String::new, |value| value.to_string())
}

fn json_string(value: &str) -> String {
    let mut output = String::with_capacity(value.len() + 2);
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{08}' => output.push_str("\\b"),
            '\u{0c}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            control if control <= '\u{1f}' => {
                use std::fmt::Write as _;
                write!(&mut output, "\\u{:04x}", control as u32).expect("write JSON escape");
            }
            other => output.push(other),
        }
    }
    output.push('"');
    output
}

fn write_json(rows: &[Row]) {
    println!("[");
    for (index, row) in rows.iter().enumerate() {
        let mut fields = Vec::with_capacity(58);
        macro_rules! string {
            ($name:literal, $value:expr) => {
                fields.push(format!("\"{}\":{}", $name, json_string($value)))
            };
        }
        macro_rules! value {
            ($name:literal, $value:expr) => {
                fields.push(format!("\"{}\":{}", $name, $value))
            };
        }
        macro_rules! optional {
            ($name:literal, $value:expr) => {
                fields.push(format!(
                    "\"{}\":{}",
                    $name,
                    $value.map_or_else(|| "null".to_owned(), |value| value.to_string())
                ))
            };
        }
        string!("suite", &row.suite);
        string!("case", &row.case);
        string!("executor", &row.executor);
        string!("algorithm", &row.algorithm);
        string!("configuration", &row.configuration);
        string!("row_kind", &row.row_kind);
        optional!("sample", row.sample);
        value!("repeats", row.repeats);
        value!("warmups", row.warmups);
        value!("seed", row.seed);
        value!("cold", row.cold);
        value!("nodes", row.nodes);
        value!("relation_kinds", row.relation_kinds);
        value!("edges", row.edges);
        value!("partitions", row.partitions);
        optional!("rocksdb_bytes", row.rocksdb_bytes);
        optional!("bundle_bytes", row.bundle_bytes);
        optional!("resident_bytes", row.resident_bytes);
        optional!("search_bytes", row.search_bytes);
        optional!("peak_memory_bytes", row.peak_memory_bytes);
        optional!("elapsed_ns", row.elapsed_ns);
        optional!("median_ns", row.median_ns);
        optional!("minimum_ns", row.minimum_ns);
        optional!("maximum_ns", row.maximum_ns);
        optional!("spread_ns", row.spread_ns);
        optional!("scan_ns", row.scan_ns);
        optional!("build_ns", row.build_ns);
        optional!("validate_ns", row.validate_ns);
        optional!("load_ns", row.load_ns);
        optional!("profile_pack_ns", row.profile_pack_ns);
        optional!("first_destination_ns", row.first_destination_ns);
        optional!("full_completion_ns", row.full_completion_ns);
        optional!("examined_edges", row.examined_edges);
        optional!("relaxation_attempts", row.relaxation_attempts);
        optional!("relaxation_updates", row.relaxation_updates);
        optional!("frontier_high_water", row.frontier_high_water);
        optional!("phases_or_buckets", row.phases_or_buckets);
        optional!("cache_hits", row.cache_hits);
        optional!("cache_misses", row.cache_misses);
        optional!("cache_evictions", row.cache_evictions);
        optional!("file_bytes", row.file_bytes);
        optional!("transfer_bytes", row.transfer_bytes);
        optional!("reserved_bytes", row.reserved_bytes);
        optional!("concurrent_routes", row.concurrent_routes);
        fields.push(format!(
            "\"throughput_per_second\":{}",
            row.throughput_per_second
                .filter(|value| value.is_finite())
                .map_or_else(|| "null".to_owned(), |value| value.to_string())
        ));
        optional!("reconstruction_ns", row.reconstruction_ns);
        optional!("hydration_ns", row.hydration_ns);
        optional!("checkpoint_ns", row.checkpoint_ns);
        optional!("restore_ns", row.restore_ns);
        value!("correctness", row.correctness);
        string!("os", &row.os);
        string!("architecture", &row.architecture);
        string!("cpu", &row.cpu);
        string!("storage", &row.storage);
        string!("rust_toolchain", &row.rust_toolchain);
        string!("kernel_toolchain", &row.kernel_toolchain);
        string!("gpu", &row.gpu);
        string!("cuda_driver", &row.cuda_driver);
        println!(
            "  {{{}}}{}",
            fields.join(","),
            if index + 1 == rows.len() { "" } else { "," }
        );
    }
    println!("]");
}

fn write_csv(rows: &[Row]) {
    println!(
        "suite,case,executor,algorithm,configuration,row_kind,sample,repeats,warmups,seed,cold,nodes,relation_kinds,edges,partitions,rocksdb_bytes,bundle_bytes,resident_bytes,search_bytes,peak_memory_bytes,elapsed_ns,median_ns,minimum_ns,maximum_ns,spread_ns,scan_ns,build_ns,validate_ns,load_ns,profile_pack_ns,first_destination_ns,full_completion_ns,examined_edges,relaxation_attempts,relaxation_updates,frontier_high_water,phases_or_buckets,cache_hits,cache_misses,cache_evictions,file_bytes,transfer_bytes,reserved_bytes,concurrent_routes,throughput_per_second,reconstruction_ns,hydration_ns,checkpoint_ns,restore_ns,correctness,os,architecture,cpu,storage,rust_toolchain,kernel_toolchain,gpu,cuda_driver"
    );
    for row in rows {
        println!(
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
            csv(&row.suite),
            csv(&row.case),
            csv(&row.executor),
            csv(&row.algorithm),
            csv(&row.configuration),
            row.row_kind,
            option(row.sample),
            row.repeats,
            row.warmups,
            row.seed,
            row.cold,
            row.nodes,
            row.relation_kinds,
            row.edges,
            row.partitions,
            option(row.rocksdb_bytes),
            option(row.bundle_bytes),
            option(row.resident_bytes),
            option(row.search_bytes),
            option(row.peak_memory_bytes),
            option(row.elapsed_ns),
            option(row.median_ns),
            option(row.minimum_ns),
            option(row.maximum_ns),
            option(row.spread_ns),
            option(row.scan_ns),
            option(row.build_ns),
            option(row.validate_ns),
            option(row.load_ns),
            option(row.profile_pack_ns),
            option(row.first_destination_ns),
            option(row.full_completion_ns),
            option(row.examined_edges),
            option(row.relaxation_attempts),
            option(row.relaxation_updates),
            option(row.frontier_high_water),
            option(row.phases_or_buckets),
            option(row.cache_hits),
            option(row.cache_misses),
            option(row.cache_evictions),
            option(row.file_bytes),
            option(row.transfer_bytes),
            option(row.reserved_bytes),
            option(row.concurrent_routes),
            option(row.throughput_per_second),
            option(row.reconstruction_ns),
            option(row.hydration_ns),
            option(row.checkpoint_ns),
            option(row.restore_ns),
            row.correctness,
            csv(&row.os),
            csv(&row.architecture),
            csv(&row.cpu),
            csv(&row.storage),
            csv(&row.rust_toolchain),
            csv(&row.kernel_toolchain),
            csv(&row.gpu),
            csv(&row.cuda_driver)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_uses_each_field_distribution_instead_of_the_first_sample() {
        let options = Options {
            repeats: 3,
            warmups: 1,
            format: OutputFormat::Json,
        };
        let base = Row::base("suite", "case", "executor", "algorithm", options)
            .with_configuration("fixture=summary-median");
        let samples = [30_u64, 10, 20]
            .into_iter()
            .map(|value| {
                (
                    Duration::from_nanos(value),
                    Row {
                        first_destination_ns: Some(value + 1),
                        reconstruction_ns: Some(value + 2),
                        throughput_per_second: Some(value as f64),
                        correctness: true,
                        ..Row::default()
                    },
                )
            })
            .collect();
        let mut report = Report::default();
        report.distribution(base, samples);
        let summary = report
            .rows
            .iter()
            .find(|row| row.row_kind == "summary")
            .unwrap();
        assert_eq!(summary.median_ns, Some(20));
        assert_eq!(summary.spread_ns, Some(20));
        assert_eq!(summary.first_destination_ns, Some(21));
        assert_eq!(summary.reconstruction_ns, Some(22));
        assert_eq!(summary.throughput_per_second, Some(20.0));
        assert!(summary.configuration.contains("fixture=summary-median"));
    }
}
