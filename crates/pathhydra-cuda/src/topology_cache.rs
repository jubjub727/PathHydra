use std::{
    collections::HashMap,
    sync::{Arc, Mutex, atomic::AtomicBool},
};

use cudarc::driver::CudaSlice;
use pathhydra_routing::ChunkedRoutingImage;

use crate::{
    CudaContextOwner, CudaError, CudaFailureKind, CudaFaultInjection, CudaFaultStage,
    staging::StagingPool,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DeviceTopologyCacheSnapshot {
    pub capacity_bytes: usize,
    pub capacity_slots: usize,
    pub current_bytes: usize,
    pub high_water_bytes: usize,
    pub entries: usize,
    pub hits: u64,
    pub misses: u64,
    pub copies: u64,
    pub evictions: u64,
    pub slot_waits: u64,
    pub in_use_slots: usize,
    pub transfer_bytes: u64,
}

#[derive(Debug)]
pub(crate) struct DevicePartition {
    pub segment_sources: CudaSlice<u32>,
    pub segment_starts: CudaSlice<u32>,
    pub segment_counts: CudaSlice<u32>,
    pub destinations: CudaSlice<u32>,
    pub relation_indexes: CudaSlice<u32>,
    pub base_weight_bits: CudaSlice<u32>,
    pub segment_count: u32,
    pub edge_count: u32,
    pub bytes: usize,
}

struct Entry {
    partition: Arc<DevicePartition>,
    last_release: u64,
}

struct State {
    entries: HashMap<u32, Entry>,
    bytes: usize,
    high: usize,
    tick: u64,
    hits: u64,
    misses: u64,
    copies: u64,
    evictions: u64,
    slot_waits: u64,
    transfer_bytes: u64,
}

pub(crate) struct DeviceTopologyCache {
    context: Arc<CudaContextOwner>,
    image: Arc<ChunkedRoutingImage>,
    staging: StagingPool,
    maximum_bytes: usize,
    maximum_slots: usize,
    state: Mutex<State>,
    faults: Arc<CudaFaultInjection>,
}

impl DeviceTopologyCache {
    pub fn new(
        context: Arc<CudaContextOwner>,
        image: Arc<ChunkedRoutingImage>,
        maximum_bytes: usize,
        maximum_slots: usize,
        staging_bytes: usize,
        faults: Arc<CudaFaultInjection>,
    ) -> Result<Self, CudaError> {
        if maximum_bytes == 0 || maximum_slots == 0 {
            return Err(CudaError::new(
                CudaFailureKind::Admission,
                "CUDA topology cache byte and slot limits must be nonzero",
            ));
        }
        let maximum_partition_bytes = image
            .bundle()
            .manifest()
            .partitions
            .iter()
            .map(device_partition_bytes)
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .max()
            .unwrap_or(0);
        if maximum_partition_bytes > maximum_bytes {
            return Err(CudaError::new(
                CudaFailureKind::Admission,
                format!(
                    "largest CUDA partition requires {maximum_partition_bytes} bytes; topology cache capacity is {maximum_bytes}"
                ),
            ));
        }
        if maximum_partition_bytes > staging_bytes {
            return Err(CudaError::new(
                CudaFailureKind::Admission,
                format!(
                    "largest CUDA partition requires {maximum_partition_bytes} staging bytes; capacity is {staging_bytes}"
                ),
            ));
        }
        Ok(Self {
            context,
            image,
            staging: StagingPool::new(staging_bytes)?,
            maximum_bytes,
            maximum_slots,
            state: Mutex::new(State {
                entries: HashMap::new(),
                bytes: 0,
                high: 0,
                tick: 0,
                hits: 0,
                misses: 0,
                copies: 0,
                evictions: 0,
                slot_waits: 0,
                transfer_bytes: 0,
            }),
            faults,
        })
    }

    pub fn acquire(
        &self,
        id: u32,
        cancellation: &AtomicBool,
    ) -> Result<Option<Arc<DevicePartition>>, CudaError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| cache_error("lock is poisoned"))?;
        state.tick = state.tick.saturating_add(1);
        let tick = state.tick;
        if let Some(entry) = state.entries.get_mut(&id) {
            entry.last_release = tick;
            let partition = Arc::clone(&entry.partition);
            state.hits = state.hits.saturating_add(1);
            return Ok(Some(partition));
        }
        state.misses = state.misses.saturating_add(1);
        let Some(host) =
            self.image
                .acquire_partition(id, cancellation)
                .map_err(|error| match error {
                    pathhydra_routing::RoutingError::ImageAccess(reason) => CudaError::new(
                        CudaFailureKind::ImageAccess,
                        format!("CUDA bundle partition access failed: {reason}"),
                    ),
                    error => cache_error(&error.to_string()),
                })?
        else {
            return Ok(None);
        };
        let bytes = host_device_bytes(&host)?;
        while state.entries.len() >= self.maximum_slots
            || state
                .bytes
                .checked_add(bytes)
                .is_none_or(|v| v > self.maximum_bytes)
        {
            let victim = state
                .entries
                .iter()
                .filter(|(_, entry)| Arc::strong_count(&entry.partition) == 1)
                .min_by_key(|(id, entry)| (entry.last_release, **id))
                .map(|(&id, _)| id);
            let Some(victim) = victim else {
                state.slot_waits = state.slot_waits.saturating_add(1);
                return Err(CudaError::new(
                    CudaFailureKind::Admission,
                    "CUDA topology cache is full and every slot is in use",
                ));
            };
            let removed = state
                .entries
                .remove(&victim)
                .expect("selected cache victim");
            state.bytes = state.bytes.saturating_sub(removed.partition.bytes);
            state.evictions = state.evictions.saturating_add(1);
        }
        self.faults.trip(CudaFaultStage::PinnedAllocation)?;
        let _staging = self.staging.reserve(bytes)?;
        let segments: Vec<_> = host.source_segments().collect();
        let sources: Vec<_> = segments.iter().map(|segment| segment.source).collect();
        let starts: Vec<_> = segments.iter().map(|segment| segment.start).collect();
        let counts: Vec<_> = segments.iter().map(|segment| segment.edge_count).collect();
        let stream = &self.context.stream;
        self.faults.trip(CudaFaultStage::Copy)?;
        let partition = Arc::new(DevicePartition {
            segment_sources: stream.clone_htod(&sources).map_err(upload_error)?,
            segment_starts: stream.clone_htod(&starts).map_err(upload_error)?,
            segment_counts: stream.clone_htod(&counts).map_err(upload_error)?,
            destinations: stream
                .clone_htod(host.destinations())
                .map_err(upload_error)?,
            relation_indexes: stream
                .clone_htod(host.relation_indexes())
                .map_err(upload_error)?,
            base_weight_bits: stream
                .clone_htod(host.base_weight_bits())
                .map_err(upload_error)?,
            segment_count: u32::try_from(segments.len())
                .map_err(|_| cache_error("segment count exceeds CUDA ABI"))?,
            edge_count: u32::try_from(host.destinations().len())
                .map_err(|_| cache_error("edge count exceeds CUDA ABI"))?,
            bytes,
        });
        if let Err(error) = self.faults.trip(CudaFaultStage::ContextLoss) {
            let _ = stream.synchronize();
            return Err(error);
        }
        if let Err(error) = self.faults.trip(CudaFaultStage::Synchronization) {
            let _ = stream.synchronize();
            return Err(error);
        }
        stream.synchronize().map_err(upload_error)?;
        state.bytes = state
            .bytes
            .checked_add(bytes)
            .ok_or_else(|| cache_error("byte accounting overflow"))?;
        state.high = state.high.max(state.bytes);
        state.copies = state.copies.saturating_add(1);
        state.transfer_bytes = state.transfer_bytes.saturating_add(bytes as u64);
        state.entries.insert(
            id,
            Entry {
                partition: Arc::clone(&partition),
                last_release: tick,
            },
        );
        Ok(Some(partition))
    }

    pub fn snapshot(&self) -> DeviceTopologyCacheSnapshot {
        let Ok(state) = self.state.lock() else {
            return DeviceTopologyCacheSnapshot {
                capacity_bytes: self.maximum_bytes,
                capacity_slots: self.maximum_slots,
                ..DeviceTopologyCacheSnapshot::default()
            };
        };
        DeviceTopologyCacheSnapshot {
            capacity_bytes: self.maximum_bytes,
            capacity_slots: self.maximum_slots,
            current_bytes: state.bytes,
            high_water_bytes: state.high,
            entries: state.entries.len(),
            hits: state.hits,
            misses: state.misses,
            copies: state.copies,
            evictions: state.evictions,
            slot_waits: state.slot_waits,
            in_use_slots: state
                .entries
                .values()
                .filter(|entry| Arc::strong_count(&entry.partition) > 1)
                .count(),
            transfer_bytes: state.transfer_bytes,
        }
    }

    pub fn staging_snapshot(&self) -> crate::staging::StagingSnapshot {
        self.staging.snapshot()
    }
}

fn device_partition_bytes(
    descriptor: &pathhydra_routing::PartitionDescriptor,
) -> Result<usize, CudaError> {
    let segments = usize::try_from(descriptor.segment_count)
        .map_err(|_| cache_error("segment count does not fit platform"))?;
    let edges = usize::try_from(descriptor.edge_count)
        .map_err(|_| cache_error("edge count does not fit platform"))?;
    segments
        .checked_mul(12)
        .and_then(|value| value.checked_add(edges.checked_mul(12)?))
        .ok_or_else(|| cache_error("partition device byte count overflow"))
}

fn host_device_bytes(host: &pathhydra_routing::PartitionLease) -> Result<usize, CudaError> {
    host.source_segments()
        .len()
        .checked_mul(12)
        .and_then(|value| value.checked_add(host.destinations().len().checked_mul(12)?))
        .ok_or_else(|| cache_error("partition device byte count overflow"))
}

fn cache_error(message: &str) -> CudaError {
    CudaError::new(
        CudaFailureKind::Upload,
        format!("CUDA topology cache failure: {message}"),
    )
}

fn upload_error(error: cudarc::driver::DriverError) -> CudaError {
    CudaError::new(
        CudaFailureKind::Upload,
        format!("CUDA partition copy failed: {error}"),
    )
}
