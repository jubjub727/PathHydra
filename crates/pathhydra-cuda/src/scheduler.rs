use std::time::Duration;

#[cfg(feature = "cuda")]
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    thread::{self, JoinHandle},
    time::Instant,
};

use pathhydra_routing::RoutingResponse;

#[cfg(feature = "cuda")]
use pathhydra_routing::RoutingRequest;

use crate::CudaAlgorithm;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CudaParallelStrategy {
    RelationThreadsAtomicBinary64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CudaResetMode {
    ExplicitClear,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CudaTargetMode {
    SortedSparseHost,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CudaProfileMode {
    InlineExact,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CudaPathEvidenceMode {
    CpuPassSameImage,
    NotRequested,
}

#[cfg(feature = "cuda")]
use crate::{CudaError, CudaFailureKind, CudaPartitionedImage, CudaResidentImage};

#[derive(Clone, Debug)]
pub struct CudaRouteDiagnostics {
    pub algorithm: CudaAlgorithm,
    pub queue_duration: Duration,
    pub batch_collection_duration: Duration,
    pub batch_width: usize,
    pub lane_index: usize,
    pub reserved_search_bytes: usize,
    pub host_to_device_bytes: usize,
    pub device_to_host_bytes: usize,
    pub kernel_launches: u64,
    pub synchronized_execution_duration: Duration,
    pub examined_edges: u64,
    pub relaxation_attempts: u64,
    pub relaxation_updates: u64,
    pub phases: u64,
    pub partitions_required: u64,
    pub host_cache_hits: u64,
    pub device_cache_hits: u64,
    pub file_bytes: u64,
    pub staged_bytes: u64,
    pub transfer_bytes: u64,
    pub parallel_strategy: CudaParallelStrategy,
    pub reset_mode: CudaResetMode,
    pub target_mode: CudaTargetMode,
    pub profile_mode: CudaProfileMode,
    pub path_evidence_mode: CudaPathEvidenceMode,
    pub state_initialization_duration: Duration,
    pub partition_scheduling_duration: Duration,
    pub relation_relaxation_duration: Duration,
    pub response_transfer_duration: Duration,
    pub frontier_compaction_duration: Duration,
    pub compacted_task_count: u32,
    pub destination_completion_duration: Duration,
    pub destination_count_checked: usize,
    pub atomic_cas_retries: u64,
    /// Time from route execution start until the first requested destination
    /// could be classified as final. CUDA currently finalizes targets only
    /// after its synchronized full-distance pass.
    pub first_destination_duration: Option<Duration>,
    /// Time spent in the same-image CPU path-evidence reconstruction pass.
    pub path_reconstruction_duration: Duration,
}

#[derive(Clone, Debug)]
pub struct CudaRouteOutput {
    pub response: RoutingResponse,
    pub diagnostics: CudaRouteDiagnostics,
}

#[cfg(feature = "cuda")]
struct RouteJob {
    image: RouteImage,
    request: RoutingRequest,
    algorithm: CudaAlgorithm,
    cancellation: Arc<AtomicBool>,
    reserved_search_bytes: usize,
    enqueued_at: Instant,
    reply: Sender<Result<CudaRouteOutput, CudaError>>,
}

#[cfg(feature = "cuda")]
enum RouteImage {
    Resident(Arc<CudaResidentImage>),
    Partitioned(Arc<CudaPartitionedImage>),
}

#[cfg(feature = "cuda")]
impl RouteImage {
    fn ptr_eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Resident(left), Self::Resident(right)) => Arc::ptr_eq(left, right),
            (Self::Partitioned(left), Self::Partitioned(right)) => Arc::ptr_eq(left, right),
            _ => false,
        }
    }

    fn route(
        &self,
        request: &RoutingRequest,
        algorithm: CudaAlgorithm,
        cancellation: &AtomicBool,
        reserved_search_bytes: usize,
    ) -> Result<CudaRouteOutput, CudaError> {
        match self {
            Self::Resident(image) => {
                image.route(request, algorithm, cancellation, reserved_search_bytes)
            }
            Self::Partitioned(image) => {
                image.route(request, algorithm, cancellation, reserved_search_bytes)
            }
        }
    }
}

#[cfg(feature = "cuda")]
enum Command {
    Route(RouteJob),
    Shutdown(Sender<usize>),
}

#[cfg(feature = "cuda")]
#[derive(Default)]
struct WorkerCounters {
    queued: AtomicUsize,
    active: AtomicUsize,
    peak_active: AtomicUsize,
    batches: AtomicU64,
    launches: AtomicU64,
    failures: AtomicU64,
    cancellations: AtomicU64,
}

#[cfg(feature = "cuda")]
struct ActiveLaneGuard {
    counters: Arc<WorkerCounters>,
}

#[cfg(feature = "cuda")]
impl Drop for ActiveLaneGuard {
    fn drop(&mut self) {
        self.counters.active.fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(feature = "cuda")]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CudaWorkerSnapshot {
    pub queued_lanes: usize,
    pub active_lanes: usize,
    pub peak_active_lanes: usize,
    pub cumulative_batches: u64,
    pub cumulative_launches: u64,
    pub cumulative_failures: u64,
    pub cumulative_cancellations: u64,
    pub running: bool,
}

#[cfg(feature = "cuda")]
pub struct CudaWorker {
    sender: Sender<Command>,
    thread: Option<JoinHandle<()>>,
    counters: Arc<WorkerCounters>,
    stopping: Arc<AtomicBool>,
    shutdown: Option<CudaWorkerShutdown>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CudaWorkerShutdown {
    pub queued_at_request: usize,
    pub active_at_request: usize,
    pub queued_routes_rejected: usize,
    pub joined: bool,
}

#[cfg(feature = "cuda")]
impl CudaWorker {
    #[must_use = "CUDA worker creation failures must be handled"]
    pub fn start(maximum_batch_lanes: usize, batch_delay: Duration) -> Result<Self, CudaError> {
        if maximum_batch_lanes == 0 {
            return Err(CudaError::new(
                CudaFailureKind::Admission,
                "maximum CUDA batch lanes must be nonzero",
            ));
        }
        let (sender, receiver) = mpsc::channel();
        let counters = Arc::new(WorkerCounters::default());
        let worker_counters = Arc::clone(&counters);
        let stopping = Arc::new(AtomicBool::new(false));
        let worker_stopping = Arc::clone(&stopping);
        let thread = thread::Builder::new()
            .name("pathhydra-cuda".to_owned())
            .spawn(move || {
                worker_loop(
                    receiver,
                    maximum_batch_lanes,
                    batch_delay,
                    worker_counters,
                    worker_stopping,
                );
            })
            .map_err(|_| {
                CudaError::new(
                    CudaFailureKind::Worker,
                    "the CUDA worker thread could not be created",
                )
            })?;
        Ok(Self {
            sender,
            thread: Some(thread),
            counters,
            stopping,
            shutdown: None,
        })
    }

    pub fn submit(
        &self,
        resident: Arc<CudaResidentImage>,
        request: &RoutingRequest,
        algorithm: CudaAlgorithm,
        cancellation: Arc<AtomicBool>,
        reserved_search_bytes: usize,
    ) -> Result<CudaRouteOutput, CudaError> {
        if self.stopping.load(Ordering::Acquire) {
            return Err(worker_error("CUDA worker is shutting down"));
        }
        let (reply, result) = mpsc::channel();
        self.counters.queued.fetch_add(1, Ordering::Relaxed);
        if self
            .sender
            .send(Command::Route(RouteJob {
                image: RouteImage::Resident(resident),
                request: request.clone(),
                algorithm,
                cancellation,
                reserved_search_bytes,
                enqueued_at: Instant::now(),
                reply,
            }))
            .is_err()
        {
            self.counters.queued.fetch_sub(1, Ordering::Relaxed);
            return Err(worker_error("CUDA worker channel is closed"));
        }
        result
            .recv()
            .map_err(|_| worker_error("CUDA worker stopped before returning a route"))?
    }

    pub fn submit_partitioned(
        &self,
        image: Arc<CudaPartitionedImage>,
        request: &RoutingRequest,
        algorithm: CudaAlgorithm,
        cancellation: Arc<AtomicBool>,
        reserved_search_bytes: usize,
    ) -> Result<CudaRouteOutput, CudaError> {
        if self.stopping.load(Ordering::Acquire) {
            return Err(worker_error("CUDA worker is shutting down"));
        }
        let (reply, result) = mpsc::channel();
        self.counters.queued.fetch_add(1, Ordering::Relaxed);
        if self
            .sender
            .send(Command::Route(RouteJob {
                image: RouteImage::Partitioned(image),
                request: request.clone(),
                algorithm,
                cancellation,
                reserved_search_bytes,
                enqueued_at: Instant::now(),
                reply,
            }))
            .is_err()
        {
            self.counters.queued.fetch_sub(1, Ordering::Relaxed);
            return Err(worker_error("CUDA worker channel is closed"));
        }
        result
            .recv()
            .map_err(|_| worker_error("CUDA worker stopped before returning a route"))?
    }

    #[must_use]
    pub fn snapshot(&self) -> CudaWorkerSnapshot {
        CudaWorkerSnapshot {
            queued_lanes: self.counters.queued.load(Ordering::Relaxed),
            active_lanes: self.counters.active.load(Ordering::Relaxed),
            peak_active_lanes: self.counters.peak_active.load(Ordering::Relaxed),
            cumulative_batches: self.counters.batches.load(Ordering::Relaxed),
            cumulative_launches: self.counters.launches.load(Ordering::Relaxed),
            cumulative_failures: self.counters.failures.load(Ordering::Relaxed),
            cumulative_cancellations: self.counters.cancellations.load(Ordering::Relaxed),
            running: self
                .thread
                .as_ref()
                .is_some_and(|thread| !thread.is_finished()),
        }
    }

    /// Stops admission, rejects work still queued behind active batches, and
    /// joins the worker. Repeated calls return the first completed report.
    pub fn shutdown(&mut self) -> CudaWorkerShutdown {
        if let Some(report) = self.shutdown {
            return report;
        }
        self.stopping.store(true, Ordering::Release);
        let queued_at_request = self.counters.queued.load(Ordering::Acquire);
        let active_at_request = self.counters.active.load(Ordering::Acquire);
        let (reply, result) = mpsc::channel();
        let sent = self.sender.send(Command::Shutdown(reply)).is_ok();
        let queued_routes_rejected = if sent { result.recv().unwrap_or(0) } else { 0 };
        let joined = self
            .thread
            .take()
            .is_none_or(|thread| thread.join().is_ok());
        let report = CudaWorkerShutdown {
            queued_at_request,
            active_at_request,
            queued_routes_rejected,
            joined,
        };
        self.shutdown = Some(report);
        report
    }
}

#[cfg(feature = "cuda")]
impl Drop for CudaWorker {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

#[cfg(feature = "cuda")]
fn worker_loop(
    receiver: Receiver<Command>,
    maximum_batch_lanes: usize,
    batch_delay: Duration,
    counters: Arc<WorkerCounters>,
    stopping: Arc<AtomicBool>,
) {
    let mut deferred = VecDeque::new();
    loop {
        let command = deferred.pop_front().or_else(|| receiver.recv().ok());
        let Some(command) = command else {
            return;
        };
        let first = match command {
            Command::Route(first) if !stopping.load(Ordering::Acquire) => first,
            Command::Route(job) => {
                reject_queued(
                    job,
                    &counters,
                    "CUDA worker rejected a route during shutdown",
                );
                continue;
            }
            Command::Shutdown(reply) => {
                let mut rejected = 0;
                for command in deferred.drain(..) {
                    if let Command::Route(job) = command {
                        rejected += 1;
                        reject_queued(
                            job,
                            &counters,
                            "CUDA worker shut down before admitting the deferred route",
                        );
                    }
                }
                rejected += drain_queued(&receiver, &counters);
                let _ = reply.send(rejected);
                return;
            }
        };
        let collected_at = Instant::now();
        let mut batch = Vec::new();
        if batch.try_reserve(1).is_err() {
            reject_queued_with_error(
                first,
                &counters,
                CudaError::new(
                    CudaFailureKind::Allocation,
                    "the CUDA worker could not allocate its batch queue",
                ),
            );
            continue;
        }
        batch.push(first);
        while batch.len() < maximum_batch_lanes {
            let remaining = batch_delay.saturating_sub(collected_at.elapsed());
            if remaining.is_zero() {
                break;
            }
            match receiver.recv_timeout(remaining) {
                Ok(Command::Route(job)) if stopping.load(Ordering::Acquire) => {
                    reject_queued(
                        job,
                        &counters,
                        "CUDA worker rejected a collected route during shutdown",
                    );
                }
                Ok(Command::Route(job))
                    if job.image.ptr_eq(&batch[0].image) && job.algorithm == batch[0].algorithm =>
                {
                    if batch.try_reserve(1).is_err() {
                        reject_queued_with_error(
                            job,
                            &counters,
                            CudaError::new(
                                CudaFailureKind::Allocation,
                                "the CUDA worker could not grow its batch queue",
                            ),
                        );
                        break;
                    }
                    batch.push(job);
                }
                Ok(command) => deferred.push_back(command),
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        let batch_collection_duration = collected_at.elapsed();
        let width = batch.len();
        std::thread::scope(|scope| {
            let mut spawned_lanes = Vec::new();
            if spawned_lanes.try_reserve(width).is_err() {
                for job in batch {
                    reject_queued_with_error(
                        job,
                        &counters,
                        CudaError::new(
                            CudaFailureKind::Allocation,
                            "the CUDA worker could not allocate its lane-join queue",
                        ),
                    );
                }
                return;
            }
            counters.batches.fetch_add(1, Ordering::Relaxed);
            counters.active.store(width, Ordering::Relaxed);
            let _ =
                counters
                    .peak_active
                    .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |peak| {
                        Some(peak.max(width))
                    });
            for (lane_index, job) in batch.into_iter().enumerate() {
                let lane_counters = Arc::clone(&counters);
                lane_counters.queued.fetch_sub(1, Ordering::Relaxed);
                let fallback_counters = Arc::clone(&lane_counters);
                let fallback_reply = job.reply.clone();
                match thread::Builder::new()
                    .name(format!("pathhydra-cuda-lane-{lane_index}"))
                    .spawn_scoped(scope, move || {
                        let _active_lane = ActiveLaneGuard {
                            counters: Arc::clone(&lane_counters),
                        };
                        let queue_duration = job.enqueued_at.elapsed();
                        let mut output = job.image.route(
                            &job.request,
                            job.algorithm,
                            &job.cancellation,
                            job.reserved_search_bytes,
                        );
                        match &mut output {
                            Ok(output) => {
                                output.diagnostics.first_destination_duration = output
                                    .diagnostics
                                    .first_destination_duration
                                    .map(|duration| duration.saturating_add(queue_duration));
                                output.diagnostics.queue_duration = queue_duration;
                                output.diagnostics.batch_collection_duration =
                                    batch_collection_duration;
                                output.diagnostics.batch_width = width;
                                output.diagnostics.lane_index = lane_index;
                                lane_counters.launches.fetch_add(
                                    output.diagnostics.kernel_launches,
                                    Ordering::Relaxed,
                                );
                                if job.cancellation.load(Ordering::Acquire) {
                                    lane_counters.cancellations.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                            Err(_) => {
                                lane_counters.failures.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        let _ = job.reply.send(output);
                    }) {
                    Ok(handle) => {
                        spawned_lanes.push((handle, fallback_counters, fallback_reply));
                    }
                    Err(_) => {
                        fallback_counters.active.fetch_sub(1, Ordering::Relaxed);
                        fallback_counters.failures.fetch_add(1, Ordering::Relaxed);
                        let _ = fallback_reply.send(Err(CudaError::new(
                            CudaFailureKind::Worker,
                            "a CUDA batch lane thread could not be created",
                        )));
                    }
                }
            }
            for (handle, fallback_counters, fallback_reply) in spawned_lanes {
                if handle.join().is_err() {
                    fallback_counters.failures.fetch_add(1, Ordering::Relaxed);
                    let _ = fallback_reply.send(Err(CudaError::new(
                        CudaFailureKind::Worker,
                        "a CUDA batch lane panicked before returning a route",
                    )));
                }
            }
        });
    }
}

#[cfg(feature = "cuda")]
fn drain_queued(receiver: &Receiver<Command>, counters: &WorkerCounters) -> usize {
    let mut rejected = 0;
    while let Ok(command) = receiver.try_recv() {
        if let Command::Route(job) = command {
            rejected += 1;
            reject_queued(
                job,
                counters,
                "CUDA worker shut down before admitting the queued route",
            );
        }
    }
    rejected
}

#[cfg(feature = "cuda")]
fn reject_queued(job: RouteJob, counters: &WorkerCounters, message: &'static str) {
    counters.queued.fetch_sub(1, Ordering::Relaxed);
    let _ = job.reply.send(Err(worker_error(message)));
}

#[cfg(feature = "cuda")]
fn reject_queued_with_error(job: RouteJob, counters: &WorkerCounters, error: CudaError) {
    counters.queued.fetch_sub(1, Ordering::Relaxed);
    counters.failures.fetch_add(1, Ordering::Relaxed);
    let _ = job.reply.send(Err(error));
}

#[cfg(feature = "cuda")]
fn worker_error(message: &'static str) -> CudaError {
    CudaError::new(CudaFailureKind::Worker, message)
}
