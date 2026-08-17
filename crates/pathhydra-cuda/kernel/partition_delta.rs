//! Partition-delta safety invariant for every unsafe block in this module: the
//! host retains aligned partition/task/profile/search buffers of the declared
//! ABI lengths through synchronization, and task/bucket/class/index values are
//! checked before access.

use core::arch::nvptx::{_block_dim_x, _syncthreads, _thread_idx_x};

use crate::{
    arithmetic::{global_thread_index, separate_multiply},
    atomic::{add, load_u32, store_u32},
    delta::delta_relax_one,
    frontier::{STATUS_INVALID_ARITHMETIC, STATUS_INVALID_INDEX, STATUS_SUCCESS},
};

/// Processes one partition light-closure or heavy-edge pass with one CUDA
/// thread per compacted edge/source task. The removed-source bitmap survives every light pass so
/// the heavy phase cannot lose a source when partitions execute in any order.
#[no_mangle]
/// # Safety
/// All arguments satisfy the module partition-delta safety invariant.
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
    let delta = f64::from_bits(delta_bits);
    if unsafe { _thread_idx_x() } == 0 && (!delta.is_finite() || delta <= 0.0 || edge_class > 1) {
        unsafe { store_u32(status, STATUS_INVALID_ARITHMETIC) };
    }
    if unsafe { _thread_idx_x() } == 0 && unsafe { load_u32(status) } == STATUS_SUCCESS {
        let end = task
            .saturating_add(unsafe { _block_dim_x() } as usize)
            .min(task_count as usize);
        let mut attempts = 0_u64;
        for index in task..end {
            let edge = unsafe { *task_edges.add(index) };
            let source = unsafe { *task_sources.add(index) };
            if edge >= edge_count || source >= node_count {
                unsafe { store_u32(status, STATUS_INVALID_INDEX) };
                break;
            }
            let relation = unsafe { *relation_indexes.add(edge as usize) };
            if relation >= relation_count {
                unsafe { store_u32(status, STATUS_INVALID_INDEX) };
                break;
            }
            let enabled = unsafe { *relation_enabled.add(relation as usize) };
            let base = f32::from_bits(unsafe { *base_weight_bits.add(edge as usize) });
            let multiplier = f32::from_bits(unsafe { *multiplier_bits.add(relation as usize) });
            if enabled > 1
                || !base.is_finite()
                || base < 0.0
                || !multiplier.is_finite()
                || multiplier < 0.0
            {
                unsafe { store_u32(status, STATUS_INVALID_ARITHMETIC) };
                break;
            }
            if enabled == 1 {
                let light = separate_multiply(base, multiplier) <= delta;
                attempts = attempts.saturating_add(u64::from((edge_class == 0) == light));
            }
        }
        if unsafe { load_u32(status) } == STATUS_SUCCESS {
            let examined = (end - task) as u64;
            let _ = unsafe { add(counters, 0, examined, status) };
            if unsafe { load_u32(status) } == STATUS_SUCCESS {
                let _ = unsafe { add(counters, 1, attempts, status) };
            }
        }
    }
    unsafe { _syncthreads() };
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
    unsafe {
        delta_relax_one(
            destinations,
            relation_indexes,
            base_weight_bits,
            edge as usize,
            node_count,
            relation_enabled,
            multiplier_bits,
            relation_count,
            source,
            delta,
            bucket,
            edge_class,
            removed_sources,
            distance_bits,
            changed_same_bucket,
            status,
            counters,
        )
    }
}
