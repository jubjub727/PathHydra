use crate::{
    arithmetic::global_thread_index,
    atomic::{increment, load_u32, store_u32},
    frontier::{
        STATUS_INVALID_ARITHMETIC, STATUS_INVALID_INDEX, STATUS_SUCCESS, relax_one,
    },
};

/// Relaxes one compacted partition frontier task per CUDA thread.
#[no_mangle]
pub unsafe extern "ptx-kernel" fn pathhydra_partition_frontier(
    task_edges: *const u32,
    task_sources: *const u32,
    task_count: u32,
    destinations: *const u32,
    relation_indexes: *const u32,
    base_weight_bits: *const u32,
    edge_count: u32,
    node_count: u32,
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
    let source = unsafe { *task_sources.add(task) };
    if edge >= edge_count {
        unsafe { store_u32(status, STATUS_INVALID_INDEX) };
        return;
    }
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
