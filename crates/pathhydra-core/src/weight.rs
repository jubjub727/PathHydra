use std::{error::Error, fmt};

/// A canonical IEEE-754 binary32 stored edge weight in the inclusive range
/// `0.0..=1.0`.
///
/// Zero is the closest possible logical distance and one is the furthest.
///
/// Negative zero is accepted and canonicalized to positive zero. NaN,
/// infinities, and values below zero are rejected.
#[derive(Clone, Copy, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BaseWeight(u32);

impl BaseWeight {
    pub const MIN: Self = Self(0.0_f32.to_bits());
    pub const MAX: Self = Self(1.0_f32.to_bits());

    pub fn new(value: f32) -> Result<Self, InvalidBaseWeight> {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(InvalidBaseWeight { value });
        }
        let canonical = if value == 0.0 { 0.0 } else { value };
        Ok(Self(canonical.to_bits()))
    }

    /// Rebuilds a weight from its durable IEEE-754 bits and validates them.
    pub fn from_bits(bits: u32) -> Result<Self, InvalidBaseWeight> {
        let value = f32::from_bits(bits);
        let canonical = Self::new(value)?;
        if canonical.to_bits() == bits {
            Ok(canonical)
        } else {
            Err(InvalidBaseWeight { value })
        }
    }

    #[must_use]
    pub const fn get(self) -> f32 {
        f32::from_bits(self.0)
    }

    #[must_use]
    pub const fn to_bits(self) -> u32 {
        self.0
    }
}

impl fmt::Debug for BaseWeight {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("BaseWeight")
            .field(&self.get())
            .finish()
    }
}

impl fmt::Display for BaseWeight {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.get().fmt(formatter)
    }
}

impl TryFrom<f32> for BaseWeight {
    type Error = InvalidBaseWeight;

    fn try_from(value: f32) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<BaseWeight> for f32 {
    fn from(value: BaseWeight) -> Self {
        value.get()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InvalidBaseWeight {
    value: f32,
}

impl InvalidBaseWeight {
    #[must_use]
    pub const fn value(self) -> f32 {
        self.value
    }
}

impl fmt::Display for InvalidBaseWeight {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "base weight {} is not finite and within 0.0..=1.0",
            self.value
        )
    }
}

impl Error for InvalidBaseWeight {}

#[cfg(test)]
mod tests {
    use super::BaseWeight;

    #[test]
    fn validates_and_canonicalizes_values() {
        assert_eq!(BaseWeight::new(-0.0).unwrap().to_bits(), 0);
        assert!(BaseWeight::from_bits((-0.0_f32).to_bits()).is_err());
        assert_eq!(BaseWeight::new(0.0).unwrap(), BaseWeight::MIN);
        assert_eq!(BaseWeight::new(1.0).unwrap(), BaseWeight::MAX);
        for invalid in [-1.0, 1.000_001, f32::NEG_INFINITY, f32::INFINITY, f32::NAN] {
            assert!(BaseWeight::new(invalid).is_err());
        }
    }
}
