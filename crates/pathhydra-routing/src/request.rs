use pathhydra_core::{BaseWeight, EdgeId, NodeId, RelationId};

use crate::{RelationMultiplier, RelationProfile};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchBudget {
    Unlimited,
    ExaminedEdges(u64),
}

impl SearchBudget {
    pub(crate) const fn permits(self, examined: u64) -> bool {
        match self {
            Self::Unlimited => true,
            Self::ExaminedEdges(maximum) => examined < maximum,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TiePolicy {
    #[default]
    StablePredecessor,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NumericPolicy {
    Binary32OperandsSeparateBinary64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoutingRequest {
    origin: NodeId,
    destinations: Box<[NodeId]>,
    profile: RelationProfile,
    return_paths: bool,
    budget: SearchBudget,
    tie_policy: TiePolicy,
}

impl RoutingRequest {
    #[must_use]
    pub fn new(
        origin: NodeId,
        destinations: impl IntoIterator<Item = NodeId>,
        profile: RelationProfile,
        return_paths: bool,
        budget: SearchBudget,
        tie_policy: TiePolicy,
    ) -> Self {
        Self {
            origin,
            destinations: destinations
                .into_iter()
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            profile,
            return_paths,
            budget,
            tie_policy,
        }
    }

    #[must_use]
    pub const fn origin(&self) -> NodeId {
        self.origin
    }
    #[must_use]
    pub fn destinations(&self) -> &[NodeId] {
        &self.destinations
    }
    #[must_use]
    pub const fn profile(&self) -> &RelationProfile {
        &self.profile
    }
    #[must_use]
    pub const fn return_paths(&self) -> bool {
        self.return_paths
    }
    #[must_use]
    pub const fn budget(&self) -> SearchBudget {
        self.budget
    }
    #[must_use]
    pub const fn tie_policy(&self) -> TiePolicy {
        self.tie_policy
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompletionReason {
    AllDestinationsFinalized,
    FrontierExhausted,
    BudgetExhausted,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PathStep {
    edge_id: EdgeId,
    source: NodeId,
    destination: NodeId,
    relation_id: RelationId,
    base_weight: BaseWeight,
    multiplier: RelationMultiplier,
    effective_weight: f64,
}

impl PathStep {
    #[must_use]
    pub const fn edge_id(&self) -> EdgeId {
        self.edge_id
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
    pub const fn relation_id(&self) -> RelationId {
        self.relation_id
    }
    #[must_use]
    pub const fn base_weight(&self) -> BaseWeight {
        self.base_weight
    }
    #[must_use]
    pub const fn multiplier(&self) -> RelationMultiplier {
        self.multiplier
    }
    #[must_use]
    pub const fn effective_weight(&self) -> f64 {
        self.effective_weight
    }

    pub(crate) const fn new(
        edge_id: EdgeId,
        source: NodeId,
        destination: NodeId,
        relation_id: RelationId,
        base_weight: BaseWeight,
        multiplier: RelationMultiplier,
        effective_weight: f64,
    ) -> Self {
        Self {
            edge_id,
            source,
            destination,
            relation_id,
            base_weight,
            multiplier,
            effective_weight,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RoutePath {
    origin: NodeId,
    destination: NodeId,
    logical_distance: f64,
    steps: Box<[PathStep]>,
}

impl RoutePath {
    #[must_use]
    pub const fn origin(&self) -> NodeId {
        self.origin
    }
    #[must_use]
    pub const fn destination(&self) -> NodeId {
        self.destination
    }
    #[must_use]
    pub const fn logical_distance(&self) -> f64 {
        self.logical_distance
    }
    #[must_use]
    pub fn steps(&self) -> &[PathStep] {
        &self.steps
    }

    pub(crate) fn new(
        origin: NodeId,
        destination: NodeId,
        logical_distance: f64,
        steps: Vec<PathStep>,
    ) -> Self {
        Self {
            origin,
            destination,
            logical_distance,
            steps: steps.into_boxed_slice(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExactRoute {
    logical_distance: f64,
    path: Option<RoutePath>,
}

impl ExactRoute {
    #[must_use]
    pub const fn logical_distance(&self) -> f64 {
        self.logical_distance
    }
    #[must_use]
    pub const fn path(&self) -> Option<&RoutePath> {
        self.path.as_ref()
    }

    pub(crate) const fn new(logical_distance: f64, path: Option<RoutePath>) -> Self {
        Self {
            logical_distance,
            path,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum DestinationState {
    Exact(ExactRoute),
    Unreachable,
    MissingNode,
    Incomplete,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DestinationResult {
    destination: NodeId,
    state: DestinationState,
}

impl DestinationResult {
    #[must_use]
    pub const fn destination(&self) -> NodeId {
        self.destination
    }
    #[must_use]
    pub const fn state(&self) -> &DestinationState {
        &self.state
    }

    pub(crate) const fn new(destination: NodeId, state: DestinationState) -> Self {
        Self { destination, state }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RoutingResponse {
    origin: NodeId,
    results: Box<[DestinationResult]>,
    profile: RelationProfile,
    numeric_policy: NumericPolicy,
    tie_policy: TiePolicy,
    paths_requested: bool,
    examined_edges: u64,
    finalized_nodes: u64,
    completion_reason: CompletionReason,
}

impl RoutingResponse {
    #[must_use]
    pub const fn origin(&self) -> NodeId {
        self.origin
    }
    #[must_use]
    pub fn results(&self) -> &[DestinationResult] {
        &self.results
    }
    #[must_use]
    pub const fn profile(&self) -> &RelationProfile {
        &self.profile
    }
    #[must_use]
    pub const fn numeric_policy(&self) -> NumericPolicy {
        self.numeric_policy
    }
    #[must_use]
    pub const fn tie_policy(&self) -> TiePolicy {
        self.tie_policy
    }
    #[must_use]
    pub const fn paths_requested(&self) -> bool {
        self.paths_requested
    }
    #[must_use]
    pub const fn examined_edges(&self) -> u64 {
        self.examined_edges
    }
    #[must_use]
    pub const fn finalized_nodes(&self) -> u64 {
        self.finalized_nodes
    }
    #[must_use]
    pub const fn completion_reason(&self) -> CompletionReason {
        self.completion_reason
    }

    pub(crate) fn new(
        origin: NodeId,
        results: Vec<DestinationResult>,
        profile: RelationProfile,
        tie_policy: TiePolicy,
        paths_requested: bool,
        counts: (u64, u64),
        completion_reason: CompletionReason,
    ) -> Self {
        Self {
            origin,
            results: results.into_boxed_slice(),
            profile,
            numeric_policy: NumericPolicy::Binary32OperandsSeparateBinary64,
            tie_policy,
            paths_requested,
            examined_edges: counts.0,
            finalized_nodes: counts.1,
            completion_reason,
        }
    }
}
