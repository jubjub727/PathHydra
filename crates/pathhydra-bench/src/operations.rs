use std::{sync::Arc, time::Instant};

use pathhydra_core::{CandidateId, ConfirmedRecord, EdgeId, NodeId, RelationId};
use pathhydra_engine::{
    ConfirmedMutation, EngineConfig, GraphEngine, PublicationOutcome, RequestId,
};
use pathhydra_routing::{
    RelationMultiplier, RelationProfile, RelationUse, RoutingRequest, SearchBudget, TiePolicy,
};
use pathhydra_store::Catalog;

#[derive(Clone, Copy, Debug)]
enum Scenario {
    NodePromotion,
    EdgePromotion,
    EdgeDeletion,
    HighDegreeNodeDeletion,
}

enum MutationTarget {
    Candidate(CandidateId),
    Edge(EdgeId),
    Node(NodeId),
}

pub(crate) fn run() {
    println!(
        "operations_schema,graph_nodes,mutation,sample,publication_observed,baseline_route_ns,concurrent_route_ns,blocked_route_ns,mutation_ns,rebuild_ns,correct"
    );
    for graph_nodes in [256_usize, 1_024, 4_096] {
        for scenario in [
            Scenario::NodePromotion,
            Scenario::EdgePromotion,
            Scenario::EdgeDeletion,
            Scenario::HighDegreeNodeDeletion,
        ] {
            for sample in 0..5 {
                run_case(graph_nodes, scenario, sample);
            }
        }
    }
}

fn run_case(graph_nodes: usize, scenario: Scenario, sample: usize) {
    let root = tempfile::tempdir().expect("temporary operations benchmark root");
    let database = root.path().join("catalog");
    let routing = root.path().join("routing");
    let (nodes, relation, edges) = build_catalog(&database, graph_nodes, scenario);
    let engine = Arc::new(
        GraphEngine::open(
            &database,
            EngineConfig {
                routing_image_root: Some(routing),
                ..EngineConfig::default()
            },
        )
        .expect("open benchmark engine"),
    );
    let profile = RelationProfile::new([(
        relation,
        RelationUse::Enabled(RelationMultiplier::new(1.0).expect("one is valid")),
    )]);
    let request = RoutingRequest::new(
        nodes[1],
        [*nodes.last().expect("nodes")],
        profile,
        false,
        SearchBudget::Unlimited,
        TiePolicy::StablePredecessor,
    );
    let baseline_started = Instant::now();
    engine
        .route(RequestId::new(1), &request)
        .expect("baseline route");
    let baseline = baseline_started.elapsed();

    let target = match scenario {
        Scenario::NodePromotion => {
            MutationTarget::Candidate(engine.insert_node_candidate("promoted-node").unwrap())
        }
        Scenario::EdgePromotion => MutationTarget::Candidate(
            engine
                .insert_edge_candidate(nodes[1], nodes[graph_nodes - 1], relation, 0.5)
                .unwrap(),
        ),
        Scenario::EdgeDeletion => MutationTarget::Edge(edges[edges.len() / 2]),
        Scenario::HighDegreeNodeDeletion => MutationTarget::Node(nodes[0]),
    };

    let mutation_engine = Arc::clone(&engine);
    let mutation = std::thread::spawn(move || {
        let started = Instant::now();
        let rebuild = match target {
            MutationTarget::Candidate(candidate) => publication_duration(
                mutation_engine
                    .confirm_validated_candidate(candidate)
                    .unwrap(),
            ),
            MutationTarget::Edge(edge) => {
                publication_duration(mutation_engine.remove_edge(edge).unwrap())
            }
            MutationTarget::Node(node) => {
                publication_duration(mutation_engine.remove_node(node).unwrap())
            }
        };
        (started.elapsed(), rebuild)
    });
    let wait_started = Instant::now();
    while !engine.publication_in_progress()
        && !mutation.is_finished()
        && wait_started.elapsed() < std::time::Duration::from_secs(30)
    {
        std::thread::yield_now();
    }
    let observed = engine.publication_in_progress();
    let concurrent_started = Instant::now();
    let routed = engine.route(RequestId::new(2), &request);
    let concurrent = concurrent_started.elapsed();
    let (mutation_duration, rebuild) = mutation.join().expect("mutation worker");
    // Route once more after publication and use that stable current image as
    // the semantic oracle.  The route admitted during publication must return
    // exactly the same current-state result, not merely avoid an error.
    let current = engine.route(RequestId::new(3), &request);
    let blocked = concurrent.saturating_sub(baseline);
    let correct = observed
        && matches!((&routed, &current), (Ok(during), Ok(after)) if during.response == after.response);
    println!(
        "1,{graph_nodes},{},{sample},{observed},{},{},{},{},{},{correct}",
        scenario_name(scenario),
        baseline.as_nanos(),
        concurrent.as_nanos(),
        blocked.as_nanos(),
        mutation_duration.as_nanos(),
        rebuild.as_nanos(),
    );
    assert!(correct, "operations benchmark correctness failed");
    assert!(engine.shutdown().unwrap().complete());
}

fn publication_duration<T>(mutation: ConfirmedMutation<T>) -> std::time::Duration {
    match mutation.publication() {
        PublicationOutcome::Published(report)
        | PublicationOutcome::RoutingUnavailable(_, report) => report.duration(),
    }
}

fn build_catalog(
    database: &std::path::Path,
    graph_nodes: usize,
    scenario: Scenario,
) -> (Vec<NodeId>, RelationId, Vec<EdgeId>) {
    let catalog = Catalog::open(database).expect("create benchmark catalog");
    let mut nodes = Vec::with_capacity(graph_nodes);
    for index in 0..graph_nodes {
        let candidate = catalog
            .insert_node_candidate(format!("node-{index}"))
            .unwrap();
        let ConfirmedRecord::Node(node) = catalog.confirm_validated_candidate(candidate).unwrap()
        else {
            unreachable!()
        };
        nodes.push(node.id());
    }
    let relation_candidate = catalog.insert_relation_candidate("relation").unwrap();
    let ConfirmedRecord::Relation(relation) = catalog
        .confirm_validated_candidate(relation_candidate)
        .unwrap()
    else {
        unreachable!()
    };
    let mut edges = Vec::new();
    for pair in nodes.windows(2) {
        edges.push(confirm_edge(&catalog, pair[0], pair[1], relation.id()));
    }
    if matches!(scenario, Scenario::HighDegreeNodeDeletion) {
        for &node in &nodes[1..] {
            edges.push(confirm_edge(&catalog, nodes[0], node, relation.id()));
            edges.push(confirm_edge(&catalog, node, nodes[0], relation.id()));
        }
    }
    drop(catalog);
    (nodes, relation.id(), edges)
}

fn confirm_edge(
    catalog: &Catalog,
    source: NodeId,
    destination: NodeId,
    relation: RelationId,
) -> EdgeId {
    let candidate = catalog
        .insert_edge_candidate(source, destination, relation, 1.0)
        .unwrap();
    let ConfirmedRecord::Edge(edge) = catalog.confirm_validated_candidate(candidate).unwrap()
    else {
        unreachable!()
    };
    edge.id()
}

const fn scenario_name(scenario: Scenario) -> &'static str {
    match scenario {
        Scenario::NodePromotion => "node-promotion",
        Scenario::EdgePromotion => "edge-promotion",
        Scenario::EdgeDeletion => "edge-deletion",
        Scenario::HighDegreeNodeDeletion => "high-degree-node-deletion",
    }
}
