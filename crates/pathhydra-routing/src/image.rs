use std::{mem::size_of, ops::Range};

use pathhydra_core::{BaseWeight, EdgeId, NodeId, RelationId};
use pathhydra_store::ConfirmedGraphRecords;

use crate::{CompileError, compile::compile_routing_image};

pub const NUMERIC_POLICY_ID: &str = "binary32-operands-separate-binary64-v1";
pub const TIE_POLICY_ID: &str = "distance-dense-node-stable-predecessor-v1";

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DenseNodeId(u32);

impl DenseNodeId {
    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self.0
    }

    pub(crate) const fn from_u32(value: u32) -> Self {
        Self(value)
    }

    pub(crate) fn as_usize(self) -> usize {
        usize::try_from(self.0).expect("compiled dense node IDs always fit usize")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ElementWidths {
    pub dense_node_id: usize,
    pub node_id: usize,
    pub offset: usize,
    pub relation_id: usize,
    pub base_weight: usize,
    pub edge_id: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImageByteCounts {
    pub external_to_dense: usize,
    pub dense_to_external: usize,
    pub offsets: usize,
    pub destinations: usize,
    pub relation_ids: usize,
    pub base_weights: usize,
    pub edge_ids: usize,
    pub confirmed_relation_ids: usize,
}

impl ImageByteCounts {
    #[must_use]
    pub const fn total(self) -> usize {
        self.external_to_dense
            + self.dense_to_external
            + self.offsets
            + self.destinations
            + self.relation_ids
            + self.base_weights
            + self.edge_ids
            + self.confirmed_relation_ids
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoutingImageManifest {
    numeric_policy: &'static str,
    tie_policy: &'static str,
    node_count: usize,
    relation_kind_count: usize,
    adjacency_count: usize,
    element_widths: ElementWidths,
    byte_counts: ImageByteCounts,
    predecessor_edge_ids_present: bool,
}

impl RoutingImageManifest {
    #[must_use]
    pub const fn numeric_policy(&self) -> &'static str {
        self.numeric_policy
    }
    #[must_use]
    pub const fn tie_policy(&self) -> &'static str {
        self.tie_policy
    }
    #[must_use]
    pub const fn node_count(&self) -> usize {
        self.node_count
    }
    #[must_use]
    pub const fn relation_kind_count(&self) -> usize {
        self.relation_kind_count
    }
    #[must_use]
    pub const fn adjacency_count(&self) -> usize {
        self.adjacency_count
    }
    #[must_use]
    pub const fn element_widths(&self) -> ElementWidths {
        self.element_widths
    }
    #[must_use]
    pub const fn byte_counts(&self) -> ImageByteCounts {
        self.byte_counts
    }
    #[must_use]
    pub const fn predecessor_edge_ids_present(&self) -> bool {
        self.predecessor_edge_ids_present
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OutgoingEdge {
    edge_id: EdgeId,
    destination: DenseNodeId,
    relation_id: RelationId,
    base_weight: BaseWeight,
}

impl OutgoingEdge {
    #[must_use]
    pub const fn edge_id(self) -> EdgeId {
        self.edge_id
    }
    #[must_use]
    pub const fn destination(self) -> DenseNodeId {
        self.destination
    }
    #[must_use]
    pub const fn relation_id(self) -> RelationId {
        self.relation_id
    }
    #[must_use]
    pub const fn base_weight(self) -> BaseWeight {
        self.base_weight
    }
}

#[derive(Clone, Debug)]
pub struct RoutingImage {
    pub(crate) external_to_dense: Box<[(NodeId, DenseNodeId)]>,
    pub(crate) dense_to_external: Box<[NodeId]>,
    pub(crate) offsets: Box<[u64]>,
    pub(crate) destinations: Box<[DenseNodeId]>,
    pub(crate) relation_ids: Box<[RelationId]>,
    pub(crate) base_weights: Box<[BaseWeight]>,
    pub(crate) edge_ids: Box<[EdgeId]>,
    pub(crate) confirmed_relation_ids: Box<[RelationId]>,
    pub(crate) manifest: RoutingImageManifest,
}

impl RoutingImage {
    pub fn compile(records: &ConfirmedGraphRecords) -> Result<Self, CompileError> {
        compile_routing_image(records)
    }

    #[must_use]
    pub const fn manifest(&self) -> &RoutingImageManifest {
        &self.manifest
    }
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.dense_to_external.len()
    }
    #[must_use]
    pub fn relation_kind_count(&self) -> usize {
        self.confirmed_relation_ids.len()
    }
    #[must_use]
    pub fn adjacency_count(&self) -> usize {
        self.edge_ids.len()
    }

    #[must_use]
    pub fn dense_node_id(&self, external: NodeId) -> Option<DenseNodeId> {
        self.external_to_dense
            .binary_search_by_key(&external, |(id, _)| *id)
            .ok()
            .map(|index| self.external_to_dense[index].1)
    }

    #[must_use]
    pub fn external_node_id(&self, dense: DenseNodeId) -> Option<NodeId> {
        self.dense_to_external.get(dense.as_usize()).copied()
    }

    #[must_use]
    pub fn confirmed_relation_ids(&self) -> &[RelationId] {
        &self.confirmed_relation_ids
    }

    pub fn outgoing_edges(
        &self,
        source: DenseNodeId,
    ) -> Option<impl ExactSizeIterator<Item = OutgoingEdge> + '_> {
        let range = self.outgoing_range(source)?;
        Some(range.map(|index| self.edge_at(index)))
    }

    pub(crate) fn relation_index(&self, id: RelationId) -> Option<usize> {
        self.confirmed_relation_ids.binary_search(&id).ok()
    }

    pub(crate) fn outgoing_range(&self, source: DenseNodeId) -> Option<Range<usize>> {
        let node = source.as_usize();
        let start = usize::try_from(*self.offsets.get(node)?).ok()?;
        let end = usize::try_from(*self.offsets.get(node + 1)?).ok()?;
        Some(start..end)
    }

    pub(crate) fn edge_at(&self, index: usize) -> OutgoingEdge {
        OutgoingEdge {
            edge_id: self.edge_ids[index],
            destination: self.destinations[index],
            relation_id: self.relation_ids[index],
            base_weight: self.base_weights[index],
        }
    }
}

pub(crate) fn manifest(
    node_count: usize,
    relation_kind_count: usize,
    adjacency_count: usize,
) -> Result<RoutingImageManifest, CompileError> {
    fn bytes(count: usize, width: usize) -> Result<usize, CompileError> {
        count
            .checked_mul(width)
            .ok_or(CompileError::CountOverflow { structure: "byte" })
    }
    let offset_count = node_count
        .checked_add(1)
        .ok_or(CompileError::CountOverflow {
            structure: "offset",
        })?;
    let byte_counts = ImageByteCounts {
        external_to_dense: bytes(node_count, size_of::<(NodeId, DenseNodeId)>())?,
        dense_to_external: bytes(node_count, size_of::<NodeId>())?,
        offsets: bytes(offset_count, size_of::<u64>())?,
        destinations: bytes(adjacency_count, size_of::<DenseNodeId>())?,
        relation_ids: bytes(adjacency_count, size_of::<RelationId>())?,
        base_weights: bytes(adjacency_count, size_of::<BaseWeight>())?,
        edge_ids: bytes(adjacency_count, size_of::<EdgeId>())?,
        confirmed_relation_ids: bytes(relation_kind_count, size_of::<RelationId>())?,
    };
    Ok(RoutingImageManifest {
        numeric_policy: NUMERIC_POLICY_ID,
        tie_policy: TIE_POLICY_ID,
        node_count,
        relation_kind_count,
        adjacency_count,
        element_widths: ElementWidths {
            dense_node_id: size_of::<DenseNodeId>(),
            node_id: size_of::<NodeId>(),
            offset: size_of::<u64>(),
            relation_id: size_of::<RelationId>(),
            base_weight: size_of::<BaseWeight>(),
            edge_id: size_of::<EdgeId>(),
        },
        byte_counts,
        predecessor_edge_ids_present: true,
    })
}
