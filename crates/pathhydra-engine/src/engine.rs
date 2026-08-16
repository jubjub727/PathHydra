use std::{
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};

use pathhydra_core::{
    BaseWeight, Candidate, CandidateId, ConfirmedRecord, EdgeId, EdgeRecord, NodeId, NodeName,
    NodePayload, NodeRecord, RelationId, RelationName, RelationRecord,
};
use pathhydra_routing::{
    CompletionReason, CpuSearchDiagnostics, NumericPolicy, RoutingImage, RoutingRequest,
    RoutingResponse, TiePolicy, estimate_cpu_working_set, route_controlled,
};
use pathhydra_store::Catalog;

use crate::{
    AdmissionController, CancellationOutcome, ConfirmedMutation, EngineCapabilities,
    EngineConfigError, EngineError, EngineHealth, ImageBuildOutcome, ImageBuildReport,
    PublicationOutcome, RequestId, RequestRegistry, RoutingHealth, RoutingState,
    RoutingUnavailableReason,
};

pub const DEFAULT_MAX_ACTIVE_IMAGE_BYTES: usize = 512 * 1024 * 1024;
pub const DEFAULT_MAX_CONCURRENT_ROUTES: usize = 8;
pub const DEFAULT_MAX_RESERVED_ROUTE_BYTES: usize = 512 * 1024 * 1024;
pub const DEFAULT_MAX_DESTINATIONS_PER_REQUEST: usize = 16_384;
pub const DEFAULT_MAX_HYDRATION_HANDLES_PER_REQUEST: usize = 65_536;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EngineConfig {
    pub max_active_image_bytes: usize,
    pub max_concurrent_routes: usize,
    pub max_reserved_route_bytes: usize,
    pub max_destinations_per_request: usize,
    pub max_hydration_handles_per_request: usize,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            max_active_image_bytes: DEFAULT_MAX_ACTIVE_IMAGE_BYTES,
            max_concurrent_routes: DEFAULT_MAX_CONCURRENT_ROUTES,
            max_reserved_route_bytes: DEFAULT_MAX_RESERVED_ROUTE_BYTES,
            max_destinations_per_request: DEFAULT_MAX_DESTINATIONS_PER_REQUEST,
            max_hydration_handles_per_request: DEFAULT_MAX_HYDRATION_HANDLES_PER_REQUEST,
        }
    }
}

impl EngineConfig {
    pub fn validate(self) -> Result<Self, EngineConfigError> {
        for (name, value) in [
            ("max_concurrent_routes", self.max_concurrent_routes),
            ("max_reserved_route_bytes", self.max_reserved_route_bytes),
            (
                "max_destinations_per_request",
                self.max_destinations_per_request,
            ),
            (
                "max_hydration_handles_per_request",
                self.max_hydration_handles_per_request,
            ),
        ] {
            if value == 0 {
                return Err(EngineConfigError::ZeroLimit { limit: name });
            }
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Executor {
    CpuReference,
}

#[derive(Clone, Debug)]
pub struct RuntimeDiagnostics {
    pub request_id: RequestId,
    pub executor: Executor,
    pub numeric_policy: NumericPolicy,
    pub tie_policy: TiePolicy,
    pub image_node_count: usize,
    pub image_relation_kind_count: usize,
    pub image_adjacency_count: usize,
    pub reserved_working_bytes: usize,
    pub admission_duration: Duration,
    pub execution_duration: Duration,
    pub completion_reason: CompletionReason,
    pub search: CpuSearchDiagnostics,
}

#[derive(Clone, Debug)]
pub struct EngineRoutingResponse {
    pub response: RoutingResponse,
    pub diagnostics: RuntimeDiagnostics,
}

/// The sole owner of a durable catalog and its current published CPU image.
///
/// Open an engine and inspect concrete capabilities:
///
/// ```no_run
/// use pathhydra_engine::{EngineConfig, GraphEngine};
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let engine = GraphEngine::open("graph.db", EngineConfig::default())?;
/// assert!(engine.capabilities().cpu_reference_routing);
/// assert!(!engine.capabilities().gpu_routing);
/// # Ok(()) }
/// ```
///
/// Promotion reports publication separately from its durable result:
///
/// ```no_run
/// use pathhydra_engine::{EngineConfig, GraphEngine, PublicationOutcome};
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let engine = GraphEngine::open("graph.db", EngineConfig::default())?;
/// let candidate = engine.insert_node_candidate("exact name")?;
/// let mutation = engine.confirm_validated_candidate(candidate)?;
/// assert!(matches!(mutation.publication(), PublicationOutcome::Published(_)));
/// # Ok(()) }
/// ```
///
/// Callers may signal a synchronous route from another thread:
///
/// ```no_run
/// use std::sync::Arc;
/// use pathhydra_engine::{CancellationOutcome, EngineConfig, GraphEngine, RequestId};
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let engine = Arc::new(GraphEngine::open("graph.db", EngineConfig::default())?);
/// let id = RequestId::new(42);
/// // A caller-owned worker can invoke `engine.route(id, &request)`.
/// let outcome = engine.cancel(id)?;
/// assert!(matches!(outcome, CancellationOutcome::NotActive));
/// # Ok(()) }
/// ```
///
/// Exact returned paths are hydrated from their own response evidence:
///
/// ```no_run
/// # use pathhydra_engine::{EngineConfig, GraphEngine};
/// # use pathhydra_routing::RoutingResponse;
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// # let engine = GraphEngine::open("graph.db", EngineConfig::default())?;
/// # let response: RoutingResponse = unimplemented!();
/// let path = engine.hydrate_path(&response, 0)?;
/// println!("{}", path.logical_distance);
/// # Ok(()) }
/// ```
///
/// Caller-owned subgraphs can be unioned and hydrated without store mutation:
///
/// ```no_run
/// use pathhydra_core::{EdgeId, NodeId};
/// use pathhydra_engine::{EngineConfig, GraphEngine, Subgraph};
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let engine = GraphEngine::open("graph.db", EngineConfig::default())?;
/// let mut selected = Subgraph::new();
/// selected.add_edge(EdgeId::from_u64(1), NodeId::from_u64(1), NodeId::from_u64(2))?;
/// let hydrated = engine.hydrate_subgraph(&selected, None)?;
/// assert!(!hydrated.complete); // the example handles are not confirmed
/// # Ok(()) }
/// ```
///
/// A committed mutation can explicitly report an unavailable replacement:
///
/// ```no_run
/// use pathhydra_engine::{EngineConfig, GraphEngine, PublicationOutcome};
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let config = EngineConfig { max_active_image_bytes: 8, ..EngineConfig::default() };
/// let engine = GraphEngine::open("small.db", config)?;
/// let candidate = engine.insert_node_candidate("commits before publication")?;
/// let mutation = engine.confirm_validated_candidate(candidate)?;
/// assert!(matches!(mutation.publication(), PublicationOutcome::RoutingUnavailable(_, _)));
/// # Ok(()) }
/// ```
pub struct GraphEngine {
    pub(crate) catalog: Catalog,
    pub(crate) config: EngineConfig,
    pub(crate) routing: RwLock<RoutingState>,
    admission: AdmissionController,
    requests: RequestRegistry,
    cancellations: AtomicU64,
    image_build_failures: AtomicU64,
}

impl GraphEngine {
    pub fn open(path: impl AsRef<Path>, config: EngineConfig) -> Result<Self, EngineError> {
        let config = config.validate().map_err(EngineError::Configuration)?;
        let catalog = Catalog::open(path)?;
        let (routing, failed) =
            Self::initial_routing_state(&catalog, config.max_active_image_bytes)?;
        Ok(Self {
            catalog,
            config,
            routing: RwLock::new(routing),
            admission: AdmissionController::new(
                config.max_concurrent_routes,
                config.max_reserved_route_bytes,
            ),
            requests: RequestRegistry::new(),
            cancellations: AtomicU64::new(0),
            image_build_failures: AtomicU64::new(u64::from(failed)),
        })
    }

    #[must_use]
    pub fn capabilities(&self) -> EngineCapabilities {
        EngineCapabilities::new(self.config)
    }

    pub fn health(&self) -> Result<EngineHealth, EngineError> {
        let routing = self.routing.read().map_err(|_| EngineError::LockPoisoned {
            lock: "published routing state",
        })?;
        let admission = self.admission.snapshot()?;
        Ok(EngineHealth {
            durable_catalog_available: true,
            routing: routing
                .unavailable_reason()
                .cloned()
                .map_or(RoutingHealth::Available, RoutingHealth::Unavailable),
            current_image_manifest: routing.manifest().cloned(),
            current_image_age: routing.age(),
            last_image_build: routing.last_build().clone(),
            active_routes: admission.active,
            peak_active_routes: admission.peak_active,
            reserved_route_bytes: admission.reserved,
            peak_reserved_route_bytes: admission.peak_reserved,
            cumulative_route_admissions: admission.admissions,
            cumulative_admission_rejections: admission.rejections,
            cumulative_cancellations: self.cancellations.load(Ordering::Relaxed),
            cumulative_image_build_failures: self.image_build_failures.load(Ordering::Relaxed),
        })
    }

    pub fn rebuild_routing_image(&self) -> Result<ImageBuildReport, EngineError> {
        let mut state = self
            .routing
            .write()
            .map_err(|_| EngineError::LockPoisoned {
                lock: "published routing state",
            })?;
        let (result, report) = self.compile_current_image();
        match result {
            Ok(image) => {
                *state = RoutingState::Available {
                    image: Arc::new(image),
                    published_at: Instant::now(),
                    last_build: report.clone(),
                }
            }
            Err(reason) => {
                saturating_increment(&self.image_build_failures);
                match &mut *state {
                    RoutingState::Available { last_build, .. } => *last_build = report.clone(),
                    RoutingState::Unavailable {
                        reason: old_reason,
                        last_build,
                    } => {
                        *old_reason = reason;
                        *last_build = report.clone();
                    }
                }
            }
        }
        Ok(report)
    }

    pub fn route(
        &self,
        request_id: RequestId,
        request: &RoutingRequest,
    ) -> Result<EngineRoutingResponse, EngineError> {
        if request.destinations().len() > self.config.max_destinations_per_request {
            return Err(self.admission.reject_destinations(
                request.destinations().len(),
                self.config.max_destinations_per_request,
            )?);
        }
        let registration = self.requests.register(request_id)?;
        let admission_started = Instant::now();
        let mut permit = self.admission.acquire_slot()?;
        let image = {
            let state = self.routing.read().map_err(|_| EngineError::LockPoisoned {
                lock: "published routing state",
            })?;
            state.image().map_err(EngineError::RoutingUnavailable)?
        };
        let estimate = estimate_cpu_working_set(&image, request)?;
        permit.reserve(estimate.bytes())?;
        let admission_duration = admission_started.elapsed();
        let execution_started = Instant::now();
        let (response, search) = route_controlled(&image, request, registration.flag())?;
        let execution_duration = execution_started.elapsed();
        let manifest = image.manifest();
        let diagnostics = RuntimeDiagnostics {
            request_id,
            executor: Executor::CpuReference,
            numeric_policy: response.numeric_policy(),
            tie_policy: response.tie_policy(),
            image_node_count: manifest.node_count(),
            image_relation_kind_count: manifest.relation_kind_count(),
            image_adjacency_count: manifest.adjacency_count(),
            reserved_working_bytes: estimate.bytes(),
            admission_duration,
            execution_duration,
            completion_reason: response.completion_reason(),
            search,
        };
        Ok(EngineRoutingResponse {
            response,
            diagnostics,
        })
    }

    pub fn cancel(&self, request_id: RequestId) -> Result<CancellationOutcome, EngineError> {
        let outcome = self.requests.cancel(request_id)?;
        if outcome == CancellationOutcome::Signalled {
            saturating_increment(&self.cancellations);
        }
        Ok(outcome)
    }

    pub fn insert_node_candidate(
        &self,
        name: impl Into<NodeName>,
    ) -> Result<CandidateId, EngineError> {
        Ok(self.catalog.insert_node_candidate(name)?)
    }
    pub fn insert_node_candidate_with_payload(
        &self,
        name: impl Into<NodeName>,
        payload: impl Into<NodePayload>,
    ) -> Result<CandidateId, EngineError> {
        Ok(self
            .catalog
            .insert_node_candidate_with_payload(name, payload)?)
    }
    pub fn insert_relation_candidate(
        &self,
        name: impl Into<RelationName>,
    ) -> Result<CandidateId, EngineError> {
        Ok(self.catalog.insert_relation_candidate(name)?)
    }
    pub fn insert_edge_candidate(
        &self,
        source: NodeId,
        destination: NodeId,
        relation_kind: RelationId,
        base_weight: f32,
    ) -> Result<CandidateId, EngineError> {
        Ok(self
            .catalog
            .insert_edge_candidate(source, destination, relation_kind, base_weight)?)
    }
    pub fn insert_edge_candidate_with_base_weight(
        &self,
        source: NodeId,
        destination: NodeId,
        relation_kind: RelationId,
        base_weight: BaseWeight,
    ) -> Result<CandidateId, EngineError> {
        Ok(self.catalog.insert_edge_candidate_with_base_weight(
            source,
            destination,
            relation_kind,
            base_weight,
        )?)
    }
    pub fn get_candidate(&self, id: CandidateId) -> Result<Candidate, EngineError> {
        Ok(self.catalog.get_candidate(id)?)
    }
    pub fn lookup_node_exact(&self, name: &str) -> Result<Option<NodeId>, EngineError> {
        Ok(self.catalog.lookup_node_exact(name)?)
    }
    pub fn lookup_relation_exact(&self, name: &str) -> Result<Option<RelationId>, EngineError> {
        Ok(self.catalog.lookup_relation_exact(name)?)
    }
    pub fn get_node(&self, id: NodeId) -> Result<NodeRecord, EngineError> {
        Ok(self.catalog.get_node(id)?)
    }
    pub fn get_relation(&self, id: RelationId) -> Result<RelationRecord, EngineError> {
        Ok(self.catalog.get_relation(id)?)
    }
    pub fn get_edge(&self, id: EdgeId) -> Result<EdgeRecord, EngineError> {
        Ok(self.catalog.get_edge(id)?)
    }

    pub fn confirm_validated_candidate(
        &self,
        id: CandidateId,
    ) -> Result<ConfirmedMutation<ConfirmedRecord>, EngineError> {
        self.confirmed_mutation(|catalog| catalog.confirm_validated_candidate(id))
    }
    pub fn remove_edge(&self, id: EdgeId) -> Result<ConfirmedMutation<()>, EngineError> {
        self.confirmed_mutation(|catalog| catalog.remove_edge(id))
    }
    pub fn remove_node(&self, id: NodeId) -> Result<ConfirmedMutation<()>, EngineError> {
        self.confirmed_mutation(|catalog| catalog.remove_node(id))
    }

    fn confirmed_mutation<T>(
        &self,
        mutation: impl FnOnce(&Catalog) -> Result<T, pathhydra_store::CatalogError>,
    ) -> Result<ConfirmedMutation<T>, EngineError> {
        let mut state = self
            .routing
            .write()
            .map_err(|_| EngineError::LockPoisoned {
                lock: "published routing state",
            })?;
        let durable_result = mutation(&self.catalog)?;
        let (compiled, report) = self.compile_current_image();
        let publication = match compiled {
            Ok(image) => {
                *state = RoutingState::Available {
                    image: Arc::new(image),
                    published_at: Instant::now(),
                    last_build: report.clone(),
                };
                PublicationOutcome::Published(report)
            }
            Err(reason) => {
                saturating_increment(&self.image_build_failures);
                *state = RoutingState::Unavailable {
                    reason: reason.clone(),
                    last_build: report.clone(),
                };
                PublicationOutcome::RoutingUnavailable(reason, report)
            }
        };
        Ok(ConfirmedMutation::new(durable_result, publication))
    }

    fn initial_routing_state(
        catalog: &Catalog,
        limit: usize,
    ) -> Result<(RoutingState, bool), EngineError> {
        let started = Instant::now();
        let records = catalog.confirmed_graph_records()?;
        let counts = (
            records.nodes().len(),
            records.relation_kinds().len(),
            records.edges().len(),
        );
        match RoutingImage::compile_with_limit(&records, limit) {
            Ok(image) => {
                let report =
                    ImageBuildReport::new(started.elapsed(), ImageBuildOutcome::Published, counts);
                Ok((
                    RoutingState::Available {
                        image: Arc::new(image),
                        published_at: Instant::now(),
                        last_build: report,
                    },
                    false,
                ))
            }
            Err(error) => {
                let reason = unavailable_reason(error);
                let report = ImageBuildReport::new(
                    started.elapsed(),
                    ImageBuildOutcome::Failed(reason.clone()),
                    counts,
                );
                Ok((
                    RoutingState::Unavailable {
                        reason,
                        last_build: report,
                    },
                    true,
                ))
            }
        }
    }

    fn compile_current_image(
        &self,
    ) -> (
        Result<RoutingImage, RoutingUnavailableReason>,
        ImageBuildReport,
    ) {
        let started = Instant::now();
        let records = match self.catalog.confirmed_graph_records() {
            Ok(records) => records,
            Err(error) => {
                let reason = RoutingUnavailableReason::ImageCompilation(format!(
                    "confirmed graph capture failed after durable operation: {error}"
                ));
                let report = ImageBuildReport::new(
                    started.elapsed(),
                    ImageBuildOutcome::Failed(reason.clone()),
                    (0, 0, 0),
                );
                return (Err(reason), report);
            }
        };
        let counts = (
            records.nodes().len(),
            records.relation_kinds().len(),
            records.edges().len(),
        );
        let result = RoutingImage::compile_with_limit(&records, self.config.max_active_image_bytes)
            .map_err(unavailable_reason);
        let outcome = result
            .as_ref()
            .map(|_| ImageBuildOutcome::Published)
            .unwrap_or_else(|reason| ImageBuildOutcome::Failed(reason.clone()));
        let report = ImageBuildReport::new(started.elapsed(), outcome, counts);
        (result, report)
    }
}

fn unavailable_reason(error: pathhydra_routing::CompileError) -> RoutingUnavailableReason {
    match error {
        pathhydra_routing::CompileError::TopologyLimitExceeded { required, limit } => {
            RoutingUnavailableReason::TopologyLimit { required, limit }
        }
        other => RoutingUnavailableReason::ImageCompilation(other.to_string()),
    }
}

fn saturating_increment(counter: &AtomicU64) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
        Some(value.saturating_add(1))
    });
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pathhydra_core::ConfirmedRecord;
    use pathhydra_routing::{
        DestinationState, RelationMultiplier, RelationProfile, RelationUse, SearchBudget,
    };

    use super::*;

    fn confirm_node(engine: &GraphEngine, name: &str) -> NodeRecord {
        let candidate = engine.insert_node_candidate(name).unwrap();
        let ConfirmedRecord::Node(node) = engine
            .confirm_validated_candidate(candidate)
            .unwrap()
            .into_parts()
            .0
        else {
            panic!()
        };
        node
    }

    #[test]
    fn acquired_image_survives_publication_but_new_routes_use_the_replacement() {
        let directory = tempfile::tempdir().unwrap();
        let engine = GraphEngine::open(directory.path(), EngineConfig::default()).unwrap();
        let source = confirm_node(&engine, "source");
        let destination = confirm_node(&engine, "destination");
        let relation_candidate = engine.insert_relation_candidate("kind").unwrap();
        let ConfirmedRecord::Relation(relation) = engine
            .confirm_validated_candidate(relation_candidate)
            .unwrap()
            .into_parts()
            .0
        else {
            panic!()
        };
        let edge_candidate = engine
            .insert_edge_candidate(source.id(), destination.id(), relation.id(), 0.5)
            .unwrap();
        engine.confirm_validated_candidate(edge_candidate).unwrap();
        let old_image = engine.routing.read().unwrap().image().unwrap();
        let request = RoutingRequest::new(
            source.id(),
            [destination.id()],
            RelationProfile::new([(
                relation.id(),
                RelationUse::Enabled(RelationMultiplier::new(1.0).unwrap()),
            )]),
            false,
            SearchBudget::Unlimited,
            TiePolicy::StablePredecessor,
        );
        engine.remove_node(destination.id()).unwrap();

        let old_response = pathhydra_routing::route(&old_image, &request).unwrap();
        assert!(matches!(
            old_response.results()[0].state(),
            DestinationState::Exact(_)
        ));
        let new_response = engine.route(RequestId::new(1), &request).unwrap();
        assert!(matches!(
            new_response.response.results()[0].state(),
            DestinationState::MissingNode
        ));
    }

    #[test]
    fn simultaneous_confirmed_mutations_serialize_publication() {
        let directory = tempfile::tempdir().unwrap();
        let engine =
            Arc::new(GraphEngine::open(directory.path(), EngineConfig::default()).unwrap());
        let candidates: Vec<_> = (0..8)
            .map(|index| {
                engine
                    .insert_node_candidate(format!("node-{index}"))
                    .unwrap()
            })
            .collect();
        let workers: Vec<_> = candidates
            .into_iter()
            .map(|candidate| {
                let engine = Arc::clone(&engine);
                std::thread::spawn(move || engine.confirm_validated_candidate(candidate).unwrap())
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(
            engine
                .health()
                .unwrap()
                .current_image_manifest
                .unwrap()
                .node_count(),
            8
        );
    }

    #[test]
    fn request_identity_and_cancellation_are_isolated_and_released() {
        let directory = tempfile::tempdir().unwrap();
        let engine = GraphEngine::open(directory.path(), EngineConfig::default()).unwrap();
        let origin = confirm_node(&engine, "origin");
        let id = RequestId::new(44);
        let registration = engine.requests.register(id).unwrap();
        let request = RoutingRequest::new(
            origin.id(),
            [origin.id()],
            RelationProfile::default(),
            false,
            SearchBudget::Unlimited,
            TiePolicy::StablePredecessor,
        );
        assert!(matches!(
            engine.route(id, &request),
            Err(EngineError::DuplicateActiveRequest(found)) if found == id
        ));
        assert_eq!(engine.cancel(id).unwrap(), CancellationOutcome::Signalled);
        assert_eq!(
            engine.cancel(id).unwrap(),
            CancellationOutcome::AlreadySignalled
        );
        assert_eq!(engine.health().unwrap().cumulative_cancellations, 1);
        drop(registration);
        assert_eq!(engine.cancel(id).unwrap(), CancellationOutcome::NotActive);
        assert!(engine.route(id, &request).is_ok());
    }
}
