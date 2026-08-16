use std::{
    sync::{
        Condvar, Mutex,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
    time::{Duration, Instant},
};

use crate::{CudaError, CudaFailureKind};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum CudaFaultStage {
    HostAllocation = 1,
    PinnedAllocation = 2,
    DeviceAllocation = 3,
    Copy = 4,
    Launch = 5,
    Event = 6,
    Synchronization = 7,
    ContextLoss = 8,
    ParallelScratch = 9,
    TaskCompaction = 10,
    PathEvidence = 11,
}

#[doc(hidden)]
#[derive(Debug, Default)]
pub struct CudaFaultInjection {
    stage: AtomicU8,
    hold_completion_events: AtomicBool,
    task_compaction: Mutex<TaskCompactionControl>,
    task_compaction_changed: Condvar,
    path_evidence: Mutex<PathEvidenceControl>,
    path_evidence_changed: Condvar,
}

#[derive(Debug, Default)]
struct PathEvidenceControl {
    held: bool,
    entered: bool,
}

#[derive(Debug, Default)]
struct TaskCompactionControl {
    held: bool,
    entered: bool,
}

impl CudaFaultInjection {
    pub fn reset(&self) {
        self.stage.store(0, Ordering::Release);
        self.hold_completion_events.store(false, Ordering::Release);
        if let Ok(mut state) = self.task_compaction.lock() {
            state.held = false;
            state.entered = false;
            self.task_compaction_changed.notify_all();
        }
        if let Ok(mut state) = self.path_evidence.lock() {
            state.held = false;
            state.entered = false;
            self.path_evidence_changed.notify_all();
        }
    }

    pub fn fail_at(&self, stage: CudaFaultStage) {
        self.stage.store(stage as u8, Ordering::Release);
    }

    #[doc(hidden)]
    pub fn hold_completion_events(&self, hold: bool) {
        self.hold_completion_events.store(hold, Ordering::Release);
    }

    /// Deterministic test control for cancellation after task-compaction
    /// admission and before the potentially large host scan.
    #[doc(hidden)]
    pub fn hold_task_compaction(&self, hold: bool) {
        if let Ok(mut state) = self.task_compaction.lock() {
            state.held = hold;
            if hold {
                state.entered = false;
            }
            self.task_compaction_changed.notify_all();
        }
    }

    /// Waits until a route has entered the held task-compaction boundary.
    #[doc(hidden)]
    pub fn wait_for_task_compaction(&self, timeout: Duration) -> bool {
        let Ok(mut state) = self.task_compaction.lock() else {
            return false;
        };
        let deadline = Instant::now() + timeout;
        while !state.entered {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            let Ok((next, result)) = self.task_compaction_changed.wait_timeout(state, remaining)
            else {
                return false;
            };
            state = next;
            if result.timed_out() && !state.entered {
                return false;
            }
        }
        true
    }

    /// Deterministic test control for cancellation during the CPU evidence
    /// stage. Releasing the hold wakes the route without a timing-only sleep.
    #[doc(hidden)]
    pub fn hold_path_evidence(&self, hold: bool) {
        if let Ok(mut state) = self.path_evidence.lock() {
            state.held = hold;
            if hold {
                state.entered = false;
            }
            self.path_evidence_changed.notify_all();
        }
    }

    /// Waits until a route has entered the held path-evidence boundary.
    #[doc(hidden)]
    pub fn wait_for_path_evidence(&self, timeout: Duration) -> bool {
        let Ok(mut state) = self.path_evidence.lock() else {
            return false;
        };
        let deadline = Instant::now() + timeout;
        while !state.entered {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            let Ok((next, result)) = self.path_evidence_changed.wait_timeout(state, remaining)
            else {
                return false;
            };
            state = next;
            if result.timed_out() && !state.entered {
                return false;
            }
        }
        true
    }

    #[cfg(feature = "cuda")]
    pub(crate) fn completion_events_held(&self) -> bool {
        self.hold_completion_events.load(Ordering::Acquire)
    }

    #[cfg(feature = "cuda")]
    pub(crate) fn pause_task_compaction(
        &self,
        cancellation: &AtomicBool,
    ) -> Result<bool, CudaError> {
        let mut state = self.task_compaction.lock().map_err(|_| {
            CudaError::new(
                CudaFailureKind::Worker,
                "task-compaction fault-control lock is poisoned",
            )
        })?;
        state.entered = true;
        self.task_compaction_changed.notify_all();
        while state.held && !cancellation.load(Ordering::Acquire) {
            let (next, _) = self
                .task_compaction_changed
                .wait_timeout(state, Duration::from_millis(10))
                .map_err(|_| {
                    CudaError::new(
                        CudaFailureKind::Worker,
                        "task-compaction fault-control lock is poisoned",
                    )
                })?;
            state = next;
        }
        Ok(!cancellation.load(Ordering::Acquire))
    }

    #[cfg(feature = "cuda")]
    pub(crate) fn pause_path_evidence(&self) -> Result<(), CudaError> {
        let mut state = self.path_evidence.lock().map_err(|_| {
            CudaError::new(
                CudaFailureKind::Worker,
                "path-evidence fault-control lock is poisoned",
            )
        })?;
        state.entered = true;
        self.path_evidence_changed.notify_all();
        while state.held {
            state = self.path_evidence_changed.wait(state).map_err(|_| {
                CudaError::new(
                    CudaFailureKind::Worker,
                    "path-evidence fault-control lock is poisoned",
                )
            })?;
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub(crate) fn trip(&self, stage: CudaFaultStage) -> Result<(), CudaError> {
        if self
            .stage
            .compare_exchange(stage as u8, 0, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            let kind = match stage {
                CudaFaultStage::HostAllocation
                | CudaFaultStage::PinnedAllocation
                | CudaFaultStage::DeviceAllocation
                | CudaFaultStage::ParallelScratch
                | CudaFaultStage::TaskCompaction => CudaFailureKind::Allocation,
                CudaFaultStage::Copy => CudaFailureKind::Upload,
                CudaFaultStage::Launch | CudaFaultStage::Event => CudaFailureKind::Launch,
                CudaFaultStage::Synchronization => CudaFailureKind::Synchronization,
                CudaFaultStage::ContextLoss => CudaFailureKind::DeviceLost,
                CudaFaultStage::PathEvidence => CudaFailureKind::KernelInvariant,
            };
            Err(CudaError::new(
                kind,
                format!("injected CUDA failure at {stage:?}"),
            ))
        } else {
            Ok(())
        }
    }
}
