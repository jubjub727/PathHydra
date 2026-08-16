use std::{
    collections::BTreeSet,
    sync::{Arc, Barrier},
    thread,
};

use pathhydra_api::{
    CancellationOutcomeDto, CompletionReasonDto, ConfirmedRecordDto, MutationDurableResultDto,
    PathHydra, PayloadDto, RelationProfileDto, RequestIdAllocation, RequestIdDto,
    RoutingRequestDto, SearchBudgetDto, TiePolicyDto,
};

#[test]
fn concurrent_automatic_request_ids_are_strictly_unique() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<PathHydra>();
    assert_send_sync::<pathhydra_api::RequestHandle>();
    assert_send_sync::<pathhydra_api::ApiError>();

    let directory = tempfile::tempdir().expect("temporary root");
    let api = Arc::new(PathHydra::open(directory.path().join("catalog")).expect("open facade"));

    let workers = (0..8)
        .map(|_| {
            let api = Arc::clone(&api);
            thread::spawn(move || {
                (0..64)
                    .map(|_| {
                        api.request_handle(RequestIdAllocation::Automatic)
                            .expect("allocate request")
                    })
                    .collect::<Vec<_>>()
            })
        })
        .collect::<Vec<_>>();
    let handles = workers
        .into_iter()
        .flat_map(|worker| worker.join().expect("request worker"))
        .collect::<Vec<_>>();
    let ids = handles
        .iter()
        .map(|handle| handle.id().as_u64().expect("canonical assigned ID"))
        .collect::<BTreeSet<_>>();

    assert_eq!(ids.len(), 8 * 64);
    assert_eq!(ids.first(), Some(&1));
    assert_eq!(ids.last(), Some(&(8 * 64)));
}

#[test]
fn supplied_ids_advance_high_water_and_max_exhaustion_is_typed() {
    let directory = tempfile::tempdir().expect("temporary root");
    let api = PathHydra::open(directory.path().join("catalog")).expect("open facade");

    assert_eq!(
        api.request_handle(RequestIdAllocation::Supplied(RequestIdDto::from_u64(8_000)))
            .expect("supplied correlation ID")
            .id()
            .as_u64()
            .expect("canonical assigned ID"),
        8_000
    );
    assert_eq!(
        api.request_handle(RequestIdAllocation::Automatic)
            .expect("automatic follows supplied")
            .id()
            .as_u64()
            .expect("canonical assigned ID"),
        8_001
    );
    assert_eq!(
        api.request_handle(RequestIdAllocation::Supplied(RequestIdDto::from_u64(8_000)))
            .expect_err("reuse is rejected before engine admission")
            .code(),
        "request_id_not_monotonic"
    );
    assert_eq!(
        api.request_handle(RequestIdAllocation::Supplied(RequestIdDto::from_u64(
            u64::MAX,
        )))
        .expect("MAX is allocated exactly once")
        .id()
        .as_u64()
        .expect("canonical assigned ID"),
        u64::MAX
    );
    assert_eq!(
        api.request_handle(RequestIdAllocation::Automatic)
            .expect_err("MAX exhausts automatic allocation")
            .code(),
        "request_id_exhausted"
    );
}

#[test]
fn owned_handle_cancellation_is_repeatable_before_admission() {
    let directory = tempfile::tempdir().expect("temporary root");
    let api = PathHydra::open(directory.path().join("catalog")).expect("open facade");
    let handle = api
        .request_handle(RequestIdAllocation::Automatic)
        .expect("allocate request");

    assert_eq!(
        handle.cancel().expect("first cancellation"),
        CancellationOutcomeDto::Signalled
    );
    assert_eq!(
        handle.cancel().expect("repeat cancellation"),
        CancellationOutcomeDto::AlreadySignalled
    );
    assert_eq!(
        api.cancel(&RequestIdDto::from_u64(u64::MAX))
            .expect("unknown cancellation"),
        CancellationOutcomeDto::UnknownRequest
    );
}

#[test]
fn cancellation_race_before_or_during_engine_admission_is_not_lost() {
    let directory = tempfile::tempdir().expect("temporary root");
    let api = PathHydra::open(directory.path().join("catalog")).expect("open facade");
    let candidate = api
        .insert_node_candidate("origin".to_owned(), PayloadDto::from_bytes(b"payload"))
        .expect("insert origin candidate");
    let confirmed = api.confirm_candidate(&candidate).expect("confirm origin");
    let MutationDurableResultDto::Confirmed(ConfirmedRecordDto::Node(origin)) =
        confirmed.durable_result
    else {
        panic!("expected confirmed node")
    };

    let handle = api
        .request_handle(RequestIdAllocation::Automatic)
        .expect("allocate request");
    let cancellation = handle.clone();
    let barrier = Arc::new(Barrier::new(2));
    let route_barrier = Arc::clone(&barrier);
    let worker = thread::spawn(move || {
        let request = RoutingRequestDto {
            origin: origin.id.clone(),
            destinations: vec![origin.id; 4_000],
            profile: RelationProfileDto::default(),
            return_paths: false,
            budget: SearchBudgetDto::Unlimited,
            tie_policy: TiePolicyDto::StablePredecessor,
        };
        route_barrier.wait();
        handle.route(&request)
    });
    barrier.wait();
    let cancellation_outcome = cancellation.cancel().expect("cancel request race");
    let response = worker
        .join()
        .expect("route worker")
        .expect("route response");

    assert!(matches!(
        cancellation_outcome,
        CancellationOutcomeDto::Signalled | CancellationOutcomeDto::AlreadyCompleted
    ));
    assert!(matches!(
        response.response.completion_reason,
        CompletionReasonDto::Cancelled | CompletionReasonDto::AllDestinationsFinalized
    ));
}
