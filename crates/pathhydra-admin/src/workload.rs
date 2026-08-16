use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use pathhydra_routing::{BundleConfig, compile_bundle};
use pathhydra_store::{
    Catalog, CheckpointRequest, ConfirmedRecord, EdgeId, FlushMode, NodeId, RelationId,
    RestoreRequest, VerificationLimits, restore_checkpoint,
};

use crate::{Arguments, CliError, output::duration_json};

const MAXIMUM_WORKLOAD_SCALE: u64 = 10_000_000;

pub(crate) fn run(arguments: &mut Arguments) -> Result<String, CliError> {
    let root = arguments.required_path("--root")?;
    let scale = arguments.optional_u64("--scale")?.unwrap_or(128);
    let samples = arguments.optional_u64("--samples")?.unwrap_or(8);
    if !(16..=MAXIMUM_WORKLOAD_SCALE).contains(&scale)
        || usize::try_from(scale).is_err()
        || samples == 0
        || samples > 10_000
    {
        return Err(CliError::InvalidValue("--scale/--samples"));
    }
    prepare_fresh_root(&root)?;
    let process_io_before = process_io_counters();
    let database = root.join("database");
    let catalog = Catalog::open(&database).map_err(CliError::operation)?;
    let mut measurements = Vec::new();
    let mut generator = Generator::new(0x5041_5448_4859_4452);

    let mut nodes = Vec::new();
    let mut node_names = Vec::new();
    let mut insert_times = Vec::new();
    let mut promotion_times = Vec::new();
    for index in 0..scale {
        let name = format!("sequential-node-{index:020}");
        let started = Instant::now();
        let candidate = catalog
            .insert_node_candidate(name.as_str())
            .map_err(CliError::operation)?;
        insert_times.push(started.elapsed());
        let started = Instant::now();
        let ConfirmedRecord::Node(node) = catalog
            .confirm_validated_candidate(candidate)
            .map_err(CliError::operation)?
        else {
            return Err(CliError::Operation);
        };
        promotion_times.push(started.elapsed());
        nodes.push(node.id());
        node_names.push(name);
    }
    measurements.push(Measurement::latencies(
        "sequential_candidate_insertion",
        scale,
        insert_times,
    ));
    measurements.push(Measurement::latencies(
        "sequential_candidate_promotion",
        scale,
        promotion_times,
    ));

    let random_count = scale / 2;
    let mut mixed_candidates = Vec::new();
    let mut random_insert_times = Vec::new();
    for index in 0..random_count {
        let started = Instant::now();
        let candidate = catalog
            .insert_node_candidate(format!("random-node-{index:020}"))
            .map_err(CliError::operation)?;
        random_insert_times.push(started.elapsed());
        mixed_candidates.push(candidate);
        let started = Instant::now();
        let candidate = catalog
            .insert_relation_candidate(format!("random-relation-{index:020}"))
            .map_err(CliError::operation)?;
        random_insert_times.push(started.elapsed());
        mixed_candidates.push(candidate);
    }
    generator.shuffle(&mut mixed_candidates);
    let mut relations = Vec::new();
    let mut random_promotion_times = Vec::new();
    for candidate in mixed_candidates {
        let started = Instant::now();
        let confirmed = catalog
            .confirm_validated_candidate(candidate)
            .map_err(CliError::operation)?;
        random_promotion_times.push(started.elapsed());
        match confirmed {
            ConfirmedRecord::Node(node) => nodes.push(node.id()),
            ConfirmedRecord::Relation(relation) => relations.push(relation.id()),
            ConfirmedRecord::Edge(_) => return Err(CliError::Operation),
        }
    }
    measurements.push(Measurement::latencies(
        "random_node_relation_candidate_insertion",
        random_count.saturating_mul(2),
        random_insert_times,
    ));
    measurements.push(Measurement::latencies(
        "random_node_relation_promotion",
        random_count.saturating_mul(2),
        random_promotion_times,
    ));

    let mut edge_candidates = Vec::new();
    let mut edge_insert_times = Vec::new();
    for _ in 0..scale {
        let source = nodes[generator.index(nodes.len())];
        let destination = nodes[generator.index(nodes.len())];
        let relation = relations[generator.index(relations.len())];
        let started = Instant::now();
        edge_candidates.push(
            catalog
                .insert_edge_candidate(source, destination, relation, 0.5)
                .map_err(CliError::operation)?,
        );
        edge_insert_times.push(started.elapsed());
    }
    generator.shuffle(&mut edge_candidates);
    let mut random_edges = Vec::new();
    let mut edge_promotion_times = Vec::new();
    for candidate in edge_candidates {
        let started = Instant::now();
        let ConfirmedRecord::Edge(edge) = catalog
            .confirm_validated_candidate(candidate)
            .map_err(CliError::operation)?
        else {
            return Err(CliError::Operation);
        };
        edge_promotion_times.push(started.elapsed());
        random_edges.push(edge.id());
    }
    measurements.push(Measurement::latencies(
        "random_edge_candidate_insertion",
        scale,
        edge_insert_times,
    ));
    measurements.push(Measurement::latencies(
        "random_edge_promotion",
        scale,
        edge_promotion_times,
    ));

    let mut hit_times = Vec::new();
    let mut miss_times = Vec::new();
    for index in 0..scale {
        let name = &node_names[generator.index(node_names.len())];
        let started = Instant::now();
        if catalog
            .lookup_node_exact(name)
            .map_err(CliError::operation)?
            .is_none()
        {
            return Err(CliError::Operation);
        }
        hit_times.push(started.elapsed());
        let started = Instant::now();
        if catalog
            .lookup_node_exact(&format!("absent-node-{index:020}"))
            .map_err(CliError::operation)?
            .is_some()
        {
            return Err(CliError::Operation);
        }
        miss_times.push(started.elapsed());
    }
    measurements.push(Measurement::latencies("exact_name_hit", scale, hit_times));
    measurements.push(Measurement::latencies("exact_name_miss", scale, miss_times));

    let relation = relations[0];
    let outgoing_hub = confirm_node(&catalog, "outgoing-hub")?;
    let incoming_hub = confirm_node(&catalog, "incoming-hub")?;
    for index in 0..scale {
        let outgoing_leaf = confirm_node(&catalog, &format!("outgoing-leaf-{index:020}"))?;
        let incoming_leaf = confirm_node(&catalog, &format!("incoming-leaf-{index:020}"))?;
        confirm_edge(&catalog, outgoing_hub, outgoing_leaf, relation)?;
        confirm_edge(&catalog, incoming_leaf, incoming_hub, relation)?;
    }
    let mut outgoing_times = Vec::new();
    let mut incoming_times = Vec::new();
    for _ in 0..samples {
        let started = Instant::now();
        if catalog
            .outgoing_edges(outgoing_hub)
            .map_err(CliError::operation)?
            .len()
            != scale as usize
        {
            return Err(CliError::Operation);
        }
        outgoing_times.push(started.elapsed());
        let started = Instant::now();
        if catalog
            .incoming_edges(incoming_hub)
            .map_err(CliError::operation)?
            .len()
            != scale as usize
        {
            return Err(CliError::Operation);
        }
        incoming_times.push(started.elapsed());
    }
    measurements.push(Measurement::latencies(
        "high_degree_outgoing_adjacency",
        samples,
        outgoing_times,
    ));
    measurements.push(Measurement::latencies(
        "high_degree_incoming_adjacency",
        samples,
        incoming_times,
    ));

    generator.shuffle(&mut random_edges);
    let delete_count = random_edges.len() / 2;
    let mut delete_times = Vec::new();
    for edge in random_edges.into_iter().take(delete_count) {
        let started = Instant::now();
        catalog.remove_edge(edge).map_err(CliError::operation)?;
        delete_times.push(started.elapsed());
    }
    measurements.push(Measurement::latencies(
        "random_edge_deletion",
        delete_count as u64,
        delete_times,
    ));

    let cascade_hub = confirm_node(&catalog, "cascade-hub")?;
    for index in 0..scale {
        let leaf = confirm_node(&catalog, &format!("cascade-leaf-{index:020}"))?;
        confirm_edge(&catalog, cascade_hub, leaf, relation)?;
        confirm_edge(&catalog, leaf, cascade_hub, relation)?;
    }
    let started = Instant::now();
    catalog
        .remove_node(cascade_hub)
        .map_err(CliError::operation)?;
    measurements.push(Measurement::latencies(
        "high_degree_cascading_node_deletion",
        scale.saturating_mul(2),
        vec![started.elapsed()],
    ));

    let bundle = root.join("bundle-initial");
    let scan = catalog
        .confirmed_graph_scan()
        .map_err(CliError::operation)?;
    let (_, bundle_metrics) =
        compile_bundle(&scan, &bundle, BundleConfig::default()).map_err(CliError::operation)?;
    drop(scan);
    measurements.push(Measurement::single(
        "streaming_bundle_scan_build",
        bundle_metrics.adjacency_count,
        bundle_metrics.total_duration,
        Some(bundle_metrics.bundle_bytes),
    ));

    catalog
        .flush(FlushMode::WalAndMemtables)
        .map_err(CliError::operation)?;
    let database_bytes_before_churn = directory_bytes(&database)?;
    drop(catalog);
    let started = Instant::now();
    let catalog = Catalog::open_existing(&database).map_err(CliError::operation)?;
    let restart_duration = started.elapsed();
    let restart_report = catalog
        .verify(VerificationLimits::default())
        .map_err(CliError::operation)?;
    measurements.push(Measurement::single(
        "restart_validation",
        restart_report.records_examined,
        restart_duration,
        Some(restart_report.decoded_bytes),
    ));

    let checkpoint = root.join("checkpoint");
    let checkpoint_report = catalog
        .create_checkpoint(&CheckpointRequest {
            destination_root: root.clone(),
            destination: checkpoint.clone(),
            routing_image_root: None,
            scratch_path: None,
            available_destination_bytes: u64::MAX,
            minimum_headroom_bytes: 0,
        })
        .map_err(CliError::operation)?;
    measurements.push(Measurement::single(
        "checkpoint",
        checkpoint_report.catalog.records_examined,
        checkpoint_report.duration,
        Some(checkpoint_report.bytes),
    ));
    let restore = root.join("restore");
    let restore_report = restore_checkpoint(&RestoreRequest {
        source_root: root.clone(),
        source_checkpoint: checkpoint,
        destination_root: root.clone(),
        destination: restore,
        routing_image_root: None,
        scratch_path: None,
        available_destination_bytes: u64::MAX,
        minimum_headroom_bytes: 0,
        verification_limits: VerificationLimits::default(),
    })
    .map_err(CliError::operation)?;
    measurements.push(Measurement::single(
        "restore_and_full_validation",
        restore_report.restored_catalog.records_examined,
        restore_report.duration,
        Some(restore_report.source_bytes),
    ));

    let churn_source = confirm_node(&catalog, "churn-source")?;
    let churn_destination = confirm_node(&catalog, "churn-destination")?;
    let mut churn_edges = Vec::new();
    for _ in 0..scale {
        churn_edges.push(confirm_edge(
            &catalog,
            churn_source,
            churn_destination,
            relation,
        )?);
    }
    let churn_started = Instant::now();
    for edge in churn_edges {
        catalog.remove_edge(edge).map_err(CliError::operation)?;
    }
    catalog
        .flush(FlushMode::WalAndMemtables)
        .map_err(CliError::operation)?;
    let churn_duration = churn_started.elapsed();
    let database_bytes_after_churn = directory_bytes(&database)?;
    let compaction = catalog.compact_all().map_err(CliError::operation)?;
    let database_bytes_after_compaction = directory_bytes(&database)?;
    measurements.push(Measurement::single(
        "fixed_scope_compaction_after_churn",
        compaction.families.len() as u64,
        compaction.duration,
        Some(database_bytes_after_compaction),
    ));
    let metrics = catalog.metrics_snapshot().map_err(CliError::operation)?;
    let pending_compaction_bytes = metrics
        .column_families
        .iter()
        .filter_map(|family| match family.pending_compaction_bytes {
            pathhydra_store::MetricValue::Available(value) => Some(value),
            pathhydra_store::MetricValue::Unavailable => None,
        })
        .fold(0_u64, u64::saturating_add);
    measurements.push(Measurement::single(
        "churn_flush_space_amplification",
        scale,
        churn_duration,
        Some(database_bytes_after_churn),
    ));

    let mutation_started = Instant::now();
    let candidate = catalog
        .insert_node_candidate("rebuild-trigger")
        .map_err(CliError::operation)?;
    catalog
        .confirm_validated_candidate(candidate)
        .map_err(CliError::operation)?;
    let mutation_duration = mutation_started.elapsed();
    let rebuild = root.join("bundle-after-mutation");
    let scan = catalog
        .confirmed_graph_scan()
        .map_err(CliError::operation)?;
    let (_, rebuild_metrics) =
        compile_bundle(&scan, &rebuild, BundleConfig::default()).map_err(CliError::operation)?;
    drop(scan);
    measurements.push(Measurement::single(
        "confirmed_mutation_before_complete_rebuild",
        1,
        mutation_duration,
        None,
    ));
    measurements.push(Measurement::single(
        "complete_rebuild_after_mutation",
        rebuild_metrics.adjacency_count,
        rebuild_metrics.total_duration,
        Some(rebuild_metrics.bundle_bytes),
    ));

    // Keep one provisional candidate so checkpoint/restore rehearsals using the
    // generated catalog prove that provisional and confirmed state coexist.
    catalog
        .insert_node_candidate("rehearsal-provisional")
        .map_err(CliError::operation)?;

    let final_report = catalog
        .verify(VerificationLimits::default())
        .map_err(CliError::operation)?;
    let store_metrics = catalog.metrics_snapshot().map_err(CliError::operation)?;
    let logical_catalog_write_bytes = store_metrics.writes.values().fold(0_u64, |total, value| {
        total.saturating_add(value.committed_bytes)
    });
    let logical_confirmed_scan_bytes = store_metrics.scans.decoded_bytes;
    let process_io = process_io_before
        .zip(process_io_counters())
        .map(|(before, after)| ProcessIoCounters {
            read_bytes: after.read_bytes.saturating_sub(before.read_bytes),
            write_bytes: after.write_bytes.saturating_sub(before.write_bytes),
        });
    Ok(report_json(&ReportFacts {
        scale,
        samples,
        measurements: &measurements,
        checksum: final_report.catalog_checksum,
        database_bytes_before_churn,
        database_bytes_after_churn,
        database_bytes_after_compaction,
        pending_compaction_bytes,
        peak_working_set_bytes: peak_working_set_bytes(),
        process_io,
        logical_catalog_write_bytes,
        logical_confirmed_scan_bytes,
    }))
}

#[derive(Clone, Copy)]
struct ProcessIoCounters {
    read_bytes: u64,
    write_bytes: u64,
}

#[cfg(windows)]
fn process_io_counters() -> Option<ProcessIoCounters> {
    // The PerfRawData fields retain the provider's `PerSec` names but expose
    // cumulative raw byte counters; taking two values produces this run's
    // transfer-byte delta. Access-denied/unsupported hosts report `None`.
    let expression = format!(
        "$p=Get-CimInstance Win32_PerfRawData_PerfProc_Process -Filter \"IDProcess = {}\"; \"$($p.IOReadBytesPerSec),$($p.IOWriteBytesPerSec)\"",
        std::process::id()
    );
    let output = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", expression.as_str()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = std::str::from_utf8(&output.stdout).ok()?.trim();
    let (read, write) = value.split_once(',')?;
    Some(ProcessIoCounters {
        read_bytes: read.parse().ok()?,
        write_bytes: write.parse().ok()?,
    })
}

#[cfg(target_os = "linux")]
fn process_io_counters() -> Option<ProcessIoCounters> {
    let contents = fs::read_to_string("/proc/self/io").ok()?;
    let value = |name: &str| {
        contents.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            (key == name).then(|| value.trim().parse().ok()).flatten()
        })
    };
    Some(ProcessIoCounters {
        read_bytes: value("read_bytes")?,
        write_bytes: value("write_bytes")?,
    })
}

#[cfg(not(any(windows, target_os = "linux")))]
const fn process_io_counters() -> Option<ProcessIoCounters> {
    None
}

#[cfg(windows)]
fn peak_working_set_bytes() -> Option<u64> {
    let expression = format!("(Get-Process -Id {}).PeakWorkingSet64", std::process::id());
    let output = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", expression.as_str()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    std::str::from_utf8(&output.stdout)
        .ok()?
        .trim()
        .parse()
        .ok()
}

#[cfg(not(windows))]
const fn peak_working_set_bytes() -> Option<u64> {
    None
}

fn prepare_fresh_root(root: &Path) -> Result<(), CliError> {
    if root.parent().is_none() {
        return Err(CliError::InvalidValue("--root"));
    }
    if root.exists() {
        if !root.is_dir()
            || fs::read_dir(root)
                .map_err(CliError::operation)?
                .next()
                .is_some()
        {
            return Err(CliError::InvalidValue("--root"));
        }
    } else {
        fs::create_dir(root).map_err(CliError::operation)?;
    }
    Ok(())
}

fn confirm_node(catalog: &Catalog, name: &str) -> Result<NodeId, CliError> {
    let candidate = catalog
        .insert_node_candidate(name)
        .map_err(CliError::operation)?;
    let ConfirmedRecord::Node(node) = catalog
        .confirm_validated_candidate(candidate)
        .map_err(CliError::operation)?
    else {
        return Err(CliError::Operation);
    };
    Ok(node.id())
}

fn confirm_edge(
    catalog: &Catalog,
    source: NodeId,
    destination: NodeId,
    relation: RelationId,
) -> Result<EdgeId, CliError> {
    let candidate = catalog
        .insert_edge_candidate(source, destination, relation, 0.5)
        .map_err(CliError::operation)?;
    let ConfirmedRecord::Edge(edge) = catalog
        .confirm_validated_candidate(candidate)
        .map_err(CliError::operation)?
    else {
        return Err(CliError::Operation);
    };
    Ok(edge.id())
}

fn directory_bytes(path: &Path) -> Result<u64, CliError> {
    let mut total = 0_u64;
    let mut pending = vec![PathBuf::from(path)];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).map_err(CliError::operation)? {
            let entry = entry.map_err(CliError::operation)?;
            let metadata = entry.metadata().map_err(CliError::operation)?;
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if metadata.is_file() {
                total = total.saturating_add(metadata.len());
            }
        }
    }
    Ok(total)
}

struct Measurement {
    name: &'static str,
    operations: u64,
    durations: Vec<Duration>,
    bytes: Option<u64>,
}

impl Measurement {
    fn latencies(name: &'static str, operations: u64, durations: Vec<Duration>) -> Self {
        Self {
            name,
            operations,
            durations,
            bytes: None,
        }
    }

    fn single(name: &'static str, operations: u64, duration: Duration, bytes: Option<u64>) -> Self {
        Self {
            name,
            operations,
            durations: vec![duration],
            bytes,
        }
    }

    fn json(&self) -> String {
        let mut sorted = self.durations.clone();
        sorted.sort_unstable();
        let median = sorted[sorted.len() / 2];
        let p95 = sorted[(sorted.len().saturating_sub(1) * 95) / 100];
        let durations = self
            .durations
            .iter()
            .map(|duration| duration.as_nanos().to_string())
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "{{\"workload\":\"{}\",\"correctness_verified\":true,\"operations\":{},\"bytes\":{},\"median\":{},\"p95\":{},\"duration_nanoseconds\":[{}]}}",
            self.name,
            self.operations,
            self.bytes
                .map_or_else(|| "null".to_owned(), |value| value.to_string()),
            duration_json(median),
            duration_json(p95),
            durations,
        )
    }
}

struct ReportFacts<'a> {
    scale: u64,
    samples: u64,
    measurements: &'a [Measurement],
    checksum: u64,
    database_bytes_before_churn: u64,
    database_bytes_after_churn: u64,
    database_bytes_after_compaction: u64,
    pending_compaction_bytes: u64,
    peak_working_set_bytes: Option<u64>,
    process_io: Option<ProcessIoCounters>,
    logical_catalog_write_bytes: u64,
    logical_confirmed_scan_bytes: u64,
}

fn report_json(facts: &ReportFacts<'_>) -> String {
    let measurements = facts
        .measurements
        .iter()
        .map(Measurement::json)
        .collect::<Vec<_>>()
        .join(",");
    let process_read_bytes = facts
        .process_io
        .map_or_else(|| "null".to_owned(), |value| value.read_bytes.to_string());
    let process_write_bytes = facts
        .process_io
        .map_or_else(|| "null".to_owned(), |value| value.write_bytes.to_string());
    let application_io_amplification = facts.process_io.and_then(|value| {
        (facts.database_bytes_after_compaction > 0).then(|| {
            format!(
                "{:.6}",
                value.read_bytes.saturating_add(value.write_bytes) as f64
                    / facts.database_bytes_after_compaction as f64
            )
        })
    });
    format!(
        "{{\"command\":\"workload\",\"format\":\"pathhydra-store-workload-v1\",\"os\":\"{}\",\"arch\":\"{}\",\"scale\":{},\"samples\":{},\"catalog_checksum\":\"{:016x}\",\"database_bytes_before_churn\":{},\"database_bytes_after_churn\":{},\"database_bytes_after_compaction\":{},\"pending_compaction_bytes_after_compaction\":{},\"peak_working_set_bytes\":{},\"process_read_transfer_bytes\":{},\"process_write_transfer_bytes\":{},\"logical_catalog_write_bytes\":{},\"logical_confirmed_scan_bytes\":{},\"application_process_io_to_catalog_size_ratio\":{},\"explicit_compaction_available\":true,\"overlay_implemented\":false,\"route_publication_blocking_measured\":false,\"measurements\":[{}]}}",
        std::env::consts::OS,
        std::env::consts::ARCH,
        facts.scale,
        facts.samples,
        facts.checksum,
        facts.database_bytes_before_churn,
        facts.database_bytes_after_churn,
        facts.database_bytes_after_compaction,
        facts.pending_compaction_bytes,
        facts
            .peak_working_set_bytes
            .map_or_else(|| "null".to_owned(), |value| value.to_string()),
        process_read_bytes,
        process_write_bytes,
        facts.logical_catalog_write_bytes,
        facts.logical_confirmed_scan_bytes,
        application_io_amplification.unwrap_or_else(|| "null".to_owned()),
        measurements,
    )
}

struct Generator(u64);

impl Generator {
    const fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0
    }

    fn index(&mut self, length: usize) -> usize {
        (self.next() as usize) % length
    }

    fn shuffle<T>(&mut self, values: &mut [T]) {
        for index in (1..values.len()).rev() {
            values.swap(index, self.index(index + 1));
        }
    }
}
