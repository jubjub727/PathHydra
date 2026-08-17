use std::sync::{Arc, Mutex};

use crate::{CudaAlgorithm, CudaError, CudaFailureKind};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CudaMemoryLimits {
    pub maximum_search_bytes: usize,
    pub maximum_concurrent_searches: usize,
}

#[derive(Debug)]
pub struct CudaSearchReservation {
    bytes: usize,
    controller: Option<Arc<AdmissionInner>>,
}

impl CudaSearchReservation {
    #[must_use]
    pub const fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn acquire(bytes: usize, limits: CudaMemoryLimits) -> Result<Self, CudaError> {
        if bytes > limits.maximum_search_bytes {
            return Err(CudaError::new(
                CudaFailureKind::Admission,
                format!(
                    "CUDA search requires {bytes} bytes; configured maximum is {}",
                    limits.maximum_search_bytes
                ),
            ));
        }
        Ok(Self {
            bytes,
            controller: None,
        })
    }
}

impl Drop for CudaSearchReservation {
    fn drop(&mut self) {
        if let Some(controller) = &self.controller
            && let Ok(mut state) = controller.state.lock()
        {
            state.active = state.active.saturating_sub(1);
            state.reserved_bytes = state.reserved_bytes.saturating_sub(self.bytes);
        }
    }
}

#[derive(Debug, Default)]
struct AdmissionState {
    active: usize,
    reserved_bytes: usize,
    peak_active: usize,
    peak_reserved_bytes: usize,
    rejections: u64,
}

#[derive(Debug)]
struct AdmissionInner {
    limits: CudaMemoryLimits,
    state: Mutex<AdmissionState>,
}

#[derive(Clone)]
pub struct CudaAdmissionController {
    inner: Arc<AdmissionInner>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CudaAdmissionSnapshot {
    pub active: usize,
    pub reserved_bytes: usize,
    pub peak_active: usize,
    pub peak_reserved_bytes: usize,
    pub rejections: u64,
}

impl CudaAdmissionController {
    #[must_use]
    pub fn new(limits: CudaMemoryLimits) -> Self {
        Self {
            inner: Arc::new(AdmissionInner {
                limits,
                state: Mutex::new(AdmissionState::default()),
            }),
        }
    }

    pub fn reserve(&self, bytes: usize) -> Result<CudaSearchReservation, CudaError> {
        let mut state = self.inner.state.lock().map_err(|_| {
            CudaError::new(CudaFailureKind::Worker, "CUDA admission lock is poisoned")
        })?;
        if state.active >= self.inner.limits.maximum_concurrent_searches {
            state.rejections = state.rejections.saturating_add(1);
            return Err(CudaError::new(
                CudaFailureKind::Admission,
                format!(
                    "maximum {} concurrent CUDA searches are active",
                    self.inner.limits.maximum_concurrent_searches
                ),
            ));
        }
        let total = state.reserved_bytes.checked_add(bytes).ok_or_else(|| {
            CudaError::new(
                CudaFailureKind::Admission,
                "CUDA reservation total overflow",
            )
        })?;
        if total > self.inner.limits.maximum_search_bytes {
            state.rejections = state.rejections.saturating_add(1);
            return Err(CudaError::new(
                CudaFailureKind::Admission,
                format!(
                    "CUDA searches would reserve {total} bytes; configured total is {}",
                    self.inner.limits.maximum_search_bytes
                ),
            ));
        }
        state.active += 1;
        state.reserved_bytes = total;
        state.peak_active = state.peak_active.max(state.active);
        state.peak_reserved_bytes = state.peak_reserved_bytes.max(total);
        Ok(CudaSearchReservation {
            bytes,
            controller: Some(Arc::clone(&self.inner)),
        })
    }

    pub fn snapshot(&self) -> Result<CudaAdmissionSnapshot, CudaError> {
        let state = self.inner.state.lock().map_err(|_| {
            CudaError::new(CudaFailureKind::Worker, "CUDA admission lock is poisoned")
        })?;
        Ok(CudaAdmissionSnapshot {
            active: state.active,
            reserved_bytes: state.reserved_bytes,
            peak_active: state.peak_active,
            peak_reserved_bytes: state.peak_reserved_bytes,
            rejections: state.rejections,
        })
    }
}

pub fn estimate_search_bytes(
    node_count: usize,
    relation_count: usize,
    simultaneously_compacted_adjacencies: usize,
    destination_count: usize,
    algorithm: CudaAlgorithm,
) -> Result<usize, CudaError> {
    estimate_search_bytes_for_request(
        node_count,
        relation_count,
        simultaneously_compacted_adjacencies,
        destination_count,
        algorithm,
        None,
    )
}

pub fn estimate_search_bytes_for_request(
    node_count: usize,
    relation_count: usize,
    simultaneously_compacted_adjacencies: usize,
    destination_count: usize,
    algorithm: CudaAlgorithm,
    cpu_evidence_working_bytes: Option<usize>,
) -> Result<usize, CudaError> {
    // This deliberately covers the worst resident/partitioned overlap: host
    // initialization and control arrays, device distance/frontier/removed
    // arrays, one downloaded distance image, and delta bucket state. Topology
    // cache/staging has its own admission controller.
    let per_node = match algorithm {
        CudaAlgorithm::Frontier => 64,
        CudaAlgorithm::DeltaStepping(_) => 72,
    };
    node_count
        .checked_mul(per_node)
        .and_then(|bytes| bytes.checked_add(relation_count.checked_mul(8)?))
        // Worst overlap while task compaction uploads one u64 edge ordinal
        // plus one u32 source per edge: host and device copies coexist. A
        // resident image passes its complete adjacency count. A partitioned
        // image passes the largest single partition because partition-local
        // task buffers are synchronized and released before the next upload.
        .and_then(|bytes| bytes.checked_add(simultaneously_compacted_adjacencies.checked_mul(24)?))
        .and_then(|bytes| bytes.checked_add(destination_count.checked_mul(64)?))
        .and_then(|bytes| bytes.checked_add(cpu_evidence_working_bytes.unwrap_or(0)))
        .and_then(|bytes| bytes.checked_add(512))
        .ok_or_else(|| {
            CudaError::new(
                CudaFailureKind::Admission,
                "CUDA search byte accounting overflow",
            )
        })
}
