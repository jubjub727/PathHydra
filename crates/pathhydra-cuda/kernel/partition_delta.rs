use crate::{
    arithmetic::{separate_add, separate_multiply},
    frontier::{STATUS_COUNTER_OVERFLOW, STATUS_INVALID_ARITHMETIC, STATUS_INVALID_INDEX},
};

pub const STATUS_BUCKET_OVERFLOW: u32 = 5;

/// Processes one light-closure or heavy-edge pass for one partition and bucket.
///
/// `edge_class` is zero for weights at most delta and one for weights greater
/// than delta. The host closes all light work before launching every heavy pass.
///
/// # Safety
/// The host owns the same fixed array and single-thread obligations as the
/// partition-frontier entry, plus a positive finite delta and canonical class.
#[no_mangle]
pub unsafe extern "ptx-kernel" fn pathhydra_partition_delta(
    segment_sources: *const u32,
    segment_starts: *const u32,
    segment_counts: *const u32,
    segment_count: u32,
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
    distance_bits: *mut u64,
    changed_same_bucket: *mut u32,
    status: *mut u32,
    counters: *mut u64,
) {
    if crate::arithmetic::global_thread_index() != 0 {
        return;
    }
    let delta = f64::from_bits(delta_bits);
    if !delta.is_finite() || delta <= 0.0 || edge_class > 1 {
        unsafe { *status = STATUS_INVALID_ARITHMETIC };
        return;
    }
    let mut segment = 0_u32;
    while segment < segment_count {
        let source = unsafe { *segment_sources.add(segment as usize) };
        let start = unsafe { *segment_starts.add(segment as usize) };
        let count = unsafe { *segment_counts.add(segment as usize) };
        let Some(end) = start.checked_add(count) else {
            unsafe { *status = STATUS_INVALID_INDEX };
            return;
        };
        if source >= node_count || count == 0 || end > edge_count {
            unsafe { *status = STATUS_INVALID_INDEX };
            return;
        }
        let source_distance = f64::from_bits(unsafe { *distance_bits.add(source as usize) });
        if source_distance.is_finite() {
            let quotient = source_distance / delta;
            if quotient >= u64::MAX as f64 {
                unsafe { *status = STATUS_BUCKET_OVERFLOW };
                return;
            }
            if quotient as u64 == bucket {
                let mut edge = start;
                while edge < end {
                    if unsafe { !increment(counters, 0) } {
                        unsafe { *status = STATUS_COUNTER_OVERFLOW };
                        return;
                    }
                    let index = edge as usize;
                    let relation = unsafe { *relation_indexes.add(index) };
                    if relation >= relation_count {
                        unsafe { *status = STATUS_INVALID_INDEX };
                        return;
                    }
                    let enabled = unsafe { *relation_enabled.add(relation as usize) };
                    if enabled > 1 {
                        unsafe { *status = STATUS_INVALID_ARITHMETIC };
                        return;
                    }
                    if enabled == 1 {
                        let base = f32::from_bits(unsafe { *base_weight_bits.add(index) });
                        let multiplier =
                            f32::from_bits(unsafe { *multiplier_bits.add(relation as usize) });
                        if !base.is_finite()
                            || base < 0.0
                            || !multiplier.is_finite()
                            || multiplier < 0.0
                        {
                            unsafe { *status = STATUS_INVALID_ARITHMETIC };
                            return;
                        }
                        let effective = separate_multiply(base, multiplier);
                        let light = effective <= delta;
                        if (edge_class == 0 && light) || (edge_class == 1 && !light) {
                            if unsafe { !increment(counters, 1) } {
                                unsafe { *status = STATUS_COUNTER_OVERFLOW };
                                return;
                            }
                            let destination = unsafe { *destinations.add(index) };
                            if destination >= node_count {
                                unsafe { *status = STATUS_INVALID_INDEX };
                                return;
                            }
                            let candidate = separate_add(source_distance, effective);
                            if !candidate.is_finite() || candidate < 0.0 {
                                unsafe { *status = STATUS_INVALID_ARITHMETIC };
                                return;
                            }
                            let slot = unsafe { distance_bits.add(destination as usize) };
                            let existing = f64::from_bits(unsafe { *slot });
                            if candidate < existing {
                                unsafe { *slot = candidate.to_bits() };
                                if unsafe { !increment(counters, 2) } {
                                    unsafe { *status = STATUS_COUNTER_OVERFLOW };
                                    return;
                                }
                                let candidate_quotient = candidate / delta;
                                if candidate_quotient >= u64::MAX as f64 {
                                    unsafe { *status = STATUS_BUCKET_OVERFLOW };
                                    return;
                                }
                                if edge_class == 0 && candidate_quotient as u64 == bucket {
                                    unsafe { *changed_same_bucket = 1 };
                                }
                            }
                        }
                    }
                    edge += 1;
                }
            }
        }
        segment += 1;
    }
    unsafe { *status = 0 };
}
unsafe fn increment(counters: *mut u64, index: usize) -> bool {
    let value = unsafe { *counters.add(index) };
    let Some(next) = value.checked_add(1) else {
        return false;
    };
    unsafe { *counters.add(index) = next };
    true
}
