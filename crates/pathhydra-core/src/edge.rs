use crate::{BaseWeight, EdgeId, NodeId, RelationId};

/// One confirmed, typed, directed, weighted edge.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EdgeRecord {
    id: EdgeId,
    source: NodeId,
    destination: NodeId,
    relation_kind: RelationId,
    base_weight: BaseWeight,
}

impl EdgeRecord {
    #[must_use]
    pub const fn new(
        id: EdgeId,
        source: NodeId,
        destination: NodeId,
        relation_kind: RelationId,
        base_weight: BaseWeight,
    ) -> Self {
        Self {
            id,
            source,
            destination,
            relation_kind,
            base_weight,
        }
    }

    #[must_use]
    pub const fn id(&self) -> EdgeId {
        self.id
    }

    #[must_use]
    pub const fn source(&self) -> NodeId {
        self.source
    }

    #[must_use]
    pub const fn destination(&self) -> NodeId {
        self.destination
    }

    #[must_use]
    pub const fn relation_kind(&self) -> RelationId {
        self.relation_kind
    }

    #[must_use]
    pub const fn base_weight(&self) -> BaseWeight {
        self.base_weight
    }
}
