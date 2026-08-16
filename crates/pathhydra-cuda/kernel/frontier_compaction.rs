/// Counts finite labels after a complete partition phase.
///
/// # Safety
/// `distance_bits` addresses `node_count` words and `finite_count` is uniquely
/// writable. The host launches exactly one thread.
#[no_mangle]
pub unsafe extern "ptx-kernel" fn pathhydra_frontier_compaction(
    distance_bits: *const u64,
    node_count: u32,
    finite_count: *mut u32,
) {
    if crate::arithmetic::global_thread_index() != 0 {
        return;
    }
    let mut count = 0_u32;
    let mut node = 0_u32;
    while node < node_count {
        if f64::from_bits(unsafe { *distance_bits.add(node as usize) }).is_finite() {
            count = count.saturating_add(1);
        }
        node += 1;
    }
    unsafe { *finite_count = count };
}
