//! Arithmetic-kernel safety invariant for every unsafe block in this module:
//! NVPTX thread-index intrinsics execute only inside a PTX kernel; these
//! arithmetic helpers dereference no pointers.

use core::arch::nvptx::{_block_dim_x, _block_idx_x, _thread_idx_x};

#[inline(always)]
pub fn global_thread_index() -> usize {
    // SAFETY: NVPTX index intrinsics are valid in a PTX kernel.
    unsafe {
        (_block_idx_x() as usize * _block_dim_x() as usize) + _thread_idx_x() as usize
    }
}

#[inline(never)]
pub fn separate_multiply(value: f32, multiplier: f32) -> f64 {
    f64::from(value) * f64::from(multiplier)
}

#[inline(never)]
pub fn separate_add(current: f64, effective: f64) -> f64 {
    current + effective
}
