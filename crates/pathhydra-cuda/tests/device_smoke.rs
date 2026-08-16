#![cfg(feature = "cuda")]

use pathhydra_cuda::CudaContextOwner;

#[test]
fn rtx_3080_round_trips_separate_binary64_arithmetic_bit_exactly() {
    let context = CudaContextOwner::initialize(0).unwrap();
    let inputs = [0.0, f32::from_bits(1), 1.0, f32::MAX];
    let multipliers = [f32::MAX, f32::MAX, 1.0, f32::MAX];
    let addends = [0.0, 1.0, f64::from_bits(1), 1.0];
    let actual = context
        .smoke_arithmetic(&inputs, &multipliers, &addends)
        .unwrap();
    let expected: Vec<_> = inputs
        .into_iter()
        .zip(multipliers)
        .zip(addends)
        .map(|((value, multiplier), addend)| {
            let product = f64::from(value) * f64::from(multiplier);
            addend + product
        })
        .collect();
    assert_eq!(
        actual
            .iter()
            .map(|value| value.to_bits())
            .collect::<Vec<_>>(),
        expected
            .iter()
            .map(|value| value.to_bits())
            .collect::<Vec<_>>()
    );
    assert_eq!(context.capabilities().compute_capability, (8, 6));
    assert_eq!(
        context.capabilities().device_name,
        "NVIDIA GeForce RTX 3080"
    );
}
