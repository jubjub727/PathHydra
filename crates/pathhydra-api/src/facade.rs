use std::{
    collections::BTreeMap,
    fmt, fs,
    path::{Component, Path, PathBuf},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
    time::Instant,
};

use pathhydra_core::{
    BaseWeight, CandidateId, EdgeId, NodeId, NodeName, NodePayload, RelationId, RelationName,
};
use pathhydra_engine::{
    CancellationOutcome as EngineCancellationOutcome, EngineConfig, EngineLifecycleState,
    EngineRestoreRequest, GraphEngine, HydrationRequest, RequestId,
};
use pathhydra_routing::{DestinationState, RelationProfile, RoutingRequest, RoutingResponse};
use pathhydra_store::{
    CheckpointRequest, OperationalPathRequest, PathTarget, ReadOnlyCatalog, ResourceKind,
    RestoreRequest, VerificationLimits, validate_operational_paths,
};
use pathhydra_subgraph::Subgraph;

use crate::convert::{capabilities, edge_removal_outcome, node_removal_outcome};
use crate::{
    ApiError, ApiLimits, Binary32Dto, CancellationOutcomeDto, CandidateDto, CandidateIdDto,
    CapabilitiesDto, CheckpointReportDto, CompactionReportDto, DecimalU64Dto, DurationDto,
    EdgeIdDto, EdgeRecordDto, EngineRestoreReportDto, EngineRoutingResponseDto, HealthDto,
    HydratedPathDto, HydratedSubgraphDto, HydrationRequestDto, HydrationResponseDto,
    ImageBuildReportDto, LifecycleSnapshotDto, MutationOutcomeDto, NodeIdDto, NodeRecordDto,
    PathHydraConfigDto, PayloadDto, RelationIdDto, RelationKindRecordDto, RelationProfileDto,
    RequestIdDto, RoutingRequestDto, ShutdownReportDto, SubgraphHandlesDto, VerificationLimitsDto,
    VerificationReportDto,
};

const REQUEST_CREATED: u8 = 0;
const REQUEST_EXECUTING: u8 = 1;
const REQUEST_COMPLETE: u8 = 2;

struct RequestCompletion<'a>(&'a AtomicU8);

impl Drop for RequestCompletion<'_> {
    fn drop(&mut self) {
        self.0.store(REQUEST_COMPLETE, Ordering::Release);
    }
}

/// Selects facade allocation or an application correlation value.
///
/// Supplied values must be strictly greater than every previously assigned
/// value in this process. Accepting a supplied value advances the allocator's
/// high-water mark; IDs are never reused.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum RequestIdAllocation {
    #[default]
    Automatic,
    Supplied(RequestIdDto),
}

#[derive(Debug)]
struct RequestState {
    id: RequestId,
    cancellation: Arc<AtomicBool>,
    phase: AtomicU8,
    response: Mutex<Option<RoutingResponse>>,
}

impl RequestState {
    fn new(id: RequestId) -> Self {
        Self {
            id,
            cancellation: Arc::new(AtomicBool::new(false)),
            phase: AtomicU8::new(REQUEST_CREATED),
            response: Mutex::new(None),
        }
    }
}

#[derive(Default)]
struct RequestAllocator {
    high_water: Option<u64>,
    requests: BTreeMap<u64, Weak<RequestState>>,
}

impl RequestAllocator {
    fn allocate(&mut self, allocation: RequestIdAllocation) -> Result<Arc<RequestState>, ApiError> {
        self.requests
            .retain(|_, request| request.strong_count() != 0);
        let value = match allocation {
            RequestIdAllocation::Automatic => match self.high_water {
                None => 1,
                Some(value) => value.checked_add(1).ok_or_else(|| {
                    ApiError::cancellation(
                        "request_id_exhausted",
                        "the process-local request ID space is exhausted",
                    )
                })?,
            },
            RequestIdAllocation::Supplied(value) => {
                let value = value.as_u64().map_err(ApiError::from)?;
                if self
                    .high_water
                    .is_some_and(|high_water| value <= high_water)
                {
                    return Err(ApiError::cancellation(
                        "request_id_not_monotonic",
                        "the supplied request ID is not above the process high-water mark",
                    ));
                }
                value
            }
        };

        let state = Arc::new(RequestState::new(RequestId::new(value)));
        self.high_water = Some(value);
        self.requests.insert(value, Arc::downgrade(&state));
        Ok(state)
    }

    fn request(&mut self, id: u64) -> Option<Arc<RequestState>> {
        let request = self.requests.get(&id).and_then(Weak::upgrade);
        if request.is_none() {
            self.requests.remove(&id);
        }
        request
    }
}

struct FacadeInner {
    engine: Arc<GraphEngine>,
    requests: Mutex<RequestAllocator>,
    shutting_down: AtomicBool,
    limits: ApiLimits,
    roots: OperationalRoots,
    engine_config: EngineConfig,
    external_operations: Mutex<()>,
}

#[derive(Clone)]
struct OperationalRoots {
    routing_images: PathBuf,
    checkpoints: PathBuf,
    restores: PathBuf,
    scratch: PathBuf,
}

/// Process-local deployment paths for one facade owner.
///
/// These paths are never canonical DTO fields and are never returned in
/// reports or errors. Checkpoint and restore commands carry opaque names that
/// are resolved strictly beneath these roots.
#[derive(Clone, Debug)]
pub struct PathHydraOpenConfig {
    database: PathBuf,
    routing_images: PathBuf,
    checkpoints: PathBuf,
    restores: PathBuf,
    scratch: PathBuf,
    limits: ApiLimits,
    engine: PathHydraConfigDto,
}

impl PathHydraOpenConfig {
    #[must_use]
    pub fn new(
        database: impl Into<PathBuf>,
        routing_images: impl Into<PathBuf>,
        checkpoints: impl Into<PathBuf>,
        restores: impl Into<PathBuf>,
        scratch: impl Into<PathBuf>,
    ) -> Self {
        Self {
            database: database.into(),
            routing_images: routing_images.into(),
            checkpoints: checkpoints.into(),
            restores: restores.into(),
            scratch: scratch.into(),
            limits: ApiLimits::default(),
            engine: PathHydraConfigDto::from(&EngineConfig::default()),
        }
    }

    #[must_use]
    pub const fn with_limits(mut self, limits: ApiLimits) -> Self {
        self.limits = limits;
        self
    }

    #[must_use]
    pub fn with_engine_config(mut self, engine: PathHydraConfigDto) -> Self {
        self.engine = engine;
        self
    }
}

impl FacadeInner {
    fn resolve_opaque_name(&self, root: &Path, name: &str) -> Result<PathBuf, ApiError> {
        if name.is_empty() || name.len() > self.limits.maximum_name_bytes {
            return Err(ApiError::invalid_input(
                "invalid_resource_name",
                "the operational resource name is invalid",
            ));
        }
        let path = Path::new(name);
        let mut components = path.components();
        if !matches!(components.next(), Some(Component::Normal(_)))
            || components.next().is_some()
            || name == "."
            || name == ".."
        {
            return Err(ApiError::invalid_input(
                "invalid_resource_name",
                "the operational resource name is invalid",
            ));
        }
        Ok(root.join(path))
    }

    fn cancel_state(&self, state: &RequestState) -> Result<CancellationOutcomeDto, ApiError> {
        if state.phase.load(Ordering::Acquire) == REQUEST_COMPLETE {
            return Ok(CancellationOutcomeDto::AlreadyCompleted);
        }
        if state.phase.load(Ordering::Acquire) == REQUEST_CREATED
            && self.shutting_down.load(Ordering::Acquire)
        {
            return Ok(CancellationOutcomeDto::ShuttingDown);
        }

        if state.phase.load(Ordering::Acquire) == REQUEST_EXECUTING {
            match self.engine.cancel(state.id)? {
                EngineCancellationOutcome::Signalled => {
                    return Ok(CancellationOutcomeDto::Signalled);
                }
                EngineCancellationOutcome::AlreadySignalled => {
                    return Ok(CancellationOutcomeDto::AlreadySignalled);
                }
                EngineCancellationOutcome::NotActive => {
                    // Fall through to the facade flag. This closes the gap
                    // between the facade entering execution and the engine
                    // registering the same flag.
                }
            }
        }

        let newly_signalled = !state.cancellation.swap(true, Ordering::AcqRel);
        if state.phase.load(Ordering::Acquire) == REQUEST_COMPLETE {
            Ok(CancellationOutcomeDto::AlreadyCompleted)
        } else if newly_signalled {
            Ok(CancellationOutcomeDto::Signalled)
        } else {
            Ok(CancellationOutcomeDto::AlreadySignalled)
        }
    }
}

impl Drop for FacadeInner {
    fn drop(&mut self) {
        // Explicit shutdown reports operational failures. This final best-effort
        // close prevents abandoned worker ownership when an application unwinds.
        self.shutting_down.store(true, Ordering::Release);
        let _ = self.engine.shutdown();
    }
}

/// The in-process application boundary and sole owner of one graph engine.
#[derive(Clone)]
pub struct PathHydra {
    inner: Arc<FacadeInner>,
}

impl PathHydra {
    #[must_use]
    pub fn default_engine_config() -> PathHydraConfigDto {
        PathHydraConfigDto::from(&EngineConfig::default())
    }

    /// Opens a graph with the bounded default engine configuration.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ApiError> {
        Self::open_with_limits(path, ApiLimits::default())
    }

    /// Opens a graph with explicit consumer allocation/cardinality limits.
    pub fn open_with_limits(path: impl AsRef<Path>, limits: ApiLimits) -> Result<Self, ApiError> {
        let database = path.as_ref().to_path_buf();
        let parent = database
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        let stem = database
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("pathhydra");
        let routing_images = parent.join(format!("{stem}.routing-images"));
        let checkpoints = parent.join(format!("{stem}.checkpoints"));
        let restores = parent.join(format!("{stem}.restores"));
        let scratch = parent.join(format!("{stem}.scratch"));
        Self::open_with_config(
            PathHydraOpenConfig::new(database, routing_images, checkpoints, restores, scratch)
                .with_limits(limits),
        )
    }

    /// Opens one engine using local deployment roots that cannot cross the
    /// canonical consumer boundary.
    pub fn open_with_config(config: PathHydraOpenConfig) -> Result<Self, ApiError> {
        let mut engine_config = EngineConfig::try_from(&config.engine)?;
        let database_parent = config.database.parent().ok_or_else(|| {
            ApiError::invalid_input("invalid_database_path", "the database path has no parent")
        })?;
        let routing_parent = config.routing_images.parent().ok_or_else(|| {
            ApiError::invalid_input(
                "invalid_routing_path",
                "the routing-image path has no parent",
            )
        })?;
        let checkpoint_parent = config.checkpoints.parent().ok_or_else(|| {
            ApiError::invalid_input(
                "invalid_checkpoint_root",
                "the checkpoint root has no parent",
            )
        })?;
        let restore_parent = config.restores.parent().ok_or_else(|| {
            ApiError::invalid_input("invalid_restore_root", "the restore root has no parent")
        })?;
        let scratch_parent = config.scratch.parent().ok_or_else(|| {
            ApiError::invalid_input("invalid_scratch_root", "the scratch root has no parent")
        })?;
        let resolved = validate_operational_paths(&OperationalPathRequest {
            database: Some(PathTarget {
                root: database_parent,
                path: &config.database,
                must_exist: false,
                must_be_fresh: false,
            }),
            routing_images: Some(PathTarget {
                root: routing_parent,
                path: &config.routing_images,
                must_exist: false,
                must_be_fresh: false,
            }),
            checkpoint: Some(PathTarget {
                root: checkpoint_parent,
                path: &config.checkpoints,
                must_exist: false,
                must_be_fresh: false,
            }),
            restore: Some(PathTarget {
                root: restore_parent,
                path: &config.restores,
                must_exist: false,
                must_be_fresh: false,
            }),
            scratch: Some(PathTarget {
                root: scratch_parent,
                path: &config.scratch,
                must_exist: false,
                must_be_fresh: false,
            }),
        })?;
        let database = resolved
            .get(ResourceKind::Database)
            .expect("database target was supplied")
            .to_path_buf();
        let roots = OperationalRoots {
            routing_images: resolved
                .get(ResourceKind::RoutingImages)
                .expect("routing target was supplied")
                .to_path_buf(),
            checkpoints: resolved
                .get(ResourceKind::Checkpoint)
                .expect("checkpoint target was supplied")
                .to_path_buf(),
            restores: resolved
                .get(ResourceKind::Restore)
                .expect("restore target was supplied")
                .to_path_buf(),
            scratch: resolved
                .get(ResourceKind::Scratch)
                .expect("scratch target was supplied")
                .to_path_buf(),
        };
        for root in [&roots.checkpoints, &roots.restores, &roots.scratch] {
            fs::create_dir_all(root).map_err(|_| {
                ApiError::new(
                    crate::ApiErrorCategory::DurableStore,
                    "operational_root_creation_failed",
                    "an operational resource root could not be created",
                    false,
                )
            })?;
        }
        engine_config.routing_image_root = Some(roots.routing_images.clone());
        let engine = GraphEngine::open(database, engine_config.clone())?;
        Ok(Self {
            inner: Arc::new(FacadeInner {
                engine: Arc::new(engine),
                requests: Mutex::new(RequestAllocator::default()),
                shutting_down: AtomicBool::new(false),
                limits: config.limits,
                roots,
                engine_config,
                external_operations: Mutex::new(()),
            }),
        })
    }

    #[must_use]
    pub fn limits(&self) -> ApiLimits {
        self.inner.limits
    }

    fn validate_name(&self, name: &str) -> Result<(), ApiError> {
        if name.len() > self.inner.limits.maximum_name_bytes {
            return Err(ApiError::new(
                crate::ApiErrorCategory::ResourceLimit,
                "name_limit_exceeded",
                "the exact name exceeds the configured boundary limit",
                false,
            ));
        }
        Ok(())
    }

    fn decode_payload(&self, payload: &PayloadDto) -> Result<Vec<u8>, ApiError> {
        let maximum_encoded = self
            .inner
            .limits
            .maximum_payload_bytes
            .saturating_add(2)
            .saturating_div(3)
            .saturating_mul(4);
        if payload.as_str().len() > maximum_encoded {
            return Err(ApiError::new(
                crate::ApiErrorCategory::ResourceLimit,
                "payload_limit_exceeded",
                "the node payload exceeds the configured boundary limit",
                false,
            ));
        }
        let decoded = payload.decode()?;
        if decoded.len() > self.inner.limits.maximum_payload_bytes {
            return Err(ApiError::new(
                crate::ApiErrorCategory::ResourceLimit,
                "payload_limit_exceeded",
                "the node payload exceeds the configured boundary limit",
                false,
            ));
        }
        Ok(decoded)
    }

    pub fn lookup_node_exact(&self, name: &str) -> Result<Option<NodeIdDto>, ApiError> {
        self.validate_name(name)?;
        self.inner
            .engine
            .lookup_node_exact(name)
            .map(|value| value.map(Into::into))
            .map_err(Into::into)
    }

    pub fn lookup_relation_kind_exact(
        &self,
        name: &str,
    ) -> Result<Option<RelationIdDto>, ApiError> {
        self.validate_name(name)?;
        self.inner
            .engine
            .lookup_relation_exact(name)
            .map(|value| value.map(Into::into))
            .map_err(Into::into)
    }

    pub fn insert_node_candidate(
        &self,
        name: String,
        payload: PayloadDto,
    ) -> Result<CandidateIdDto, ApiError> {
        self.validate_name(&name)?;
        let payload = self.decode_payload(&payload)?;
        self.inner
            .engine
            .insert_node_candidate_with_payload(
                NodeName::from(name),
                NodePayload::new(payload.into_boxed_slice()),
            )
            .map(Into::into)
            .map_err(Into::into)
    }

    pub fn insert_relation_kind_candidate(&self, name: String) -> Result<CandidateIdDto, ApiError> {
        self.validate_name(&name)?;
        self.inner
            .engine
            .insert_relation_candidate(RelationName::from(name))
            .map(Into::into)
            .map_err(Into::into)
    }

    pub fn insert_edge_candidate(
        &self,
        source: &NodeIdDto,
        destination: &NodeIdDto,
        relation_kind: &RelationIdDto,
        base_weight: &Binary32Dto,
    ) -> Result<CandidateIdDto, ApiError> {
        let source = NodeId::try_from(source)?;
        let destination = NodeId::try_from(destination)?;
        let relation_kind = RelationId::try_from(relation_kind)?;
        let base_weight = BaseWeight::try_from(base_weight)?;
        self.inner
            .engine
            .insert_edge_candidate_with_base_weight(source, destination, relation_kind, base_weight)
            .map(Into::into)
            .map_err(Into::into)
    }

    pub fn get_candidate(&self, id: &CandidateIdDto) -> Result<CandidateDto, ApiError> {
        let candidate = self
            .inner
            .engine
            .get_candidate(CandidateId::try_from(id)?)?;
        Ok(CandidateDto::from(&candidate))
    }

    /// Confirms a candidate after the caller has already made the external
    /// validation decision.
    pub fn confirm_candidate(&self, id: &CandidateIdDto) -> Result<MutationOutcomeDto, ApiError> {
        let mutation = self
            .inner
            .engine
            .confirm_validated_candidate(CandidateId::try_from(id)?)?;
        Ok(MutationOutcomeDto::from(&mutation))
    }

    pub fn get_confirmed_node(&self, id: &NodeIdDto) -> Result<NodeRecordDto, ApiError> {
        let record = self.inner.engine.get_node(NodeId::try_from(id)?)?;
        Ok(NodeRecordDto::from(&record))
    }

    pub fn get_confirmed_relation_kind(
        &self,
        id: &RelationIdDto,
    ) -> Result<RelationKindRecordDto, ApiError> {
        let record = self.inner.engine.get_relation(RelationId::try_from(id)?)?;
        Ok(RelationKindRecordDto::from(&record))
    }

    pub fn get_confirmed_edge(&self, id: &EdgeIdDto) -> Result<EdgeRecordDto, ApiError> {
        let record = self.inner.engine.get_edge(EdgeId::try_from(id)?)?;
        Ok(EdgeRecordDto::from(&record))
    }

    pub fn remove_confirmed_edge(&self, id: &EdgeIdDto) -> Result<MutationOutcomeDto, ApiError> {
        let id = EdgeId::try_from(id)?;
        let mutation = self.inner.engine.remove_edge(id)?;
        Ok(edge_removal_outcome(id, &mutation))
    }

    pub fn remove_confirmed_node(&self, id: &NodeIdDto) -> Result<MutationOutcomeDto, ApiError> {
        let id = NodeId::try_from(id)?;
        let mutation = self.inner.engine.remove_node(id)?;
        Ok(node_removal_outcome(id, &mutation))
    }

    pub fn hydrate(&self, request: &HydrationRequestDto) -> Result<HydrationResponseDto, ApiError> {
        let handle_count = request
            .node_ids
            .len()
            .checked_add(request.edge_ids.len())
            .ok_or_else(|| {
                ApiError::new(
                    crate::ApiErrorCategory::ResourceLimit,
                    "hydration_handle_overflow",
                    "the hydration handle count overflowed",
                    false,
                )
            })?;
        if handle_count > self.inner.limits.maximum_hydration_handles {
            return Err(ApiError::new(
                crate::ApiErrorCategory::ResourceLimit,
                "hydration_handle_limit_exceeded",
                "the hydration request exceeds the configured handle limit",
                false,
            ));
        }
        if request.profile.as_ref().is_some_and(|profile| {
            profile.entries.len() > self.inner.limits.maximum_profile_entries
        }) {
            return Err(ApiError::new(
                crate::ApiErrorCategory::ResourceLimit,
                "profile_limit_exceeded",
                "the relation profile exceeds the configured entry limit",
                false,
            ));
        }
        let request = HydrationRequest::try_from(request)?;
        let started = Instant::now();
        let response = self.inner.engine.hydrate(&request)?;
        let mut response = HydrationResponseDto::from(&response);
        response.diagnostics.execution_duration = DurationDto::from_duration(started.elapsed());
        Ok(response)
    }

    #[must_use]
    pub fn new_subgraph(&self) -> SubgraphHandlesDto {
        SubgraphHandlesDto::default()
    }

    pub fn subgraph_add_node(
        &self,
        subgraph: &mut SubgraphHandlesDto,
        node: &NodeIdDto,
    ) -> Result<bool, ApiError> {
        self.check_subgraph_dto_limits(subgraph)?;
        let mut value = Subgraph::try_from(&*subgraph)?;
        let inserted = value.add_node(NodeId::try_from(node)?);
        self.check_subgraph_limits(&value)?;
        *subgraph = SubgraphHandlesDto::from(&value.handles());
        Ok(inserted)
    }

    pub fn subgraph_add_edge(
        &self,
        subgraph: &mut SubgraphHandlesDto,
        edge: &EdgeIdDto,
        source: &NodeIdDto,
        destination: &NodeIdDto,
    ) -> Result<bool, ApiError> {
        self.check_subgraph_dto_limits(subgraph)?;
        let mut value = Subgraph::try_from(&*subgraph)?;
        let inserted = value.add_edge(
            EdgeId::try_from(edge)?,
            NodeId::try_from(source)?,
            NodeId::try_from(destination)?,
        )?;
        self.check_subgraph_limits(&value)?;
        *subgraph = SubgraphHandlesDto::from(&value.handles());
        Ok(inserted)
    }

    pub fn subgraph_remove_edge(
        &self,
        subgraph: &mut SubgraphHandlesDto,
        edge: &EdgeIdDto,
    ) -> Result<bool, ApiError> {
        self.check_subgraph_dto_limits(subgraph)?;
        let mut value = Subgraph::try_from(&*subgraph)?;
        let removed = value.remove_edge(EdgeId::try_from(edge)?);
        *subgraph = SubgraphHandlesDto::from(&value.handles());
        Ok(removed)
    }

    pub fn subgraph_remove_node(
        &self,
        subgraph: &mut SubgraphHandlesDto,
        node: &NodeIdDto,
    ) -> Result<bool, ApiError> {
        self.check_subgraph_dto_limits(subgraph)?;
        let mut value = Subgraph::try_from(&*subgraph)?;
        let removed = value.remove_node(NodeId::try_from(node)?);
        *subgraph = SubgraphHandlesDto::from(&value.handles());
        Ok(removed)
    }

    pub fn subgraph_union(
        &self,
        subgraph: &mut SubgraphHandlesDto,
        other: &SubgraphHandlesDto,
    ) -> Result<(), ApiError> {
        self.check_subgraph_dto_limits(subgraph)?;
        self.check_subgraph_dto_limits(other)?;
        let mut value = Subgraph::try_from(&*subgraph)?;
        value.union(&Subgraph::try_from(other)?)?;
        self.check_subgraph_limits(&value)?;
        *subgraph = SubgraphHandlesDto::from(&value.handles());
        Ok(())
    }

    pub fn hydrate_subgraph(
        &self,
        subgraph: &SubgraphHandlesDto,
        profile: Option<&RelationProfileDto>,
    ) -> Result<HydratedSubgraphDto, ApiError> {
        self.check_subgraph_dto_limits(subgraph)?;
        if profile.is_some_and(|profile| {
            profile.entries.len() > self.inner.limits.maximum_profile_entries
        }) {
            return Err(ApiError::new(
                crate::ApiErrorCategory::ResourceLimit,
                "profile_limit_exceeded",
                "the relation profile exceeds the configured entry limit",
                false,
            ));
        }
        let subgraph = Subgraph::try_from(subgraph)?;
        self.check_subgraph_limits(&subgraph)?;
        let profile = profile.map(RelationProfile::try_from).transpose()?;
        let started = Instant::now();
        let hydrated = self.inner.engine.hydrate_subgraph(&subgraph, profile)?;
        let mut hydrated = HydratedSubgraphDto::from(&hydrated);
        hydrated.diagnostics.execution_duration = DurationDto::from_duration(started.elapsed());
        Ok(hydrated)
    }

    #[must_use]
    pub fn capabilities(&self) -> CapabilitiesDto {
        capabilities(&self.inner.engine.capabilities(), &self.inner.limits)
    }

    pub fn health(&self) -> Result<HealthDto, ApiError> {
        Ok(HealthDto::from(&self.inner.engine.health()?))
    }

    pub fn maintenance_status(&self) -> Result<LifecycleSnapshotDto, ApiError> {
        Ok(LifecycleSnapshotDto::from(
            &self.inner.engine.lifecycle_snapshot()?,
        ))
    }

    pub fn rebuild_routing(&self) -> Result<ImageBuildReportDto, ApiError> {
        Ok(ImageBuildReportDto::from(
            &self.inner.engine.rebuild_routing_image()?,
        ))
    }

    pub fn rebuild_cuda_residency(&self) -> Result<(), ApiError> {
        self.inner
            .engine
            .rebuild_cuda_residency()
            .map_err(Into::into)
    }

    pub fn reinitialize_cuda(&self) -> Result<(), ApiError> {
        self.inner.engine.reinitialize_cuda().map_err(Into::into)
    }

    pub fn verify_catalog(
        &self,
        limits: &VerificationLimitsDto,
    ) -> Result<VerificationReportDto, ApiError> {
        let report = self
            .inner
            .engine
            .verify_catalog(VerificationLimits::try_from(limits)?)?;
        Ok(VerificationReportDto::from(&report))
    }

    pub fn create_checkpoint(
        &self,
        checkpoint_name: &str,
        available_destination_bytes: &DecimalU64Dto,
        minimum_headroom_bytes: &DecimalU64Dto,
    ) -> Result<CheckpointReportDto, ApiError> {
        let destination = self
            .inner
            .resolve_opaque_name(&self.inner.roots.checkpoints, checkpoint_name)?;
        let report = self.inner.engine.create_checkpoint(&CheckpointRequest {
            destination_root: self.inner.roots.checkpoints.clone(),
            destination,
            routing_image_root: Some(self.inner.roots.routing_images.clone()),
            scratch_path: None,
            available_destination_bytes: available_destination_bytes.as_u64()?,
            minimum_headroom_bytes: minimum_headroom_bytes.as_u64()?,
        })?;
        Ok(CheckpointReportDto::from(&report))
    }

    pub fn validate_checkpoint(
        &self,
        checkpoint_name: &str,
        limits: &VerificationLimitsDto,
    ) -> Result<VerificationReportDto, ApiError> {
        let _operation = self.inner.external_operations.lock().map_err(|_| {
            ApiError::new(
                crate::ApiErrorCategory::InternalInvariant,
                "external_operation_lock_failure",
                "the external operation synchronization invariant failed",
                false,
            )
        })?;
        if self.inner.shutting_down.load(Ordering::Acquire) {
            return Err(ApiError::new(
                crate::ApiErrorCategory::Shutdown,
                "engine_not_running",
                "the graph engine is closing or shut down",
                false,
            ));
        }
        let checkpoint = self
            .inner
            .resolve_opaque_name(&self.inner.roots.checkpoints, checkpoint_name)?;
        let catalog = ReadOnlyCatalog::open(checkpoint).map_err(ApiError::from)?;
        let mut requested = VerificationLimits::try_from(limits)?;
        requested.maximum_records = Some(
            requested
                .maximum_records
                .unwrap_or(self.inner.engine_config.maximum_verification_records)
                .min(self.inner.engine_config.maximum_verification_records),
        );
        requested.maximum_duration = Some(
            requested
                .maximum_duration
                .unwrap_or(self.inner.engine_config.maximum_verification_duration)
                .min(self.inner.engine_config.maximum_verification_duration),
        );
        let report = catalog.verify(requested).map_err(ApiError::from)?;
        Ok(VerificationReportDto::from(&report))
    }

    pub fn compact_store(&self) -> Result<CompactionReportDto, ApiError> {
        Ok(CompactionReportDto::from(
            &self.inner.engine.compact_store()?,
        ))
    }

    pub fn restore_checkpoint(
        &self,
        checkpoint_name: &str,
        restore_name: &str,
        available_destination_bytes: &DecimalU64Dto,
        minimum_headroom_bytes: &DecimalU64Dto,
        verification_limits: &VerificationLimitsDto,
    ) -> Result<EngineRestoreReportDto, ApiError> {
        let _operation = self.inner.external_operations.lock().map_err(|_| {
            ApiError::new(
                crate::ApiErrorCategory::InternalInvariant,
                "external_operation_lock_failure",
                "the external operation synchronization invariant failed",
                false,
            )
        })?;
        if self.inner.shutting_down.load(Ordering::Acquire) {
            return Err(ApiError::new(
                crate::ApiErrorCategory::Shutdown,
                "engine_not_running",
                "the graph engine is closing or shut down",
                false,
            ));
        }
        let source = self
            .inner
            .resolve_opaque_name(&self.inner.roots.checkpoints, checkpoint_name)?;
        let destination = self
            .inner
            .resolve_opaque_name(&self.inner.roots.restores, restore_name)?;
        let routing = self.inner.resolve_opaque_name(
            &self.inner.roots.routing_images,
            &format!("restored-{restore_name}"),
        )?;
        let scratch = self.inner.resolve_opaque_name(
            &self.inner.roots.scratch,
            &format!("restore-{restore_name}"),
        )?;
        let mut engine_config = self.inner.engine_config.clone();
        engine_config.routing_image_root = Some(routing.clone());
        let report = GraphEngine::restore_checkpoint(&EngineRestoreRequest {
            store: RestoreRequest {
                source_root: self.inner.roots.checkpoints.clone(),
                source_checkpoint: source,
                destination_root: self.inner.roots.restores.clone(),
                destination,
                routing_image_root: Some(routing),
                scratch_path: Some(scratch),
                available_destination_bytes: available_destination_bytes.as_u64()?,
                minimum_headroom_bytes: minimum_headroom_bytes.as_u64()?,
                verification_limits: VerificationLimits::try_from(verification_limits)?,
            },
            engine_config,
        })?;
        Ok(EngineRestoreReportDto::from(&report))
    }

    pub fn shutdown(&self) -> Result<ShutdownReportDto, ApiError> {
        let requests = self.inner.requests.lock().map_err(|_| {
            ApiError::new(
                crate::ApiErrorCategory::InternalInvariant,
                "request_allocator_lock_failure",
                "the request allocator synchronization invariant failed",
                false,
            )
        })?;
        self.inner.shutting_down.store(true, Ordering::Release);
        drop(requests);
        let _external = self.inner.external_operations.lock().map_err(|_| {
            ApiError::new(
                crate::ApiErrorCategory::InternalInvariant,
                "external_operation_lock_failure",
                "the external operation synchronization invariant failed",
                false,
            )
        })?;
        let report = self.inner.engine.shutdown()?;
        Ok(ShutdownReportDto::from(&report))
    }

    fn check_subgraph_limits(&self, subgraph: &Subgraph) -> Result<(), ApiError> {
        if subgraph.node_count() > self.inner.limits.maximum_subgraph_nodes {
            return Err(ApiError::new(
                crate::ApiErrorCategory::ResourceLimit,
                "subgraph_node_limit_exceeded",
                "the subgraph exceeds the configured node limit",
                false,
            ));
        }
        if subgraph.edge_count() > self.inner.limits.maximum_subgraph_edges {
            return Err(ApiError::new(
                crate::ApiErrorCategory::ResourceLimit,
                "subgraph_edge_limit_exceeded",
                "the subgraph exceeds the configured edge limit",
                false,
            ));
        }
        Ok(())
    }

    fn check_subgraph_dto_limits(&self, subgraph: &SubgraphHandlesDto) -> Result<(), ApiError> {
        if subgraph.nodes.len() > self.inner.limits.maximum_subgraph_nodes {
            return Err(ApiError::new(
                crate::ApiErrorCategory::ResourceLimit,
                "subgraph_node_limit_exceeded",
                "the subgraph exceeds the configured node limit",
                false,
            ));
        }
        if subgraph.edges.len() > self.inner.limits.maximum_subgraph_edges {
            return Err(ApiError::new(
                crate::ApiErrorCategory::ResourceLimit,
                "subgraph_edge_limit_exceeded",
                "the subgraph exceeds the configured edge limit",
                false,
            ));
        }
        Ok(())
    }

    /// Allocates one owned, never-reusable request handle.
    pub fn request_handle(
        &self,
        allocation: RequestIdAllocation,
    ) -> Result<RequestHandle, ApiError> {
        if self.inner.shutting_down.load(Ordering::Acquire) {
            return Err(ApiError::new(
                crate::ApiErrorCategory::Shutdown,
                "engine_not_running",
                "the graph engine is closing or shut down",
                false,
            ));
        }
        let mut requests = self.inner.requests.lock().map_err(|_| {
            ApiError::new(
                crate::ApiErrorCategory::InternalInvariant,
                "request_allocator_lock_failure",
                "the request allocator synchronization invariant failed",
                false,
            )
        })?;
        if self.inner.shutting_down.load(Ordering::Acquire) {
            return Err(ApiError::new(
                crate::ApiErrorCategory::Shutdown,
                "engine_not_running",
                "the graph engine is closing or shut down",
                false,
            ));
        }
        let state = requests.allocate(allocation)?;
        Ok(RequestHandle {
            facade: Arc::clone(&self.inner),
            state,
        })
    }

    /// Signals a live request by its process-local ID.
    pub fn cancel(&self, request_id: &RequestIdDto) -> Result<CancellationOutcomeDto, ApiError> {
        let request_id = request_id.as_u64().map_err(ApiError::from)?;
        let request = self
            .inner
            .requests
            .lock()
            .map_err(|_| {
                ApiError::new(
                    crate::ApiErrorCategory::InternalInvariant,
                    "request_allocator_lock_failure",
                    "the request allocator synchronization invariant failed",
                    false,
                )
            })?
            .request(request_id);
        if let Some(request) = request {
            return self.inner.cancel_state(&request);
        }
        let lifecycle = self.inner.engine.lifecycle_snapshot()?;
        Ok(if lifecycle.state == EngineLifecycleState::Running {
            CancellationOutcomeDto::UnknownRequest
        } else {
            CancellationOutcomeDto::ShuttingDown
        })
    }
}

/// An owned coordination token for exactly one route attempt.
///
/// Clones refer to the same request and may be moved to another thread for
/// cancellation. The route itself may execute only once; cancellation can be
/// called repeatedly and reports the race outcome without panicking.
#[derive(Clone)]
pub struct RequestHandle {
    facade: Arc<FacadeInner>,
    state: Arc<RequestState>,
}

impl fmt::Debug for RequestHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RequestHandle")
            .field("request_id", &self.state.id.get())
            .finish_non_exhaustive()
    }
}

impl RequestHandle {
    #[must_use]
    pub fn id(&self) -> RequestIdDto {
        RequestIdDto::from_u64(self.state.id.get())
    }

    pub fn cancel(&self) -> Result<CancellationOutcomeDto, ApiError> {
        self.facade.cancel_state(&self.state)
    }

    /// Executes this request exactly once and returns one complete response.
    pub fn route(&self, request: &RoutingRequestDto) -> Result<EngineRoutingResponseDto, ApiError> {
        self.begin()?;
        let _completion = RequestCompletion(&self.state.phase);
        (|| {
            if request.destinations.len() > self.facade.limits.maximum_destinations {
                return Err(ApiError::new(
                    crate::ApiErrorCategory::ResourceLimit,
                    "destination_limit_exceeded",
                    "the route exceeds the configured destination limit",
                    false,
                ));
            }
            if request.profile.entries.len() > self.facade.limits.maximum_profile_entries {
                return Err(ApiError::new(
                    crate::ApiErrorCategory::ResourceLimit,
                    "profile_limit_exceeded",
                    "the relation profile exceeds the configured entry limit",
                    false,
                ));
            }
            let request = RoutingRequest::try_from(request)?;
            let response = self.facade.engine.route_with_cancellation(
                self.state.id,
                &request,
                Arc::clone(&self.state.cancellation),
            )?;
            self.state.phase.store(REQUEST_COMPLETE, Ordering::Release);
            if response.response.results().iter().any(|result| {
                matches!(result.state(), DestinationState::Exact(exact)
                    if exact.path().is_some_and(|path| path.steps().len() > self.facade.limits.maximum_path_steps))
            }) {
                return Err(ApiError::new(
                    crate::ApiErrorCategory::ResourceLimit,
                    "path_step_limit_exceeded",
                    "a returned path exceeds the configured step limit",
                    false,
                ));
            }
            let dto = EngineRoutingResponseDto::from(&response);
            *self.state.response.lock().map_err(|_| {
                ApiError::new(
                    crate::ApiErrorCategory::InternalInvariant,
                    "request_response_lock_failure",
                    "the request response synchronization invariant failed",
                    false,
                )
            })? = Some(response.response);
            Ok(dto)
        })()
    }

    /// Hydrates one exact path from this handle's completed response against
    /// current confirmed records.
    pub fn hydrate_path(
        &self,
        destination_position: &DecimalU64Dto,
    ) -> Result<HydratedPathDto, ApiError> {
        let position = usize::try_from(destination_position.as_u64()?).map_err(|_| {
            ApiError::invalid_input(
                "destination_position_out_of_range",
                "the destination position is out of range",
            )
        })?;
        let response = self.completed_response()?;
        let started = Instant::now();
        let hydrated = self.facade.engine.hydrate_path(&response, position)?;
        let mut hydrated = HydratedPathDto::from(&hydrated);
        hydrated.diagnostics.execution_duration = DurationDto::from_duration(started.elapsed());
        Ok(hydrated)
    }

    /// Adds one exact returned path to a caller-owned subgraph.
    pub fn add_path_to_subgraph(
        &self,
        destination_position: &DecimalU64Dto,
        subgraph: &mut SubgraphHandlesDto,
    ) -> Result<(), ApiError> {
        let position = usize::try_from(destination_position.as_u64()?).map_err(|_| {
            ApiError::invalid_input(
                "destination_position_out_of_range",
                "the destination position is out of range",
            )
        })?;
        let response = self.completed_response()?;
        let destination = response.results().get(position).ok_or_else(|| {
            ApiError::invalid_input(
                "destination_position_out_of_range",
                "the destination position is out of range",
            )
        })?;
        let DestinationState::Exact(exact) = destination.state() else {
            return Err(ApiError::invalid_input(
                "destination_has_no_path",
                "the selected destination has no exact returned path",
            ));
        };
        let path = exact.path().ok_or_else(|| {
            ApiError::invalid_input(
                "destination_has_no_path",
                "the selected destination has no exact returned path",
            )
        })?;
        if subgraph.nodes.len() > self.facade.limits.maximum_subgraph_nodes
            || subgraph.edges.len() > self.facade.limits.maximum_subgraph_edges
        {
            return Err(ApiError::new(
                crate::ApiErrorCategory::ResourceLimit,
                "subgraph_limit_exceeded",
                "the subgraph exceeds a configured cardinality limit",
                false,
            ));
        }
        let mut value = Subgraph::try_from(&*subgraph)?;
        value.add_path(path)?;
        if value.node_count() > self.facade.limits.maximum_subgraph_nodes
            || value.edge_count() > self.facade.limits.maximum_subgraph_edges
        {
            return Err(ApiError::new(
                crate::ApiErrorCategory::ResourceLimit,
                "subgraph_limit_exceeded",
                "the subgraph exceeds a configured cardinality limit",
                false,
            ));
        }
        *subgraph = SubgraphHandlesDto::from(&value.handles());
        Ok(())
    }

    fn completed_response(&self) -> Result<RoutingResponse, ApiError> {
        self.state
            .response
            .lock()
            .map_err(|_| {
                ApiError::new(
                    crate::ApiErrorCategory::InternalInvariant,
                    "request_response_lock_failure",
                    "the request response synchronization invariant failed",
                    false,
                )
            })?
            .clone()
            .ok_or_else(|| {
                ApiError::cancellation(
                    "request_has_no_response",
                    "the request has no completed routing response",
                )
            })
    }

    fn begin(&self) -> Result<(), ApiError> {
        if self.facade.shutting_down.load(Ordering::Acquire) {
            return Err(ApiError::new(
                crate::ApiErrorCategory::Shutdown,
                "engine_not_running",
                "the graph engine is closing or shut down",
                false,
            ));
        }
        self.state
            .phase
            .compare_exchange(
                REQUEST_CREATED,
                REQUEST_EXECUTING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map(|_| ())
            .map_err(|_| {
                ApiError::cancellation(
                    "request_already_executed",
                    "the request handle has already been executed",
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{RequestAllocator, RequestIdAllocation, RequestIdDto};

    #[test]
    fn supplied_ids_advance_the_high_water_mark_and_exhaust_checked() {
        let mut allocator = RequestAllocator::default();
        assert_eq!(
            allocator
                .allocate(RequestIdAllocation::Supplied(RequestIdDto::from_u64(41)))
                .expect("41 is the first supplied ID")
                .id
                .get(),
            41
        );
        assert_eq!(
            allocator
                .allocate(RequestIdAllocation::Automatic)
                .expect("automatic allocation follows the high-water mark")
                .id
                .get(),
            42
        );
        assert_eq!(
            allocator
                .allocate(RequestIdAllocation::Supplied(RequestIdDto::from_u64(41)))
                .expect_err("lower and reused IDs are rejected")
                .code(),
            "request_id_not_monotonic"
        );
        assert_eq!(
            allocator
                .allocate(RequestIdAllocation::Supplied(RequestIdDto::from_u64(
                    u64::MAX,
                )))
                .expect("MAX itself is assigned once")
                .id
                .get(),
            u64::MAX
        );
        assert_eq!(
            allocator
                .allocate(RequestIdAllocation::Automatic)
                .expect_err("allocation after MAX is a typed error")
                .code(),
            "request_id_exhausted"
        );
    }
}
