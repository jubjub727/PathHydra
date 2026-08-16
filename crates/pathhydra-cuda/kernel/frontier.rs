use crate::arithmetic::{separate_add, separate_multiply};

pub const STATUS_SUCCESS: u32 = 0;
pub const STATUS_INVALID_INDEX: u32 = 2;
pub const STATUS_INVALID_ARITHMETIC: u32 = 3;
pub const STATUS_COUNTER_OVERFLOW: u32 = 4;

/// # Safety
/// The host ABI guarantees every pointer and logical length. The kernel uses a
/// single device thread, so all writes are exclusive for the launch.
#[no_mangle]
pub unsafe extern "ptx-kernel" fn pathhydra_frontier_route(
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
    distance_bits: *mut u64,
    status: *mut u32,
    counters: *mut u64,
) {
    if crate::arithmetic::global_thread_index() != 0 {
        return;
    }
    // SAFETY: forwarded host ABI and single-thread ownership satisfy the
    // implementation contract.
    unsafe {
        route_impl(
            offsets, destinations, relation_indexes, base_weight_bits, node_count,
            adjacency_count, relation_enabled, multiplier_bits, relation_count, origin,
            distance_bits, status, counters,
        )
    }
}

#[allow(clippy::too_many_arguments)]
pub unsafe fn route_impl(
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
    distance_bits: *mut u64,
    status: *mut u32,
    counters: *mut u64,
) {
    if origin >= node_count {
        unsafe { *status = STATUS_INVALID_INDEX };
        return;
    }
    unsafe { *distance_bits.add(origin as usize) = 0.0_f64.to_bits() };
    let mut changed = true;
    let mut phase = 0_u64;
    while changed && phase < u64::from(node_count) {
        changed = false;
        let mut source = 0_u32;
        while source < node_count {
            let source_distance = f64::from_bits(unsafe { *distance_bits.add(source as usize) });
            if source_distance.is_finite() {
                let (start, end) = unsafe {
                    (*offsets.add(source as usize), *offsets.add(source as usize + 1))
                };
                if start > end || end > adjacency_count {
                    unsafe { *status = STATUS_INVALID_INDEX };
                    return;
                }
                let mut adjacency = start;
                while adjacency < end {
                    if unsafe { !increment(counters, 0) } {
                        unsafe { *status = STATUS_COUNTER_OVERFLOW };
                        return;
                    }
                    let index = adjacency as usize;
                    let relation_index = unsafe { *relation_indexes.add(index) };
                    if relation_index >= relation_count {
                        unsafe { *status = STATUS_INVALID_INDEX };
                        return;
                    }
                    let enabled = unsafe { *relation_enabled.add(relation_index as usize) };
                    if enabled > 1 {
                        unsafe { *status = STATUS_INVALID_ARITHMETIC };
                        return;
                    }
                    if enabled == 1 {
                        if unsafe { !increment(counters, 1) } {
                            unsafe { *status = STATUS_COUNTER_OVERFLOW };
                            return;
                        }
                        let destination = unsafe { *destinations.add(index) };
                        if destination >= node_count {
                            unsafe { *status = STATUS_INVALID_INDEX };
                            return;
                        }
                        let base = f32::from_bits(unsafe { *base_weight_bits.add(index) });
                        let multiplier = f32::from_bits(unsafe {
                            *multiplier_bits.add(relation_index as usize)
                        });
                        if !base.is_finite() || base < 0.0 || !multiplier.is_finite() || multiplier < 0.0 {
                            unsafe { *status = STATUS_INVALID_ARITHMETIC };
                            return;
                        }
                        let effective = separate_multiply(base, multiplier);
                        let candidate = separate_add(source_distance, effective);
                        if !candidate.is_finite() || candidate < 0.0 {
                            unsafe { *status = STATUS_INVALID_ARITHMETIC };
                            return;
                        }
                        let destination_index = destination as usize;
                        let existing = f64::from_bits(unsafe { *distance_bits.add(destination_index) });
                        if candidate < existing {
                            unsafe { *distance_bits.add(destination_index) = candidate.to_bits() };
                            if unsafe { !increment(counters, 2) } {
                                unsafe { *status = STATUS_COUNTER_OVERFLOW };
                                return;
                            }
                            changed = true;
                        }
                    }
                    adjacency += 1;
                }
            }
            source += 1;
        }
        phase += 1;
    }
    unsafe { *counters.add(3) = phase };
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

/// # Safety
/// `counters` addresses at least five writable u64 values.
pub unsafe fn increment(counters: *mut u64, index: usize) -> bool {
    let value = unsafe { *counters.add(index) };
    let Some(next) = value.checked_add(1) else {
        return false;
    };
    unsafe { *counters.add(index) = next };
    true
}
