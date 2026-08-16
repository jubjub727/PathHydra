use std::{cell::Cell, sync::atomic::AtomicBool};

use pathhydra_core::{
    BaseWeight, EdgeId, EdgeRecord, NodeId, NodeRecord, RelationId, RelationName, RelationRecord,
};
use pathhydra_routing::{
    CancellationSignal, CompletionReason, DestinationState, RelationMultiplier, RelationProfile,
    RelationUse, RoutingImage, RoutingRequest, SearchBudget, TiePolicy, estimate_cpu_working_set,
    route_controlled,
};
use pathhydra_store::ConfirmedGraphRecords;

fn fixture() -> (RoutingImage, RoutingRequest) {
    let nodes = (1..=3)
        .map(|id| {
            NodeRecord::new(
                NodeId::from_u64(id),
                format!("n{id}").into(),
                Vec::<u8>::new().into(),
            )
        })
        .collect();
    let relation = RelationRecord::new(RelationId::from_u64(1), RelationName::from("kind"));
    let edges = vec![
        EdgeRecord::new(
            EdgeId::from_u64(1),
            NodeId::from_u64(1),
            NodeId::from_u64(2),
            relation.id(),
            BaseWeight::new(0.2).unwrap(),
        ),
        EdgeRecord::new(
            EdgeId::from_u64(2),
            NodeId::from_u64(2),
            NodeId::from_u64(3),
            relation.id(),
            BaseWeight::new(0.3).unwrap(),
        ),
    ];
    let image = RoutingImage::compile(&ConfirmedGraphRecords::new(
        nodes,
        vec![relation.clone()],
        edges,
    ))
    .unwrap();
    let request = RoutingRequest::new(
        NodeId::from_u64(1),
        [
            NodeId::from_u64(1),
            NodeId::from_u64(3),
            NodeId::from_u64(99),
        ],
        RelationProfile::new([(
            relation.id(),
            RelationUse::Enabled(RelationMultiplier::new(1.0).unwrap()),
        )]),
        true,
        SearchBudget::Unlimited,
        TiePolicy::StablePredecessor,
    );
    (image, request)
}

#[test]
fn estimate_is_nonzero_and_accounts_for_paths() {
    let (image, request) = fixture();
    let with_paths = estimate_cpu_working_set(&image, &request).unwrap();
    let without_paths = estimate_cpu_working_set(
        &image,
        &RoutingRequest::new(
            request.origin(),
            request.destinations().iter().copied(),
            request.profile().clone(),
            false,
            request.budget(),
            request.tie_policy(),
        ),
    )
    .unwrap();
    assert!(with_paths.bytes() > without_paths.bytes());
    assert_eq!(with_paths.frontier_entries(), image.adjacency_count() + 1);
}

#[test]
fn presignalled_cancellation_preserves_origin_and_missing_destinations() {
    let (image, request) = fixture();
    let cancelled = AtomicBool::new(true);
    let (response, diagnostics) = route_controlled(&image, &request, &cancelled).unwrap();
    assert_eq!(response.completion_reason(), CompletionReason::Cancelled);
    assert!(matches!(
        response.results()[0].state(),
        DestinationState::Exact(_)
    ));
    assert!(matches!(
        response.results()[1].state(),
        DestinationState::Incomplete
    ));
    assert!(matches!(
        response.results()[2].state(),
        DestinationState::MissingNode
    ));
    assert_eq!(diagnostics.examined_edges, 0);
}

#[test]
fn topology_limit_is_checked_before_image_construction() {
    let (image, _) = fixture();
    let required = image.manifest().byte_counts().total();
    let records = ConfirmedGraphRecords::new(
        (1..=3)
            .map(|id| {
                NodeRecord::new(
                    NodeId::from_u64(id),
                    format!("n{id}").into(),
                    Vec::<u8>::new().into(),
                )
            })
            .collect(),
        vec![RelationRecord::new(RelationId::from_u64(1), "kind".into())],
        vec![],
    );
    assert!(RoutingImage::compile_with_limit(&records, required.saturating_sub(required)).is_err());
}

#[test]
fn duplicate_destinations_share_reconstructed_path_storage_work() {
    let (image, request) = fixture();
    let request = RoutingRequest::new(
        request.origin(),
        [NodeId::from_u64(3), NodeId::from_u64(3)],
        request.profile().clone(),
        true,
        SearchBudget::Unlimited,
        TiePolicy::StablePredecessor,
    );
    let (response, diagnostics) =
        route_controlled(&image, &request, &AtomicBool::new(false)).unwrap();
    assert!(
        response
            .results()
            .iter()
            .all(|result| matches!(result.state(), DestinationState::Exact(_)))
    );
    assert_eq!(diagnostics.path_reconstruction_steps, 2);
    assert_eq!(diagnostics.unique_present_destinations, 1);
}

struct CancelOnCall {
    calls: Cell<usize>,
    cancel_at: usize,
}

impl CancellationSignal for CancelOnCall {
    fn is_cancelled(&self) -> bool {
        let call = self.calls.get() + 1;
        self.calls.set(call);
        call >= self.cancel_at
    }
}

#[test]
fn cancellation_is_checked_during_expansion_and_wins_over_budget() {
    let (image, request) = fixture();
    let request = RoutingRequest::new(
        request.origin(),
        [NodeId::from_u64(3)],
        request.profile().clone(),
        false,
        SearchBudget::ExaminedEdges(0),
        request.tie_policy(),
    );
    let signal = CancelOnCall {
        calls: Cell::new(0),
        cancel_at: 3,
    };
    let (response, diagnostics) = route_controlled(&image, &request, &signal).unwrap();
    assert_eq!(response.completion_reason(), CompletionReason::Cancelled);
    assert!(matches!(
        response.results()[0].state(),
        DestinationState::Incomplete
    ));
    assert_eq!(diagnostics.examined_edges, 0);
}

#[test]
fn cancellation_during_reconstruction_keeps_finalized_distance_exact() {
    let (image, request) = fixture();
    let request = RoutingRequest::new(
        request.origin(),
        [NodeId::from_u64(3)],
        request.profile().clone(),
        true,
        SearchBudget::Unlimited,
        request.tie_policy(),
    );
    let signal = CancelOnCall {
        calls: Cell::new(0),
        cancel_at: 7,
    };
    let (response, _) = route_controlled(&image, &request, &signal).unwrap();
    assert_eq!(response.completion_reason(), CompletionReason::Cancelled);
    let DestinationState::Exact(exact) = response.results()[0].state() else {
        panic!()
    };
    assert_eq!(
        exact.logical_distance(),
        f64::from(0.2_f32) + f64::from(0.3_f32)
    );
    assert!(exact.path().is_none());
}
