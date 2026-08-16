#![no_std]
#![feature(abi_ptx)]
#![feature(stdarch_nvptx)]
#![deny(unsafe_op_in_unsafe_fn)]

mod arithmetic;
mod delta;
mod frontier;
mod frontier_compaction;
mod partition_delta;
mod partition_frontier;

use core::panic::PanicInfo;

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    loop {}
}

/// # Safety
/// Every pointer must address at least `length` correctly aligned elements in
/// the active CUDA context. `output` must be uniquely writable for the launch.
#[no_mangle]
pub unsafe extern "ptx-kernel" fn pathhydra_smoke_arithmetic(
    input: *const f32,
    multipliers: *const f32,
    addends: *const f64,
    output: *mut f64,
    length: u32,
) {
    let index = arithmetic::global_thread_index();
    if index >= length as usize {
        return;
    }
    // SAFETY: the kernel ABI requires every buffer to contain `length`
    // elements, and this thread proved its index is in range.
    let (value, multiplier, addend) = unsafe {
        (
            *input.add(index),
            *multipliers.add(index),
            *addends.add(index),
        )
    };
    let product = arithmetic::separate_multiply(value, multiplier);
    let result = arithmetic::separate_add(addend, product);
    // SAFETY: output is uniquely writable per in-range thread index.
    unsafe { *output.add(index) = result };
}

/// # Safety
/// Topology pointers have the fixed logical lengths carried by the scalar
/// counts. `status` and four checksum words are exclusively writable.
#[no_mangle]
pub unsafe extern "ptx-kernel" fn pathhydra_validate_topology(
    offsets: *const u64,
    destinations: *const u32,
    relation_indexes: *const u32,
    base_weight_bits: *const u32,
    node_count: u32,
    relation_count: u32,
    adjacency_count: u64,
    status: *mut u32,
    checksum: *mut u64,
) {
    if arithmetic::global_thread_index() != 0 {
        return;
    }
    let mut previous = 0_u64;
    let mut node = 0_u32;
    while node <= node_count {
        let current = unsafe { *offsets.add(node as usize) };
        if current < previous || current > adjacency_count {
            unsafe { *status = 2 };
            return;
        }
        previous = current;
        node += 1;
    }
    if previous != adjacency_count {
        unsafe { *status = 2 };
        return;
    }
    let mut destination_xor = 0_u64;
    let mut relation_weight_xor = 0_u64;
    let mut adjacency = 0_u64;
    while adjacency < adjacency_count {
        let index = adjacency as usize;
        let destination = unsafe { *destinations.add(index) };
        let relation = unsafe { *relation_indexes.add(index) };
        let weight = unsafe { *base_weight_bits.add(index) };
        if destination >= node_count || relation >= relation_count {
            unsafe { *status = 2 };
            return;
        }
        destination_xor ^= u64::from(destination);
        relation_weight_xor ^= (u64::from(relation) << 32) | u64::from(weight);
        adjacency += 1;
    }
    unsafe {
        *checksum = u64::from(node_count);
        *checksum.add(1) = adjacency_count;
        *checksum.add(2) = destination_xor;
        *checksum.add(3) = relation_weight_xor;
        *status = 0;
    }
}
