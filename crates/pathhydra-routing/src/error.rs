use std::{error::Error, fmt};

use pathhydra_core::{EdgeId, NodeId, RelationId};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CompileError {
    TooManyNodes { count: usize },
    DuplicateNodeId(NodeId),
    DuplicateRelationId(RelationId),
    DuplicateEdgeId(EdgeId),
    MissingEndpoint { edge: EdgeId, node: NodeId },
    MissingRelationKind { edge: EdgeId, relation: RelationId },
    InvalidBaseWeight { edge: EdgeId },
    CountOverflow { structure: &'static str },
    TopologyLimitExceeded { required: usize, limit: usize },
    AllocationFailed { structure: &'static str },
    InvalidImage { reason: &'static str },
}

impl fmt::Display for CompileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooManyNodes { count } => write!(
                formatter,
                "{count} nodes cannot be represented by the routing image's u32 dense IDs"
            ),
            Self::DuplicateNodeId(id) => write!(formatter, "duplicate node ID {id}"),
            Self::DuplicateRelationId(id) => write!(formatter, "duplicate relation ID {id}"),
            Self::DuplicateEdgeId(id) => write!(formatter, "duplicate edge ID {id}"),
            Self::MissingEndpoint { edge, node } => {
                write!(formatter, "edge {edge} names missing endpoint node {node}")
            }
            Self::MissingRelationKind { edge, relation } => {
                write!(
                    formatter,
                    "edge {edge} names missing relation kind {relation}"
                )
            }
            Self::InvalidBaseWeight { edge } => {
                write!(formatter, "edge {edge} has a noncanonical base weight")
            }
            Self::CountOverflow { structure } => {
                write!(formatter, "routing image {structure} count overflow")
            }
            Self::TopologyLimitExceeded { required, limit } => write!(
                formatter,
                "routing image requires {required} logical payload bytes; limit is {limit}"
            ),
            Self::AllocationFailed { structure } => {
                write!(formatter, "routing image could not reserve {structure}")
            }
            Self::InvalidImage { reason } => write!(formatter, "invalid routing image: {reason}"),
        }
    }
}

impl Error for CompileError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProfileError {
    DuplicateRelation(RelationId),
    MissingRelation(RelationId),
    UnknownRelation(RelationId),
    AllocationFailed,
}

impl fmt::Display for ProfileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateRelation(id) => write!(formatter, "duplicate profile entry for {id}"),
            Self::MissingRelation(id) => write!(formatter, "missing profile entry for {id}"),
            Self::UnknownRelation(id) => {
                write!(formatter, "profile names unconfirmed relation {id}")
            }
            Self::AllocationFailed => formatter.write_str("profile allocation failed"),
        }
    }
}

impl Error for ProfileError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArithmeticOperation {
    EffectiveWeight,
    PathAddition,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArithmeticError {
    operation: ArithmeticOperation,
}

impl ArithmeticError {
    #[must_use]
    pub const fn operation(self) -> ArithmeticOperation {
        self.operation
    }

    pub(crate) const fn new(operation: ArithmeticOperation) -> Self {
        Self { operation }
    }
}

impl fmt::Display for ArithmeticError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "non-finite {:?} routing arithmetic",
            self.operation
        )
    }
}

impl Error for ArithmeticError {}

#[derive(Debug)]
pub enum RoutingError {
    MissingOrigin(NodeId),
    InvalidProfile(ProfileError),
    Arithmetic(ArithmeticError),
    ResourceEstimateOverflow,
    AllocationFailed { structure: &'static str },
    InternalInvariant { reason: &'static str },
}

impl fmt::Display for RoutingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingOrigin(id) => write!(formatter, "origin node {id} is not in the image"),
            Self::InvalidProfile(error) => error.fmt(formatter),
            Self::Arithmetic(error) => error.fmt(formatter),
            Self::ResourceEstimateOverflow => {
                formatter.write_str("CPU routing working-set estimate overflow")
            }
            Self::AllocationFailed { structure } => {
                write!(formatter, "CPU routing could not reserve {structure}")
            }
            Self::InternalInvariant { reason } => {
                write!(formatter, "routing internal invariant failed: {reason}")
            }
        }
    }
}

impl Error for RoutingError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidProfile(error) => Some(error),
            Self::Arithmetic(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ProfileError> for RoutingError {
    fn from(value: ProfileError) -> Self {
        Self::InvalidProfile(value)
    }
}

impl From<ArithmeticError> for RoutingError {
    fn from(value: ArithmeticError) -> Self {
        Self::Arithmetic(value)
    }
}
