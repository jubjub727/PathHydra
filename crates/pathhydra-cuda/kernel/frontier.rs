use crate::{
    arithmetic::{global_thread_index, separate_add, separate_multiply},
    atomic::{add, distance_min, increment, load_u32, load_u64, store_u32},
};

pub const STATUS_SUCCESS: u32 = 0;
pub const STATUS_INVALID_INDEX: u32 = 2;
pub const STATUS_INVALID_ARITHMETIC: u32 = 3;
pub const STATUS_COUNTER_OVERFLOW: u32 = 4;

/// Relaxes one compacted resident frontier task per CUDA thread.
#[no_mangle]
pub unsafe extern "ptx-kernel" fn pathhydra_frontier_phase(
    task_edges: *const u64,
    task_sources: *const u32,
    task_count: u32,
    destinations: *const u32,
    relation_indexes: *const u32,
    base_weight_bits: *const u32,
    node_count: u32,
    adjacency_count: u64,
    relation_enabled: *const u32,
    multiplier_bits: *const u32,
    relation_count: u32,
    next_active_sources: *mut u32,
    distance_bits: *mut u64,
    changed: *mut u32,
    status: *mut u32,
    counters: *mut u64,
) {
    let task = global_thread_index();
    if task >= task_count as usize || unsafe { load_u32(status) } != STATUS_SUCCESS {
        return;
    }
    let edge = unsafe { *task_edges.add(task) };
    if edge >= adjacency_count || unsafe { load_u32(status) } != STATUS_SUCCESS {
        return;
    }
    let source = unsafe { *task_sources.add(task) };
    if source >= node_count {
        unsafe { store_u32(status, STATUS_INVALID_INDEX) };
        return;
    }
    if unsafe { !increment(counters, 0, status) } {
        return;
    }
    unsafe {
        relax_one(
            destinations, relation_indexes, base_weight_bits, edge as usize, node_count,
            relation_enabled, multiplier_bits, relation_count, source, next_active_sources,
            distance_bits, changed, status, counters,
        )
    }
}

#[allow(clippy::too_many_arguments)]
pub unsafe fn relax_one(
    destinations: *const u32,
    relation_indexes: *const u32,
    base_weight_bits: *const u32,
    edge: usize,
    node_count: u32,
    relation_enabled: *const u32,
    multiplier_bits: *const u32,
    relation_count: u32,
    source: u32,
    next_active_sources: *mut u32,
    distance_bits: *mut u64,
    changed: *mut u32,
    status: *mut u32,
    counters: *mut u64,
) {
    let relation = unsafe { *relation_indexes.add(edge) };
    if relation >= relation_count {
        unsafe { store_u32(status, STATUS_INVALID_INDEX) };
        return;
    }
    let enabled = unsafe { *relation_enabled.add(relation as usize) };
    if enabled > 1 {
        unsafe { store_u32(status, STATUS_INVALID_ARITHMETIC) };
        return;
    }
    if enabled == 0 {
        return;
    }
    if unsafe { !increment(counters, 1, status) } {
        return;
    }
    let destination = unsafe { *destinations.add(edge) };
    if destination >= node_count {
        unsafe { store_u32(status, STATUS_INVALID_INDEX) };
        return;
    }
    let base = f32::from_bits(unsafe { *base_weight_bits.add(edge) });
    let multiplier = f32::from_bits(unsafe { *multiplier_bits.add(relation as usize) });
    if !base.is_finite() || base < 0.0 || !multiplier.is_finite() || multiplier < 0.0 {
        unsafe { store_u32(status, STATUS_INVALID_ARITHMETIC) };
        return;
    }
    let source_distance = f64::from_bits(unsafe { load_u64(distance_bits.add(source as usize)) });
    if !source_distance.is_finite() {
        return;
    }
    let candidate = separate_add(source_distance, separate_multiply(base, multiplier));
    if !candidate.is_finite() || candidate < 0.0 {
        unsafe { store_u32(status, STATUS_INVALID_ARITHMETIC) };
        return;
    }
    let (updated, retries) = unsafe { distance_min(distance_bits.add(destination as usize), candidate) };
    if unsafe { !add(counters, 4, retries, status) } {
        return;
    }
    if updated {
        unsafe {
            store_u32(next_active_sources.add(destination as usize), 1);
            store_u32(changed, 1);
        }
        let _ = unsafe { increment(counters, 2, status) };
    }
}

pub unsafe fn csr_source(
    offsets: *const u64,
    node_count: u32,
    edge: u64,
    adjacency_count: u64,
) -> Option<u32> {
    if edge >= adjacency_count {
        return None;
    }
    let mut low = 0_u32;
    let mut high = node_count;
    while low < high {
        let middle = low + (high - low) / 2;
        if unsafe { *offsets.add(middle as usize + 1) } <= edge {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    if low < node_count
        && unsafe { *offsets.add(low as usize) } <= edge
        && edge < unsafe { *offsets.add(low as usize + 1) }
    {
        Some(low)
    } else {
        None
    }
}
