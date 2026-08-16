//! Concrete NVIDIA CUDA execution for PathHydra routing images.

#![deny(unsafe_code)]
#![deny(unsafe_op_in_unsafe_fn)]

mod abi;
mod algorithm;
#[cfg(feature = "cuda")]
mod context;
#[cfg(feature = "cuda")]
mod device;
mod error;
#[cfg(feature = "cuda")]
#[allow(unsafe_code)]
mod launch;
mod memory;
#[cfg(feature = "cuda")]
mod resident;
mod scheduler;

pub use abi::{KernelDiagnostics, KernelParameters, KernelStatus};
pub use algorithm::{CudaAlgorithm, DeltaConfiguration};
#[cfg(feature = "cuda")]
pub use context::CudaContextOwner;
#[cfg(feature = "cuda")]
pub use device::{CudaDeviceCapabilities, probe_device};
pub use error::{CudaError, CudaFailureKind};
pub use memory::{
    CudaAdmissionController, CudaAdmissionSnapshot, CudaMemoryLimits, CudaSearchReservation,
    estimate_search_bytes,
};
#[cfg(feature = "cuda")]
pub use resident::CudaResidentImage;
pub use scheduler::{CudaRouteDiagnostics, CudaRouteOutput};
#[cfg(feature = "cuda")]
pub use scheduler::{CudaWorker, CudaWorkerSnapshot};

pub const KERNEL_PTX_TARGET: &str = "sm_86";
pub const KERNEL_ABI_ID: &str = "pathhydra-cuda-abi-v1";

#[cfg(feature = "cuda")]
#[must_use]
pub fn embedded_ptx() -> &'static str {
    include_str!(concat!(env!("OUT_DIR"), "/pathhydra.ptx"))
}

#[cfg(feature = "cuda")]
pub fn validate_embedded_ptx() -> Result<(), CudaError> {
    let ptx = embedded_ptx();
    for required in [
        ".target sm_86",
        ".address_size 64",
        ".visible .entry pathhydra_smoke_arithmetic",
        ".visible .entry pathhydra_validate_topology",
        ".visible .entry pathhydra_frontier_route",
        ".visible .entry pathhydra_delta_route",
        "mul.rn.f64",
        "add.rn.f64",
    ] {
        if !ptx.contains(required) {
            return Err(CudaError::ptx(format!(
                "generated PTX is missing required declaration or instruction {required}"
            )));
        }
    }
    if ptx.contains("fma.rn.f64") || ptx.contains(".extern .func") {
        return Err(CudaError::ptx(
            "generated PTX contains fused arithmetic or an external function dependency",
        ));
    }
    Ok(())
}
