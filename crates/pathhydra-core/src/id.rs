use std::fmt;

macro_rules! durable_id {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(u64);

        impl $name {
            /// Creates an ID from its durable integer representation.
            #[must_use]
            pub const fn from_u64(value: u64) -> Self {
                Self(value)
            }

            /// Returns the durable integer representation.
            #[must_use]
            pub const fn as_u64(self) -> u64 {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

durable_id!(CandidateId);
durable_id!(NodeId);
durable_id!(RelationId);
