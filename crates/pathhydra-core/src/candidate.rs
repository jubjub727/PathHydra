use crate::{
    BaseWeight, CandidateId, EdgeRecord, NodeId, NodeName, NodePayload, RelationId, RelationName,
};

/// Stable identity used by a provisional edge endpoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CandidateNodeReference {
    Confirmed(NodeId),
    Candidate(CandidateId),
}

/// Stable identity used by a provisional edge relation kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CandidateRelationReference {
    Confirmed(RelationId),
    Candidate(CandidateId),
}

/// Stored proposed graph material that is not visible through confirmed lookup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Candidate {
    Node {
        id: CandidateId,
        name: NodeName,
        payload: NodePayload,
        incoming_reference_count: u64,
    },
    Relation {
        id: CandidateId,
        name: RelationName,
        incoming_reference_count: u64,
    },
    Edge {
        id: CandidateId,
        source: CandidateNodeReference,
        destination: CandidateNodeReference,
        relation_kind: CandidateRelationReference,
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
    provisional_reference_count: u64,
    confirmed_edge_count: u64,
}

impl RelationRecord {
    #[must_use]
    pub fn new(id: RelationId, name: RelationName) -> Self {
        Self::with_usage(id, name, 0, 0)
    }

    #[must_use]
    pub fn with_usage(
        id: RelationId,
        name: RelationName,
        provisional_reference_count: u64,
        confirmed_edge_count: u64,
    ) -> Self {
        Self {
            id,
            name,
            provisional_reference_count,
            confirmed_edge_count,
        }
    }

    #[must_use]
    pub const fn id(&self) -> RelationId {
        self.id
    }

    #[must_use]
    pub const fn name(&self) -> &RelationName {
        &self.name
    }

    #[must_use]
    pub const fn provisional_reference_count(&self) -> u64 {
        self.provisional_reference_count
    }

    #[must_use]
    pub const fn confirmed_edge_count(&self) -> u64 {
        self.confirmed_edge_count
    }

    #[must_use]
    pub fn total_reference_count(&self) -> Option<u64> {
        self.provisional_reference_count
            .checked_add(self.confirmed_edge_count)
    }
}

/// The stable record created by confirming a candidate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfirmedRecord {
    Node(NodeRecord),
    Relation(RelationRecord),
    Edge(EdgeRecord),
}
