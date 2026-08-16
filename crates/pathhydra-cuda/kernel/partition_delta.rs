use crate::{
    arithmetic::global_thread_index,
    atomic::{load_u32, store_u32},
    delta::delta_relax_one,
    frontier::{STATUS_INVALID_ARITHMETIC, STATUS_INVALID_INDEX, STATUS_SUCCESS},
};

/// Processes one partition light-closure or heavy-edge pass with one CUDA
/// thread per compacted edge/source task. The removed-source bitmap survives every light pass so
/// the heavy phase cannot lose a source when partitions execute in any order.
#[no_mangle]
pub unsafe extern "ptx-kernel" fn pathhydra_partition_delta(
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
    delta_bits: u64,
    bucket: u64,
    edge_class: u32,
    removed_sources: *mut u32,
    distance_bits: *mut u64,
    changed_same_bucket: *mut u32,
    status: *mut u32,
    counters: *mut u64,
) {
    let task = global_thread_index();
    if task >= task_count as usize || unsafe { load_u32(status) } != STATUS_SUCCESS {
        return;
    }
    let edge = unsafe { *task_edges.add(task) };
    let delta = f64::from_bits(delta_bits);
    if !delta.is_finite() || delta <= 0.0 || edge_class > 1 {
        unsafe { store_u32(status, STATUS_INVALID_ARITHMETIC) };
        return;
    }
    let source = unsafe { *task_sources.add(task) };
    if edge >= edge_count {
        unsafe { store_u32(status, STATUS_INVALID_INDEX) };
        return;
    }
    if source >= node_count {
        unsafe { store_u32(status, STATUS_INVALID_INDEX) };
        return;
    }
    unsafe {
        delta_relax_one(
            destinations, relation_indexes, base_weight_bits, edge as usize, node_count,
            relation_enabled, multiplier_bits, relation_count, source, delta, bucket, edge_class,
            removed_sources, distance_bits, changed_same_bucket, status, counters,
        )
    }
}
