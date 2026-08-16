use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use cudarc::driver::{CudaSlice, LaunchConfig, PushKernelArg, sys};

use pathhydra_routing::{
    CompletionReason, DestinationResult, DestinationState, ExactRoute, RoutingRequest,
    RoutingResponse, SearchBudget,
};

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
    let image = resident.cpu_image();
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

    let cancelled_before_launch = cancellation.load(Ordering::Acquire);
    let no_search_needed = image.adjacency_count() == 0
        || mapped_destinations
            .iter()
            .all(|destination| destination.is_none_or(|destination| destination == origin));
    let started = Instant::now();
    let mut host_to_device_bytes = 0_usize;
    let mut device_to_host_bytes = 0_usize;
    let mut launches = 0_u64;
    let mut counters = [0_u64; 5];
    let distance_bits = if cancelled_before_launch || no_search_needed {
        let mut bits = vec![f64::INFINITY.to_bits(); image.node_count()];
        bits[origin.as_u32() as usize] = 0.0_f64.to_bits();
        bits
    } else {
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
        match algorithm {
            CudaAlgorithm::Frontier => {
                let mut builder = stream.launch_builder(&resident.context.frontier_function);
                builder.arg(&resident.offsets);
                builder.arg(&resident.destinations);
                builder.arg(&resident.relation_indexes);
                builder.arg(&resident.base_weight_bits);
                builder.arg(&node_count);
                builder.arg(&adjacency_count);
                builder.arg(&enabled_device);
                builder.arg(&multiplier_device);
                builder.arg(&relation_count);
                builder.arg(&origin_u32);
                builder.arg(&mut distance_device);
                builder.arg(&mut status_device);
                builder.arg(&mut counters_device);
                // SAFETY: the function ABI exactly matches the ordered arguments;
                // resident topology arrays were uploaded and synchronized together;
                // node/relation/adjacency counts were checked into their fixed-width
                // types; all mutable buffers are initialized, correctly aligned,
                // uniquely borrowed, and long enough; the single-thread kernel bounds
                // every access; downloads below synchronize before any lifetime ends.
                unsafe {
                    builder.launch(LaunchConfig {
                        grid_dim: (1, 1, 1),
                        block_dim: (1, 1, 1),
                        shared_mem_bytes: 0,
                    })
                }
                .map_err(launch_error)?;
            }
            CudaAlgorithm::DeltaStepping(delta) => {
                let delta_bits = delta.delta().to_bits();
                let scratch_len = image.node_count().checked_mul(2).ok_or_else(|| {
                    CudaError::new(CudaFailureKind::Allocation, "delta scratch length overflow")
                })?;
                let mut scratch_device = stream
                    .alloc_zeros::<u32>(scratch_len)
                    .map_err(allocation_error)?;
                let mut builder = stream.launch_builder(&resident.context.delta_function);
                builder.arg(&resident.offsets);
                builder.arg(&resident.destinations);
                builder.arg(&resident.relation_indexes);
                builder.arg(&resident.base_weight_bits);
                builder.arg(&node_count);
                builder.arg(&adjacency_count);
                builder.arg(&enabled_device);
                builder.arg(&multiplier_device);
                builder.arg(&relation_count);
                builder.arg(&origin_u32);
                builder.arg(&delta_bits);
                builder.arg(&mut distance_device);
                builder.arg(&mut scratch_device);
                builder.arg(&mut status_device);
                builder.arg(&mut counters_device);
                // SAFETY: this is the audited delta ABI in the exact declared
                // order. All topology/profile/count obligations above apply;
                // scratch has exactly two node-sized initialized arrays, output
                // buffers are uniquely borrowed, and synchronous downloads keep
                // every allocation and function/context handle alive to completion.
                unsafe {
                    builder.launch(LaunchConfig {
                        grid_dim: (1, 1, 1),
                        block_dim: (1, 1, 1),
                        shared_mem_bytes: 0,
                    })
                }
                .map_err(launch_error)?;
            }
        }
        launches = 1;
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
        device_to_host_bytes = distances.len() * 8 + 4 + counters.len() * 8;
        if status != [0] {
            return Err(CudaError::new(
                CudaFailureKind::KernelInvariant,
                format!("CUDA routing kernel returned status {}", status[0]),
            ));
        }
        distances
    };
    let cancelled = cancelled_before_launch || cancellation.load(Ordering::Acquire);
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
    let response = RoutingResponse::distance_only(
        request.origin(),
        results,
        packed.canonical().clone(),
        request.tie_policy(),
        (counters[0], counters[4]),
        completion_reason,
    );
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
        },
    })
}

fn abi_count(structure: &'static str) -> CudaError {
    CudaError::new(
        CudaFailureKind::Admission,
        format!("CUDA {structure} count does not fit the kernel ABI"),
    )
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
