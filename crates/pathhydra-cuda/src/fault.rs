use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

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
}

#[doc(hidden)]
#[derive(Default)]
pub struct CudaFaultInjection {
    stage: AtomicU8,
    hold_completion_events: AtomicBool,
}

impl CudaFaultInjection {
    pub fn reset(&self) {
        self.stage.store(0, Ordering::Release);
        self.hold_completion_events.store(false, Ordering::Release);
    }

    pub fn fail_at(&self, stage: CudaFaultStage) {
        self.stage.store(stage as u8, Ordering::Release);
    }

    #[doc(hidden)]
    pub fn hold_completion_events(&self, hold: bool) {
        self.hold_completion_events.store(hold, Ordering::Release);
    }

    #[cfg(feature = "cuda")]
    pub(crate) fn completion_events_held(&self) -> bool {
        self.hold_completion_events.load(Ordering::Acquire)
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
                | CudaFaultStage::DeviceAllocation => CudaFailureKind::Allocation,
                CudaFaultStage::Copy => CudaFailureKind::Upload,
                CudaFaultStage::Launch | CudaFaultStage::Event => CudaFailureKind::Launch,
                CudaFaultStage::Synchronization => CudaFailureKind::Synchronization,
                CudaFaultStage::ContextLoss => CudaFailureKind::DeviceLost,
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
