#![cfg(feature = "cuda")]

use std::sync::{Arc, atomic::AtomicBool};

use pathhydra_core::{ConfirmedRecord, NodeId, RelationId};
use pathhydra_cuda::{
    CudaAlgorithm, CudaContextOwner, CudaResidentImage, DeltaConfiguration, estimate_search_bytes,
};
use pathhydra_routing::{
    DestinationState, RelationMultiplier, RelationProfile, RelationUse, RoutingImage,
    RoutingRequest, SearchBudget, TiePolicy, route,
};
use pathhydra_store::Catalog;

fn fixture() -> (Arc<RoutingImage>, RoutingRequest) {
    let directory = tempfile::tempdir().unwrap();
    let catalog = Catalog::open(directory.path()).unwrap();
    let nodes: Vec<_> = (0..6)
        .map(|index| {
            let candidate = catalog
                .insert_node_candidate(format!("node-{index}"))
                .unwrap();
            let ConfirmedRecord::Node(node) =
                catalog.confirm_validated_candidate(candidate).unwrap()
            else {
                panic!()
            };
            node.id()
        })
        .collect();
    let relations: Vec<_> = ["road", "rail"]
        .into_iter()
        .map(|name| {
            let candidate = catalog.insert_relation_candidate(name).unwrap();
            let ConfirmedRecord::Relation(relation) =
                catalog.confirm_validated_candidate(candidate).unwrap()
            else {
                panic!()
            };
            relation.id()
        })
        .collect();
    for (source, destination, relation, weight) in [
        (0, 1, 0, 1.0),
        (1, 2, 0, 0.0),
        (2, 1, 0, 0.0),
        (2, 3, 0, 0.2),
        (0, 3, 1, 0.8),
        (3, 4, 1, 0.05),
        (0, 4, 0, 1.0),
        (0, 4, 1, 0.6),
    ] {
        let candidate = catalog
            .insert_edge_candidate(
                nodes[source],
                nodes[destination],
                relations[relation],
                weight,
            )
            .unwrap();
        catalog.confirm_validated_candidate(candidate).unwrap();
    }
    let image =
        Arc::new(RoutingImage::compile(&catalog.confirmed_graph_records().unwrap()).unwrap());
    let request = RoutingRequest::new(
        nodes[0],
        [
            nodes[4],
            nodes[3],
            NodeId::from_u64(u64::MAX),
            nodes[4],
            nodes[5],
            nodes[0],
        ],
        RelationProfile::new([
            (
                relations[0],
                RelationUse::Enabled(RelationMultiplier::new(1.0).unwrap()),
            ),
            (
                relations[1],
                RelationUse::Enabled(RelationMultiplier::new(0.25).unwrap()),
            ),
        ]),
        false,
        SearchBudget::Unlimited,
        TiePolicy::StablePredecessor,
    );
    (image, request)
}

fn assert_states_equal(
    cpu: &[pathhydra_routing::DestinationResult],
    cuda: &[pathhydra_routing::DestinationResult],
) {
    assert_eq!(cpu.len(), cuda.len());
    for (cpu, cuda) in cpu.iter().zip(cuda) {
        assert_eq!(cpu.destination(), cuda.destination());
        match (cpu.state(), cuda.state()) {
            (DestinationState::Exact(cpu), DestinationState::Exact(cuda)) => {
                assert_eq!(
                    cpu.logical_distance().to_bits(),
                    cuda.logical_distance().to_bits()
                );
                assert!(cuda.path().is_none());
            }
            (left, right) => assert_eq!(left, right),
        }
    }
}

#[test]
fn frontier_and_delta_match_the_cpu_oracle_bit_for_bit() {
    let (image, request) = fixture();
    let cpu = route(&image, &request).unwrap();
    let context = CudaContextOwner::initialize(0).unwrap();
    let resident = CudaResidentImage::upload(context, image.clone(), usize::MAX, 0).unwrap();
    assert_eq!(
        resident.device_array_lengths(),
        (
            image.node_count() + 1,
            image.adjacency_count(),
            image.adjacency_count(),
            image.adjacency_count()
        )
    );
    for algorithm in [
        CudaAlgorithm::Frontier,
        CudaAlgorithm::DeltaStepping(DeltaConfiguration::new(0.1).unwrap()),
    ] {
        let reserved = estimate_search_bytes(
            image.node_count(),
            image.relation_kind_count(),
            request.destinations().len(),
            algorithm,
        )
        .unwrap();
        let cuda = resident
            .route(&request, algorithm, &AtomicBool::new(false), reserved)
            .unwrap();
        assert_states_equal(cpu.results(), cuda.response.results());
        assert!(cuda.diagnostics.kernel_launches > 0);
    }
}

#[test]
fn cancelled_cuda_search_never_reports_discovered_tentative_values_as_exact() {
    let (image, request) = fixture();
    let context = CudaContextOwner::initialize(0).unwrap();
    let resident = CudaResidentImage::upload(context, image, usize::MAX, 0).unwrap();
    let cuda = resident
        .route(&request, CudaAlgorithm::Frontier, &AtomicBool::new(true), 1)
        .unwrap();
    assert_eq!(cuda.diagnostics.kernel_launches, 0);
    assert!(matches!(
        cuda.response.results()[0].state(),
        DestinationState::Incomplete
    ));
    assert!(matches!(
        cuda.response.results()[2].state(),
        DestinationState::MissingNode
    ));
    assert!(
        matches!(cuda.response.results()[5].state(), DestinationState::Exact(exact) if exact.logical_distance() == 0.0)
    );
}

#[test]
fn complete_profiles_remain_distinct_between_independent_searches() {
    let (image, request) = fixture();
    let context = CudaContextOwner::initialize(0).unwrap();
    let resident = CudaResidentImage::upload(context, image, usize::MAX, 0).unwrap();
    let disabled = RoutingRequest::new(
        request.origin(),
        request.destinations().iter().copied(),
        RelationProfile::new([
            (RelationId::from_u64(1), RelationUse::Disabled),
            (RelationId::from_u64(2), RelationUse::Disabled),
        ]),
        false,
        SearchBudget::Unlimited,
        TiePolicy::StablePredecessor,
    );
    let enabled = resident
        .route(
            &request,
            CudaAlgorithm::Frontier,
            &AtomicBool::new(false),
            1,
        )
        .unwrap();
    let disabled = resident
        .route(
            &disabled,
            CudaAlgorithm::Frontier,
            &AtomicBool::new(false),
            1,
        )
        .unwrap();
    assert_ne!(
        enabled.response.results()[0].state(),
        disabled.response.results()[0].state()
    );
}
