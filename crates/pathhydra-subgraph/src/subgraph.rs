use std::collections::{BTreeMap, BTreeSet};

use pathhydra_core::{EdgeId, EdgeRecord, NodeId};
use pathhydra_routing::RoutePath;

use crate::SubgraphError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EdgeHandle {
    edge_id: EdgeId,
    source: NodeId,
    destination: NodeId,
}

impl EdgeHandle {
    #[must_use]
    pub const fn new(edge_id: EdgeId, source: NodeId, destination: NodeId) -> Self {
        Self {
            edge_id,
            source,
            destination,
        }
    }
    #[must_use]
    pub const fn edge_id(self) -> EdgeId {
        self.edge_id
    }
    #[must_use]
    pub const fn source(self) -> NodeId {
        self.source
    }
    #[must_use]
    pub const fn destination(self) -> NodeId {
        self.destination
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SubgraphHandles {
    nodes: Box<[NodeId]>,
    edges: Box<[EdgeHandle]>,
}

impl SubgraphHandles {
    #[must_use]
    pub fn nodes(&self) -> &[NodeId] {
        &self.nodes
    }
    #[must_use]
    pub fn edges(&self) -> &[EdgeHandle] {
        &self.edges
    }
}

/// A deterministic caller-owned collection of graph handles.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Subgraph {
    nodes: BTreeSet<NodeId>,
    edges: BTreeMap<EdgeId, (NodeId, NodeId)>,
}

impl Subgraph {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            nodes: BTreeSet::new(),
            edges: BTreeMap::new(),
        }
    }

    pub fn add_node(&mut self, node: NodeId) -> bool {
        self.nodes.insert(node)
    }

    pub fn add_edge(
        &mut self,
        edge: EdgeId,
        source: NodeId,
        destination: NodeId,
    ) -> Result<bool, SubgraphError> {
        self.validate_edge(edge, source, destination)?;
        let inserted = self.edges.insert(edge, (source, destination)).is_none();
        self.nodes.insert(source);
        self.nodes.insert(destination);
        Ok(inserted)
    }

    pub fn add_edge_record(&mut self, edge: &EdgeRecord) -> Result<bool, SubgraphError> {
        self.add_edge(edge.id(), edge.source(), edge.destination())
    }

    pub fn add_path(&mut self, path: &RoutePath) -> Result<(), SubgraphError> {
        let steps = path.steps();
        if steps.is_empty() && path.origin() != path.destination() {
            return Err(SubgraphError::InvalidPath {
                reason: "a nontrivial path has no steps",
            });
        }
        if let Some(first) = steps.first().filter(|step| step.source() != path.origin()) {
            let _ = first;
            return Err(SubgraphError::InvalidPath {
                reason: "first step does not start at the path origin",
            });
        }
        if let Some(last) = steps
            .last()
            .filter(|step| step.destination() != path.destination())
        {
            let _ = last;
            return Err(SubgraphError::InvalidPath {
                reason: "last step does not end at the path destination",
            });
        }
        if steps
            .windows(2)
            .any(|pair| pair[0].destination() != pair[1].source())
        {
            return Err(SubgraphError::InvalidPath {
                reason: "step endpoints are discontinuous",
            });
        }
        for step in steps {
            self.validate_edge(step.edge_id(), step.source(), step.destination())?;
        }
        self.nodes.insert(path.origin());
        for step in steps {
            self.edges
                .insert(step.edge_id(), (step.source(), step.destination()));
            self.nodes.insert(step.source());
            self.nodes.insert(step.destination());
        }
        Ok(())
    }

    pub fn union(&mut self, other: &Self) -> Result<(), SubgraphError> {
        for (&edge, &(source, destination)) in &other.edges {
            self.validate_edge(edge, source, destination)?;
        }
        self.nodes.extend(other.nodes.iter().copied());
        self.edges
            .extend(other.edges.iter().map(|(&id, &endpoints)| (id, endpoints)));
        Ok(())
    }

    pub fn remove_edge(&mut self, edge: EdgeId) -> bool {
        self.edges.remove(&edge).is_some()
    }

    pub fn remove_node(&mut self, node: NodeId) -> bool {
        let removed = self.nodes.remove(&node);
        self.edges
            .retain(|_, (source, destination)| *source != node && *destination != node);
        removed
    }

    #[must_use]
    pub fn contains_node(&self, node: NodeId) -> bool {
        self.nodes.contains(&node)
    }
    #[must_use]
    pub fn contains_edge(&self, edge: EdgeId) -> bool {
        self.edges.contains_key(&edge)
    }
    pub fn nodes(&self) -> impl ExactSizeIterator<Item = NodeId> + '_ {
        self.nodes.iter().copied()
    }
    pub fn edges(&self) -> impl ExactSizeIterator<Item = EdgeHandle> + '_ {
        self.edges
            .iter()
            .map(|(&id, &(source, destination))| EdgeHandle::new(id, source, destination))
    }
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
    #[must_use]
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    #[must_use]
    pub fn handles(&self) -> SubgraphHandles {
        SubgraphHandles {
            nodes: self.nodes().collect::<Vec<_>>().into_boxed_slice(),
            edges: self.edges().collect::<Vec<_>>().into_boxed_slice(),
        }
    }

    fn validate_edge(
        &self,
        edge: EdgeId,
        source: NodeId,
        destination: NodeId,
    ) -> Result<(), SubgraphError> {
        if let Some(&(existing_source, existing_destination)) = self.edges.get(&edge)
            && (existing_source, existing_destination) != (source, destination)
        {
            return Err(SubgraphError::EdgeIdentityConflict {
                edge,
                existing_source,
                existing_destination,
                proposed_source: source,
                proposed_destination: destination,
            });
        }
        Ok(())
    }
}
