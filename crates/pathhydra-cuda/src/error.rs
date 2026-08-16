use std::{error::Error, fmt};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CudaFailureKind {
    Unavailable,
    Incompatible,
    Ptx,
    Context,
    Module,
    Allocation,
    Upload,
    ImageAccess,
    Admission,
    Launch,
    Synchronization,
    KernelInvariant,
    DeviceLost,
    Worker,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CudaError {
    kind: CudaFailureKind,
    message: String,
    poisons_context: bool,
}

impl CudaError {
    #[must_use]
    pub fn new(kind: CudaFailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            poisons_context: matches!(
                kind,
                CudaFailureKind::DeviceLost
                    | CudaFailureKind::Launch
                    | CudaFailureKind::Synchronization
            ),
        }
    }

    #[must_use]
    pub fn ptx(message: impl Into<String>) -> Self {
        Self::new(CudaFailureKind::Ptx, message)
    }

    #[must_use]
    pub const fn kind(&self) -> CudaFailureKind {
        self.kind
    }

    #[must_use]
    pub const fn poisons_context(&self) -> bool {
        self.poisons_context
    }
}

impl fmt::Display for CudaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for CudaError {}
