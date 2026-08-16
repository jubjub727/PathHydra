use std::{error::Error, fmt};

use pathhydra_core::RelationId;

use crate::{ProfileError, RoutingImage};

#[derive(Clone, Copy, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RelationMultiplier(u32);

impl RelationMultiplier {
    pub fn new(value: f32) -> Result<Self, InvalidRelationMultiplier> {
        if !value.is_finite() || value < 0.0 {
            return Err(InvalidRelationMultiplier { value });
        }
        let canonical = if value == 0.0 { 0.0 } else { value };
        Ok(Self(canonical.to_bits()))
    }

    pub fn from_bits(bits: u32) -> Result<Self, InvalidRelationMultiplier> {
        let value = f32::from_bits(bits);
        let canonical = Self::new(value)?;
        if canonical.to_bits() == bits {
            Ok(canonical)
        } else {
            Err(InvalidRelationMultiplier { value })
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

impl fmt::Debug for RelationMultiplier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("RelationMultiplier")
            .field(&self.get())
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InvalidRelationMultiplier {
    value: f32,
}

impl InvalidRelationMultiplier {
    #[must_use]
    pub const fn value(self) -> f32 {
        self.value
    }
}

impl fmt::Display for InvalidRelationMultiplier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "relation multiplier {} is not finite and nonnegative",
            self.value
        )
    }
}

impl Error for InvalidRelationMultiplier {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelationUse {
    Disabled,
    Enabled(RelationMultiplier),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RelationProfile {
    entries: Box<[(RelationId, RelationUse)]>,
}

impl RelationProfile {
    #[must_use]
    pub fn new(entries: impl IntoIterator<Item = (RelationId, RelationUse)>) -> Self {
        Self {
            entries: entries.into_iter().collect::<Vec<_>>().into_boxed_slice(),
        }
    }

    #[must_use]
    pub fn entries(&self) -> &[(RelationId, RelationUse)] {
        &self.entries
    }

    pub fn pack(&self, image: &RoutingImage) -> Result<PackedRelationProfile, ProfileError> {
        let mut by_index = Vec::new();
        by_index
            .try_reserve_exact(image.relation_kind_count())
            .map_err(|_| ProfileError::AllocationFailed)?;
        by_index.resize(image.relation_kind_count(), None);
        for &(id, relation_use) in &self.entries {
            let index = image
                .relation_index(id)
                .ok_or(ProfileError::UnknownRelation(id))?;
            if by_index[index].replace(relation_use).is_some() {
                return Err(ProfileError::DuplicateRelation(id));
            }
        }
        let mut packed = Vec::new();
        packed
            .try_reserve_exact(image.relation_kind_count())
            .map_err(|_| ProfileError::AllocationFailed)?;
        let mut canonical = Vec::new();
        canonical
            .try_reserve_exact(image.relation_kind_count())
            .map_err(|_| ProfileError::AllocationFailed)?;
        for (index, &id) in image.confirmed_relation_ids().iter().enumerate() {
            let relation_use = by_index[index].ok_or(ProfileError::MissingRelation(id))?;
            packed.push(relation_use);
            canonical.push((id, relation_use));
        }
        Ok(PackedRelationProfile {
            uses: packed.into_boxed_slice(),
            canonical: RelationProfile::new(canonical),
        })
    }
}

#[derive(Clone, Debug)]
pub struct PackedRelationProfile {
    uses: Box<[RelationUse]>,
    canonical: RelationProfile,
}

impl PackedRelationProfile {
    #[must_use]
    pub const fn canonical(&self) -> &RelationProfile {
        &self.canonical
    }

    pub(crate) fn relation_use(&self, image: &RoutingImage, id: RelationId) -> Option<RelationUse> {
        image
            .relation_index(id)
            .and_then(|index| self.uses.get(index).copied())
    }
}

#[cfg(test)]
mod tests {
    use super::RelationMultiplier;

    #[test]
    fn multiplier_validates_and_preserves_exact_bits() {
        assert_eq!(RelationMultiplier::new(-0.0).unwrap().to_bits(), 0);
        assert!(RelationMultiplier::from_bits((-0.0_f32).to_bits()).is_err());
        for value in [0.0, f32::from_bits(1), 1.0, f32::MAX] {
            let multiplier = RelationMultiplier::new(value).unwrap();
            assert_eq!(multiplier.get().to_bits(), value.to_bits());
            assert_eq!(
                RelationMultiplier::from_bits(value.to_bits()).unwrap(),
                multiplier
            );
        }
        for value in [-1.0, f32::NEG_INFINITY, f32::INFINITY, f32::NAN] {
            assert!(RelationMultiplier::new(value).is_err());
        }
    }
}
