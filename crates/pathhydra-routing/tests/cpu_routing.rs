use std::{sync::Arc, thread};

use pathhydra_core::{
    BaseWeight, EdgeId, EdgeRecord, NodeId, NodeName, NodePayload, NodeRecord, RelationId,
    RelationName, RelationRecord,
};
use pathhydra_routing::{
    CompletionReason, DestinationState, ProfileError, RelationMultiplier, RelationProfile,
    RelationUse, RoutingError, RoutingImage, RoutingRequest, SearchBudget, TiePolicy, route,
};
use pathhydra_store::{Catalog, ConfirmedGraphRecords, ConfirmedRecord};
use tempfile::TempDir;

#[test]
fn context_profiles_choose_different_exact_directed_paths_with_complete_evidence() {
    let image = context_image();
    let direct = route(
        &image,
        &request(1, [3], profile([(10, 0.5), (20, 1.0)]), true),
    )
    .unwrap();
    let scenic = route(
        &image,
        &request(1, [3], profile([(10, 1.0), (20, 0.5)]), true),
    )
    .unwrap();

    let direct = exact(&direct.results()[0]);
    let direct_distance = f64::from(0.8_f32) * f64::from(0.5_f32);
    assert_eq!(direct.logical_distance(), direct_distance);
    let direct_step = &direct.path().unwrap().steps()[0];
    assert_eq!(direct_step.edge_id(), EdgeId::from_u64(101));
    assert_eq!(direct_step.source(), NodeId::from_u64(1));
    assert_eq!(direct_step.destination(), NodeId::from_u64(3));
    assert_eq!(direct_step.relation_id(), RelationId::from_u64(10));
    assert_eq!(direct_step.base_weight(), BaseWeight::new(0.8).unwrap());
    assert_eq!(direct_step.multiplier(), multiplier(0.5));
    assert_eq!(direct_step.effective_weight(), direct_distance);

    let scenic = exact(&scenic.results()[0]);
    assert_eq!(scenic.logical_distance(), f64::from(0.3_f32));
    assert_eq!(
        scenic
            .path()
            .unwrap()
            .steps()
            .iter()
            .map(|step| step.edge_id().as_u64())
            .collect::<Vec<_>>(),
        [102, 103]
    );
    assert_eq!(
        scenic
            .path()
            .unwrap()
            .steps()
            .iter()
            .map(|step| step.effective_weight())
            .sum::<f64>(),
        scenic.logical_distance()
    );

    let backward = route(
        &image,
        &request(3, [1], profile([(10, 1.0), (20, 1.0)]), false),
    )
    .unwrap();
    assert!(matches!(
        backward.results()[0].state(),
        DestinationState::Unreachable
    ));
}

#[test]
fn disabled_differs_from_enabled_zero_and_zero_cycles_terminate() {
    let image = image(
        &[1, 2, 3],
        &[10, 20],
        &[
            (1, 1, 2, 10, 1.0),
            (2, 2, 1, 10, 1.0),
            (3, 2, 2, 10, 0.0),
            (4, 2, 3, 20, 1.0),
        ],
    );
    let disabled = RelationProfile::new([
        (RelationId::from_u64(10), RelationUse::Disabled),
        (
            RelationId::from_u64(20),
            RelationUse::Enabled(multiplier(1.0)),
        ),
    ]);
    let response = route(&image, &request(1, [3], disabled, false)).unwrap();
    assert!(matches!(
        response.results()[0].state(),
        DestinationState::Unreachable
    ));
    assert_eq!(response.examined_edges(), 1);

    let zero = RelationProfile::new([
        (
            RelationId::from_u64(10),
            RelationUse::Enabled(multiplier(0.0)),
        ),
        (
            RelationId::from_u64(20),
            RelationUse::Enabled(multiplier(1.0)),
        ),
    ]);
    let response = route(&image, &request(1, [3], zero, true)).unwrap();
    let exact = exact(&response.results()[0]);
    assert_eq!(exact.logical_distance(), 1.0);
    assert_eq!(exact.path().unwrap().steps().len(), 2);
    assert!(response.finalized_nodes() <= 3);
}

#[test]
fn destination_membership_order_and_origin_contract_are_exact() {
    let image = context_image();
    let response = route(
        &image,
        &request(1, [3, 999, 3, 1], profile([(10, 1.0), (20, 1.0)]), true),
    )
    .unwrap();
    assert_eq!(
        response
            .results()
            .iter()
            .map(|result| result.destination().as_u64())
            .collect::<Vec<_>>(),
        [3, 999, 3, 1]
    );
    assert!(matches!(
        response.results()[1].state(),
        DestinationState::MissingNode
    ));
    let multi_hop = f64::from(0.3_f32) + f64::from(0.3_f32);
    assert_eq!(exact(&response.results()[0]).logical_distance(), multi_hop);
    assert_eq!(exact(&response.results()[2]).logical_distance(), multi_hop);
    let origin = exact(&response.results()[3]);
    assert_eq!(origin.logical_distance(), 0.0);
    assert!(origin.path().unwrap().steps().is_empty());

    let empty = route(
        &image,
        &request(1, [], profile([(10, 1.0), (20, 1.0)]), false),
    )
    .unwrap();
    assert!(empty.results().is_empty());
    assert_eq!(empty.finalized_nodes(), 0);
    assert_eq!(
        empty.completion_reason(),
        CompletionReason::AllDestinationsFinalized
    );

    assert!(matches!(
        route(
            &image,
            &request(999, [3], profile([(10, 1.0), (20, 1.0)]), false)
        ),
        Err(RoutingError::MissingOrigin(_))
    ));
}

#[test]
fn invalid_profiles_fail_before_search() {
    let image = context_image();
    for (profile, expected) in [
        (
            RelationProfile::new([(RelationId::from_u64(10), RelationUse::Disabled)]),
            "missing",
        ),
        (
            RelationProfile::new([
                (RelationId::from_u64(10), RelationUse::Disabled),
                (RelationId::from_u64(10), RelationUse::Disabled),
                (RelationId::from_u64(20), RelationUse::Disabled),
            ]),
            "duplicate",
        ),
        (
            RelationProfile::new([
                (RelationId::from_u64(10), RelationUse::Disabled),
                (RelationId::from_u64(20), RelationUse::Disabled),
                (RelationId::from_u64(99), RelationUse::Disabled),
            ]),
            "unknown",
        ),
    ] {
        let error = route(&image, &request(1, [3], profile, false)).unwrap_err();
        match (expected, error) {
            ("missing", RoutingError::InvalidProfile(ProfileError::MissingRelation(_)))
            | ("duplicate", RoutingError::InvalidProfile(ProfileError::DuplicateRelation(_)))
            | ("unknown", RoutingError::InvalidProfile(ProfileError::UnknownRelation(_))) => {}
            _ => panic!("unexpected profile result"),
        }
    }
}

#[test]
fn stable_tie_policy_selects_predecessor_tuple_and_parallel_edge_identity() {
    let image = image(
        &[1, 2, 3, 4],
        &[10],
        &[
            (20, 1, 2, 10, 0.0),
            (10, 1, 3, 10, 0.0),
            (40, 2, 4, 10, 1.0),
            (30, 3, 4, 10, 1.0),
            (41, 2, 4, 10, 1.0),
        ],
    );
    let response = route(&image, &request(1, [4], profile([(10, 1.0)]), true)).unwrap();
    let edges: Vec<_> = exact(&response.results()[0])
        .path()
        .unwrap()
        .steps()
        .iter()
        .map(|step| step.edge_id().as_u64())
        .collect();
    assert_eq!(edges, [20, 40]);
}

#[test]
fn budget_never_misreports_incomplete_as_unreachable() {
    let image = image(
        &[1, 2, 3, 4],
        &[10],
        &[(1, 1, 2, 10, 1.0), (2, 2, 3, 10, 1.0)],
    );
    let zero = route(&image, &budget_request(1, [2], 0)).unwrap();
    assert_eq!(zero.examined_edges(), 0);
    assert_eq!(zero.completion_reason(), CompletionReason::BudgetExhausted);
    assert!(matches!(
        zero.results()[0].state(),
        DestinationState::Incomplete
    ));

    let partial = route(&image, &budget_request(1, [2, 3, 4], 1)).unwrap();
    assert!(matches!(
        partial.results()[0].state(),
        DestinationState::Exact(_)
    ));
    assert!(matches!(
        partial.results()[1].state(),
        DestinationState::Incomplete
    ));
    assert!(matches!(
        partial.results()[2].state(),
        DestinationState::Incomplete
    ));
    assert_eq!(partial.examined_edges(), 1);

    let complete = route(&image, &request(1, [4], profile([(10, 1.0)]), false)).unwrap();
    assert_eq!(
        complete.completion_reason(),
        CompletionReason::FrontierExhausted
    );
    assert!(matches!(
        complete.results()[0].state(),
        DestinationState::Unreachable
    ));
}

#[test]
fn target_aware_search_stops_before_expanding_unrelated_region_and_distance_only_has_no_paths() {
    let image = image(
        &[1, 2, 3, 4],
        &[10],
        &[
            (1, 1, 2, 10, 0.1),
            (2, 1, 3, 10, 0.2),
            (3, 3, 4, 10, 0.2),
            (4, 4, 4, 10, 0.0),
        ],
    );
    let response = route(&image, &request(1, [2], profile([(10, 1.0)]), false)).unwrap();
    assert_eq!(
        response.completion_reason(),
        CompletionReason::AllDestinationsFinalized
    );
    assert_eq!(response.examined_edges(), 2);
    assert_eq!(response.finalized_nodes(), 2);
    assert!(exact(&response.results()[0]).path().is_none());
}

#[test]
fn independent_threaded_requests_share_only_the_immutable_image() {
    let image = Arc::new(context_image());
    let handles: Vec<_> = [
        (1, 3, f64::from(0.3_f32) + f64::from(0.3_f32)),
        (2, 3, f64::from(0.3_f32)),
        (3, 1, f64::NAN),
    ]
    .into_iter()
    .map(|(origin, destination, expected)| {
        let image = Arc::clone(&image);
        thread::spawn(move || {
            let response = route(
                &image,
                &request(
                    origin,
                    [destination],
                    profile([(10, 1.0), (20, 1.0)]),
                    false,
                ),
            )
            .unwrap();
            match response.results()[0].state() {
                DestinationState::Exact(exact) => assert_eq!(exact.logical_distance(), expected),
                DestinationState::Unreachable => assert!(expected.is_nan()),
                state => panic!("unexpected state {state:?}"),
            }
        })
    })
    .collect();
    for handle in handles {
        handle.join().unwrap();
    }
}

#[test]
fn old_image_is_immutable_while_new_compilation_reflects_catalog_mutations() {
    let directory = TempDir::new().unwrap();
    let catalog = Catalog::open(directory.path()).unwrap();
    let source = confirm_node(&catalog, "source");
    let middle = confirm_node(&catalog, "middle");
    let removed = confirm_node(&catalog, "removed");
    let relation = confirm_relation(&catalog, "relation");
    let first = confirm_catalog_edge(&catalog, source, middle, relation, 0.2);
    confirm_catalog_edge(&catalog, middle, removed, relation, 0.2);
    let old = RoutingImage::compile(&catalog.confirmed_graph_records().unwrap()).unwrap();

    let promoted = confirm_node(&catalog, "promoted");
    let replacement = confirm_catalog_edge(&catalog, source, promoted, relation, 0.1);
    catalog.remove_edge(first).unwrap();
    catalog.remove_node(removed).unwrap();
    let new = RoutingImage::compile(&catalog.confirmed_graph_records().unwrap()).unwrap();

    assert_eq!(old.node_count(), 3);
    assert_eq!(old.adjacency_count(), 2);
    assert!(old.dense_node_id(removed).is_some());
    assert!(old.dense_node_id(promoted).is_none());
    assert_eq!(new.node_count(), 3);
    assert_eq!(new.adjacency_count(), 1);
    assert!(new.dense_node_id(removed).is_none());
    let new_source = new.dense_node_id(source).unwrap();
    assert_eq!(
        new.outgoing_edges(new_source)
            .unwrap()
            .next()
            .unwrap()
            .edge_id(),
        replacement
    );

    drop(catalog);
    let reopened = Catalog::open(directory.path()).unwrap();
    let reopened_image =
        RoutingImage::compile(&reopened.confirmed_graph_records().unwrap()).unwrap();
    assert_eq!(reopened_image.manifest(), new.manifest());
    assert_eq!(
        reopened_image
            .outgoing_edges(reopened_image.dense_node_id(source).unwrap())
            .unwrap()
            .next()
            .unwrap()
            .edge_id(),
        replacement,
    );
}

#[test]
fn enumerated_small_graphs_match_full_bellman_ford_distances() {
    let directed_pairs: Vec<_> = (1_u64..=3)
        .flat_map(|source| (1_u64..=3).map(move |destination| (source, destination)))
        .collect();
    for mask in 0_u16..(1_u16 << directed_pairs.len()) {
        let edges: Vec<_> = directed_pairs
            .iter()
            .enumerate()
            .filter_map(|(index, &(source, destination))| {
                ((mask & (1 << index)) != 0).then_some((
                    index as u64 + 1,
                    source,
                    destination,
                    10,
                    if index % 2 == 0 { 0.0 } else { 0.5 },
                ))
            })
            .collect();
        let image = image(&[1, 2, 3], &[10], &edges);
        let response = route(&image, &request(1, [1, 2, 3], profile([(10, 1.0)]), false)).unwrap();
        let expected = bellman_ford(&edges);
        for (result, expected) in response.results().iter().zip(expected) {
            match (result.state(), expected) {
                (DestinationState::Exact(route), Some(distance)) => {
                    assert_eq!(route.logical_distance(), distance)
                }
                (DestinationState::Unreachable, None) => {}
                (actual, expected) => panic!("mask {mask}: {actual:?} disagrees with {expected:?}"),
            }
        }
    }
}

fn context_image() -> RoutingImage {
    image(
        &[1, 2, 3, 4],
        &[10, 20],
        &[
            (101, 1, 3, 10, 0.8),
            (102, 1, 2, 20, 0.3),
            (103, 2, 3, 20, 0.3),
        ],
    )
}

fn image(nodes: &[u64], relations: &[u64], edges: &[(u64, u64, u64, u64, f32)]) -> RoutingImage {
    let records = ConfirmedGraphRecords::new(
        nodes
            .iter()
            .map(|&id| {
                NodeRecord::new(
                    NodeId::from_u64(id),
                    NodeName::from(format!("node-{id}")),
                    NodePayload::default(),
                )
            })
            .collect(),
        relations
            .iter()
            .map(|&id| {
                RelationRecord::new(
                    RelationId::from_u64(id),
                    RelationName::from(format!("relation-{id}")),
                )
            })
            .collect(),
        edges
            .iter()
            .map(|&(id, source, destination, relation, weight)| {
                EdgeRecord::new(
                    EdgeId::from_u64(id),
                    NodeId::from_u64(source),
                    NodeId::from_u64(destination),
                    RelationId::from_u64(relation),
                    BaseWeight::new(weight).unwrap(),
                )
            })
            .collect(),
    );
    RoutingImage::compile(&records).unwrap()
}

fn profile<const N: usize>(uses: [(u64, f32); N]) -> RelationProfile {
    RelationProfile::new(uses.map(|(id, multiplier)| {
        (
            RelationId::from_u64(id),
            RelationUse::Enabled(RelationMultiplier::new(multiplier).unwrap()),
        )
    }))
}

fn multiplier(value: f32) -> RelationMultiplier {
    RelationMultiplier::new(value).unwrap()
}

fn request<const N: usize>(
    origin: u64,
    destinations: [u64; N],
    profile: RelationProfile,
    paths: bool,
) -> RoutingRequest {
    RoutingRequest::new(
        NodeId::from_u64(origin),
        destinations.map(NodeId::from_u64),
        profile,
        paths,
        SearchBudget::Unlimited,
        TiePolicy::StablePredecessor,
    )
}

fn budget_request<const N: usize>(
    origin: u64,
    destinations: [u64; N],
    maximum: u64,
) -> RoutingRequest {
    RoutingRequest::new(
        NodeId::from_u64(origin),
        destinations.map(NodeId::from_u64),
        profile([(10, 1.0)]),
        false,
        SearchBudget::ExaminedEdges(maximum),
        TiePolicy::StablePredecessor,
    )
}

fn exact(result: &pathhydra_routing::DestinationResult) -> &pathhydra_routing::ExactRoute {
    let DestinationState::Exact(exact) = result.state() else {
        panic!("result was not exact")
    };
    exact
}

fn confirm_node(catalog: &Catalog, name: &str) -> NodeId {
    let candidate = catalog.insert_node_candidate(name).unwrap();
    let ConfirmedRecord::Node(record) = catalog.confirm_validated_candidate(candidate).unwrap()
    else {
        panic!("node candidate changed kind")
    };
    record.id()
}

fn confirm_relation(catalog: &Catalog, name: &str) -> RelationId {
    let candidate = catalog.insert_relation_candidate(name).unwrap();
    let ConfirmedRecord::Relation(record) = catalog.confirm_validated_candidate(candidate).unwrap()
    else {
        panic!("relation candidate changed kind")
    };
    record.id()
}

fn confirm_catalog_edge(
    catalog: &Catalog,
    source: NodeId,
    destination: NodeId,
    relation: RelationId,
    weight: f32,
) -> EdgeId {
    let candidate = catalog
        .insert_edge_candidate(source, destination, relation, weight)
        .unwrap();
    let ConfirmedRecord::Edge(record) = catalog.confirm_validated_candidate(candidate).unwrap()
    else {
        panic!("edge candidate changed kind")
    };
    record.id()
}

fn bellman_ford(edges: &[(u64, u64, u64, u64, f32)]) -> [Option<f64>; 3] {
    let mut distances = [Some(0.0), None, None];
    for _ in 0..2 {
        let mut changed = false;
        for &(_, source, destination, _, weight) in edges {
            let Some(source_distance) = distances[source as usize - 1] else {
                continue;
            };
            let candidate = source_distance + f64::from(weight);
            let slot = &mut distances[destination as usize - 1];
            if slot.is_none_or(|existing| candidate < existing) {
                *slot = Some(candidate);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    distances
}
