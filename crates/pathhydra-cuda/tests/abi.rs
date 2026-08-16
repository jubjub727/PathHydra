use std::mem::{align_of, size_of};

use pathhydra_cuda::{KernelDiagnostics, KernelParameters, KernelStatus};

#[test]
fn fixed_width_kernel_abi_has_stable_layout() {
    assert_eq!(size_of::<KernelParameters>(), 32);
    assert_eq!(align_of::<KernelParameters>(), 8);
    assert_eq!(size_of::<KernelDiagnostics>(), 40);
    assert_eq!(align_of::<KernelDiagnostics>(), 8);
    assert_eq!(size_of::<KernelStatus>(), 4);
}
