use std::{
    collections::HashMap,
    ops::Deref,
    sync::{Arc, Condvar, Mutex, atomic::AtomicBool},
    time::Duration,
};

use cudarc::driver::{CudaEvent, CudaSlice, CudaStream};
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
    pub host_loading_entries: usize,
    pub copying_entries: usize,
    pub evicting_entries: usize,
    pub ready_entries: usize,
    pub failed_entries: usize,
    pub hits: u64,
    pub misses: u64,
    pub coalesced_waits: u64,
    pub copies: u64,
    pub evictions: u64,
    pub slot_waits: u64,
    pub completion_waits: u64,
    pub in_use_slots: usize,
    pub transfer_bytes: u64,
}

#[derive(Debug)]
pub(crate) struct DevicePartition {
    pub destinations: CudaSlice<u32>,
    pub relation_indexes: CudaSlice<u32>,
    pub base_weight_bits: CudaSlice<u32>,
    pub edge_count: u32,
    pub host_segments: Vec<(u32, u32, u32)>,
    pub bytes: usize,
    pub transfer_bytes: usize,
}

struct ReadyEntry {
    partition: Arc<DevicePartition>,
    active_users: usize,
    completion_events: Vec<CudaEvent>,
    last_use: u64,
}

enum EntryState {
    HostLoading { bytes: usize },
    Copying { bytes: usize },
    Evicting,
    Ready(ReadyEntry),
    Failed(String),
}

struct State {
    entries: HashMap<u32, EntryState>,
    bytes: usize,
    high: usize,
    tick: u64,
    hits: u64,
    misses: u64,
    coalesced_waits: u64,
    copies: u64,
    evictions: u64,
    slot_waits: u64,
    completion_waits: u64,
    transfer_bytes: u64,
    poison: Option<String>,
}

struct CacheInner {
    context: Arc<CudaContextOwner>,
    copy_stream: Arc<CudaStream>,
    image: Arc<ChunkedRoutingImage>,
    staging: StagingPool,
    maximum_bytes: usize,
    maximum_slots: usize,
    state: Mutex<State>,
    changed: Condvar,
    faults: Arc<CudaFaultInjection>,
}

pub(crate) struct DeviceTopologyCache {
    inner: Arc<CacheInner>,
}

pub(crate) struct DevicePartitionLease {
    id: u32,
    partition: Arc<DevicePartition>,
    cache: Arc<CacheInner>,
}

impl Deref for DevicePartitionLease {
    type Target = DevicePartition;

    fn deref(&self) -> &Self::Target {
        &self.partition
    }
}

impl DevicePartitionLease {
    pub fn record_completion(&self) -> Result<(), CudaError> {
        let event = self
            .cache
            .context
            .stream
            .record_event(None)
            .map_err(event_error)?;
        let mut state = self
            .cache
            .state
            .lock()
            .map_err(|_| cache_error("lock is poisoned"))?;
        let Some(EntryState::Ready(entry)) = state.entries.get_mut(&self.id) else {
            return Err(cache_error(
                "in-use partition disappeared before event record",
            ));
        };
        if !Arc::ptr_eq(&entry.partition, &self.partition) {
            return Err(cache_error("partition slot changed before event record"));
        }
        entry.completion_events.push(event);
        Ok(())
    }
}

impl Drop for DevicePartitionLease {
    fn drop(&mut self) {
        if let Ok(mut state) = self.cache.state.lock()
            && let Some(EntryState::Ready(entry)) = state.entries.get_mut(&self.id)
            && Arc::ptr_eq(&entry.partition, &self.partition)
        {
            entry.active_users = entry.active_users.saturating_sub(1);
            self.cache.changed.notify_all();
        }
    }
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
        let copy_stream = context.context.new_stream().map_err(|error| {
            CudaError::new(
                CudaFailureKind::Context,
                format!("CUDA topology copy stream creation failed: {error}"),
            )
        })?;
        Ok(Self {
            inner: Arc::new(CacheInner {
                context,
                copy_stream,
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
                    coalesced_waits: 0,
                    copies: 0,
                    evictions: 0,
                    slot_waits: 0,
                    completion_waits: 0,
                    transfer_bytes: 0,
                    poison: None,
                }),
                changed: Condvar::new(),
                faults,
            }),
        })
    }

    pub fn acquire(
        &self,
        id: u32,
        cancellation: &AtomicBool,
    ) -> Result<Option<DevicePartitionLease>, CudaError> {
        let descriptor = self
            .inner
            .image
            .bundle()
            .manifest()
            .partitions
            .get(id as usize)
            .ok_or_else(|| cache_error("partition ID is outside the manifest"))?;
        let required = device_partition_bytes(descriptor)?;
        loop {
            if cancellation.load(std::sync::atomic::Ordering::Acquire) {
                return Ok(None);
            }
            let mut state = self
                .inner
                .state
                .lock()
                .map_err(|_| cache_error("lock is poisoned"))?;
            if let Some(reason) = &state.poison {
                return Err(cache_error(&format!("cache is poisoned: {reason}")));
            }
            state.tick = state.tick.saturating_add(1);
            let tick = state.tick;
            match state.entries.get_mut(&id) {
                Some(EntryState::Ready(entry)) => {
                    prune_completed_events(entry, self.inner.faults.completion_events_held());
                    entry.active_users = entry.active_users.saturating_add(1);
                    entry.last_use = tick;
                    let partition = Arc::clone(&entry.partition);
                    state.hits = state.hits.saturating_add(1);
                    return Ok(Some(DevicePartitionLease {
                        id,
                        partition,
                        cache: Arc::clone(&self.inner),
                    }));
                }
                Some(EntryState::HostLoading { .. } | EntryState::Copying { .. }) => {
                    state.coalesced_waits = state.coalesced_waits.saturating_add(1);
                    drop(self.wait(state)?);
                    continue;
                }
                Some(EntryState::Evicting) => {
                    state.slot_waits = state.slot_waits.saturating_add(1);
                    drop(self.wait(state)?);
                    continue;
                }
                Some(EntryState::Failed(reason)) => {
                    let reason = reason.clone();
                    state.entries.remove(&id);
                    self.inner.changed.notify_all();
                    return Err(cache_error(&reason));
                }
                None => {
                    match prepare_slot(
                        &mut state,
                        required,
                        self.inner.maximum_bytes,
                        self.inner.maximum_slots,
                        self.inner.faults.completion_events_held(),
                    ) {
                        SlotDecision::Available => {}
                        SlotDecision::Evict { id, entry } => {
                            drop(state);
                            // Releasing a CUDA allocation may wait for driver bookkeeping. Keep
                            // that work outside the cache mutex while the explicit Evicting state
                            // prevents another requester from reusing the slot prematurely.
                            drop(entry);
                            let mut state = self
                                .inner
                                .state
                                .lock()
                                .map_err(|_| cache_error("lock is poisoned"))?;
                            if matches!(state.entries.get(&id), Some(EntryState::Evicting)) {
                                state.entries.remove(&id);
                            }
                            self.inner.changed.notify_all();
                            drop(state);
                            continue;
                        }
                        SlotDecision::Wait => {
                            state.slot_waits = state.slot_waits.saturating_add(1);
                            drop(self.wait(state)?);
                            continue;
                        }
                    }
                    state
                        .entries
                        .insert(id, EntryState::HostLoading { bytes: required });
                    state.bytes = state
                        .bytes
                        .checked_add(required)
                        .ok_or_else(|| cache_error("byte accounting overflow"))?;
                    state.high = state.high.max(state.bytes);
                    state.misses = state.misses.saturating_add(1);
                    drop(state);
                    return self.load_reserved(id, required, cancellation);
                }
            }
        }
    }

    fn wait<'a>(
        &self,
        state: std::sync::MutexGuard<'a, State>,
    ) -> Result<std::sync::MutexGuard<'a, State>, CudaError> {
        self.inner
            .changed
            .wait_timeout(state, Duration::from_millis(2))
            .map(|(state, _)| state)
            .map_err(|_| cache_error("wait is poisoned"))
    }

    fn load_reserved(
        &self,
        id: u32,
        bytes: usize,
        cancellation: &AtomicBool,
    ) -> Result<Option<DevicePartitionLease>, CudaError> {
        let host = match self.inner.image.acquire_partition(id, cancellation) {
            Ok(Some(host)) => host,
            Ok(None) => {
                self.cancel_reserved(id, bytes)?;
                return Ok(None);
            }
            Err(error) => {
                let error = match error {
                    pathhydra_routing::RoutingError::ImageAccess(reason) => CudaError::new(
                        CudaFailureKind::ImageAccess,
                        format!("CUDA bundle partition access failed: {reason}"),
                    ),
                    error => cache_error(&error.to_string()),
                };
                self.fail_reserved(id, bytes, &error)?;
                return Err(error);
            }
        };
        if cancellation.load(std::sync::atomic::Ordering::Acquire) {
            self.cancel_reserved(id, bytes)?;
            return Ok(None);
        }
        if let Err(error) = self.inner.faults.trip(CudaFaultStage::PinnedAllocation) {
            self.fail_reserved(id, bytes, &error)?;
            return Err(error);
        }
        let _staging = match self.inner.staging.reserve(bytes) {
            Ok(staging) => staging,
            Err(error) => {
                self.fail_reserved(id, bytes, &error)?;
                return Err(error);
            }
        };
        {
            let mut state = self
                .inner
                .state
                .lock()
                .map_err(|_| cache_error("lock is poisoned"))?;
            if let Some(reason) = &state.poison {
                return Err(cache_error(&format!("cache is poisoned: {reason}")));
            }
            match state.entries.get_mut(&id) {
                Some(entry @ EntryState::HostLoading { .. }) => {
                    *entry = EntryState::Copying { bytes };
                }
                _ => return Err(cache_error("host-loading state was lost")),
            }
            self.inner.changed.notify_all();
        }
        let partition = match self.copy_partition(&host) {
            Ok(partition) => partition,
            Err(error) => {
                self.fail_reserved(id, bytes, &error)?;
                return Err(error);
            }
        };
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| cache_error("lock is poisoned"))?;
        if let Some(reason) = &state.poison {
            return Err(cache_error(&format!("cache is poisoned: {reason}")));
        }
        let Some(EntryState::Copying { bytes: reserved }) = state.entries.get(&id) else {
            return Err(cache_error("copying state was lost"));
        };
        if *reserved != bytes {
            return Err(cache_error("copy reservation changed"));
        }
        let tick = state.tick;
        state.entries.insert(
            id,
            EntryState::Ready(ReadyEntry {
                partition: Arc::clone(&partition),
                active_users: 1,
                completion_events: Vec::new(),
                last_use: tick,
            }),
        );
        state.copies = state.copies.saturating_add(1);
        state.transfer_bytes = state
            .transfer_bytes
            .saturating_add(partition.transfer_bytes as u64);
        self.inner.changed.notify_all();
        Ok(Some(DevicePartitionLease {
            id,
            partition,
            cache: Arc::clone(&self.inner),
        }))
    }

    fn copy_partition(
        &self,
        host: &pathhydra_routing::PartitionLease,
    ) -> Result<Arc<DevicePartition>, CudaError> {
        self.inner.faults.trip(CudaFaultStage::Copy)?;
        let segments: Vec<_> = host.source_segments().collect();
        let stream = &self.inner.copy_stream;
        let partition = Arc::new(DevicePartition {
            destinations: stream
                .clone_htod(host.destinations())
                .map_err(upload_error)?,
            relation_indexes: stream
                .clone_htod(host.relation_indexes())
                .map_err(upload_error)?,
            base_weight_bits: stream
                .clone_htod(host.base_weight_bits())
                .map_err(upload_error)?,
            edge_count: u32::try_from(host.destinations().len())
                .map_err(|_| cache_error("edge count exceeds CUDA ABI"))?,
            host_segments: segments
                .iter()
                .map(|segment| (segment.source, segment.start, segment.edge_count))
                .collect(),
            bytes: cached_partition_bytes(host)?,
            transfer_bytes: host
                .destinations()
                .len()
                .checked_mul(12)
                .ok_or_else(|| cache_error("partition transfer byte count overflow"))?,
        });
        let copy_complete = stream.record_event(None).map_err(upload_error)?;
        if let Err(error) = self.inner.faults.trip(CudaFaultStage::ContextLoss) {
            let _ = copy_complete.synchronize();
            return Err(error);
        }
        if let Err(error) = self.inner.faults.trip(CudaFaultStage::Synchronization) {
            let _ = copy_complete.synchronize();
            return Err(error);
        }
        copy_complete.synchronize().map_err(upload_error)?;
        Ok(partition)
    }

    fn cancel_reserved(&self, id: u32, bytes: usize) -> Result<(), CudaError> {
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| cache_error("lock is poisoned"))?;
        if matches!(
            state.entries.get(&id),
            Some(EntryState::HostLoading { .. } | EntryState::Copying { .. })
        ) {
            state.entries.remove(&id);
            state.bytes = state.bytes.saturating_sub(bytes);
        }
        self.inner.changed.notify_all();
        Ok(())
    }

    fn fail_reserved(&self, id: u32, bytes: usize, error: &CudaError) -> Result<(), CudaError> {
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| cache_error("lock is poisoned"))?;
        if error.poisons_context() {
            let reason = error.to_string();
            state.poison = Some(reason.clone());
            let mut released = 0_usize;
            for entry in state.entries.values_mut() {
                match entry {
                    EntryState::HostLoading { bytes } | EntryState::Copying { bytes } => {
                        released = released.saturating_add(*bytes);
                        *entry = EntryState::Failed(reason.clone());
                    }
                    EntryState::Evicting | EntryState::Ready(_) | EntryState::Failed(_) => {}
                }
            }
            state.bytes = state.bytes.saturating_sub(released);
        } else if matches!(
            state.entries.get(&id),
            Some(EntryState::HostLoading { .. } | EntryState::Copying { .. })
        ) {
            state.bytes = state.bytes.saturating_sub(bytes);
            state
                .entries
                .insert(id, EntryState::Failed(error.to_string()));
        }
        self.inner.changed.notify_all();
        Ok(())
    }

    pub fn snapshot(&self) -> DeviceTopologyCacheSnapshot {
        let Ok(mut state) = self.inner.state.lock() else {
            return DeviceTopologyCacheSnapshot {
                capacity_bytes: self.inner.maximum_bytes,
                capacity_slots: self.inner.maximum_slots,
                ..DeviceTopologyCacheSnapshot::default()
            };
        };
        for entry in state.entries.values_mut() {
            if let EntryState::Ready(entry) = entry {
                prune_completed_events(entry, self.inner.faults.completion_events_held());
            }
        }
        let mut snapshot = DeviceTopologyCacheSnapshot {
            capacity_bytes: self.inner.maximum_bytes,
            capacity_slots: self.inner.maximum_slots,
            current_bytes: state.bytes,
            high_water_bytes: state.high,
            entries: state.entries.len(),
            hits: state.hits,
            misses: state.misses,
            coalesced_waits: state.coalesced_waits,
            copies: state.copies,
            evictions: state.evictions,
            slot_waits: state.slot_waits,
            completion_waits: state.completion_waits,
            transfer_bytes: state.transfer_bytes,
            ..DeviceTopologyCacheSnapshot::default()
        };
        for entry in state.entries.values() {
            match entry {
                EntryState::HostLoading { .. } => snapshot.host_loading_entries += 1,
                EntryState::Copying { .. } => snapshot.copying_entries += 1,
                EntryState::Evicting => snapshot.evicting_entries += 1,
                EntryState::Ready(entry) => {
                    snapshot.ready_entries += 1;
                    if entry.active_users != 0 || !entry.completion_events.is_empty() {
                        snapshot.in_use_slots += 1;
                    }
                }
                EntryState::Failed(_) => snapshot.failed_entries += 1,
            }
        }
        snapshot
    }

    pub fn staging_snapshot(&self) -> crate::staging::StagingSnapshot {
        self.inner.staging.snapshot()
    }
}

enum SlotDecision {
    Available,
    Evict { id: u32, entry: ReadyEntry },
    Wait,
}

fn prepare_slot(
    state: &mut State,
    required: usize,
    maximum_bytes: usize,
    maximum_slots: usize,
    hold_completion_events: bool,
) -> SlotDecision {
    loop {
        for entry in state.entries.values_mut() {
            if let EntryState::Ready(entry) = entry {
                prune_completed_events(entry, hold_completion_events);
            }
        }
        let slot_full = state.entries.len() >= maximum_slots;
        let bytes_full = state
            .bytes
            .checked_add(required)
            .is_none_or(|total| total > maximum_bytes);
        if !slot_full && !bytes_full {
            return SlotDecision::Available;
        }
        let victim = state
            .entries
            .iter()
            .filter_map(|(&id, entry)| match entry {
                EntryState::Ready(entry)
                    if entry.active_users == 0 && entry.completion_events.is_empty() =>
                {
                    Some((id, entry.last_use, entry.partition.bytes))
                }
                EntryState::Failed(_) => Some((id, 0, 0)),
                EntryState::HostLoading { .. }
                | EntryState::Copying { .. }
                | EntryState::Evicting
                | EntryState::Ready(_) => None,
            })
            .min_by_key(|(id, last, _)| (*last, *id));
        let Some((victim, _, bytes)) = victim else {
            if state.entries.values().any(|entry| {
                matches!(entry, EntryState::Ready(ready) if !ready.completion_events.is_empty())
            }) {
                state.completion_waits = state.completion_waits.saturating_add(1);
            }
            return SlotDecision::Wait;
        };
        let removed = state
            .entries
            .remove(&victim)
            .expect("selected cache victim must remain present");
        match removed {
            EntryState::Ready(entry) => {
                state.entries.insert(victim, EntryState::Evicting);
                state.bytes = state.bytes.saturating_sub(bytes);
                state.evictions = state.evictions.saturating_add(1);
                return SlotDecision::Evict { id: victim, entry };
            }
            EntryState::Failed(_) => {}
            EntryState::HostLoading { .. } | EntryState::Copying { .. } | EntryState::Evicting => {
                unreachable!("only ready or failed cache entries can be selected for eviction")
            }
        }
    }
}

fn prune_completed_events(entry: &mut ReadyEntry, hold: bool) {
    if hold {
        return;
    }
    entry.completion_events.retain(|event| !event.is_complete());
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

fn cached_partition_bytes(host: &pathhydra_routing::PartitionLease) -> Result<usize, CudaError> {
    let segment_count = host.source_segments().count();
    host.destinations()
        .len()
        .checked_mul(12)
        .and_then(|bytes| bytes.checked_add(segment_count.checked_mul(12)?))
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

fn event_error(error: cudarc::driver::DriverError) -> CudaError {
    CudaError::new(
        CudaFailureKind::Launch,
        format!("CUDA partition completion event failed: {error}"),
    )
}
