use pathhydra_cuda::{CudaAdmissionController, CudaMemoryLimits};

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
