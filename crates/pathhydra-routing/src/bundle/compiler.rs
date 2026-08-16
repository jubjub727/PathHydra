use std::{
    fs::{self, File},
    io::{BufWriter, Write},
    path::Path,
    time::{Duration, Instant},
};

use pathhydra_core::{EdgeRecord, NodeId, RelationId};
use pathhydra_store::{ConfirmedGraphScan, ScanError};

use super::{
    BundleError,
    codec::{checksum, put_u32, put_u64},
    layout::{
        EDGE_TOPOLOGY_BYTES, MIN_NONEMPTY_PARTITION_BYTES, PARTITION_HEADER_BYTES,
        PartitionDescriptor, SEGMENT_ENCODED_BYTES, SegmentDescriptor,
    },
    manifest::{BundleManifest, FileDescriptor},
};
use crate::{NUMERIC_POLICY_ID, TIE_POLICY_ID};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BundleConfig {
    pub target_partition_topology_bytes: u64,
    pub hard_maximum_partition_topology_bytes: u64,
    pub maximum_total_bundle_bytes: u64,
}
impl Default for BundleConfig {
    fn default() -> Self {
        Self {
            target_partition_topology_bytes: 4 * 1024 * 1024,
            hard_maximum_partition_topology_bytes: 8 * 1024 * 1024,
            maximum_total_bundle_bytes: 64 * 1024 * 1024 * 1024,
        }
    }
}
impl BundleConfig {
    fn validate(self) -> Result<Self, BundleError> {
        if self.target_partition_topology_bytes == 0
            || self.target_partition_topology_bytes > self.hard_maximum_partition_topology_bytes
        {
            return Err(BundleError::Invalid(
                "partition target must be nonzero and no larger than the hard maximum".into(),
            ));
        }
        if self.hard_maximum_partition_topology_bytes < MIN_NONEMPTY_PARTITION_BYTES {
            return Err(BundleError::Limit {
                resource: "hard partition minimum",
                required: MIN_NONEMPTY_PARTITION_BYTES,
                limit: self.hard_maximum_partition_topology_bytes,
            });
        }
        if self.maximum_total_bundle_bytes == 0 {
            return Err(BundleError::Invalid(
                "maximum total bundle bytes must be nonzero".into(),
            ));
        }
        Ok(self)
    }
}

#[derive(Clone, Debug, Default)]
pub struct BundleBuildMetrics {
    pub node_count: u64,
    pub relation_kind_count: u64,
    pub adjacency_count: u64,
    pub segment_count: u64,
    pub partition_count: u64,
    pub split_source_count: u64,
    pub peak_partition_buffer_bytes: u64,
    pub scan_duration: Duration,
    pub write_duration: Duration,
    pub validation_duration: Duration,
    pub total_duration: Duration,
}

#[derive(Default)]
struct Partition {
    segments: Vec<LocalSegment>,
    destinations: Vec<u32>,
    relations: Vec<u32>,
    weights: Vec<u32>,
    edges: Vec<u64>,
}
#[derive(Clone, Copy)]
struct LocalSegment {
    source: u32,
    first_ordinal: u64,
    start: u32,
    count: u32,
    global_descriptor: usize,
}
impl Partition {
    fn topology_bytes(&self) -> Result<u64, BundleError> {
        PARTITION_HEADER_BYTES
            .checked_add(
                u64::try_from(self.segments.len())
                    .map_err(|_| invalid("segment count overflow"))?
                    .checked_mul(SEGMENT_ENCODED_BYTES)
                    .ok_or_else(|| invalid("segment byte count overflow"))?,
            )
            .and_then(|v| {
                v.checked_add(
                    u64::try_from(self.edges.len())
                        .ok()?
                        .checked_mul(EDGE_TOPOLOGY_BYTES)?,
                )
            })
            .ok_or_else(|| invalid("partition byte count overflow"))
    }
}
fn invalid(reason: &str) -> BundleError {
    BundleError::Invalid(reason.into())
}
fn scan_result<E>(result: Result<(), ScanError<E>>) -> Result<(), BundleError>
where
    E: Into<BundleError>,
{
    match result {
        Ok(()) => Ok(()),
        Err(ScanError::Catalog(e)) => Err(e.into()),
        Err(ScanError::Visitor(e)) => Err(e.into()),
    }
}

pub fn compile_bundle(
    scan: &ConfirmedGraphScan<'_>,
    directory: &Path,
    config: BundleConfig,
) -> Result<(BundleManifest, BundleBuildMetrics), BundleError> {
    let started = Instant::now();
    let config = config.validate()?;
    fs::create_dir(directory)?;
    let mut nodes = Vec::<NodeId>::new();
    let mut relations = Vec::<RelationId>::new();
    let scan_started = Instant::now();
    scan_result(scan.for_each_node(|node| {
        if nodes.last().is_some_and(|id| *id >= node.id()) {
            return Err(invalid("node IDs are duplicate or nonascending"));
        }
        nodes
            .try_reserve(1)
            .map_err(|_| invalid("node identity allocation failed"))?;
        nodes.push(node.id());
        Ok(())
    }))?;
    scan_result(scan.for_each_relation_kind(|relation| {
        if relations.last().is_some_and(|id| *id >= relation.id()) {
            return Err(invalid("relation IDs are duplicate or nonascending"));
        }
        relations
            .try_reserve(1)
            .map_err(|_| invalid("relation identity allocation failed"))?;
        relations.push(relation.id());
        Ok(())
    }))?;
    if nodes.len() > u32::MAX as usize || relations.len() > u32::MAX as usize {
        return Err(invalid("dense identity count exceeds u32"));
    }
    let mut identities = Vec::new();
    identities
        .try_reserve(
            nodes
                .len()
                .checked_add(relations.len())
                .and_then(|n| n.checked_mul(8))
                .ok_or_else(|| invalid("identity size overflow"))?,
        )
        .map_err(|_| invalid("identity buffer allocation failed"))?;
    for id in &nodes {
        put_u64(&mut identities, id.as_u64());
    }
    for id in &relations {
        put_u64(&mut identities, id.as_u64());
    }
    write_sync(&directory.join("identities.bin"), &identities)?;
    let mut topology = BufWriter::new(File::create(directory.join("topology.bin"))?);
    let mut evidence = BufWriter::new(File::create(directory.join("evidence.bin"))?);
    let mut topology_offset = 0_u64;
    let mut evidence_offset = 0_u64;
    let mut partitions = Vec::new();
    let mut descriptors = Vec::new();
    let mut source_offsets = vec![0_u64];
    let mut partition = Partition::default();
    let mut current_source = None::<u32>;
    let mut source_ordinal = 0_u64;
    let mut previous_edge = None;
    let mut metrics = BundleBuildMetrics {
        node_count: nodes.len() as u64,
        relation_kind_count: relations.len() as u64,
        ..Default::default()
    };
    let flush = |partition: &mut Partition,
                 partitions: &mut Vec<PartitionDescriptor>,
                 _descriptors: &mut Vec<SegmentDescriptor>,
                 topology: &mut BufWriter<File>,
                 evidence: &mut BufWriter<File>,
                 topology_offset: &mut u64,
                 evidence_offset: &mut u64,
                 metrics: &mut BundleBuildMetrics|
     -> Result<(), BundleError> {
        if partition.edges.is_empty() {
            return Ok(());
        }
        let id =
            u32::try_from(partitions.len()).map_err(|_| invalid("partition count exceeds u32"))?;
        let mut top = Vec::new();
        put_u32(&mut top, partition.segments.len() as u32);
        put_u64(&mut top, partition.edges.len() as u64);
        for s in &partition.segments {
            put_u32(&mut top, s.source);
            put_u64(&mut top, s.first_ordinal);
            put_u32(&mut top, s.start);
            put_u32(&mut top, s.count);
            put_u32(&mut top, 0);
        }
        for v in &partition.destinations {
            put_u32(&mut top, *v);
        }
        for v in &partition.relations {
            put_u32(&mut top, *v);
        }
        for v in &partition.weights {
            put_u32(&mut top, *v);
        }
        let mut ev = Vec::with_capacity(partition.edges.len() * 8);
        for v in &partition.edges {
            put_u64(&mut ev, *v);
        }
        let top_len = top.len() as u64;
        let ev_len = ev.len() as u64;
        if top_len > config.hard_maximum_partition_topology_bytes {
            return Err(BundleError::Limit {
                resource: "partition topology",
                required: top_len,
                limit: config.hard_maximum_partition_topology_bytes,
            });
        }
        topology.write_all(&top)?;
        evidence.write_all(&ev)?;
        partitions.push(PartitionDescriptor {
            id,
            topology_offset: *topology_offset,
            topology_length: top_len,
            topology_checksum: checksum(&top),
            evidence_offset: *evidence_offset,
            evidence_length: ev_len,
            evidence_checksum: checksum(&ev),
            segment_count: partition.segments.len() as u32,
            edge_count: partition.edges.len() as u64,
        });
        *topology_offset = topology_offset
            .checked_add(top_len)
            .ok_or_else(|| invalid("topology offset overflow"))?;
        *evidence_offset = evidence_offset
            .checked_add(ev_len)
            .ok_or_else(|| invalid("evidence offset overflow"))?;
        metrics.peak_partition_buffer_bytes =
            metrics.peak_partition_buffer_bytes.max(top_len + ev_len);
        *partition = Partition::default();
        Ok(())
    };
    scan_result(scan.for_each_outgoing_edge(|edge: &EdgeRecord| {
        let source = nodes
            .binary_search(&edge.source())
            .map_err(|_| invalid("edge source is absent"))? as u32;
        let destination = nodes
            .binary_search(&edge.destination())
            .map_err(|_| invalid("edge destination is absent"))? as u32;
        let relation = relations
            .binary_search(&edge.relation_kind())
            .map_err(|_| invalid("edge relation is absent"))? as u32;
        if current_source != Some(source) {
            if current_source.is_some_and(|old| source <= old) {
                return Err(invalid("outgoing sources are not strictly ascending"));
            }
            if current_source.is_some()
                && partition.topology_bytes()? >= config.target_partition_topology_bytes
            {
                flush(
                    &mut partition,
                    &mut partitions,
                    &mut descriptors,
                    &mut topology,
                    &mut evidence,
                    &mut topology_offset,
                    &mut evidence_offset,
                    &mut metrics,
                )?;
            }
            while source_offsets.len() <= source as usize {
                source_offsets.push(descriptors.len() as u64);
            }
            current_source = Some(source);
            source_ordinal = 0;
            previous_edge = None;
        }
        if previous_edge.is_some_and(|old| old >= edge.id()) {
            return Err(invalid(
                "outgoing edge IDs are not ascending within their source",
            ));
        }
        previous_edge = Some(edge.id());
        let same_segment = partition
            .segments
            .last()
            .is_some_and(|s| s.source == source);
        let extra = EDGE_TOPOLOGY_BYTES
            + if same_segment {
                0
            } else {
                SEGMENT_ENCODED_BYTES
            };
        if partition
            .topology_bytes()?
            .checked_add(extra)
            .ok_or_else(|| invalid("partition size overflow"))?
            > config.hard_maximum_partition_topology_bytes
        {
            flush(
                &mut partition,
                &mut partitions,
                &mut descriptors,
                &mut topology,
                &mut evidence,
                &mut topology_offset,
                &mut evidence_offset,
                &mut metrics,
            )?;
        }
        if partition.segments.last().is_none_or(|s| s.source != source) {
            let partition_id = u32::try_from(partitions.len())
                .map_err(|_| invalid("partition count exceeds u32"))?;
            let local_segment = u32::try_from(partition.segments.len())
                .map_err(|_| invalid("partition segment count exceeds u32"))?;
            let global_descriptor = descriptors.len();
            descriptors.push(SegmentDescriptor {
                source,
                partition: partition_id,
                local_segment,
                first_edge_ordinal: source_ordinal,
                edge_count: 0,
            });
            partition.segments.push(LocalSegment {
                source,
                first_ordinal: source_ordinal,
                start: partition.edges.len() as u32,
                count: 0,
                global_descriptor,
            });
        }
        let segment = partition.segments.last_mut().expect("created segment");
        segment.count = segment
            .count
            .checked_add(1)
            .ok_or_else(|| invalid("segment edge count exceeds u32"))?;
        descriptors[segment.global_descriptor].edge_count = segment.count;
        partition.destinations.push(destination);
        partition.relations.push(relation);
        partition.weights.push(edge.base_weight().to_bits());
        partition.edges.push(edge.id().as_u64());
        source_ordinal += 1;
        metrics.adjacency_count += 1;
        Ok(())
    }))?;
    flush(
        &mut partition,
        &mut partitions,
        &mut descriptors,
        &mut topology,
        &mut evidence,
        &mut topology_offset,
        &mut evidence_offset,
        &mut metrics,
    )?;
    while source_offsets.len() < nodes.len() + 1 {
        source_offsets.push(descriptors.len() as u64);
    }
    topology.flush()?;
    topology.get_ref().sync_all()?;
    evidence.flush()?;
    evidence.get_ref().sync_all()?;
    metrics.scan_duration = scan_started.elapsed();
    let mut directory_bytes = Vec::new();
    for offset in &source_offsets {
        put_u64(&mut directory_bytes, *offset);
    }
    for s in &descriptors {
        put_u32(&mut directory_bytes, s.source);
        put_u32(&mut directory_bytes, s.partition);
        put_u32(&mut directory_bytes, s.local_segment);
        put_u64(&mut directory_bytes, s.first_edge_ordinal);
        put_u32(&mut directory_bytes, s.edge_count);
    }
    write_sync(&directory.join("source-directory.bin"), &directory_bytes)?;
    metrics.segment_count = descriptors.len() as u64;
    metrics.partition_count = partitions.len() as u64;
    metrics.split_source_count = (0..nodes.len())
        .filter(|&i| source_offsets[i + 1] - source_offsets[i] > 1)
        .count() as u64;
    let file_desc = |bytes: &[u8]| FileDescriptor {
        length: bytes.len() as u64,
        checksum: checksum(bytes),
    };
    let topology_descriptor = file_descriptor(&directory.join("topology.bin"))?;
    let evidence_descriptor = file_descriptor(&directory.join("evidence.bin"))?;
    let manifest = BundleManifest {
        numeric_policy: NUMERIC_POLICY_ID.into(),
        tie_policy: TIE_POLICY_ID.into(),
        node_count: nodes.len() as u64,
        relation_kind_count: relations.len() as u64,
        adjacency_count: metrics.adjacency_count,
        segment_count: descriptors.len() as u64,
        target_partition_bytes: config.target_partition_topology_bytes,
        hard_maximum_partition_bytes: config.hard_maximum_partition_topology_bytes,
        identities: file_desc(&identities),
        source_directory: file_desc(&directory_bytes),
        topology: topology_descriptor,
        evidence: evidence_descriptor,
        partitions,
    };
    let manifest_bytes = manifest.encode()?;
    let total = (manifest_bytes.len() as u64)
        .checked_add(manifest.identities.length)
        .and_then(|v| v.checked_add(manifest.source_directory.length))
        .and_then(|v| v.checked_add(manifest.topology.length))
        .and_then(|v| v.checked_add(manifest.evidence.length))
        .ok_or_else(|| invalid("bundle total overflow"))?;
    if total > config.maximum_total_bundle_bytes {
        return Err(BundleError::Limit {
            resource: "total bytes",
            required: total,
            limit: config.maximum_total_bundle_bytes,
        });
    }
    write_sync(&directory.join("manifest.bin"), &manifest_bytes)?;
    metrics.write_duration = started.elapsed().saturating_sub(metrics.scan_duration);
    metrics.total_duration = started.elapsed();
    Ok((manifest, metrics))
}

fn write_sync(path: &Path, bytes: &[u8]) -> Result<(), BundleError> {
    let mut file = File::create(path)?;
    file.write_all(bytes)?;
    file.flush()?;
    file.sync_all()?;
    Ok(())
}

fn file_descriptor(path: &Path) -> Result<FileDescriptor, BundleError> {
    use std::io::Read;
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(FileDescriptor {
        length,
        checksum: *hasher.finalize().as_bytes(),
    })
}
