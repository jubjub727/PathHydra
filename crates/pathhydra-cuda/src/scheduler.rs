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

#[cfg(feature = "cuda")]
use crate::{CudaError, CudaFailureKind, CudaResidentImage};

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
    pub frontier_high_water: u32,
}

#[derive(Clone, Debug)]
pub struct CudaRouteOutput {
    pub response: RoutingResponse,
    pub diagnostics: CudaRouteDiagnostics,
}

#[cfg(feature = "cuda")]
struct RouteJob {
    resident: Arc<CudaResidentImage>,
    request: RoutingRequest,
    algorithm: CudaAlgorithm,
    cancellation: Arc<AtomicBool>,
    reserved_search_bytes: usize,
    enqueued_at: Instant,
    reply: Sender<Result<CudaRouteOutput, CudaError>>,
}

#[cfg(feature = "cuda")]
enum Command {
    Route(RouteJob),
    Shutdown,
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
}

#[cfg(feature = "cuda")]
impl CudaWorker {
    #[must_use]
    pub fn start(maximum_batch_lanes: usize, batch_delay: Duration) -> Self {
        let (sender, receiver) = mpsc::channel();
        let counters = Arc::new(WorkerCounters::default());
        let worker_counters = Arc::clone(&counters);
        let thread = thread::Builder::new()
            .name("pathhydra-cuda".to_owned())
            .spawn(move || {
                worker_loop(
                    receiver,
                    maximum_batch_lanes.max(1),
                    batch_delay,
                    worker_counters,
                );
            })
            .expect("the CUDA worker thread must be creatable");
        Self {
            sender,
            thread: Some(thread),
            counters,
        }
    }

    pub fn submit(
        &self,
        resident: Arc<CudaResidentImage>,
        request: &RoutingRequest,
        algorithm: CudaAlgorithm,
        cancellation: Arc<AtomicBool>,
        reserved_search_bytes: usize,
    ) -> Result<CudaRouteOutput, CudaError> {
        let (reply, result) = mpsc::channel();
        self.counters.queued.fetch_add(1, Ordering::Relaxed);
        if self
            .sender
            .send(Command::Route(RouteJob {
                resident,
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
}

#[cfg(feature = "cuda")]
impl Drop for CudaWorker {
    fn drop(&mut self) {
        let _ = self.sender.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(feature = "cuda")]
fn worker_loop(
    receiver: Receiver<Command>,
    maximum_batch_lanes: usize,
    batch_delay: Duration,
    counters: Arc<WorkerCounters>,
) {
    let mut deferred = VecDeque::new();
    loop {
        let command = deferred.pop_front().or_else(|| receiver.recv().ok());
        let Some(command) = command else {
            return;
        };
        let Command::Route(first) = command else {
            for command in deferred.drain(..) {
                if let Command::Route(job) = command {
                    let _ = job.reply.send(Err(worker_error(
                        "CUDA worker shut down before admitting the deferred route",
                    )));
                }
            }
            drain_queued(&receiver);
            return;
        };
        let collected_at = Instant::now();
        let mut batch = vec![first];
        while batch.len() < maximum_batch_lanes {
            let remaining = batch_delay.saturating_sub(collected_at.elapsed());
            if remaining.is_zero() {
                break;
            }
            match receiver.recv_timeout(remaining) {
                Ok(Command::Route(job))
                    if Arc::ptr_eq(&job.resident, &batch[0].resident)
                        && job.algorithm == batch[0].algorithm =>
                {
                    batch.push(job);
                }
                Ok(command) => deferred.push_back(command),
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        counters.batches.fetch_add(1, Ordering::Relaxed);
        counters.active.store(batch.len(), Ordering::Relaxed);
        let _ = counters
            .peak_active
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |peak| {
                Some(peak.max(batch.len()))
            });
        let width = batch.len();
        for (lane_index, job) in batch.into_iter().enumerate() {
            counters.queued.fetch_sub(1, Ordering::Relaxed);
            let queue_duration = job.enqueued_at.elapsed();
            let mut output = job.resident.route(
                &job.request,
                job.algorithm,
                &job.cancellation,
                job.reserved_search_bytes,
            );
            match &mut output {
                Ok(output) => {
                    output.diagnostics.queue_duration = queue_duration;
                    output.diagnostics.batch_collection_duration = collected_at.elapsed();
                    output.diagnostics.batch_width = width;
                    output.diagnostics.lane_index = lane_index;
                    counters
                        .launches
                        .fetch_add(output.diagnostics.kernel_launches, Ordering::Relaxed);
                    if job.cancellation.load(Ordering::Acquire) {
                        counters.cancellations.fetch_add(1, Ordering::Relaxed);
                    }
                }
                Err(_) => {
                    counters.failures.fetch_add(1, Ordering::Relaxed);
                }
            }
            let _ = job.reply.send(output);
        }
        counters.active.store(0, Ordering::Relaxed);
    }
}

#[cfg(feature = "cuda")]
fn drain_queued(receiver: &Receiver<Command>) {
    while let Ok(command) = receiver.try_recv() {
        if let Command::Route(job) = command {
            let _ = job.reply.send(Err(worker_error(
                "CUDA worker shut down before admitting the queued route",
            )));
        }
    }
}

#[cfg(feature = "cuda")]
fn worker_error(message: &'static str) -> CudaError {
    CudaError::new(CudaFailureKind::Worker, message)
}
