use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use cudarc::driver::CudaSlice;
use pathhydra_routing::{
    ChunkedRoutingImage, CompletionReason, DestinationResult, DestinationState, ExactRoute,
    RoutingRequest, RoutingResponse, SearchBudget,
};

use crate::{
    CudaAlgorithm, CudaContextOwner, CudaError, CudaFailureKind, CudaFaultInjection,
    CudaFaultStage, CudaRouteDiagnostics, CudaRouteOutput, DeviceTopologyCacheSnapshot,
    StagingSnapshot, launch, topology_cache::DeviceTopologyCache,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CudaPartitionedConfig {
    pub maximum_topology_cache_bytes: usize,
    pub maximum_topology_cache_slots: usize,
    pub maximum_host_staging_bytes: usize,
    pub minimum_free_memory_headroom: usize,
    pub reserved_concurrent_search_bytes: usize,
    pub reverse_partition_order: bool,
}

pub struct CudaPartitionedImage {
    pub(crate) context: Arc<CudaContextOwner>,
    cpu_image: Arc<ChunkedRoutingImage>,
    cache: DeviceTopologyCache,
    config: CudaPartitionedConfig,
    faults: Arc<CudaFaultInjection>,
}

impl CudaPartitionedImage {
    pub fn upload(
        context: Arc<CudaContextOwner>,
        cpu_image: Arc<ChunkedRoutingImage>,
        config: CudaPartitionedConfig,
    ) -> Result<Arc<Self>, CudaError> {
        let reserved = config
            .minimum_free_memory_headroom
            .checked_add(config.maximum_topology_cache_bytes)
            .and_then(|value| value.checked_add(config.reserved_concurrent_search_bytes))
            .and_then(|value| value.checked_add(4096))
            .ok_or_else(|| {
                CudaError::new(
                    CudaFailureKind::Admission,
                    "partitioned CUDA reservation overflow",
                )
            })?;
        let (free, _) = context.current_memory()?;
        if free < reserved {
            return Err(CudaError::new(
                CudaFailureKind::Admission,
                format!(
                    "partitioned CUDA headroom, search state, and topology cache reserve {reserved} bytes; driver reports {free} free"
                ),
            ));
        }
        let faults = Arc::new(CudaFaultInjection::default());
        let cache = DeviceTopologyCache::new(
            Arc::clone(&context),
            Arc::clone(&cpu_image),
            config.maximum_topology_cache_bytes,
            config.maximum_topology_cache_slots,
            config.maximum_host_staging_bytes,
            Arc::clone(&faults),
        )?;
        Ok(Arc::new(Self {
            context,
            cpu_image,
            cache,
            config,
            faults,
        }))
    }

    #[must_use]
    pub const fn cpu_image(&self) -> &Arc<ChunkedRoutingImage> {
        &self.cpu_image
    }

    #[must_use]
    pub fn node_count(&self) -> usize {
        self.cpu_image.node_count()
    }

    #[must_use]
    pub fn adjacency_count(&self) -> usize {
        self.cpu_image.adjacency_count()
    }

    #[must_use]
    pub fn allocated_bytes(&self) -> usize {
        self.cache.snapshot().current_bytes
    }

    #[must_use]
    pub fn topology_cache_snapshot(&self) -> DeviceTopologyCacheSnapshot {
        self.cache.snapshot()
    }

    #[must_use]
    pub fn staging_snapshot(&self) -> StagingSnapshot {
        self.cache.staging_snapshot()
    }

    #[doc(hidden)]
    pub fn fault_injection(&self) -> Arc<CudaFaultInjection> {
        Arc::clone(&self.faults)
    }

    pub fn route(
        &self,
        request: &RoutingRequest,
        algorithm: CudaAlgorithm,
        cancellation: &AtomicBool,
        reserved_search_bytes: usize,
    ) -> Result<CudaRouteOutput, CudaError> {
        if request.return_paths() {
            return Err(CudaError::new(
                CudaFailureKind::Admission,
                "paths are unsupported by CUDA",
            ));
        }
        if request.budget() != SearchBudget::Unlimited {
            return Err(CudaError::new(
                CudaFailureKind::Admission,
                "finite examined-edge budgets are unsupported by CUDA",
            ));
        }
        let origin = self
            .cpu_image
            .dense_node_id(request.origin())
            .ok_or_else(|| invariant("origin is absent from the acquired partitioned image"))?;
        let packed = self
            .cpu_image
            .pack_profile(request.profile())
            .map_err(|error| invariant(&format!("invalid relation profile: {error}")))?;
        let (enabled, multipliers) = packed.accelerator_arrays();
        let mapped: Vec<_> = request
            .destinations()
            .iter()
            .map(|&destination| self.cpu_image.dense_node_id(destination))
            .collect();
        let cancelled_before_launch = cancellation.load(Ordering::Acquire);
        let no_search = self.cpu_image.adjacency_count() == 0
            || mapped
                .iter()
                .all(|destination| destination.is_none_or(|dense| dense == origin));
        let started = Instant::now();
        let before_host = self.cpu_image.cache_snapshot();
        let before_device = self.cache.snapshot();
        let mut host_to_device_bytes = 0_usize;
        let mut device_to_host_bytes = 0_usize;
        let mut launches = 0_u64;
        let mut logical_phases = 0_u64;
        let mut counters = [0_u64; 5];
        let mut cancelled = cancelled_before_launch;
        let distance_bits = if cancelled_before_launch || no_search {
            let mut distances = vec![f64::INFINITY.to_bits(); self.cpu_image.node_count()];
            distances[origin.as_u32() as usize] = 0.0_f64.to_bits();
            distances
        } else {
            self.faults.trip(CudaFaultStage::HostAllocation)?;
            let stream = &self.context.stream;
            let node_count = u32::try_from(self.cpu_image.node_count())
                .map_err(|_| invariant("node count exceeds CUDA ABI"))?;
            let relation_count = u32::try_from(self.cpu_image.relation_kind_count())
                .map_err(|_| invariant("relation count exceeds CUDA ABI"))?;
            let mut initial = vec![f64::INFINITY.to_bits(); self.cpu_image.node_count()];
            initial[origin.as_u32() as usize] = 0.0_f64.to_bits();
            self.faults.trip(CudaFaultStage::DeviceAllocation)?;
            let enabled_device = stream.clone_htod(&enabled).map_err(allocation_error)?;
            let multiplier_device = stream.clone_htod(&multipliers).map_err(allocation_error)?;
            let mut distances = stream.clone_htod(&initial).map_err(allocation_error)?;
            let mut changed = stream.alloc_zeros::<u32>(1).map_err(allocation_error)?;
            let mut status = stream.alloc_zeros::<u32>(1).map_err(allocation_error)?;
            let mut device_counters = stream.alloc_zeros::<u64>(5).map_err(allocation_error)?;
            let mut active_host = vec![0_u32; self.cpu_image.node_count()];
            active_host[origin.as_u32() as usize] = 1;
            let zero_active = vec![0_u32; self.cpu_image.node_count()];
            let mut active_device = stream.clone_htod(&active_host).map_err(allocation_error)?;
            let mut next_active_device =
                stream.clone_htod(&zero_active).map_err(allocation_error)?;
            host_to_device_bytes = enabled
                .len()
                .checked_mul(4)
                .and_then(|value| value.checked_add(multipliers.len().checked_mul(4)?))
                .and_then(|value| value.checked_add(initial.len().checked_mul(8)?))
                .and_then(|value| value.checked_add(initial.len().checked_mul(8)?))
                .ok_or_else(|| invariant("request transfer byte count overflow"))?;
            match algorithm {
                CudaAlgorithm::Frontier => {
                    let phase_limit = self.cpu_image.node_count().max(1);
                    for _ in 0..phase_limit {
                        let partition_ids = self.active_partition_ids(&active_host);
                        if partition_ids.is_empty() {
                            break;
                        }
                        stream
                            .memcpy_htod(&[0_u32], &mut changed)
                            .map_err(synchronization_error)?;
                        stream
                            .memcpy_htod(&zero_active, &mut next_active_device)
                            .map_err(synchronization_error)?;
                        if !self.launch_all_frontier(
                            &partition_ids,
                            cancellation,
                            &enabled_device,
                            &multiplier_device,
                            node_count,
                            relation_count,
                            &active_device,
                            &mut next_active_device,
                            &mut distances,
                            &mut changed,
                            &mut status,
                            &mut device_counters,
                            &mut launches,
                        )? {
                            cancelled = true;
                            break;
                        }
                        logical_phases = logical_phases.saturating_add(1);
                        let next_active = stream
                            .clone_dtoh(&next_active_device)
                            .map_err(synchronization_error)?;
                        device_to_host_bytes = device_to_host_bytes
                            .saturating_add(next_active.len().saturating_mul(4));
                        check_status(stream, &status, &mut device_to_host_bytes)?;
                        if next_active.iter().all(|&active| active == 0) {
                            break;
                        }
                        active_host = next_active;
                        std::mem::swap(&mut active_device, &mut next_active_device);
                    }
                }
                CudaAlgorithm::DeltaStepping(delta) => {
                    let mut bucket = 0_u64;
                    let mut current_distances = initial;
                    loop {
                        loop {
                            let partition_ids = self.bucket_partition_ids(
                                &current_distances,
                                delta.delta(),
                                bucket,
                            )?;
                            if partition_ids.is_empty() {
                                break;
                            }
                            stream
                                .memcpy_htod(&[0_u32], &mut changed)
                                .map_err(synchronization_error)?;
                            if !self.launch_all_delta(
                                &partition_ids,
                                cancellation,
                                &enabled_device,
                                &multiplier_device,
                                node_count,
                                relation_count,
                                delta.delta().to_bits(),
                                bucket,
                                0,
                                &mut distances,
                                &mut changed,
                                &mut status,
                                &mut device_counters,
                                &mut launches,
                            )? {
                                cancelled = true;
                                break;
                            }
                            let changed_host =
                                stream.clone_dtoh(&changed).map_err(synchronization_error)?;
                            current_distances = stream
                                .clone_dtoh(&distances)
                                .map_err(synchronization_error)?;
                            device_to_host_bytes = device_to_host_bytes
                                .saturating_add(4)
                                .saturating_add(current_distances.len().saturating_mul(8));
                            check_status(stream, &status, &mut device_to_host_bytes)?;
                            if changed_host == [0] {
                                break;
                            }
                        }
                        if cancelled {
                            break;
                        }
                        let heavy_partitions =
                            self.bucket_partition_ids(&current_distances, delta.delta(), bucket)?;
                        if !self.launch_all_delta(
                            &heavy_partitions,
                            cancellation,
                            &enabled_device,
                            &multiplier_device,
                            node_count,
                            relation_count,
                            delta.delta().to_bits(),
                            bucket,
                            1,
                            &mut distances,
                            &mut changed,
                            &mut status,
                            &mut device_counters,
                            &mut launches,
                        )? {
                            cancelled = true;
                            break;
                        }
                        check_status(stream, &status, &mut device_to_host_bytes)?;
                        logical_phases = logical_phases.saturating_add(1);
                        current_distances = stream
                            .clone_dtoh(&distances)
                            .map_err(synchronization_error)?;
                        device_to_host_bytes = device_to_host_bytes
                            .saturating_add(current_distances.len().saturating_mul(8));
                        let mut next = None;
                        for &bits in &current_distances {
                            let distance = f64::from_bits(bits);
                            if !distance.is_finite() {
                                continue;
                            }
                            let quotient = distance / delta.delta();
                            if quotient >= u64::MAX as f64 {
                                return Err(invariant("delta bucket index overflow"));
                            }
                            let candidate = quotient as u64;
                            if candidate > bucket {
                                next = Some(next.map_or(candidate, |old: u64| old.min(candidate)));
                            }
                        }
                        let Some(next) = next else { break };
                        bucket = next;
                    }
                }
            }
            let mut finite = stream.alloc_zeros::<u32>(1).map_err(allocation_error)?;
            launch::launch_frontier_compaction(&self.context, &distances, node_count, &mut finite)?;
            launches = launches.saturating_add(1);
            let finite = stream.clone_dtoh(&finite).map_err(synchronization_error)?;
            let output = stream
                .clone_dtoh(&distances)
                .map_err(synchronization_error)?;
            let downloaded = stream
                .clone_dtoh(&device_counters)
                .map_err(synchronization_error)?;
            counters.copy_from_slice(&downloaded);
            counters[3] = logical_phases;
            counters[4] = u64::from(finite[0]);
            device_to_host_bytes = device_to_host_bytes
                .saturating_add(4 + output.len().saturating_mul(8) + counters.len() * 8);
            output
        };
        cancelled |= cancellation.load(Ordering::Acquire);
        let results: Vec<_> = request
            .destinations()
            .iter()
            .zip(mapped)
            .map(|(&destination, dense)| {
                let state = match dense {
                    None => DestinationState::MissingNode,
                    Some(dense) if dense == origin => {
                        DestinationState::Exact(ExactRoute::distance_only(0.0))
                    }
                    Some(_) if cancelled => DestinationState::Incomplete,
                    Some(dense) => {
                        let value = f64::from_bits(distance_bits[dense.as_u32() as usize]);
                        if value == f64::INFINITY {
                            DestinationState::Unreachable
                        } else {
                            DestinationState::Exact(ExactRoute::distance_only(value))
                        }
                    }
                };
                DestinationResult::from_state(destination, state)
            })
            .collect();
        let all_final = results.iter().all(|result| {
            matches!(
                result.state(),
                DestinationState::Exact(_) | DestinationState::MissingNode
            )
        });
        let completion = if cancelled {
            CompletionReason::Cancelled
        } else if all_final {
            CompletionReason::AllDestinationsFinalized
        } else {
            CompletionReason::FrontierExhausted
        };
        let response = RoutingResponse::distance_only(
            request.origin(),
            results,
            packed.canonical().clone(),
            request.tie_policy(),
            (counters[0], counters[4]),
            completion,
        );
        let after_host = self.cpu_image.cache_snapshot();
        let after_device = self.cache.snapshot();
        let transfer_bytes = after_device
            .transfer_bytes
            .saturating_sub(before_device.transfer_bytes);
        host_to_device_bytes = host_to_device_bytes
            .saturating_add(usize::try_from(transfer_bytes).unwrap_or(usize::MAX));
        Ok(CudaRouteOutput {
            response,
            diagnostics: CudaRouteDiagnostics {
                algorithm,
                queue_duration: Duration::ZERO,
                batch_collection_duration: Duration::ZERO,
                batch_width: 1,
                lane_index: 0,
                reserved_search_bytes,
                host_to_device_bytes,
                device_to_host_bytes,
                kernel_launches: launches,
                synchronized_execution_duration: started.elapsed(),
                examined_edges: counters[0],
                relaxation_attempts: counters[1],
                relaxation_updates: counters[2],
                phases: counters[3],
                frontier_high_water: u32::try_from(counters[4]).unwrap_or(u32::MAX),
                partitions_required: after_device.misses.saturating_sub(before_device.misses)
                    + after_device.hits.saturating_sub(before_device.hits),
                host_cache_hits: after_host.hits.saturating_sub(before_host.hits),
                device_cache_hits: after_device.hits.saturating_sub(before_device.hits),
                file_bytes: after_host.read_bytes.saturating_sub(before_host.read_bytes),
                staged_bytes: transfer_bytes,
                transfer_bytes,
            },
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn launch_all_frontier(
        &self,
        partition_ids: &[u32],
        cancellation: &AtomicBool,
        enabled: &CudaSlice<u32>,
        multipliers: &CudaSlice<u32>,
        node_count: u32,
        relation_count: u32,
        active_sources: &CudaSlice<u32>,
        next_active_sources: &mut CudaSlice<u32>,
        distances: &mut CudaSlice<u64>,
        changed: &mut CudaSlice<u32>,
        status: &mut CudaSlice<u32>,
        counters: &mut CudaSlice<u64>,
        launches: &mut u64,
    ) -> Result<bool, CudaError> {
        let mut queued = false;
        for &id in partition_ids {
            if cancellation.load(Ordering::Acquire) {
                if queued {
                    self.synchronize_partition_work()?;
                }
                return Ok(false);
            }
            let partition = match self.cache.acquire(id, cancellation) {
                Ok(Some(partition)) => partition,
                Ok(None) => {
                    if queued {
                        self.synchronize_partition_work()?;
                    }
                    return Ok(false);
                }
                Err(error) => {
                    if queued {
                        self.synchronize_partition_work_after_failure();
                    }
                    return Err(error);
                }
            };
            if cancellation.load(Ordering::Acquire) {
                if queued {
                    self.synchronize_partition_work()?;
                }
                return Ok(false);
            }
            if let Err(error) = self.faults.trip(CudaFaultStage::Launch) {
                if queued {
                    self.synchronize_partition_work_after_failure();
                }
                return Err(error);
            }
            if let Err(error) = launch::launch_partition_frontier(
                &self.context,
                &partition,
                enabled,
                multipliers,
                node_count,
                relation_count,
                active_sources,
                next_active_sources,
                distances,
                changed,
                status,
                counters,
            ) {
                self.synchronize_partition_work_after_failure();
                return Err(error);
            }
            queued = true;
            if let Err(error) = self.faults.trip(CudaFaultStage::Event) {
                self.synchronize_partition_work_after_failure();
                return Err(error);
            }
            if let Err(error) = partition.record_completion() {
                self.synchronize_partition_work_after_failure();
                return Err(error);
            }
            *launches = launches.saturating_add(1);
        }
        if queued {
            if let Err(error) = self.faults.trip(CudaFaultStage::Synchronization) {
                self.synchronize_partition_work_after_failure();
                return Err(error);
            }
            self.synchronize_partition_work()?;
        }
        Ok(true)
    }

    #[allow(clippy::too_many_arguments)]
    fn launch_all_delta(
        &self,
        partition_ids: &[u32],
        cancellation: &AtomicBool,
        enabled: &CudaSlice<u32>,
        multipliers: &CudaSlice<u32>,
        node_count: u32,
        relation_count: u32,
        delta_bits: u64,
        bucket: u64,
        class: u32,
        distances: &mut CudaSlice<u64>,
        changed: &mut CudaSlice<u32>,
        status: &mut CudaSlice<u32>,
        counters: &mut CudaSlice<u64>,
        launches: &mut u64,
    ) -> Result<bool, CudaError> {
        let mut queued = false;
        for &id in partition_ids {
            if cancellation.load(Ordering::Acquire) {
                if queued {
                    self.synchronize_partition_work()?;
                }
                return Ok(false);
            }
            let partition = match self.cache.acquire(id, cancellation) {
                Ok(Some(partition)) => partition,
                Ok(None) => {
                    if queued {
                        self.synchronize_partition_work()?;
                    }
                    return Ok(false);
                }
                Err(error) => {
                    if queued {
                        self.synchronize_partition_work_after_failure();
                    }
                    return Err(error);
                }
            };
            if cancellation.load(Ordering::Acquire) {
                if queued {
                    self.synchronize_partition_work()?;
                }
                return Ok(false);
            }
            if let Err(error) = self.faults.trip(CudaFaultStage::Launch) {
                if queued {
                    self.synchronize_partition_work_after_failure();
                }
                return Err(error);
            }
            if let Err(error) = launch::launch_partition_delta(
                &self.context,
                &partition,
                enabled,
                multipliers,
                node_count,
                relation_count,
                delta_bits,
                bucket,
                class,
                distances,
                changed,
                status,
                counters,
            ) {
                self.synchronize_partition_work_after_failure();
                return Err(error);
            }
            queued = true;
            if let Err(error) = self.faults.trip(CudaFaultStage::Event) {
                self.synchronize_partition_work_after_failure();
                return Err(error);
            }
            if let Err(error) = partition.record_completion() {
                self.synchronize_partition_work_after_failure();
                return Err(error);
            }
            *launches = launches.saturating_add(1);
        }
        if queued {
            if let Err(error) = self.faults.trip(CudaFaultStage::Synchronization) {
                self.synchronize_partition_work_after_failure();
                return Err(error);
            }
            self.synchronize_partition_work()?;
        }
        Ok(true)
    }

    fn synchronize_partition_work(&self) -> Result<(), CudaError> {
        self.context
            .stream
            .synchronize()
            .map_err(synchronization_error)
    }

    fn synchronize_partition_work_after_failure(&self) {
        let _ = self.context.stream.synchronize();
    }

    fn active_partition_ids(&self, active: &[u32]) -> Vec<u32> {
        let mut partitions = std::collections::BTreeSet::new();
        for (source, &is_active) in active.iter().enumerate() {
            if is_active == 0 {
                continue;
            }
            let Ok(source) = u32::try_from(source) else {
                break;
            };
            partitions.extend(self.cpu_image.source_partition_ids(source));
        }
        let mut partitions: Vec<_> = partitions.into_iter().collect();
        if self.config.reverse_partition_order {
            partitions.reverse();
        }
        partitions
    }

    fn bucket_partition_ids(
        &self,
        distance_bits: &[u64],
        delta: f64,
        bucket: u64,
    ) -> Result<Vec<u32>, CudaError> {
        let mut partitions = std::collections::BTreeSet::new();
        for (source, &bits) in distance_bits.iter().enumerate() {
            let distance = f64::from_bits(bits);
            if !distance.is_finite() {
                continue;
            }
            let quotient = distance / delta;
            if quotient >= u64::MAX as f64 {
                return Err(invariant("delta bucket index overflow"));
            }
            if quotient as u64 != bucket {
                continue;
            }
            let source = u32::try_from(source)
                .map_err(|_| invariant("dense source does not fit the CUDA ABI"))?;
            partitions.extend(self.cpu_image.source_partition_ids(source));
        }
        let mut partitions: Vec<_> = partitions.into_iter().collect();
        if self.config.reverse_partition_order {
            partitions.reverse();
        }
        Ok(partitions)
    }
}

fn check_status(
    stream: &Arc<cudarc::driver::CudaStream>,
    status: &CudaSlice<u32>,
    downloaded_bytes: &mut usize,
) -> Result<(), CudaError> {
    let status = stream.clone_dtoh(status).map_err(synchronization_error)?;
    *downloaded_bytes = downloaded_bytes.saturating_add(4);
    if status != [0] {
        return Err(invariant(&format!(
            "partitioned CUDA kernel returned status {}",
            status[0]
        )));
    }
    Ok(())
}

fn invariant(message: &str) -> CudaError {
    CudaError::new(CudaFailureKind::KernelInvariant, message)
}

fn allocation_error(error: cudarc::driver::DriverError) -> CudaError {
    CudaError::new(
        CudaFailureKind::Allocation,
        format!("partitioned CUDA search allocation failed: {error}"),
    )
}

fn synchronization_error(error: cudarc::driver::DriverError) -> CudaError {
    CudaError::new(
        CudaFailureKind::Synchronization,
        format!("partitioned CUDA synchronization failed: {error}"),
    )
}
