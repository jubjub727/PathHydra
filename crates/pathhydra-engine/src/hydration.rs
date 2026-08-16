use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
};

use pathhydra_core::{EdgeId, EdgeRecord, NodeId, NodeRecord, RelationRecord};
use pathhydra_routing::{
    DestinationState, NumericPolicy, RelationMultiplier, RelationProfile, RelationUse,
    RoutingResponse, TiePolicy, accumulate_distance, effective_weight,
};
use pathhydra_subgraph::Subgraph;

use crate::{EngineError, GraphEngine};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HydrationError {
    HandleLimit {
        requested: usize,
        maximum: usize,
    },
    InvalidProfile(String),
    RoutingUnavailable(crate::RoutingUnavailableReason),
    AllocationFailed,
    DestinationPositionOutOfBounds {
        position: usize,
    },
    DestinationHasNoPath {
        position: usize,
    },
    HydrationUnavailable {
        node_ids: Vec<NodeId>,
        edge_ids: Vec<EdgeId>,
    },
    Integrity {
        reason: String,
    },
}

impl fmt::Display for HydrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HandleLimit { requested, maximum } => write!(
                formatter,
                "hydration has {requested} handles; maximum is {maximum}"
            ),
            Self::InvalidProfile(reason) => {
                write!(formatter, "invalid hydration profile: {reason}")
            }
            Self::RoutingUnavailable(reason) => {
                write!(formatter, "profile validation requires routing: {reason}")
            }
            Self::AllocationFailed => formatter.write_str("hydration allocation failed"),
            Self::DestinationPositionOutOfBounds { position } => write!(
                formatter,
                "destination position {position} is out of bounds"
            ),
            Self::DestinationHasNoPath { position } => write!(
                formatter,
                "destination position {position} has no exact returned path"
            ),
            Self::HydrationUnavailable { .. } => formatter
                .write_str("current confirmed records cannot hydrate every requested path handle"),
            Self::Integrity { reason } => {
                write!(formatter, "hydration integrity failure: {reason}")
            }
        }
    }
}
impl Error for HydrationError {}

#[derive(Clone, Debug)]
pub struct HydrationRequest {
    node_ids: Box<[NodeId]>,
    edge_ids: Box<[EdgeId]>,
    profile: Option<RelationProfile>,
}
impl HydrationRequest {
    #[must_use]
    pub fn new(
        node_ids: impl IntoIterator<Item = NodeId>,
        edge_ids: impl IntoIterator<Item = EdgeId>,
        profile: Option<RelationProfile>,
    ) -> Self {
        Self {
            node_ids: node_ids.into_iter().collect(),
            edge_ids: edge_ids.into_iter().collect(),
            profile,
        }
    }
    #[must_use]
    pub fn node_ids(&self) -> &[NodeId] {
        &self.node_ids
    }
    #[must_use]
    pub fn edge_ids(&self) -> &[EdgeId] {
        &self.edge_ids
    }
    #[must_use]
    pub const fn profile(&self) -> Option<&RelationProfile> {
        self.profile.as_ref()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HydratedNodeState {
    Found(NodeRecord),
    Missing,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HydratedNodeResult {
    pub requested_node_id: NodeId,
    pub state: HydratedNodeState,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EdgeEvaluation {
    Unprofiled,
    Disabled,
    Enabled {
        multiplier: RelationMultiplier,
        effective_weight: f64,
    },
}
#[derive(Clone, Debug, PartialEq)]
pub struct HydratedEdge {
    pub edge: EdgeRecord,
    pub relation_kind: RelationRecord,
    pub evaluation: EdgeEvaluation,
}
#[derive(Clone, Debug, PartialEq)]
pub enum HydratedEdgeState {
    Found(HydratedEdge),
    Missing,
}
#[derive(Clone, Debug, PartialEq)]
pub struct HydratedEdgeResult {
    pub requested_edge_id: EdgeId,
    pub state: HydratedEdgeState,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HydrationResponse {
    pub nodes: Box<[HydratedNodeResult]>,
    pub edges: Box<[HydratedEdgeResult]>,
    pub profile: Option<RelationProfile>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HydratedPath {
    pub nodes: Box<[NodeRecord]>,
    pub edges: Box<[HydratedEdge]>,
    pub logical_distance: f64,
    pub numeric_policy: NumericPolicy,
    pub tie_policy: TiePolicy,
    pub profile: RelationProfile,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HydratedSubgraph {
    pub nodes: Box<[NodeRecord]>,
    pub edges: Box<[HydratedEdge]>,
    pub missing_node_ids: Box<[NodeId]>,
    pub missing_edge_ids: Box<[EdgeId]>,
    pub profile: Option<RelationProfile>,
    pub complete: bool,
}

impl GraphEngine {
    pub fn hydrate(&self, request: &HydrationRequest) -> Result<HydrationResponse, EngineError> {
        let _operation = self.lifecycle.begin_route()?;
        let handle_count = request
            .node_ids
            .len()
            .checked_add(request.edge_ids.len())
            .ok_or(HydrationError::AllocationFailed)?;
        if handle_count > self.config.max_hydration_handles_per_request {
            return Err(HydrationError::HandleLimit {
                requested: handle_count,
                maximum: self.config.max_hydration_handles_per_request,
            }
            .into());
        }
        let published = self.routing.read().map_err(|_| EngineError::LockPoisoned {
            lock: "published routing state",
        })?;
        let canonical_profile = if let Some(profile) = &request.profile {
            let image = published
                .image()
                .map_err(HydrationError::RoutingUnavailable)?;
            Some(
                profile
                    .pack(&image)
                    .map_err(|error| HydrationError::InvalidProfile(error.to_string()))?
                    .canonical()
                    .clone(),
            )
        } else {
            None
        };
        let batch = self.with_catalog(|catalog| {
            Ok(catalog.confirmed_records_by_id(&request.node_ids, &request.edge_ids)?)
        })?;
        let mut nodes = try_vec(request.node_ids.len())?;
        for &id in &request.node_ids {
            let state = batch
                .node(id)
                .cloned()
                .map_or(HydratedNodeState::Missing, HydratedNodeState::Found);
            nodes.push(HydratedNodeResult {
                requested_node_id: id,
                state,
            });
        }
        let uses = canonical_profile.as_ref().map(profile_uses).transpose()?;
        let mut edges = try_vec(request.edge_ids.len())?;
        for &id in &request.edge_ids {
            let state = match batch.edge(id) {
                None => HydratedEdgeState::Missing,
                Some(edge) => HydratedEdgeState::Found(hydrate_edge(edge, &batch, uses.as_ref())?),
            };
            edges.push(HydratedEdgeResult {
                requested_edge_id: id,
                state,
            });
        }
        drop(published);
        Ok(HydrationResponse {
            nodes: nodes.into_boxed_slice(),
            edges: edges.into_boxed_slice(),
            profile: canonical_profile,
        })
    }

    pub fn hydrate_path(
        &self,
        response: &RoutingResponse,
        destination_position: usize,
    ) -> Result<HydratedPath, EngineError> {
        let _operation = self.lifecycle.begin_route()?;
        let destination = response.results().get(destination_position).ok_or(
            HydrationError::DestinationPositionOutOfBounds {
                position: destination_position,
            },
        )?;
        let DestinationState::Exact(exact) = destination.state() else {
            return Err(HydrationError::DestinationHasNoPath {
                position: destination_position,
            }
            .into());
        };
        let path = exact.path().ok_or(HydrationError::DestinationHasNoPath {
            position: destination_position,
        })?;
        let mut node_ids = try_vec(path.steps().len().saturating_add(1))?;
        node_ids.push(path.origin());
        node_ids.extend(path.steps().iter().map(|step| step.destination()));
        let edge_ids: Vec<_> = path.steps().iter().map(|step| step.edge_id()).collect();
        self.check_hydration_limit(node_ids.len(), edge_ids.len())?;
        let _published = self.routing.read().map_err(|_| EngineError::LockPoisoned {
            lock: "published routing state",
        })?;
        let batch = self
            .with_catalog(|catalog| Ok(catalog.confirmed_records_by_id(&node_ids, &edge_ids)?))?;
        let missing_nodes: Vec<_> = node_ids
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter(|id| batch.node(*id).is_none())
            .collect();
        let missing_edges: Vec<_> = edge_ids
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter(|id| batch.edge(*id).is_none())
            .collect();
        if !missing_nodes.is_empty() || !missing_edges.is_empty() {
            return Err(HydrationError::HydrationUnavailable {
                node_ids: missing_nodes,
                edge_ids: missing_edges,
            }
            .into());
        }
        let mut hydrated_nodes = try_vec(node_ids.len())?;
        for id in node_ids {
            hydrated_nodes.push(batch.node(id).expect("checked above").clone());
        }
        let mut hydrated_edges = try_vec(path.steps().len())?;
        let mut sum = 0.0;
        for step in path.steps() {
            let edge = batch.edge(step.edge_id()).expect("checked above");
            if edge.source() != step.source()
                || edge.destination() != step.destination()
                || edge.relation_kind() != step.relation_id()
                || edge.base_weight() != step.base_weight()
            {
                return Err(HydrationError::Integrity {
                    reason: format!("edge {} no longer matches route evidence", step.edge_id()),
                }
                .into());
            }
            let effective = effective_weight(edge.base_weight(), step.multiplier())
                .map_err(pathhydra_routing::RoutingError::Arithmetic)?;
            if effective != step.effective_weight() {
                return Err(HydrationError::Integrity {
                    reason: format!("edge {} effective weight differs", step.edge_id()),
                }
                .into());
            }
            sum = accumulate_distance(sum, effective)
                .map_err(pathhydra_routing::RoutingError::Arithmetic)?;
            hydrated_edges.push(HydratedEdge {
                edge: edge.clone(),
                relation_kind: batch
                    .relation_kind(edge.relation_kind())
                    .expect("batch validates relation kinds")
                    .clone(),
                evaluation: EdgeEvaluation::Enabled {
                    multiplier: step.multiplier(),
                    effective_weight: effective,
                },
            });
        }
        if sum != exact.logical_distance() || sum != path.logical_distance() {
            return Err(HydrationError::Integrity {
                reason: "path steps do not reproduce logical distance".into(),
            }
            .into());
        }
        Ok(HydratedPath {
            nodes: hydrated_nodes.into_boxed_slice(),
            edges: hydrated_edges.into_boxed_slice(),
            logical_distance: sum,
            numeric_policy: response.numeric_policy(),
            tie_policy: response.tie_policy(),
            profile: response.profile().clone(),
        })
    }

    pub fn hydrate_subgraph(
        &self,
        subgraph: &Subgraph,
        profile: Option<RelationProfile>,
    ) -> Result<HydratedSubgraph, EngineError> {
        let _operation = self.lifecycle.begin_route()?;
        let handles = subgraph.handles();
        let response = self.hydrate(&HydrationRequest::new(
            handles.nodes().iter().copied(),
            handles.edges().iter().map(|edge| edge.edge_id()),
            profile,
        ))?;
        let mut nodes = Vec::new();
        let mut missing_nodes = Vec::new();
        for result in response.nodes {
            match result.state {
                HydratedNodeState::Found(node) => nodes.push(node),
                HydratedNodeState::Missing => missing_nodes.push(result.requested_node_id),
            }
        }
        let evidence: BTreeMap<_, _> = handles
            .edges()
            .iter()
            .map(|edge| (edge.edge_id(), (edge.source(), edge.destination())))
            .collect();
        let mut edges = Vec::new();
        let mut missing_edges = Vec::new();
        for result in response.edges {
            match result.state {
                HydratedEdgeState::Missing => missing_edges.push(result.requested_edge_id),
                HydratedEdgeState::Found(edge) => {
                    let endpoints = evidence[&result.requested_edge_id];
                    if (edge.edge.source(), edge.edge.destination()) != endpoints {
                        return Err(HydrationError::Integrity {
                            reason: format!(
                                "edge {} endpoint evidence differs",
                                result.requested_edge_id
                            ),
                        }
                        .into());
                    }
                    edges.push(edge);
                }
            }
        }
        let complete = missing_nodes.is_empty() && missing_edges.is_empty();
        Ok(HydratedSubgraph {
            nodes: nodes.into_boxed_slice(),
            edges: edges.into_boxed_slice(),
            missing_node_ids: missing_nodes.into_boxed_slice(),
            missing_edge_ids: missing_edges.into_boxed_slice(),
            profile: response.profile,
            complete,
        })
    }

    fn check_hydration_limit(&self, nodes: usize, edges: usize) -> Result<(), EngineError> {
        let requested = nodes
            .checked_add(edges)
            .ok_or(HydrationError::AllocationFailed)?;
        if requested > self.config.max_hydration_handles_per_request {
            return Err(HydrationError::HandleLimit {
                requested,
                maximum: self.config.max_hydration_handles_per_request,
            }
            .into());
        }
        Ok(())
    }
}

fn try_vec<T>(capacity: usize) -> Result<Vec<T>, HydrationError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(capacity)
        .map_err(|_| HydrationError::AllocationFailed)?;
    Ok(values)
}

fn profile_uses(
    profile: &RelationProfile,
) -> Result<BTreeMap<pathhydra_core::RelationId, RelationUse>, HydrationError> {
    Ok(profile.entries().iter().copied().collect())
}

fn hydrate_edge(
    edge: &EdgeRecord,
    batch: &pathhydra_store::ConfirmedRecordBatch,
    uses: Option<&BTreeMap<pathhydra_core::RelationId, RelationUse>>,
) -> Result<HydratedEdge, EngineError> {
    let relation_kind = batch
        .relation_kind(edge.relation_kind())
        .ok_or_else(|| HydrationError::Integrity {
            reason: format!("edge {} relation kind is unavailable", edge.id()),
        })?
        .clone();
    let evaluation = match uses {
        None => EdgeEvaluation::Unprofiled,
        Some(uses) => match uses.get(&edge.relation_kind()).copied().ok_or_else(|| {
            HydrationError::Integrity {
                reason: format!("profile lacks edge {} relation kind", edge.id()),
            }
        })? {
            RelationUse::Disabled => EdgeEvaluation::Disabled,
            RelationUse::Enabled(multiplier) => EdgeEvaluation::Enabled {
                multiplier,
                effective_weight: effective_weight(edge.base_weight(), multiplier)
                    .map_err(pathhydra_routing::RoutingError::Arithmetic)?,
            },
        },
    };
    Ok(HydratedEdge {
        edge: edge.clone(),
        relation_kind,
        evaluation,
    })
}
