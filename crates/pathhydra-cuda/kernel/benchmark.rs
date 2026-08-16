use crate::{
    arithmetic::{global_thread_index, separate_multiply},
    atomic::{load_u32, store_u32},
};

#[no_mangle]
pub unsafe extern "ptx-kernel" fn pathhydra_benchmark_reset(
    distance_bits: *mut u64,
    frontier: *mut u32,
    stamps: *mut u32,
    node_count: u32,
    generation: u32,
    mode: u32,
) {
    let node = global_thread_index();
    if node >= node_count as usize {
        return;
    }
    if mode == 0 {
        unsafe {
            *distance_bits.add(node) = f64::INFINITY.to_bits();
            store_u32(frontier.add(node), 0);
        }
    } else if unsafe { load_u32(stamps.add(node)) } != generation {
        unsafe {
            *distance_bits.add(node) = f64::INFINITY.to_bits();
            store_u32(frontier.add(node), 0);
            store_u32(stamps.add(node), generation);
        }
    }
}

#[no_mangle]
pub unsafe extern "ptx-kernel" fn pathhydra_benchmark_target(
    representation: *const u32,
    representation_len: u32,
    node_count: u32,
    generation: u32,
    mode: u32,
    membership: *mut u32,
) {
    let node = global_thread_index();
    if node >= node_count as usize {
        return;
    }
    let member = if mode == 0 {
        let mut low = 0_usize;
        let mut high = representation_len as usize;
        while low < high {
            let middle = low + (high - low) / 2;
            let value = unsafe { *representation.add(middle) };
            if value < node as u32 {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        low < representation_len as usize && unsafe { *representation.add(low) } == node as u32
    } else if mode == 1 {
        (unsafe { *representation.add(node) }) == 1
    } else {
        (unsafe { *representation.add(node) }) == generation
    };
    unsafe { store_u32(membership.add(node), u32::from(member)) };
}

#[no_mangle]
pub unsafe extern "ptx-kernel" fn pathhydra_benchmark_profile_inline(
    base_weight_bits: *const u32,
    relation_indexes: *const u32,
    multiplier_bits: *const u32,
    relation_count: u32,
    edge_count: u32,
    output: *mut u64,
) {
    let edge = global_thread_index();
    if edge >= edge_count as usize {
        return;
    }
    let relation = unsafe { *relation_indexes.add(edge) };
    if relation >= relation_count {
        return;
    }
    let base = f32::from_bits(unsafe { *base_weight_bits.add(edge) });
    let multiplier = f32::from_bits(unsafe { *multiplier_bits.add(relation as usize) });
    unsafe { *output.add(edge) = separate_multiply(base, multiplier).to_bits() };
}

#[no_mangle]
pub unsafe extern "ptx-kernel" fn pathhydra_benchmark_profile_consume(
    materialized: *const u64,
    edge_count: u32,
    output: *mut u64,
) {
    let edge = global_thread_index();
    if edge < edge_count as usize {
        unsafe { *output.add(edge) = *materialized.add(edge) };
    }
}
