use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use cudarc::driver::{CudaSlice, LaunchConfig, PushKernelArg, sys};

use pathhydra_routing::{
    CompletionReason, DestinationResult, DestinationState, ExactRoute, RoutingRequest,
    RoutingResponse, SearchBudget,
};

use crate::topology_cache::DevicePartition;
use crate::{
    CudaAlgorithm, CudaContextOwner, CudaError, CudaFailureKind, CudaResidentImage,
    CudaRouteDiagnostics, CudaRouteOutput,
};

pub(crate) fn driver_version() -> Result<i32, CudaError> {
    let mut version = 0_i32;
    // SAFETY: CUDA initializes before this call through `CudaContext::new`; the
    // pointer is aligned, writable, and lives for the entire synchronous call.
    unsafe { sys::cuDriverGetVersion(&mut version) }
        .result()
        .map_err(driver_error("failed to query CUDA driver version"))?;
    Ok(version)
}

pub(crate) fn stream_priority_range() -> Result<(i32, i32), CudaError> {
    let mut least = 0_i32;
    let mut greatest = 0_i32;
    // SAFETY: both pointers are aligned, initialized writable `i32` values and
    // remain live for the complete synchronous Driver API call.
    unsafe { sys::cuCtxGetStreamPriorityRange(&mut least, &mut greatest) }
        .result()
        .map_err(driver_error("failed to query CUDA stream priority range"))?;
    Ok((least, greatest))
}

pub(crate) fn smoke_arithmetic(
    owner: &CudaContextOwner,
    input: &[f32],
    multipliers: &[f32],
    addends: &[f64],
) -> Result<Vec<f64>, CudaError> {
    if input.len() != multipliers.len() || input.len() != addends.len() {
        return Err(CudaError::new(
            CudaFailureKind::Launch,
            "smoke-kernel input lengths disagree",
        ));
    }
    if input.is_empty() {
        return Ok(Vec::new());
    }
    let length = u32::try_from(input.len()).map_err(|_| {
        CudaError::new(
            CudaFailureKind::Launch,
            "smoke-kernel length does not fit the u32 ABI",
        )
    })?;
    let input_device = owner.stream.clone_htod(input).map_err(allocation_error)?;
    let multiplier_device = owner
        .stream
        .clone_htod(multipliers)
        .map_err(allocation_error)?;
    let addend_device = owner.stream.clone_htod(addends).map_err(allocation_error)?;
    let mut output_device = owner
        .stream
        .alloc_zeros::<f64>(input.len())
        .map_err(allocation_error)?;
    let mut builder = owner.stream.launch_builder(&owner.smoke_function);
    builder.arg(&input_device);
    builder.arg(&multiplier_device);
    builder.arg(&addend_device);
    builder.arg(&mut output_device);
    builder.arg(&length);
    // SAFETY: `pathhydra_smoke_arithmetic` has exactly the five arguments above;
    // every device buffer has `length` aligned initialized elements, output is
    // uniquely borrowed, the launch bounds cover but do not require writes past
    // `length`, all allocations share this stream/context, and the following
    // device-to-host copy synchronizes before any buffer can be dropped.
    unsafe { builder.launch(LaunchConfig::for_num_elems(length)) }.map_err(launch_error)?;
    owner
        .stream
        .clone_dtoh(&output_device)
        .map_err(synchronization_error)
}

pub(crate) fn benchmark_reset_strategy(
    owner: &CudaContextOwner,
    node_count: usize,
    strategy: crate::CudaBenchmarkResetStrategy,
) -> Result<(), CudaError> {
    let count = u32::try_from(node_count).map_err(|_| abi_count("benchmark node"))?;
    if count == 0 {
        return Ok(());
    }
    let generation = 7_u32;
    let mode = u32::from(matches!(
        strategy,
        crate::CudaBenchmarkResetStrategy::GenerationStamp
    ));
    let mut distances = owner
        .stream
        .clone_htod(&vec![0_u64; node_count])
        .map_err(allocation_error)?;
    let mut frontier = owner
        .stream
        .clone_htod(&vec![1_u32; node_count])
        .map_err(allocation_error)?;
    let stamp_count = if matches!(strategy, crate::CudaBenchmarkResetStrategy::ExplicitClear) {
        1
    } else {
        node_count
    };
    let mut stamps = owner
        .stream
        .clone_htod(&vec![0_u32; stamp_count])
        .map_err(allocation_error)?;
    let mut builder = owner.stream.launch_builder(&owner.benchmark_reset_function);
    builder.arg(&mut distances);
    builder.arg(&mut frontier);
    builder.arg(&mut stamps);
    builder.arg(&count);
    builder.arg(&generation);
    builder.arg(&mode);
    // SAFETY: the benchmark reset ABI is exactly the six arguments above; all
    // distance/frontier contain `count` aligned initialized elements. Stamps
    // has `count` elements in generation mode and one sentinel in explicit
    // mode, where the kernel never dereferences it. Buffers are uniquely
    // writable and synchronous downloads keep them alive through completion.
    unsafe { builder.launch(LaunchConfig::for_num_elems(count)) }.map_err(launch_error)?;
    let distances = owner
        .stream
        .clone_dtoh(&distances)
        .map_err(synchronization_error)?;
    let frontier = owner
        .stream
        .clone_dtoh(&frontier)
        .map_err(synchronization_error)?;
    let stamps = owner
        .stream
        .clone_dtoh(&stamps)
        .map_err(synchronization_error)?;
    let stamps_valid = match strategy {
        crate::CudaBenchmarkResetStrategy::ExplicitClear => stamps.iter().all(|&value| value == 0),
        crate::CudaBenchmarkResetStrategy::GenerationStamp => {
            stamps.iter().all(|&value| value == generation)
        }
    };
    if distances
        .iter()
        .any(|&bits| bits != f64::INFINITY.to_bits())
        || frontier.iter().any(|&value| value != 0)
        || !stamps_valid
    {
        return Err(CudaError::new(
            CudaFailureKind::KernelInvariant,
            "benchmark reset strategy produced invalid state",
        ));
    }
    Ok(())
}

pub(crate) fn benchmark_target_strategy(
    owner: &CudaContextOwner,
    node_count: usize,
    destinations: &[u32],
    strategy: crate::CudaBenchmarkTargetStrategy,
) -> Result<(), CudaError> {
    let count = u32::try_from(node_count).map_err(|_| abi_count("benchmark node"))?;
    if count == 0 || destinations.is_empty() {
        return Ok(());
    }
    if destinations.iter().any(|&node| node as usize >= node_count) {
        return Err(CudaError::new(
            CudaFailureKind::Admission,
            "benchmark target is outside the dense node range",
        ));
    }
    let generation = 7_u32;
    let (representation, mode) = match strategy {
        crate::CudaBenchmarkTargetStrategy::SortedSparse => {
            let mut sparse = destinations.to_vec();
            sparse.sort_unstable();
            sparse.dedup();
            (sparse, 0_u32)
        }
        crate::CudaBenchmarkTargetStrategy::DenseBitset => {
            let mut dense = vec![0_u32; node_count];
            for &node in destinations {
                dense[node as usize] = 1;
            }
            (dense, 1_u32)
        }
        crate::CudaBenchmarkTargetStrategy::GenerationDense => {
            let mut dense = vec![0_u32; node_count];
            for &node in destinations {
                dense[node as usize] = generation;
            }
            (dense, 2_u32)
        }
    };
    let representation_len =
        u32::try_from(representation.len()).map_err(|_| abi_count("target representation"))?;
    let representation = owner
        .stream
        .clone_htod(&representation)
        .map_err(allocation_error)?;
    let mut membership = owner
        .stream
        .alloc_zeros::<u32>(node_count)
        .map_err(allocation_error)?;
    let mut builder = owner
        .stream
        .launch_builder(&owner.benchmark_target_function);
    builder.arg(&representation);
    builder.arg(&representation_len);
    builder.arg(&count);
    builder.arg(&generation);
    builder.arg(&mode);
    builder.arg(&mut membership);
    // SAFETY: the target benchmark ABI is exactly the six arguments above;
    // representation length matches its allocation, membership has `count`
    // uniquely writable elements, all target IDs were range checked, and the
    // synchronous download preserves every buffer through completion.
    unsafe { builder.launch(LaunchConfig::for_num_elems(count)) }.map_err(launch_error)?;
    let membership = owner
        .stream
        .clone_dtoh(&membership)
        .map_err(synchronization_error)?;
    for (node, &member) in membership.iter().enumerate() {
        let expected = u32::from(destinations.contains(&(node as u32)));
        if member != expected {
            return Err(CudaError::new(
                CudaFailureKind::KernelInvariant,
                "benchmark target strategy produced invalid membership",
            ));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn benchmark_profile_strategy(
    owner: &CudaContextOwner,
    base_weight_bits: &[u32],
    relation_indexes: &[u32],
    multiplier_bits: &[u32],
    reuse_count: usize,
    maximum_chunk_edges: usize,
    strategy: crate::CudaBenchmarkProfileStrategy,
) -> Result<(), CudaError> {
    if base_weight_bits.len() != relation_indexes.len()
        || multiplier_bits.is_empty()
        || reuse_count == 0
        || maximum_chunk_edges == 0
    {
        return Err(CudaError::new(
            CudaFailureKind::Admission,
            "invalid benchmark profile shape",
        ));
    }
    let relation_count =
        u32::try_from(multiplier_bits.len()).map_err(|_| abi_count("benchmark relation"))?;
    if relation_indexes
        .iter()
        .any(|&index| index >= relation_count)
    {
        return Err(CudaError::new(
            CudaFailureKind::Admission,
            "benchmark profile relation index is out of range",
        ));
    }
    let multiplier_device = owner
        .stream
        .clone_htod(multiplier_bits)
        .map_err(allocation_error)?;
    for chunk_start in (0..base_weight_bits.len()).step_by(maximum_chunk_edges) {
        let chunk_end = base_weight_bits
            .len()
            .min(chunk_start.saturating_add(maximum_chunk_edges));
        let bases = &base_weight_bits[chunk_start..chunk_end];
        let relations = &relation_indexes[chunk_start..chunk_end];
        let edge_count = u32::try_from(bases.len()).map_err(|_| abi_count("benchmark edge"))?;
        let bases_device = owner.stream.clone_htod(bases).map_err(allocation_error)?;
        let relations_device = owner
            .stream
            .clone_htod(relations)
            .map_err(allocation_error)?;
        let mut output = owner
            .stream
            .alloc_zeros::<u64>(bases.len())
            .map_err(allocation_error)?;
        match strategy {
            crate::CudaBenchmarkProfileStrategy::Inline => {
                for _ in 0..reuse_count {
                    let mut builder = owner
                        .stream
                        .launch_builder(&owner.benchmark_profile_inline_function);
                    builder.arg(&bases_device);
                    builder.arg(&relations_device);
                    builder.arg(&multiplier_device);
                    builder.arg(&relation_count);
                    builder.arg(&edge_count);
                    builder.arg(&mut output);
                    // SAFETY: the inline benchmark ABI matches these six
                    // arguments, the edge arrays/output have `edge_count`
                    // elements, relation indexes were checked, and the final
                    // synchronous download covers all repeated launches.
                    unsafe { builder.launch(LaunchConfig::for_num_elems(edge_count)) }
                        .map_err(launch_error)?;
                }
            }
            crate::CudaBenchmarkProfileStrategy::Materialized => {
                let mut materialized = owner
                    .stream
                    .alloc_zeros::<u64>(bases.len())
                    .map_err(allocation_error)?;
                let mut builder = owner
                    .stream
                    .launch_builder(&owner.benchmark_profile_inline_function);
                builder.arg(&bases_device);
                builder.arg(&relations_device);
                builder.arg(&multiplier_device);
                builder.arg(&relation_count);
                builder.arg(&edge_count);
                builder.arg(&mut materialized);
                // SAFETY: the materialization launch has the same checked ABI
                // and bounds as the inline launch; later consume launches and
                // the final download keep materialized storage alive.
                unsafe { builder.launch(LaunchConfig::for_num_elems(edge_count)) }
                    .map_err(launch_error)?;
                for _ in 0..reuse_count {
                    let mut builder = owner
                        .stream
                        .launch_builder(&owner.benchmark_profile_consume_function);
                    builder.arg(&materialized);
                    builder.arg(&edge_count);
                    builder.arg(&mut output);
                    // SAFETY: the consume ABI matches these three arguments;
                    // input/output each have `edge_count` elements and the
                    // synchronous download follows all launches.
                    unsafe { builder.launch(LaunchConfig::for_num_elems(edge_count)) }
                        .map_err(launch_error)?;
                }
            }
        }
        let output = owner
            .stream
            .clone_dtoh(&output)
            .map_err(synchronization_error)?;
        for (index, &actual) in output.iter().enumerate() {
            let expected = f64::from(f32::from_bits(bases[index]))
                * f64::from(f32::from_bits(multiplier_bits[relations[index] as usize]));
            if actual != expected.to_bits() {
                return Err(CudaError::new(
                    CudaFailureKind::KernelInvariant,
                    "benchmark profile strategy produced invalid effective weight",
                ));
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn validate_topology(
    owner: &CudaContextOwner,
    offsets: &CudaSlice<u64>,
    destinations: &CudaSlice<u32>,
    relation_indexes: &CudaSlice<u32>,
    base_weight_bits: &CudaSlice<u32>,
    node_count: usize,
    relation_count: usize,
    adjacency_count: usize,
    expected_destination_xor: u64,
    expected_relation_weight_xor: u64,
) -> Result<(), CudaError> {
    let node_count = u32::try_from(node_count).map_err(|_| abi_count("node"))?;
    let relation_count = u32::try_from(relation_count).map_err(|_| abi_count("relation"))?;
    let adjacency_count = u64::try_from(adjacency_count).map_err(|_| abi_count("adjacency"))?;
    let mut status = owner
        .stream
        .alloc_zeros::<u32>(1)
        .map_err(allocation_error)?;
    let mut checksum = owner
        .stream
        .alloc_zeros::<u64>(4)
        .map_err(allocation_error)?;
    let mut builder = owner
        .stream
        .launch_builder(&owner.validate_topology_function);
    builder.arg(offsets);
    builder.arg(destinations);
    builder.arg(relation_indexes);
    builder.arg(base_weight_bits);
    builder.arg(&node_count);
    builder.arg(&relation_count);
    builder.arg(&adjacency_count);
    builder.arg(&mut status);
    builder.arg(&mut checksum);
    // SAFETY: the validation entry has exactly these nine ordered parameters;
    // immutable device topology buffers have their declared logical lengths,
    // zero-adjacency sentinels are never dereferenced, scalar counts are checked,
    // status/checksum are initialized and uniquely writable, all allocations
    // share this context/stream, and the synchronous downloads below preserve
    // every handle and allocation through completion.
    unsafe {
        builder.launch(LaunchConfig {
            grid_dim: (1, 1, 1),
            block_dim: (1, 1, 1),
            shared_mem_bytes: 0,
        })
    }
    .map_err(launch_error)?;
    let status = owner
        .stream
        .clone_dtoh(&status)
        .map_err(synchronization_error)?;
    let checksum = owner
        .stream
        .clone_dtoh(&checksum)
        .map_err(synchronization_error)?;
    let expected = [
        u64::from(node_count),
        adjacency_count,
        expected_destination_xor,
        expected_relation_weight_xor,
    ];
    if status != [0] || checksum != expected {
        return Err(CudaError::new(
            CudaFailureKind::Upload,
            format!(
                "device topology validation disagreed: status {status:?}, checksum {checksum:?}, expected {expected:?}"
            ),
        ));
    }
    Ok(())
}

pub(crate) fn route(
    resident: &CudaResidentImage,
    request: &RoutingRequest,
    algorithm: CudaAlgorithm,
    cancellation: &AtomicBool,
    reserved_search_bytes: usize,
) -> Result<CudaRouteOutput, CudaError> {
    let route_started = Instant::now();
    if request.budget() != SearchBudget::Unlimited {
        return Err(CudaError::new(
            CudaFailureKind::Admission,
            "finite examined-edge budgets are unsupported by CUDA",
        ));
    }
    let image = resident.cpu_image();
    let cpu_evidence_working_bytes = if request.return_paths() {
        Some(
            pathhydra_routing::estimate_cpu_working_set(image, request)
                .map_err(|_| {
                    CudaError::new(
                        CudaFailureKind::Admission,
                        "CPU path-evidence working bytes could not be estimated",
                    )
                })?
                .bytes(),
        )
    } else {
        None
    };
    let required_search_bytes = crate::estimate_search_bytes_for_request(
        image.node_count(),
        image.relation_kind_count(),
        image.adjacency_count(),
        request.destinations().len(),
        algorithm,
        cpu_evidence_working_bytes,
    )?;
    if reserved_search_bytes < required_search_bytes {
        return Err(CudaError::new(
            CudaFailureKind::Admission,
            "the caller-provided CUDA search reservation is smaller than the request estimate",
        ));
    }
    let origin = image.dense_node_id(request.origin()).ok_or_else(|| {
        CudaError::new(
            CudaFailureKind::KernelInvariant,
            format!(
                "origin node {} is absent from the acquired image",
                request.origin()
            ),
        )
    })?;
    let packed = request.profile().pack(image).map_err(|error| {
        CudaError::new(
            CudaFailureKind::KernelInvariant,
            format!("relation profile is invalid for the acquired image: {error}"),
        )
    })?;
    let (enabled, multiplier_bits) = packed.accelerator_arrays();
    let mapped_destinations: Vec<_> = request
        .destinations()
        .iter()
        .map(|&destination| image.dense_node_id(destination))
        .collect();
    let mut target_set: Vec<_> = mapped_destinations
        .iter()
        .filter_map(|dense| dense.map(|value| value.as_u32()))
        .collect();
    target_set.sort_unstable();
    target_set.dedup();

    let cancelled_before_launch = cancellation.load(Ordering::Acquire);
    let no_search_needed = image.adjacency_count() == 0
        || target_set
            .iter()
            .all(|&destination| destination == origin.as_u32());
    let started = Instant::now();
    let mut host_to_device_bytes = 0_usize;
    let mut device_to_host_bytes = 0_usize;
    let mut launches = 0_u64;
    let mut counters = [0_u64; 5];
    let mut atomic_cas_retries = 0_u64;
    let mut state_initialization_duration = Duration::ZERO;
    let mut relation_relaxation_duration = Duration::ZERO;
    let mut response_transfer_duration = Duration::ZERO;
    let mut frontier_compaction_duration = Duration::ZERO;
    let mut compacted_task_count = 0_u64;
    let distance_bits = if cancelled_before_launch || no_search_needed {
        let mut bits = vec![f64::INFINITY.to_bits(); image.node_count()];
        bits[origin.as_u32() as usize] = 0.0_f64.to_bits();
        bits
    } else {
        let initialization_started = Instant::now();
        let stream = &resident.context.stream;
        let node_count = u32::try_from(image.node_count()).map_err(|_| abi_count("node"))?;
        let relation_count =
            u32::try_from(image.relation_kind_count()).map_err(|_| abi_count("relation"))?;
        let adjacency_count =
            u64::try_from(image.adjacency_count()).map_err(|_| abi_count("adjacency"))?;
        let origin_u32 = origin.as_u32();
        let mut initial_distances = vec![f64::INFINITY.to_bits(); image.node_count()];
        initial_distances[origin.as_u32() as usize] = 0.0_f64.to_bits();
        let enabled_device = stream.clone_htod(&enabled).map_err(allocation_error)?;
        let multiplier_device = stream
            .clone_htod(&multiplier_bits)
            .map_err(allocation_error)?;
        let mut distance_device = stream
            .clone_htod(&initial_distances)
            .map_err(allocation_error)?;
        let mut status_device = stream.alloc_zeros::<u32>(1).map_err(allocation_error)?;
        let mut counters_device = stream.alloc_zeros::<u64>(5).map_err(allocation_error)?;
        host_to_device_bytes =
            enabled.len() * 4 + multiplier_bits.len() * 4 + image.node_count() * 8;
        state_initialization_duration = initialization_started.elapsed();
        let relaxation_started = Instant::now();
        let execution = match algorithm {
            CudaAlgorithm::Frontier => run_resident_frontier(
                resident,
                image.arrays().offsets,
                &enabled_device,
                &multiplier_device,
                node_count,
                relation_count,
                adjacency_count,
                origin_u32,
                &mut distance_device,
                &mut status_device,
                &mut counters_device,
                cancellation,
            )?,
            CudaAlgorithm::DeltaStepping(delta) => run_resident_delta(
                resident,
                image.arrays().offsets,
                &enabled_device,
                &multiplier_device,
                node_count,
                relation_count,
                adjacency_count,
                origin_u32,
                delta.delta(),
                &mut distance_device,
                &mut status_device,
                &mut counters_device,
                cancellation,
            )?,
        };
        launches = execution.launches;
        host_to_device_bytes = host_to_device_bytes
            .saturating_add(execution.task_upload_bytes)
            .saturating_add(execution.control_upload_bytes);
        device_to_host_bytes =
            device_to_host_bytes.saturating_add(execution.control_download_bytes);
        counters[3] = execution.phases;
        compacted_task_count = execution.compacted_tasks;
        frontier_compaction_duration = execution.compaction_duration;
        relation_relaxation_duration = relaxation_started
            .elapsed()
            .saturating_sub(frontier_compaction_duration);
        let transfer_started = Instant::now();
        let distances = stream
            .clone_dtoh(&distance_device)
            .map_err(synchronization_error)?;
        let status = stream
            .clone_dtoh(&status_device)
            .map_err(synchronization_error)?;
        let downloaded_counters = stream
            .clone_dtoh(&counters_device)
            .map_err(synchronization_error)?;
        counters.copy_from_slice(&downloaded_counters);
        counters[3] = execution.phases;
        atomic_cas_retries = counters[4];
        response_transfer_duration = transfer_started.elapsed();
        device_to_host_bytes = device_to_host_bytes
            .saturating_add(distances.len().saturating_mul(8))
            .saturating_add(4)
            .saturating_add(counters.len().saturating_mul(8));
        if status != [0] {
            return Err(CudaError::new(
                CudaFailureKind::KernelInvariant,
                format!("CUDA routing kernel returned status {}", status[0]),
            ));
        }
        distances
    };
    let cancelled = cancelled_before_launch || cancellation.load(Ordering::Acquire);
    let finalized_nodes = if cancelled {
        0
    } else {
        distance_bits
            .iter()
            .filter(|&&bits| f64::from_bits(bits).is_finite())
            .count() as u64
    };
    let has_present_destination = mapped_destinations.iter().any(Option::is_some);
    let first_destination_duration =
        (!cancelled && has_present_destination).then(|| route_started.elapsed());
    let destination_started = Instant::now();
    let results: Vec<DestinationResult> = request
        .destinations()
        .iter()
        .zip(mapped_destinations)
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
    let all_destinations_final = results.iter().all(|result| {
        matches!(
            result.state(),
            DestinationState::Exact(_) | DestinationState::MissingNode
        )
    });
    let completion_reason = if cancelled {
        CompletionReason::Cancelled
    } else if all_destinations_final {
        CompletionReason::AllDestinationsFinalized
    } else {
        CompletionReason::FrontierExhausted
    };
    let destination_completion_duration = destination_started.elapsed();
    let mut response = RoutingResponse::distance_only(
        request.origin(),
        results,
        packed.canonical().clone(),
        request.tie_policy(),
        (counters[0], finalized_nodes),
        completion_reason,
    );
    let mut path_reconstruction_duration = Duration::ZERO;
    let mut first_destination_duration = if request.return_paths() {
        None
    } else {
        first_destination_duration
    };
    if request.return_paths() {
        let faults = resident.fault_injection();
        if !cancelled {
            faults.trip(crate::CudaFaultStage::PathEvidence)?;
            faults.pause_path_evidence()?;
        }
        let (evidence, evidence_diagnostics) =
            pathhydra_routing::route_controlled(image, request, cancellation).map_err(|error| {
                CudaError::new(
                    CudaFailureKind::KernelInvariant,
                    format!("same-image CPU path evidence pass failed: {error}"),
                )
            })?;
        path_reconstruction_duration = evidence_diagnostics.path_reconstruction_duration;
        if evidence.completion_reason() != CompletionReason::Cancelled {
            verify_path_evidence(&response, &evidence)?;
            // A requested path is not complete until its stable predecessor
            // evidence has been reconstructed and verified.
            first_destination_duration = has_present_destination.then(|| route_started.elapsed());
        }
        response = evidence;
    }
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
            partitions_required: 0,
            host_cache_hits: 0,
            device_cache_hits: 0,
            file_bytes: 0,
            staged_bytes: 0,
            transfer_bytes: 0,
            parallel_strategy: crate::CudaParallelStrategy::RelationThreadsAtomicBinary64,
            reset_mode: crate::CudaResetMode::ExplicitClear,
            target_mode: crate::CudaTargetMode::SortedSparseHost,
            profile_mode: crate::CudaProfileMode::InlineExact,
            path_evidence_mode: if request.return_paths() {
                crate::CudaPathEvidenceMode::CpuPassSameImage
            } else {
                crate::CudaPathEvidenceMode::NotRequested
            },
            state_initialization_duration,
            partition_scheduling_duration: Duration::ZERO,
            relation_relaxation_duration,
            response_transfer_duration,
            frontier_compaction_duration,
            compacted_task_count: u32::try_from(compacted_task_count).unwrap_or(u32::MAX),
            destination_completion_duration,
            destination_count_checked: request.destinations().len(),
            atomic_cas_retries,
            first_destination_duration,
            path_reconstruction_duration,
        },
    })
}

#[derive(Default)]
struct PhaseExecution {
    launches: u64,
    phases: u64,
    compaction_duration: Duration,
    compacted_tasks: u64,
    task_upload_bytes: usize,
    control_upload_bytes: usize,
    control_download_bytes: usize,
}

type ResidentTasks = (Vec<u64>, Vec<u32>);

fn compact_resident_tasks(
    offsets: &[u64],
    selected_sources: impl Fn(usize) -> bool,
    cancellation: &AtomicBool,
) -> Result<Option<ResidentTasks>, CudaError> {
    let mut selected_edge_count = 0_usize;
    for source in 0..offsets.len().saturating_sub(1) {
        if cancellation.load(Ordering::Acquire) {
            return Ok(None);
        }
        if selected_sources(source) {
            let count = usize::try_from(offsets[source + 1].saturating_sub(offsets[source]))
                .map_err(|_| host_compaction_allocation_error())?;
            selected_edge_count = selected_edge_count
                .checked_add(count)
                .ok_or_else(host_compaction_allocation_error)?;
        }
    }
    let mut edges = Vec::new();
    let mut sources = Vec::new();
    edges
        .try_reserve_exact(selected_edge_count)
        .map_err(|_| host_compaction_allocation_error())?;
    sources
        .try_reserve_exact(selected_edge_count)
        .map_err(|_| host_compaction_allocation_error())?;
    for source in 0..offsets.len().saturating_sub(1) {
        if cancellation.load(Ordering::Acquire) {
            return Ok(None);
        }
        if !selected_sources(source) {
            continue;
        }
        for edge in offsets[source]..offsets[source + 1] {
            if edge & 1023 == 0 && cancellation.load(Ordering::Acquire) {
                return Ok(None);
            }
            edges.push(edge);
            sources.push(source as u32);
        }
    }
    Ok(Some((edges, sources)))
}

fn host_compaction_allocation_error() -> CudaError {
    CudaError::new(
        CudaFailureKind::Allocation,
        "CUDA host task-compaction allocation failed",
    )
}

fn abi_count(structure: &'static str) -> CudaError {
    CudaError::new(
        CudaFailureKind::Admission,
        format!("CUDA {structure} count does not fit the kernel ABI"),
    )
}

pub(crate) fn verify_path_evidence(
    distances: &RoutingResponse,
    evidence: &RoutingResponse,
) -> Result<(), CudaError> {
    if distances.results().len() != evidence.results().len() {
        return Err(CudaError::new(
            CudaFailureKind::KernelInvariant,
            "CPU path evidence changed destination cardinality",
        ));
    }
    for (distance, path) in distances.results().iter().zip(evidence.results()) {
        let agrees = match (distance.state(), path.state()) {
            (DestinationState::Exact(left), DestinationState::Exact(right)) => {
                left.logical_distance().to_bits() == right.logical_distance().to_bits()
                    && right.path().is_some()
            }
            (left, right) => left == right,
        };
        if !agrees {
            return Err(CudaError::new(
                CudaFailureKind::KernelInvariant,
                "CPU path evidence disagreed with exact CUDA distance state",
            ));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_resident_frontier(
    resident: &CudaResidentImage,
    offsets: &[u64],
    enabled: &CudaSlice<u32>,
    multipliers: &CudaSlice<u32>,
    node_count: u32,
    relation_count: u32,
    adjacency_count: u64,
    origin: u32,
    distances: &mut CudaSlice<u64>,
    status: &mut CudaSlice<u32>,
    counters: &mut CudaSlice<u64>,
    cancellation: &AtomicBool,
) -> Result<PhaseExecution, CudaError> {
    let stream = &resident.context.stream;
    let mut active_host = vec![0_u32; node_count as usize];
    active_host[origin as usize] = 1;
    let mut next = stream
        .alloc_zeros::<u32>(node_count as usize)
        .map_err(allocation_error)?;
    let mut changed = stream.alloc_zeros::<u32>(1).map_err(allocation_error)?;
    let mut launches = 0_u64;
    let mut phases = 0_u64;
    let mut compaction_duration = Duration::ZERO;
    let mut compacted_tasks = 0_u64;
    let mut control_upload_bytes = 0_usize;
    let mut control_download_bytes = 0_usize;
    for _ in 0..node_count.max(1) {
        if cancellation.load(Ordering::Acquire) {
            break;
        }
        stream
            .memcpy_htod(&vec![0_u32; node_count as usize], &mut next)
            .map_err(synchronization_error)?;
        stream
            .memcpy_htod(&[0_u32], &mut changed)
            .map_err(synchronization_error)?;
        control_upload_bytes = control_upload_bytes
            .saturating_add(node_count as usize * 4)
            .saturating_add(4);
        resident
            .fault_injection()
            .trip(crate::CudaFaultStage::TaskCompaction)?;
        if !resident
            .fault_injection()
            .pause_task_compaction(cancellation)?
        {
            break;
        }
        let compact_started = Instant::now();
        let Some((task_edges, task_sources)) =
            compact_resident_tasks(offsets, |source| active_host[source] == 1, cancellation)?
        else {
            break;
        };
        compaction_duration = compaction_duration.saturating_add(compact_started.elapsed());
        compacted_tasks = compacted_tasks.saturating_add(task_edges.len() as u64);
        if task_edges.is_empty() {
            break;
        }
        let (phase_launches, task_upload_duration) = launch_resident_frontier_chunks(
            resident,
            enabled,
            multipliers,
            node_count,
            relation_count,
            adjacency_count,
            &task_edges,
            &task_sources,
            &mut next,
            distances,
            &mut changed,
            status,
            counters,
        )?;
        launches = launches.saturating_add(phase_launches);
        compaction_duration = compaction_duration.saturating_add(task_upload_duration);
        phases = phases.saturating_add(1);
        let changed_host = stream.clone_dtoh(&changed).map_err(synchronization_error)?;
        let next_host = stream.clone_dtoh(&next).map_err(synchronization_error)?;
        control_download_bytes = control_download_bytes
            .saturating_add(4)
            .saturating_add(next_host.len().saturating_mul(4))
            .saturating_add(4);
        check_kernel_status(stream, status)?;
        if changed_host == [0] {
            break;
        }
        active_host = next_host;
    }
    Ok(PhaseExecution {
        launches,
        phases,
        compaction_duration,
        compacted_tasks,
        task_upload_bytes: usize::try_from(compacted_tasks)
            .unwrap_or(usize::MAX)
            .saturating_mul(12),
        control_upload_bytes,
        control_download_bytes,
    })
}

#[allow(clippy::too_many_arguments)]
fn launch_resident_frontier_chunks(
    resident: &CudaResidentImage,
    enabled: &CudaSlice<u32>,
    multipliers: &CudaSlice<u32>,
    node_count: u32,
    relation_count: u32,
    adjacency_count: u64,
    task_edges: &[u64],
    task_sources: &[u32],
    next: &mut CudaSlice<u32>,
    distances: &mut CudaSlice<u64>,
    changed: &mut CudaSlice<u32>,
    status: &mut CudaSlice<u32>,
    counters: &mut CudaSlice<u64>,
) -> Result<(u64, Duration), CudaError> {
    let mut launches = 0_u64;
    let mut upload_duration = Duration::ZERO;
    for (edge_chunk, source_chunk) in task_edges
        .chunks(u32::MAX as usize)
        .zip(task_sources.chunks(u32::MAX as usize))
    {
        let upload_started = Instant::now();
        let edge_tasks = resident
            .context
            .stream
            .clone_htod(edge_chunk)
            .map_err(allocation_error)?;
        let source_tasks = resident
            .context
            .stream
            .clone_htod(source_chunk)
            .map_err(allocation_error)?;
        upload_duration = upload_duration.saturating_add(upload_started.elapsed());
        let count = edge_chunk.len() as u32;
        let mut builder = resident
            .context
            .stream
            .launch_builder(&resident.context.frontier_function);
        builder.arg(&edge_tasks);
        builder.arg(&source_tasks);
        builder.arg(&count);
        builder.arg(&resident.destinations);
        builder.arg(&resident.relation_indexes);
        builder.arg(&resident.base_weight_bits);
        builder.arg(&node_count);
        builder.arg(&adjacency_count);
        builder.arg(enabled);
        builder.arg(multipliers);
        builder.arg(&relation_count);
        builder.arg(&mut *next);
        builder.arg(&mut *distances);
        builder.arg(&mut *changed);
        builder.arg(&mut *status);
        builder.arg(&mut *counters);
        // SAFETY: the parallel frontier ABI exactly matches these arguments.
        // Immutable topology spans adjacency_count entries; the chunk bounds
        // every thread. Mutable words are initialized atomic storage retained
        // through the synchronous status download after all chunk launches.
        unsafe { builder.launch(LaunchConfig::for_num_elems(count)) }.map_err(launch_error)?;
        resident
            .context
            .stream
            .synchronize()
            .map_err(synchronization_error)?;
        launches = launches.saturating_add(1);
    }
    Ok((launches, upload_duration))
}

#[allow(clippy::too_many_arguments)]
fn run_resident_delta(
    resident: &CudaResidentImage,
    offsets: &[u64],
    enabled: &CudaSlice<u32>,
    multipliers: &CudaSlice<u32>,
    node_count: u32,
    relation_count: u32,
    adjacency_count: u64,
    origin: u32,
    delta: f64,
    distances: &mut CudaSlice<u64>,
    status: &mut CudaSlice<u32>,
    counters: &mut CudaSlice<u64>,
    cancellation: &AtomicBool,
) -> Result<PhaseExecution, CudaError> {
    let stream = &resident.context.stream;
    let mut removed = stream
        .alloc_zeros::<u32>(node_count as usize)
        .map_err(allocation_error)?;
    let mut changed = stream.alloc_zeros::<u32>(1).map_err(allocation_error)?;
    let mut bucket = 0_u64;
    let mut launches = 0_u64;
    let mut phases = 0_u64;
    let mut compaction_duration = Duration::ZERO;
    let mut compacted_tasks = 0_u64;
    let mut control_upload_bytes = 0_usize;
    let mut control_download_bytes = 0_usize;
    let mut current = vec![f64::INFINITY.to_bits(); node_count as usize];
    current[origin as usize] = 0.0_f64.to_bits();
    'search: loop {
        if cancellation.load(Ordering::Acquire) {
            break;
        }
        stream
            .memcpy_htod(&vec![0_u32; node_count as usize], &mut removed)
            .map_err(synchronization_error)?;
        control_upload_bytes = control_upload_bytes.saturating_add(node_count as usize * 4);
        loop {
            stream
                .memcpy_htod(&[0_u32], &mut changed)
                .map_err(synchronization_error)?;
            control_upload_bytes = control_upload_bytes.saturating_add(4);
            resident
                .fault_injection()
                .trip(crate::CudaFaultStage::TaskCompaction)?;
            if !resident
                .fault_injection()
                .pause_task_compaction(cancellation)?
            {
                break 'search;
            }
            let compact_started = Instant::now();
            let Some((task_edges, task_sources)) = compact_resident_tasks(
                offsets,
                |source| bucket_index(f64::from_bits(current[source]), delta) == Some(bucket),
                cancellation,
            )?
            else {
                break 'search;
            };
            compaction_duration = compaction_duration.saturating_add(compact_started.elapsed());
            compacted_tasks = compacted_tasks.saturating_add(task_edges.len() as u64);
            if task_edges.is_empty() {
                break;
            }
            let (phase_launches, task_upload_duration) = launch_resident_delta_chunks(
                resident,
                enabled,
                multipliers,
                node_count,
                relation_count,
                adjacency_count,
                delta.to_bits(),
                bucket,
                0,
                &task_edges,
                &task_sources,
                &mut removed,
                distances,
                &mut changed,
                status,
                counters,
            )?;
            launches = launches.saturating_add(phase_launches);
            compaction_duration = compaction_duration.saturating_add(task_upload_duration);
            let changed_host = stream.clone_dtoh(&changed).map_err(synchronization_error)?;
            current = stream
                .clone_dtoh(distances)
                .map_err(synchronization_error)?;
            control_download_bytes = control_download_bytes
                .saturating_add(4)
                .saturating_add(current.len().saturating_mul(8))
                .saturating_add(4);
            check_kernel_status(stream, status)?;
            if changed_host == [0] {
                break;
            }
        }
        let removed_host = stream.clone_dtoh(&removed).map_err(synchronization_error)?;
        control_download_bytes =
            control_download_bytes.saturating_add(removed_host.len().saturating_mul(4));
        resident
            .fault_injection()
            .trip(crate::CudaFaultStage::TaskCompaction)?;
        if !resident
            .fault_injection()
            .pause_task_compaction(cancellation)?
        {
            break;
        }
        let compact_started = Instant::now();
        let Some((task_edges, task_sources)) =
            compact_resident_tasks(offsets, |source| removed_host[source] == 1, cancellation)?
        else {
            break;
        };
        compaction_duration = compaction_duration.saturating_add(compact_started.elapsed());
        compacted_tasks = compacted_tasks.saturating_add(task_edges.len() as u64);
        let (phase_launches, task_upload_duration) = launch_resident_delta_chunks(
            resident,
            enabled,
            multipliers,
            node_count,
            relation_count,
            adjacency_count,
            delta.to_bits(),
            bucket,
            1,
            &task_edges,
            &task_sources,
            &mut removed,
            distances,
            &mut changed,
            status,
            counters,
        )?;
        launches = launches.saturating_add(phase_launches);
        compaction_duration = compaction_duration.saturating_add(task_upload_duration);
        check_kernel_status(stream, status)?;
        phases = phases.saturating_add(1);
        current = stream
            .clone_dtoh(distances)
            .map_err(synchronization_error)?;
        control_download_bytes = control_download_bytes
            .saturating_add(4)
            .saturating_add(current.len().saturating_mul(8));
        let Some(next) = next_bucket(&current, delta, bucket)? else {
            break;
        };
        bucket = next;
    }
    Ok(PhaseExecution {
        launches,
        phases,
        compaction_duration,
        compacted_tasks,
        task_upload_bytes: usize::try_from(compacted_tasks)
            .unwrap_or(usize::MAX)
            .saturating_mul(12),
        control_upload_bytes,
        control_download_bytes,
    })
}

#[allow(clippy::too_many_arguments)]
fn launch_resident_delta_chunks(
    resident: &CudaResidentImage,
    enabled: &CudaSlice<u32>,
    multipliers: &CudaSlice<u32>,
    node_count: u32,
    relation_count: u32,
    adjacency_count: u64,
    delta_bits: u64,
    bucket: u64,
    edge_class: u32,
    task_edges: &[u64],
    task_sources: &[u32],
    removed: &mut CudaSlice<u32>,
    distances: &mut CudaSlice<u64>,
    changed: &mut CudaSlice<u32>,
    status: &mut CudaSlice<u32>,
    counters: &mut CudaSlice<u64>,
) -> Result<(u64, Duration), CudaError> {
    let mut launches = 0_u64;
    let mut upload_duration = Duration::ZERO;
    for (edge_chunk, source_chunk) in task_edges
        .chunks(u32::MAX as usize)
        .zip(task_sources.chunks(u32::MAX as usize))
    {
        let upload_started = Instant::now();
        let edge_tasks = resident
            .context
            .stream
            .clone_htod(edge_chunk)
            .map_err(allocation_error)?;
        let source_tasks = resident
            .context
            .stream
            .clone_htod(source_chunk)
            .map_err(allocation_error)?;
        upload_duration = upload_duration.saturating_add(upload_started.elapsed());
        let count = edge_chunk.len() as u32;
        let mut builder = resident
            .context
            .stream
            .launch_builder(&resident.context.delta_function);
        builder.arg(&edge_tasks);
        builder.arg(&source_tasks);
        builder.arg(&count);
        builder.arg(&resident.destinations);
        builder.arg(&resident.relation_indexes);
        builder.arg(&resident.base_weight_bits);
        builder.arg(&node_count);
        builder.arg(&adjacency_count);
        builder.arg(enabled);
        builder.arg(multipliers);
        builder.arg(&relation_count);
        builder.arg(&delta_bits);
        builder.arg(&bucket);
        builder.arg(&edge_class);
        builder.arg(&mut *removed);
        builder.arg(&mut *distances);
        builder.arg(&mut *changed);
        builder.arg(&mut *status);
        builder.arg(&mut *counters);
        // SAFETY: the parallel delta ABI has these exact fixed-width values and
        // buffers. Each chunk covers disjoint immutable relation inputs while
        // distance, removed, status, and counters use device atomics.
        unsafe { builder.launch(LaunchConfig::for_num_elems(count)) }.map_err(launch_error)?;
        resident
            .context
            .stream
            .synchronize()
            .map_err(synchronization_error)?;
        launches = launches.saturating_add(1);
    }
    Ok((launches, upload_duration))
}

fn next_bucket(distances: &[u64], delta: f64, current: u64) -> Result<Option<u64>, CudaError> {
    let mut next = None;
    for &bits in distances {
        let distance = f64::from_bits(bits);
        if !distance.is_finite() {
            continue;
        }
        let quotient = distance / delta;
        if !quotient.is_finite() || quotient < 0.0 || quotient >= u64::MAX as f64 {
            return Err(CudaError::new(
                CudaFailureKind::KernelInvariant,
                "delta bucket index overflow",
            ));
        }
        let candidate = quotient as u64;
        if candidate > current {
            next = Some(next.map_or(candidate, |old: u64| old.min(candidate)));
        }
    }
    Ok(next)
}

pub(crate) fn bucket_index(distance: f64, delta: f64) -> Option<u64> {
    let quotient = distance / delta;
    (quotient.is_finite() && quotient >= 0.0 && quotient < u64::MAX as f64)
        .then_some(quotient as u64)
}

fn check_kernel_status(
    stream: &std::sync::Arc<cudarc::driver::CudaStream>,
    status: &CudaSlice<u32>,
) -> Result<(), CudaError> {
    let status = stream.clone_dtoh(status).map_err(synchronization_error)?;
    if status == [0] {
        Ok(())
    } else {
        Err(CudaError::new(
            CudaFailureKind::KernelInvariant,
            format!("CUDA routing kernel returned status {}", status[0]),
        ))
    }
}

fn driver_error(context: &'static str) -> impl FnOnce(cudarc::driver::DriverError) -> CudaError {
    move |error| CudaError::new(CudaFailureKind::Context, format!("{context}: {error}"))
}

fn allocation_error(error: cudarc::driver::DriverError) -> CudaError {
    CudaError::new(
        CudaFailureKind::Allocation,
        format!("CUDA allocation or upload failed: {error}"),
    )
}

fn launch_error(error: cudarc::driver::DriverError) -> CudaError {
    CudaError::new(
        CudaFailureKind::Launch,
        format!("CUDA kernel launch failed: {error}"),
    )
}

fn synchronization_error(error: cudarc::driver::DriverError) -> CudaError {
    CudaError::new(
        CudaFailureKind::Synchronization,
        format!("CUDA synchronization or download failed: {error}"),
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn launch_partition_frontier(
    owner: &CudaContextOwner,
    partition: &DevicePartition,
    enabled: &CudaSlice<u32>,
    multipliers: &CudaSlice<u32>,
    node_count: u32,
    relation_count: u32,
    task_edges: &CudaSlice<u32>,
    task_sources: &CudaSlice<u32>,
    task_count: u32,
    next_active_sources: &mut CudaSlice<u32>,
    distances: &mut CudaSlice<u64>,
    changed: &mut CudaSlice<u32>,
    status: &mut CudaSlice<u32>,
    counters: &mut CudaSlice<u64>,
) -> Result<(), CudaError> {
    let mut builder = owner
        .stream
        .launch_builder(&owner.partition_frontier_function);
    builder.arg(task_edges);
    builder.arg(task_sources);
    builder.arg(&task_count);
    builder.arg(&partition.destinations);
    builder.arg(&partition.relation_indexes);
    builder.arg(&partition.base_weight_bits);
    builder.arg(&partition.edge_count);
    builder.arg(&node_count);
    builder.arg(enabled);
    builder.arg(multipliers);
    builder.arg(&relation_count);
    builder.arg(next_active_sources);
    builder.arg(distances);
    builder.arg(changed);
    builder.arg(status);
    builder.arg(counters);
    // SAFETY: the partition cache uploads and validates the three immutable
    // topology arrays together. The compact edge/source arrays have identical
    // task_count lengths, and one thread is launched per task. Search and task
    // buffers remain alive until stream synchronization and event-gated
    // cache-pin release.
    unsafe { builder.launch(LaunchConfig::for_num_elems(task_count)) }
        .map(|_| ())
        .map_err(launch_error)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn launch_partition_delta(
    owner: &CudaContextOwner,
    partition: &DevicePartition,
    enabled: &CudaSlice<u32>,
    multipliers: &CudaSlice<u32>,
    node_count: u32,
    relation_count: u32,
    delta_bits: u64,
    bucket: u64,
    edge_class: u32,
    task_edges: &CudaSlice<u32>,
    task_sources: &CudaSlice<u32>,
    task_count: u32,
    removed_sources: &mut CudaSlice<u32>,
    distances: &mut CudaSlice<u64>,
    changed: &mut CudaSlice<u32>,
    status: &mut CudaSlice<u32>,
    counters: &mut CudaSlice<u64>,
) -> Result<(), CudaError> {
    let mut builder = owner.stream.launch_builder(&owner.partition_delta_function);
    builder.arg(task_edges);
    builder.arg(task_sources);
    builder.arg(&task_count);
    builder.arg(&partition.destinations);
    builder.arg(&partition.relation_indexes);
    builder.arg(&partition.base_weight_bits);
    builder.arg(&partition.edge_count);
    builder.arg(&node_count);
    builder.arg(enabled);
    builder.arg(multipliers);
    builder.arg(&relation_count);
    builder.arg(&delta_bits);
    builder.arg(&bucket);
    builder.arg(&edge_class);
    builder.arg(removed_sources);
    builder.arg(distances);
    builder.arg(changed);
    builder.arg(status);
    builder.arg(counters);
    // SAFETY: the frontier partition obligations apply, and validated host
    // configuration supplies a positive finite delta, canonical class, and
    // bucket index. Stream synchronization precedes every slot release/reuse.
    unsafe { builder.launch(LaunchConfig::for_num_elems(task_count)) }
        .map(|_| ())
        .map_err(launch_error)
}
