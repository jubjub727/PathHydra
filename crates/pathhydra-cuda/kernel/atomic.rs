use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::frontier::STATUS_COUNTER_OVERFLOW;

#[inline(always)]
pub unsafe fn load_u64(slot: *const u64) -> u64 {
    unsafe { (&*slot.cast::<AtomicU64>()).load(Ordering::Relaxed) }
}

#[inline(always)]
pub unsafe fn store_u32(slot: *mut u32, value: u32) {
    unsafe { (&*slot.cast::<AtomicU32>()).store(value, Ordering::Relaxed) }
}

#[inline(always)]
pub unsafe fn load_u32(slot: *const u32) -> u32 {
    unsafe { (&*slot.cast::<AtomicU32>()).load(Ordering::Relaxed) }
}

/// Applies an exact minimum to a non-negative finite binary64 value or the
/// positive-infinity sentinel. Those bit patterns have the same ordering as
/// their numeric values, so the integer CAS preserves exact distance order.
#[inline(always)]
pub unsafe fn distance_min(slot: *mut u64, candidate: f64) -> (bool, u64) {
    let atomic = unsafe { &*slot.cast::<AtomicU64>() };
    let candidate_bits = candidate.to_bits();
    let mut current = atomic.load(Ordering::Relaxed);
    let mut retries = 0_u64;
    while candidate_bits < current {
        match atomic.compare_exchange_weak(
            current,
            candidate_bits,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => return (true, retries),
            Err(observed) => {
                retries = retries.saturating_add(1);
                current = observed;
            }
        }
    }
    (false, retries)
}

#[inline(always)]
pub unsafe fn add(counters: *mut u64, index: usize, value: u64, status: *mut u32) -> bool {
    if value == 0 {
        return true;
    }
    let counter = unsafe { &*counters.add(index).cast::<AtomicU64>() };
    let mut current = counter.load(Ordering::Relaxed);
    loop {
        let Some(next) = current.checked_add(value) else {
            unsafe { store_u32(status, STATUS_COUNTER_OVERFLOW) };
            return false;
        };
        match counter.compare_exchange_weak(
            current,
            next,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => return true,
            Err(observed) => current = observed,
        }
    }
}

#[inline(always)]
pub unsafe fn increment(counters: *mut u64, index: usize, status: *mut u32) -> bool {
    let counter = unsafe { &*counters.add(index).cast::<AtomicU64>() };
    let mut current = counter.load(Ordering::Relaxed);
    loop {
        let Some(next) = current.checked_add(1) else {
            unsafe { store_u32(status, STATUS_COUNTER_OVERFLOW) };
            return false;
        };
        match counter.compare_exchange_weak(
            current,
            next,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => return true,
            Err(observed) => current = observed,
        }
    }
}
