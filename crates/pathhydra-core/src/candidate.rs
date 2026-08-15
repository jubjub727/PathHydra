use crate::{
    BaseWeight, CandidateId, EdgeRecord, NodeId, NodeName, NodePayload, RelationId, RelationName,
};

/// Stored proposed graph material that is not visible through confirmed lookup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Candidate {
    Node {
        id: CandidateId,
        name: NodeName,
        payload: NodePayload,
    },
    Relation {
        id: CandidateId,
        name: RelationName,
    },
    Edge {
        id: CandidateId,
        source: NodeId,
        destination: NodeId,
        relation_kind: RelationId,
        base_weight: BaseWeight,
    },
}

impl Candidate {
    #[must_use]
    pub const fn id(&self) -> CandidateId {
        match self {
            Self::Node { id, .. } | Self::Relation { id, .. } | Self::Edge { id, .. } => *id,
        }
    }
}

/// A confirmed node identity record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NodeRecord {
    id: NodeId,
    name: NodeName,
    payload: NodePayload,
}

impl NodeRecord {
    #[must_use]
    pub fn new(id: NodeId, name: NodeName, payload: NodePayload) -> Self {
        Self { id, name, payload }
    }

    #[must_use]
    pub const fn id(&self) -> NodeId {
        self.id
    }

    #[must_use]
    pub const fn name(&self) -> &NodeName {
        &self.name
    }

    #[must_use]
    pub const fn payload(&self) -> &NodePayload {
        &self.payload
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
    Edge(EdgeRecord),
}
