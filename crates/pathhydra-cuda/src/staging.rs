use std::sync::{Arc, Mutex};

use crate::{CudaError, CudaFailureKind};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StagingSnapshot {
    pub capacity_bytes: usize,
    pub current_bytes: usize,
    pub high_water_bytes: usize,
    pub waits: u64,
}

#[derive(Default)]
struct State {
    current: usize,
    high: usize,
    waits: u64,
}

struct Inner {
    capacity: usize,
    state: Mutex<State>,
}

pub(crate) struct StagingPool {
    inner: Arc<Inner>,
}

pub(crate) struct StagingReservation {
    bytes: usize,
    inner: Arc<Inner>,
}

impl StagingPool {
    pub fn new(capacity: usize) -> Result<Self, CudaError> {
        if capacity == 0 {
            return Err(CudaError::new(
                CudaFailureKind::Admission,
                "CUDA host staging capacity must be nonzero",
            ));
        }
        Ok(Self {
            inner: Arc::new(Inner {
                capacity,
                state: Mutex::new(State::default()),
            }),
        })
    }

    pub fn reserve(&self, bytes: usize) -> Result<StagingReservation, CudaError> {
        let mut state = self.inner.state.lock().map_err(|_| {
            CudaError::new(CudaFailureKind::Worker, "CUDA staging lock is poisoned")
        })?;
        let total = state.current.checked_add(bytes).ok_or_else(|| {
            CudaError::new(CudaFailureKind::Admission, "CUDA staging byte overflow")
        })?;
        if total > self.inner.capacity {
            state.waits = state.waits.saturating_add(1);
            return Err(CudaError::new(
                CudaFailureKind::Admission,
                format!(
                    "CUDA staging requires {total} bytes; capacity is {}",
                    self.inner.capacity
                ),
            ));
        }
        state.current = total;
        state.high = state.high.max(total);
        drop(state);
        Ok(StagingReservation {
            bytes,
            inner: Arc::clone(&self.inner),
        })
    }

    pub fn snapshot(&self) -> StagingSnapshot {
        self.inner.state.lock().map_or(
            StagingSnapshot {
                capacity_bytes: self.inner.capacity,
                ..StagingSnapshot::default()
            },
            |state| StagingSnapshot {
                capacity_bytes: self.inner.capacity,
                current_bytes: state.current,
                high_water_bytes: state.high,
                waits: state.waits,
            },
        )
    }
}

impl Drop for StagingReservation {
    fn drop(&mut self) {
        if let Ok(mut state) = self.inner.state.lock() {
            state.current = state.current.saturating_sub(self.bytes);
        }
    }
}
