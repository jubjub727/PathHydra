use std::mem::size_of;

use pathhydra_cuda::KernelStatus;

#[test]
fn fixed_width_kernel_abi_has_stable_layout() {
    assert_eq!(size_of::<KernelStatus>(), 4);
    assert_eq!(KernelStatus::Success as u32, 0);
    assert_eq!(KernelStatus::BucketUnrepresentable as u32, 6);
}
