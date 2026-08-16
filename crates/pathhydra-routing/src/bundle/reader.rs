use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};

use pathhydra_core::{BaseWeight, EdgeId, NodeId, RelationId};

use super::{
    BundleError,
    codec::{Decoder, checksum},
    layout::{PartitionDescriptor, SegmentDescriptor},
    manifest::BundleManifest,
};
use crate::{
    CancellationSignal, CpuSearchDiagnostics, CpuWorkingSetEstimate, DenseNodeId, NeverCancelled,
    OutgoingEdge, RoutingError, RoutingImage, RoutingRequest, RoutingResponse,
    cpu::{RoutingTopology, estimate_topology_working_set, route_topology_controlled},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostCacheConfig {
    pub maximum_bytes: u64,
    pub maximum_entries: usize,
    pub io_worker_count: usize,
    pub maximum_queued_reads: usize,
}
impl Default for HostCacheConfig {
    fn default() -> Self {
        Self {
            maximum_bytes: 256 * 1024 * 1024,
            maximum_entries: 64,
            io_worker_count: 2,
            maximum_queued_reads: 64,
        }
    }
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HostCacheSnapshot {
    pub capacity_bytes: u64,
    pub current_bytes: u64,
    pub high_water_bytes: u64,
    pub hits: u64,
    pub misses: u64,
    pub coalesced_waits: u64,
    pub evictions: u64,
    pub read_bytes: u64,
    pub checksum_failures: u64,
    pub entries: usize,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PartitionedCpuDiagnostics {
    pub cache: HostCacheSnapshot,
    pub partitions: u64,
    pub file_bytes: u64,
    pub io_wait: Duration,
}

#[derive(Debug)]
pub struct RoutingBundle {
    root: PathBuf,
    manifest: BundleManifest,
    manifest_checksum: [u8; 32],
    nodes: Box<[NodeId]>,
    relations: Box<[RelationId]>,
    source_segment_offsets: Box<[u64]>,
    segments: Box<[SegmentDescriptor]>,
    source_edge_offsets: Box<[u64]>,
    partition_edge_offsets: Box<[u64]>,
}
impl RoutingBundle {
    pub fn manifest(&self) -> &BundleManifest {
        &self.manifest
    }
    pub const fn manifest_checksum(&self) -> &[u8; 32] {
        &self.manifest_checksum
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn identity_directory_bytes(&self) -> u64 {
        self.manifest.identities.length + self.manifest.source_directory.length
    }
    pub fn segment_count(&self) -> usize {
        self.segments.len()
    }
    pub fn source_segment_offsets(&self) -> &[u64] {
        &self.source_segment_offsets
    }
    pub fn routing_manifest(&self) -> Result<crate::RoutingImageManifest, BundleError> {
        crate::image::manifest(
            usize::try_from(self.manifest.node_count)
                .map_err(|_| invalid("node count does not fit platform"))?,
            usize::try_from(self.manifest.relation_kind_count)
                .map_err(|_| invalid("relation count does not fit platform"))?,
            usize::try_from(self.manifest.adjacency_count)
                .map_err(|_| invalid("adjacency count does not fit platform"))?,
        )
        .map_err(|error| invalid(&error.to_string()))
    }
    pub fn to_resident_image(&self) -> Result<RoutingImage, BundleError> {
        let mut destinations = Vec::with_capacity(self.manifest.adjacency_count as usize);
        let mut relation_ids = Vec::with_capacity(destinations.capacity());
        let mut relation_indexes = Vec::with_capacity(destinations.capacity());
        let mut weights = Vec::with_capacity(destinations.capacity());
        let mut edges = Vec::with_capacity(destinations.capacity());
        for p in &self.manifest.partitions {
            let decoded = self.load_partition(p)?;
            for i in 0..decoded.edges.len() {
                destinations.push(decoded.destinations[i]);
                relation_indexes.push(decoded.relations[i]);
                relation_ids.push(
                    *self
                        .relations
                        .get(decoded.relations[i] as usize)
                        .ok_or_else(|| invalid("relation index out of bounds"))?,
                );
                weights.push(decoded.weights[i]);
                edges.push(EdgeId::from_u64(decoded.edges[i]));
            }
        }
        let mut external = Vec::with_capacity(self.nodes.len());
        for (dense, id) in self.nodes.iter().enumerate() {
            external.push((*id, DenseNodeId::from_u32(dense as u32)));
        }
        let manifest = crate::image::manifest(self.nodes.len(), self.relations.len(), edges.len())
            .map_err(|e| invalid(&e.to_string()))?;
        Ok(RoutingImage {
            external_to_dense: external.into_boxed_slice(),
            dense_to_external: self.nodes.clone(),
            offsets: self.source_edge_offsets.clone(),
            destinations: destinations.into_boxed_slice(),
            relation_ids: relation_ids.into_boxed_slice(),
            relation_indexes: relation_indexes.into_boxed_slice(),
            base_weight_bits: weights.into_boxed_slice(),
            edge_ids: edges.into_boxed_slice(),
            confirmed_relation_ids: self.relations.clone(),
            manifest,
        })
    }
    fn load_partition(&self, p: &PartitionDescriptor) -> Result<DecodedPartition, BundleError> {
        let top = read_range(
            &self.root.join("topology.bin"),
            p.topology_offset,
            p.topology_length,
        )?;
        let evidence = read_range(
            &self.root.join("evidence.bin"),
            p.evidence_offset,
            p.evidence_length,
        )?;
        if checksum(&top) != p.topology_checksum || checksum(&evidence) != p.evidence_checksum {
            return Err(invalid("partition checksum mismatch"));
        }
        decode_partition(
            &top,
            &evidence,
            p,
            self.manifest.node_count,
            self.manifest.relation_kind_count,
        )
    }
}

pub fn open_bundle(root: &Path) -> Result<RoutingBundle, BundleError> {
    let names = [
        "manifest.bin",
        "identities.bin",
        "source-directory.bin",
        "topology.bin",
        "evidence.bin",
    ];
    let mut actual = fs::read_dir(root)?
        .map(|e| e.map(|v| v.file_name().to_string_lossy().into_owned()))
        .collect::<Result<Vec<_>, _>>()?;
    actual.sort();
    let mut expected = names.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    expected.sort();
    if actual != expected {
        return Err(invalid(
            "bundle directory does not contain exactly the current five files",
        ));
    }
    let manifest_bytes = fs::read(root.join("manifest.bin"))?;
    let manifest_checksum = checksum(&manifest_bytes);
    let manifest = BundleManifest::decode(&manifest_bytes)?;
    let identities = validated_file(root, "identities.bin", &manifest.identities)?;
    let directory = validated_file(root, "source-directory.bin", &manifest.source_directory)?;
    validate_large_file(root, "topology.bin", &manifest.topology)?;
    validate_large_file(root, "evidence.bin", &manifest.evidence)?;
    let identity_count = manifest
        .node_count
        .checked_add(manifest.relation_kind_count)
        .and_then(|v| v.checked_mul(8))
        .ok_or_else(|| invalid("identity length overflow"))?;
    if identity_count != manifest.identities.length {
        return Err(invalid("identity file length disagrees with counts"));
    }
    let mut d = Decoder::new(&identities);
    let mut nodes = Vec::new();
    for _ in 0..manifest.node_count {
        nodes.push(NodeId::from_u64(d.u64()?));
    }
    let mut relations = Vec::new();
    for _ in 0..manifest.relation_kind_count {
        relations.push(RelationId::from_u64(d.u64()?));
    }
    d.finish()?;
    if nodes.windows(2).any(|w| w[0] >= w[1]) || relations.windows(2).any(|w| w[0] >= w[1]) {
        return Err(invalid(
            "stable identity tables are duplicate or nonascending",
        ));
    }
    let offset_count = manifest
        .node_count
        .checked_add(1)
        .ok_or_else(|| invalid("source offset count overflow"))?;
    let directory_len = offset_count
        .checked_mul(8)
        .and_then(|v| v.checked_add(manifest.segment_count.checked_mul(24)?))
        .ok_or_else(|| invalid("source directory length overflow"))?;
    if directory_len != manifest.source_directory.length {
        return Err(invalid("source directory length disagrees with counts"));
    }
    let mut d = Decoder::new(&directory);
    let mut source_segment_offsets = Vec::new();
    for _ in 0..offset_count {
        source_segment_offsets.push(d.u64()?);
    }
    let mut segments = Vec::new();
    for _ in 0..manifest.segment_count {
        segments.push(SegmentDescriptor {
            source: d.u32()?,
            partition: d.u32()?,
            local_segment: d.u32()?,
            first_edge_ordinal: d.u64()?,
            edge_count: d.u32()?,
        });
    }
    d.finish()?;
    validate_ranges(root, &manifest)?;
    if source_segment_offsets.first() != Some(&0)
        || source_segment_offsets.last() != Some(&manifest.segment_count)
        || source_segment_offsets.windows(2).any(|w| w[0] > w[1])
    {
        return Err(invalid(
            "source segment offsets are not bounded and monotonic",
        ));
    }
    let mut source_edge_offsets = vec![0_u64];
    for source in 0..manifest.node_count as usize {
        let begin = source_segment_offsets[source] as usize;
        let end = source_segment_offsets[source + 1] as usize;
        let mut ordinal = 0_u64;
        for s in &segments[begin..end] {
            if s.source as usize != source || s.first_edge_ordinal != ordinal {
                return Err(invalid("source segments have a mismatch, gap, or overlap"));
            }
            ordinal = ordinal
                .checked_add(s.edge_count as u64)
                .ok_or_else(|| invalid("source edge ordinal overflow"))?;
        }
        source_edge_offsets.push(
            source_edge_offsets
                .last()
                .copied()
                .unwrap()
                .checked_add(ordinal)
                .ok_or_else(|| invalid("adjacency offset overflow"))?,
        );
    }
    if source_edge_offsets.last() != Some(&manifest.adjacency_count) {
        return Err(invalid("source segments do not cover every adjacency"));
    }
    let mut partition_edge_offsets = vec![0_u64];
    for p in &manifest.partitions {
        let topology = read_range(
            &root.join("topology.bin"),
            p.topology_offset,
            p.topology_length,
        )?;
        let evidence = read_range(
            &root.join("evidence.bin"),
            p.evidence_offset,
            p.evidence_length,
        )?;
        let decoded = decode_partition(
            &topology,
            &evidence,
            p,
            manifest.node_count,
            manifest.relation_kind_count,
        )?;
        for (local, s) in decoded.segments.iter().enumerate() {
            let global = segments
                .iter()
                .find(|v| v.partition == p.id && v.local_segment == local as u32)
                .ok_or_else(|| invalid("partition has an extra source segment"))?;
            if global.source != s.source
                || global.first_edge_ordinal != s.first_ordinal
                || global.edge_count != s.count
            {
                return Err(invalid(
                    "partition and directory segment descriptors disagree",
                ));
            }
        }
        partition_edge_offsets.push(partition_edge_offsets.last().copied().unwrap() + p.edge_count);
    }
    if partition_edge_offsets.last() != Some(&manifest.adjacency_count) {
        return Err(invalid("partition adjacency counts disagree"));
    }
    Ok(RoutingBundle {
        root: root.to_path_buf(),
        manifest,
        manifest_checksum,
        nodes: nodes.into_boxed_slice(),
        relations: relations.into_boxed_slice(),
        source_segment_offsets: source_segment_offsets.into_boxed_slice(),
        segments: segments.into_boxed_slice(),
        source_edge_offsets: source_edge_offsets.into_boxed_slice(),
        partition_edge_offsets: partition_edge_offsets.into_boxed_slice(),
    })
}

fn validated_file(
    root: &Path,
    name: &str,
    descriptor: &super::manifest::FileDescriptor,
) -> Result<Vec<u8>, BundleError> {
    let bytes = fs::read(root.join(name))?;
    if bytes.len() as u64 != descriptor.length {
        return Err(invalid(&format!("{name} length mismatch")));
    }
    if checksum(&bytes) != descriptor.checksum {
        return Err(invalid(&format!("{name} checksum mismatch")));
    }
    Ok(bytes)
}
fn validate_ranges(root: &Path, m: &BundleManifest) -> Result<(), BundleError> {
    let mut te = 0;
    let mut ee = 0;
    for p in &m.partitions {
        if p.topology_offset != te
            || p.evidence_offset != ee
            || p.topology_length == 0
            || p.evidence_length == 0
        {
            return Err(invalid(
                "partition ranges are empty, unordered, overlapping, or unaligned",
            ));
        }
        te = te
            .checked_add(p.topology_length)
            .ok_or_else(|| invalid("topology range overflow"))?;
        ee = ee
            .checked_add(p.evidence_length)
            .ok_or_else(|| invalid("evidence range overflow"))?;
        if te > m.topology.length || ee > m.evidence.length {
            return Err(invalid("partition range lies outside its file"));
        }
        let topology = read_range(
            &root.join("topology.bin"),
            p.topology_offset,
            p.topology_length,
        )?;
        let evidence = read_range(
            &root.join("evidence.bin"),
            p.evidence_offset,
            p.evidence_length,
        )?;
        if checksum(&topology) != p.topology_checksum || checksum(&evidence) != p.evidence_checksum
        {
            return Err(invalid("partition region checksum mismatch"));
        }
    }
    if te != m.topology.length || ee != m.evidence.length {
        return Err(invalid(
            "topology or evidence has undeclared trailing bytes",
        ));
    }
    if m.adjacency_count == 0
        && (!m.partitions.is_empty() || m.topology.length != 0 || m.evidence.length != 0)
    {
        return Err(invalid("empty adjacency has noncanonical partition bytes"));
    }
    Ok(())
}

fn validate_large_file(
    root: &Path,
    name: &str,
    descriptor: &super::manifest::FileDescriptor,
) -> Result<(), BundleError> {
    use std::io::Read;
    let mut file = fs::File::open(root.join(name))?;
    if file.metadata()?.len() != descriptor.length {
        return Err(invalid(&format!("{name} length mismatch")));
    }
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    if hasher.finalize().as_bytes() != &descriptor.checksum {
        return Err(invalid(&format!("{name} checksum mismatch")));
    }
    Ok(())
}

#[derive(Debug)]
struct DecodedSegment {
    source: u32,
    first_ordinal: u64,
    start: u32,
    count: u32,
}
#[derive(Debug)]
struct DecodedPartition {
    segments: Vec<DecodedSegment>,
    destinations: Vec<u32>,
    relations: Vec<u32>,
    weights: Vec<u32>,
    edges: Vec<u64>,
}
fn decode_partition(
    top: &[u8],
    ev: &[u8],
    p: &PartitionDescriptor,
    node_count: u64,
    relation_count: u64,
) -> Result<DecodedPartition, BundleError> {
    let mut d = Decoder::new(top);
    let sc = d.u32()?;
    let ec = d.u64()?;
    if sc != p.segment_count || ec != p.edge_count || sc == 0 || ec == 0 {
        return Err(invalid(
            "partition header disagrees with manifest or is empty",
        ));
    }
    let mut segments = Vec::new();
    for _ in 0..sc {
        let source = d.u32()?;
        let first_ordinal = d.u64()?;
        let start = d.u32()?;
        let count = d.u32()?;
        if d.u32()? != 0 {
            return Err(invalid("partition reserved word is nonzero"));
        }
        segments.push(DecodedSegment {
            source,
            first_ordinal,
            start,
            count,
        });
    }
    let mut destinations = Vec::new();
    for _ in 0..ec {
        let v = d.u32()?;
        if v as u64 >= node_count {
            return Err(invalid("destination index is out of bounds"));
        }
        destinations.push(v);
    }
    let mut relations = Vec::new();
    for _ in 0..ec {
        let v = d.u32()?;
        if v as u64 >= relation_count {
            return Err(invalid("relation index is out of bounds"));
        }
        relations.push(v);
    }
    let mut weights = Vec::new();
    for _ in 0..ec {
        let v = d.u32()?;
        BaseWeight::from_bits(v).map_err(|_| invalid("base-weight bits are noncanonical"))?;
        weights.push(v);
    }
    d.finish()?;
    let mut e = Decoder::new(ev);
    let mut edges = Vec::new();
    for _ in 0..ec {
        edges.push(e.u64()?);
    }
    e.finish()?;
    let mut end = 0_u64;
    for s in &segments {
        if s.source as u64 >= node_count || s.start as u64 != end || s.count == 0 {
            return Err(invalid("partition local source bounds are invalid"));
        }
        let begin = s.start as usize;
        let finish = begin + s.count as usize;
        if edges[begin..finish].windows(2).any(|w| w[0] >= w[1]) {
            return Err(invalid(
                "source segment edge IDs are not strictly ascending",
            ));
        }
        end += s.count as u64;
    }
    if end != ec {
        return Err(invalid("partition segments do not cover every edge"));
    }
    Ok(DecodedPartition {
        segments,
        destinations,
        relations,
        weights,
        edges,
    })
}
fn read_range(path: &Path, offset: u64, length: u64) -> Result<Vec<u8>, BundleError> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = fs::File::open(path)?;
    f.seek(SeekFrom::Start(offset))?;
    let len =
        usize::try_from(length).map_err(|_| invalid("partition length does not fit platform"))?;
    let mut out = vec![0; len];
    f.read_exact(&mut out)?;
    Ok(out)
}
fn invalid(reason: &str) -> BundleError {
    BundleError::Invalid(reason.into())
}

enum CacheEntry {
    Loading,
    Ready {
        data: Arc<DecodedPartition>,
        bytes: u64,
        last: u64,
    },
    Failed(String),
}
struct CacheState {
    entries: HashMap<u32, CacheEntry>,
    bytes: u64,
    high: u64,
    tick: u64,
    hits: u64,
    misses: u64,
    waits: u64,
    evictions: u64,
    read_bytes: u64,
    checksum_failures: u64,
}
struct HostCache {
    bundle: Arc<RoutingBundle>,
    config: HostCacheConfig,
    state: Mutex<CacheState>,
    changed: Condvar,
}
impl HostCache {
    fn acquire(&self, id: u32) -> Result<Arc<DecodedPartition>, RoutingError> {
        let p = self
            .bundle
            .manifest
            .partitions
            .get(id as usize)
            .ok_or_else(|| RoutingError::ImageAccess("partition ID is out of bounds".into()))?;
        let required = p
            .topology_length
            .checked_add(p.evidence_length)
            .and_then(|v| v.checked_mul(2))
            .ok_or_else(|| RoutingError::ImageAccess("cache admission size overflow".into()))?;
        if required > self.config.maximum_bytes {
            return Err(RoutingError::ImageAccess(format!(
                "partition requires {required} cache bytes; limit is {}",
                self.config.maximum_bytes
            )));
        }
        loop {
            let mut s = self
                .state
                .lock()
                .map_err(|_| RoutingError::ImageAccess("host cache lock is poisoned".into()))?;
            s.tick = s.tick.saturating_add(1);
            let tick = s.tick;
            match s.entries.get_mut(&id) {
                Some(CacheEntry::Ready { data, last, .. }) => {
                    *last = tick;
                    let data = Arc::clone(data);
                    s.hits = s.hits.saturating_add(1);
                    return Ok(data);
                }
                Some(CacheEntry::Loading) => {
                    s.waits = s.waits.saturating_add(1);
                    drop(self.changed.wait(s).map_err(|_| {
                        RoutingError::ImageAccess("host cache wait is poisoned".into())
                    })?);
                    continue;
                }
                Some(CacheEntry::Failed(reason)) => {
                    return Err(RoutingError::ImageAccess(reason.clone()));
                }
                None => {
                    while s.bytes + required > self.config.maximum_bytes
                        || s.entries.len() >= self.config.maximum_entries
                    {
                        let victim = s
                            .entries
                            .iter()
                            .filter_map(|(&key, e)| match e {
                                CacheEntry::Ready { data, bytes, last }
                                    if Arc::strong_count(data) == 1 =>
                                {
                                    Some((key, *bytes, *last))
                                }
                                _ => None,
                            })
                            .min_by_key(|v| (v.2, v.0));
                        let Some((key, bytes, _)) = victim else {
                            return Err(RoutingError::ImageAccess(
                                "host partition cache is full and every entry is pinned or loading"
                                    .into(),
                            ));
                        };
                        s.entries.remove(&key);
                        s.bytes -= bytes;
                        s.evictions = s.evictions.saturating_add(1);
                    }
                    s.entries.insert(id, CacheEntry::Loading);
                    s.bytes += required;
                    s.high = s.high.max(s.bytes);
                    s.misses = s.misses.saturating_add(1);
                    drop(s);
                    let loaded = self.bundle.load_partition(p);
                    let mut s = self.state.lock().map_err(|_| {
                        RoutingError::ImageAccess("host cache lock is poisoned".into())
                    })?;
                    match loaded {
                        Ok(data) => {
                            s.read_bytes = s
                                .read_bytes
                                .saturating_add(p.topology_length + p.evidence_length);
                            let data = Arc::new(data);
                            s.entries.insert(
                                id,
                                CacheEntry::Ready {
                                    data: Arc::clone(&data),
                                    bytes: required,
                                    last: tick,
                                },
                            );
                            self.changed.notify_all();
                            return Ok(data);
                        }
                        Err(e) => {
                            s.bytes = s.bytes.saturating_sub(required);
                            if e.to_string().contains("checksum") {
                                s.checksum_failures = s.checksum_failures.saturating_add(1);
                            }
                            s.entries.insert(id, CacheEntry::Failed(e.to_string()));
                            self.changed.notify_all();
                            return Err(RoutingError::ImageAccess(e.to_string()));
                        }
                    }
                }
            }
        }
    }
    fn snapshot(&self) -> HostCacheSnapshot {
        let s = self.state.lock().expect("cache snapshot lock");
        HostCacheSnapshot {
            capacity_bytes: self.config.maximum_bytes,
            current_bytes: s.bytes,
            high_water_bytes: s.high,
            hits: s.hits,
            misses: s.misses,
            coalesced_waits: s.waits,
            evictions: s.evictions,
            read_bytes: s.read_bytes,
            checksum_failures: s.checksum_failures,
            entries: s.entries.len(),
        }
    }
}

pub struct ChunkedRoutingImage {
    bundle: Arc<RoutingBundle>,
    cache: Arc<HostCache>,
}
impl ChunkedRoutingImage {
    pub fn open(bundle: RoutingBundle, config: HostCacheConfig) -> Result<Self, BundleError> {
        if config.maximum_bytes == 0
            || config.maximum_entries == 0
            || config.io_worker_count == 0
            || config.maximum_queued_reads == 0
        {
            return Err(invalid("host cache and I/O limits must be nonzero"));
        }
        for p in &bundle.manifest.partitions {
            let required = (p.topology_length + p.evidence_length)
                .checked_mul(2)
                .ok_or_else(|| invalid("cache admission overflow"))?;
            if required > config.maximum_bytes {
                return Err(BundleError::Limit {
                    resource: "host partition cache",
                    required,
                    limit: config.maximum_bytes,
                });
            }
        }
        let bundle = Arc::new(bundle);
        let cache = Arc::new(HostCache {
            bundle: Arc::clone(&bundle),
            config,
            state: Mutex::new(CacheState {
                entries: HashMap::new(),
                bytes: 0,
                high: 0,
                tick: 0,
                hits: 0,
                misses: 0,
                waits: 0,
                evictions: 0,
                read_bytes: 0,
                checksum_failures: 0,
            }),
            changed: Condvar::new(),
        });
        Ok(Self { bundle, cache })
    }
    pub fn bundle(&self) -> &Arc<RoutingBundle> {
        &self.bundle
    }
    pub fn cache_snapshot(&self) -> HostCacheSnapshot {
        self.cache.snapshot()
    }
    pub fn routing_manifest(&self) -> Result<crate::RoutingImageManifest, BundleError> {
        self.bundle.routing_manifest()
    }
    pub fn estimate_working_set(
        &self,
        request: &RoutingRequest,
    ) -> Result<CpuWorkingSetEstimate, RoutingError> {
        estimate_topology_working_set(self, request)
    }
}
impl RoutingTopology for ChunkedRoutingImage {
    fn node_count(&self) -> usize {
        self.bundle.nodes.len()
    }
    fn relation_kind_count(&self) -> usize {
        self.bundle.relations.len()
    }
    fn adjacency_count(&self) -> usize {
        self.bundle.manifest.adjacency_count as usize
    }
    fn dense_node_id(&self, id: NodeId) -> Option<DenseNodeId> {
        self.bundle
            .nodes
            .binary_search(&id)
            .ok()
            .map(|v| DenseNodeId::from_u32(v as u32))
    }
    fn external_node_id(&self, id: DenseNodeId) -> Option<NodeId> {
        self.bundle.nodes.get(id.as_usize()).copied()
    }
    fn relation_ids(&self) -> &[RelationId] {
        &self.bundle.relations
    }
    fn outgoing_range(&self, source: DenseNodeId) -> Result<std::ops::Range<usize>, RoutingError> {
        let i = source.as_usize();
        Ok(self.bundle.source_edge_offsets[i] as usize
            ..self.bundle.source_edge_offsets[i + 1] as usize)
    }
    fn edge_at(&self, index: usize) -> Result<OutgoingEdge, RoutingError> {
        let global = index as u64;
        let partition = self
            .bundle
            .partition_edge_offsets
            .partition_point(|&v| v <= global)
            .saturating_sub(1);
        let local = global - self.bundle.partition_edge_offsets[partition];
        let data = self.cache.acquire(partition as u32)?;
        let i = local as usize;
        let relation_index = data.relations[i];
        Ok(OutgoingEdge::from_bundle(
            EdgeId::from_u64(data.edges[i]),
            DenseNodeId::from_u32(data.destinations[i]),
            self.bundle.relations[relation_index as usize],
            BaseWeight::from_bits(data.weights[i])
                .map_err(|_| RoutingError::ImageAccess("cached weight became invalid".into()))?,
            relation_index,
        ))
    }
}
pub fn route_partitioned(
    image: &ChunkedRoutingImage,
    request: &RoutingRequest,
) -> Result<RoutingResponse, RoutingError> {
    route_partitioned_controlled(image, request, &NeverCancelled).map(|v| v.0)
}
pub fn route_partitioned_controlled(
    image: &ChunkedRoutingImage,
    request: &RoutingRequest,
    cancellation: &impl CancellationSignal,
) -> Result<
    (
        RoutingResponse,
        CpuSearchDiagnostics,
        PartitionedCpuDiagnostics,
    ),
    RoutingError,
> {
    let before = image.cache.snapshot();
    let (response, cpu) = route_topology_controlled(image, request, cancellation)?;
    let after = image.cache.snapshot();
    Ok((
        response,
        cpu,
        PartitionedCpuDiagnostics {
            cache: after,
            partitions: after.misses.saturating_sub(before.misses),
            file_bytes: after.read_bytes.saturating_sub(before.read_bytes),
            io_wait: Duration::ZERO,
        },
    ))
}
