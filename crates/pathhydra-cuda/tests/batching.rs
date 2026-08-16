#[cfg(feature = "cuda")]
use pathhydra_cuda::CudaWorker;
use pathhydra_cuda::{
    CudaAdmissionController, CudaAlgorithm, CudaMemoryLimits, estimate_search_bytes,
    estimate_search_bytes_for_request,
};

#[test]
fn lane_and_total_byte_limits_are_raii_and_do_not_leak() {
    let admission = CudaAdmissionController::new(CudaMemoryLimits {
        maximum_search_bytes: 100,
        maximum_concurrent_searches: 2,
    });
    let first = admission.reserve(40).unwrap();
    let second = admission.reserve(60).unwrap();
    assert!(admission.reserve(1).is_err());
    let snapshot = admission.snapshot().unwrap();
    assert_eq!(snapshot.active, 2);
    assert_eq!(snapshot.reserved_bytes, 100);
    drop(first);
    assert_eq!(admission.snapshot().unwrap().reserved_bytes, 60);
    drop(second);
    let replacement = admission.reserve(100).unwrap();
    assert_eq!(replacement.bytes(), 100);
    drop(replacement);
    assert_eq!(admission.snapshot().unwrap().active, 0);
}

#[test]
fn one_request_cannot_exceed_the_complete_search_reservation() {
    let admission = CudaAdmissionController::new(CudaMemoryLimits {
        maximum_search_bytes: 64,
        maximum_concurrent_searches: 1,
    });
    assert!(admission.reserve(65).is_err());
    assert_eq!(admission.snapshot().unwrap().active, 0);
}

#[test]
fn path_evidence_reservation_accounts_for_predecessor_state() {
    let adjacency_count = 512;
    let distance_only =
        estimate_search_bytes(128, 4, adjacency_count, 8, CudaAlgorithm::Frontier).unwrap();
    let cpu_evidence_working_bytes = 1_048_576;
    let with_paths = estimate_search_bytes_for_request(
        128,
        4,
        adjacency_count,
        8,
        CudaAlgorithm::Frontier,
        Some(cpu_evidence_working_bytes),
    )
    .unwrap();
    assert_eq!(with_paths - distance_only, cpu_evidence_working_bytes);
}

#[test]
fn task_compaction_reservation_covers_host_and_device_overlap_per_edge() {
    let without_edges = estimate_search_bytes(128, 4, 0, 8, CudaAlgorithm::Frontier).unwrap();
    let with_edges = estimate_search_bytes(128, 4, 512, 8, CudaAlgorithm::Frontier).unwrap();
    assert_eq!(with_edges - without_edges, 512 * 24);
}

#[test]
#[cfg(feature = "cuda")]
fn explicit_worker_shutdown_is_joined_and_idempotent() {
    let mut worker = CudaWorker::start(2, std::time::Duration::ZERO);
    let first = worker.shutdown();
    assert!(first.joined);
    assert_eq!(first.queued_at_request, 0);
    assert_eq!(first.active_at_request, 0);
    assert_eq!(first.queued_routes_rejected, 0);
    assert_eq!(worker.shutdown(), first);
    assert!(!worker.snapshot().running);
}
