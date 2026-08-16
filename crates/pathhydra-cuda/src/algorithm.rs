#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CudaAlgorithm {
    Frontier,
    DeltaStepping(DeltaConfiguration),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeltaConfiguration {
    delta_bits: u64,
}

impl DeltaConfiguration {
    pub fn new(delta: f64) -> Option<Self> {
        (delta.is_finite() && delta > 0.0).then_some(Self {
            delta_bits: delta.to_bits(),
        })
    }

    #[must_use]
    pub const fn delta(self) -> f64 {
        f64::from_bits(self.delta_bits)
    }
}
