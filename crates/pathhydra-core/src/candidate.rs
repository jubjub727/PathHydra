use crate::{CandidateId, NodeId, NodeName, RelationId, RelationName};

/// Stored proposed graph material that is not visible through confirmed lookup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Candidate {
    Node { id: CandidateId, name: NodeName },
    Relation { id: CandidateId, name: RelationName },
}

impl Candidate {
    #[must_use]
    pub const fn id(&self) -> CandidateId {
        match self {
            Self::Node { id, .. } | Self::Relation { id, .. } => *id,
        }
    }
}

/// A confirmed node identity record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NodeRecord {
    id: NodeId,
    name: NodeName,
}

impl NodeRecord {
    #[must_use]
    pub fn new(id: NodeId, name: NodeName) -> Self {
        Self { id, name }
    }

    #[must_use]
    pub const fn id(&self) -> NodeId {
        self.id
    }

    #[must_use]
    pub const fn name(&self) -> &NodeName {
        &self.name
    }
}

/// A confirmed relation-kind identity record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelationRecord {
    id: RelationId,
    name: RelationName,
}

impl RelationRecord {
    #[must_use]
    pub fn new(id: RelationId, name: RelationName) -> Self {
        Self { id, name }
    }

    #[must_use]
    pub const fn id(&self) -> RelationId {
        self.id
    }

    #[must_use]
    pub const fn name(&self) -> &RelationName {
        &self.name
    }
}

/// The stable record created by confirming a candidate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfirmedRecord {
    Node(NodeRecord),
    Relation(RelationRecord),
}
