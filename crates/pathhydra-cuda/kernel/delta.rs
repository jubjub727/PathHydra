use crate::{
    arithmetic::{separate_add, separate_multiply},
    frontier::{STATUS_COUNTER_OVERFLOW, STATUS_INVALID_ARITHMETIC, STATUS_INVALID_INDEX, STATUS_SUCCESS, increment},
};

const MAX_BUCKET_BOUND: f64 = 18_446_744_073_709_551_616.0;
const STATUS_BUCKET_UNREPRESENTABLE: u32 = 6;

/// # Safety
/// The host provides bounded topology/profile arrays, `2 * node_count` scratch
/// words, and exclusive output buffers. One device thread owns the lane.
#[no_mangle]
pub unsafe extern "ptx-kernel" fn pathhydra_delta_route(
    offsets: *const u64,
    destinations: *const u32,
    relation_indexes: *const u32,
    base_weight_bits: *const u32,
    node_count: u32,
    adjacency_count: u64,
    relation_enabled: *const u32,
    multiplier_bits: *const u32,
    relation_count: u32,
    origin: u32,
    delta_bits: u64,
    distance_bits: *mut u64,
    scratch: *mut u32,
    status: *mut u32,
    counters: *mut u64,
) {
    if crate::arithmetic::global_thread_index() != 0 {
        return;
    }
    let delta = f64::from_bits(delta_bits);
    if !delta.is_finite() || delta <= 0.0 || origin >= node_count {
        unsafe { *status = STATUS_INVALID_ARITHMETIC };
        return;
    }
    unsafe { *distance_bits.add(origin as usize) = 0.0_f64.to_bits() };
    let settled = scratch;
    let retained = unsafe { scratch.add(node_count as usize) };
    let mut phases = 0_u64;
    loop {
        let mut current_bucket = u64::MAX;
        let mut node = 0_u32;
        while node < node_count {
            let index = node as usize;
            let is_settled = unsafe { *settled.add(index) };
            if is_settled > 1 {
                unsafe { *status = STATUS_INVALID_INDEX };
                return;
            }
            if is_settled == 0 {
                let distance = f64::from_bits(unsafe { *distance_bits.add(index) });
                if distance.is_finite() {
                    let Some(bucket) = bucket_index(distance, delta) else {
                        unsafe { *status = STATUS_BUCKET_UNREPRESENTABLE };
                        return;
                    };
                    if bucket < current_bucket {
                        current_bucket = bucket;
                    }
                }
            }
            unsafe { *retained.add(index) = 0 };
            node += 1;
        }
        if current_bucket == u64::MAX {
            break;
        }

        loop {
            let mut added = false;
            let mut source = 0_u32;
            while source < node_count {
                let source_index = source as usize;
                let distance = f64::from_bits(unsafe { *distance_bits.add(source_index) });
                let in_bucket = distance.is_finite() && bucket_index(distance, delta) == Some(current_bucket);
                if in_bucket && unsafe { *retained.add(source_index) == 0 } {
                    unsafe { *retained.add(source_index) = 1 };
                    added = true;
                    if unsafe {
                        !relax_edges(
                            offsets, destinations, relation_indexes, base_weight_bits,
                            adjacency_count, relation_enabled, multiplier_bits, relation_count,
                            node_count, source, distance, delta, true, distance_bits, settled,
                            status, counters,
                        )
                    } {
                        return;
                    }
                }
                source += 1;
            }
            if !added {
                break;
            }
        }

        let mut source = 0_u32;
        while source < node_count {
            let index = source as usize;
            if unsafe { *retained.add(index) == 1 } {
                let distance = f64::from_bits(unsafe { *distance_bits.add(index) });
                if unsafe {
                    !relax_edges(
                        offsets, destinations, relation_indexes, base_weight_bits,
                        adjacency_count, relation_enabled, multiplier_bits, relation_count,
                        node_count, source, distance, delta, false, distance_bits, settled,
                        status, counters,
                    )
                } {
                    return;
                }
                unsafe { *settled.add(index) = 1 };
            }
            source += 1;
        }
        phases = match phases.checked_add(1) {
            Some(value) => value,
            None => {
                unsafe { *status = STATUS_COUNTER_OVERFLOW };
                return;
            }
        };
    }
    unsafe { *counters.add(3) = phases };
    let mut reached = 0_u64;
    let mut node = 0_u32;
    while node < node_count {
        if f64::from_bits(unsafe { *distance_bits.add(node as usize) }).is_finite() {
            reached += 1;
        }
        node += 1;
    }
    unsafe { *counters.add(4) = reached };
    unsafe { *status = STATUS_SUCCESS };
}

#[allow(clippy::too_many_arguments)]
unsafe fn relax_edges(
    offsets: *const u64,
    destinations: *const u32,
    relation_indexes: *const u32,
    base_weight_bits: *const u32,
    adjacency_count: u64,
    relation_enabled: *const u32,
    multiplier_bits: *const u32,
    relation_count: u32,
    node_count: u32,
    source: u32,
    source_distance: f64,
    delta: f64,
    light: bool,
    distance_bits: *mut u64,
    settled: *mut u32,
    status: *mut u32,
    counters: *mut u64,
) -> bool {
    let start = unsafe { *offsets.add(source as usize) };
    let end = unsafe { *offsets.add(source as usize + 1) };
    if start > end || end > adjacency_count {
        unsafe { *status = STATUS_INVALID_INDEX };
        return false;
    }
    let mut adjacency = start;
    while adjacency < end {
        if unsafe { !increment(counters, 0) } {
            unsafe { *status = STATUS_COUNTER_OVERFLOW };
            return false;
        }
        let index = adjacency as usize;
        let relation_index = unsafe { *relation_indexes.add(index) };
        if relation_index >= relation_count {
            unsafe { *status = STATUS_INVALID_INDEX };
            return false;
        }
        let enabled = unsafe { *relation_enabled.add(relation_index as usize) };
        if enabled > 1 {
            unsafe { *status = STATUS_INVALID_ARITHMETIC };
            return false;
        }
        if enabled == 1 {
            let base = f32::from_bits(unsafe { *base_weight_bits.add(index) });
            let multiplier = f32::from_bits(unsafe { *multiplier_bits.add(relation_index as usize) });
            if !base.is_finite() || base < 0.0 || !multiplier.is_finite() || multiplier < 0.0 {
                unsafe { *status = STATUS_INVALID_ARITHMETIC };
                return false;
            }
            let effective = separate_multiply(base, multiplier);
            if (effective <= delta) == light {
                if unsafe { !increment(counters, 1) } {
                    unsafe { *status = STATUS_COUNTER_OVERFLOW };
                    return false;
                }
                let destination = unsafe { *destinations.add(index) };
                if destination >= node_count {
                    unsafe { *status = STATUS_INVALID_INDEX };
                    return false;
                }
                let candidate = separate_add(source_distance, effective);
                if !candidate.is_finite() || candidate < 0.0 {
                    unsafe { *status = STATUS_INVALID_ARITHMETIC };
                    return false;
                }
                let destination_index = destination as usize;
                let existing = f64::from_bits(unsafe { *distance_bits.add(destination_index) });
                if candidate < existing {
                    if unsafe { *settled.add(destination_index) != 0 } {
                        unsafe { *status = STATUS_INVALID_INDEX };
                        return false;
                    }
                    unsafe { *distance_bits.add(destination_index) = candidate.to_bits() };
                    if unsafe { !increment(counters, 2) } {
                        unsafe { *status = STATUS_COUNTER_OVERFLOW };
                        return false;
                    }
                }
            }
        }
        adjacency += 1;
    }
    true
}

fn bucket_index(distance: f64, delta: f64) -> Option<u64> {
    let quotient = distance / delta;
    (quotient.is_finite() && quotient >= 0.0 && quotient < MAX_BUCKET_BOUND).then_some(quotient as u64)
}
