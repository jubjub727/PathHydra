use std::sync::atomic::{AtomicU8, Ordering};

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
}

impl CudaFaultInjection {
    pub fn reset(&self) {
        self.stage.store(0, Ordering::Release);
    }

    pub fn fail_at(&self, stage: CudaFaultStage) {
        self.stage.store(stage as u8, Ordering::Release);
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
