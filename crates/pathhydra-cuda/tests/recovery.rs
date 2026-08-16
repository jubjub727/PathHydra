#![cfg(feature = "cuda")]

use pathhydra_cuda::CudaContextOwner;

#[test]
fn a_fresh_context_reloads_ptx_after_the_old_context_drops() {
    let expected = [1.5_f64];
    for _ in 0..2 {
        let context = CudaContextOwner::initialize(0).unwrap();
        let actual = context.smoke_arithmetic(&[1.0], &[0.5], &[1.0]).unwrap();
        assert_eq!(actual[0].to_bits(), expected[0].to_bits());
    }
}
