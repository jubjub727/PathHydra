use crate::{
    arithmetic::{global_thread_index, separate_add, separate_multiply},
    atomic::{add, distance_min, increment, load_u32, load_u64, store_u32},
    frontier::{
        STATUS_INVALID_ARITHMETIC, STATUS_INVALID_INDEX, STATUS_SUCCESS,
    },
};

const MAX_BUCKET_BOUND: f64 = 18_446_744_073_709_551_616.0;
pub const STATUS_BUCKET_UNREPRESENTABLE: u32 = 6;

/// Processes one resident light-closure or heavy-edge pass in parallel.
#[no_mangle]
pub unsafe extern "ptx-kernel" fn pathhydra_delta_phase(
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
    if edge >= adjacency_count || unsafe { load_u32(status) } != STATUS_SUCCESS {
        return;
    }
    let delta = f64::from_bits(delta_bits);
    if !delta.is_finite() || delta <= 0.0 || edge_class > 1 {
        unsafe { store_u32(status, STATUS_INVALID_ARITHMETIC) };
        return;
    }
    let source = unsafe { *task_sources.add(task) };
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

#[allow(clippy::too_many_arguments)]
pub unsafe fn delta_relax_one(
    destinations: *const u32,
    relation_indexes: *const u32,
    base_weight_bits: *const u32,
    edge: usize,
    node_count: u32,
    relation_enabled: *const u32,
    multiplier_bits: *const u32,
    relation_count: u32,
    source: u32,
    delta: f64,
    bucket: u64,
    edge_class: u32,
    removed_sources: *mut u32,
    distance_bits: *mut u64,
    changed_same_bucket: *mut u32,
    status: *mut u32,
    counters: *mut u64,
) {
    let source_distance = f64::from_bits(unsafe { load_u64(distance_bits.add(source as usize)) });
    let source_selected = if edge_class == 0 {
        match bucket_index(source_distance, delta) {
            Some(index) if index == bucket => {
                unsafe { store_u32(removed_sources.add(source as usize), 1) };
                true
            }
            Some(_) | None if !source_distance.is_finite() => false,
            Some(_) => false,
            None => {
                unsafe { store_u32(status, STATUS_BUCKET_UNREPRESENTABLE) };
                return;
            }
        }
    } else {
        unsafe { load_u32(removed_sources.add(source as usize)) == 1 }
    };
    if !source_selected {
        return;
    }
    if unsafe { !increment(counters, 0, status) } {
        return;
    }
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
    let base = f32::from_bits(unsafe { *base_weight_bits.add(edge) });
    let multiplier = f32::from_bits(unsafe { *multiplier_bits.add(relation as usize) });
    if !base.is_finite() || base < 0.0 || !multiplier.is_finite() || multiplier < 0.0 {
        unsafe { store_u32(status, STATUS_INVALID_ARITHMETIC) };
        return;
    }
    let effective = separate_multiply(base, multiplier);
    let light = effective <= delta;
    if (edge_class == 0) != light {
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
    let candidate = separate_add(source_distance, effective);
    if !candidate.is_finite() || candidate < 0.0 {
        unsafe { store_u32(status, STATUS_INVALID_ARITHMETIC) };
        return;
    }
    let Some(candidate_bucket) = bucket_index(candidate, delta) else {
        unsafe { store_u32(status, STATUS_BUCKET_UNREPRESENTABLE) };
        return;
    };
    let (updated, retries) = unsafe { distance_min(distance_bits.add(destination as usize), candidate) };
    if unsafe { !add(counters, 4, retries, status) } {
        return;
    }
    if updated {
        if edge_class == 0 && candidate_bucket == bucket {
            unsafe { store_u32(changed_same_bucket, 1) };
        }
        let _ = unsafe { increment(counters, 2, status) };
    }
}

pub fn bucket_index(distance: f64, delta: f64) -> Option<u64> {
    let quotient = distance / delta;
    (quotient.is_finite() && quotient >= 0.0 && quotient < MAX_BUCKET_BOUND)
        .then_some(quotient as u64)
}
