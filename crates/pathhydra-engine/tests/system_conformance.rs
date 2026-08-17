use std::collections::{BTreeMap, BTreeSet};

use pathhydra_core::{
    BaseWeight, ConfirmedRecord, EdgeRecord, NodeId, NodeRecord, RelationId, RelationRecord,
};
#[cfg(feature = "cuda")]
use pathhydra_engine::{CudaAlgorithmSelection, CudaConfig, CudaExecutorPolicy};
use pathhydra_engine::{
    EngineConfig, EngineRestoreRequest, Executor, GraphEngine, HydratedEdgeState,
    HydratedNodeState, HydrationRequest, RequestId, StartupBundlePolicy,
};
use pathhydra_routing::{
    CompletionReason, DestinationState, RelationMultiplier, RelationProfile, RelationUse,
    RoutingRequest, SearchBudget, TiePolicy,
};
use pathhydra_store::{CheckpointRequest, RestoreRequest, VerificationLimits};

const ORDINARY_SEEDS: [u64; 3] = [
    0x243f_6a88_85a3_08d3,
    0x1319_8a2e_0370_7344,
    0xa409_3822_299f_31d0,
];

#[derive(Clone, Copy, Debug)]
struct EdgeSpec {
    source: usize,
    destination: usize,
    relation: usize,
    base_weight: f32,
}

#[derive(Clone, Debug)]
struct GraphSpec {
    seed: u64,
    node_count: usize,
    edges: Vec<EdgeSpec>,
}

#[derive(Clone, Debug)]
struct MaterializedGraph {
    nodes: Vec<NodeRecord>,
    relations: Vec<RelationRecord>,
    edges: Vec<EdgeRecord>,
    first_equal_edge: pathhydra_core::EdgeId,
    first_equal_tail: pathhydra_core::EdgeId,
}

#[derive(Clone, Copy)]
struct ReplayRng(u64);

impl ReplayRng {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    fn next(&mut self) -> u64 {
        let mut value = self.0;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.0 = value;
        value
    }

    fn index(&mut self, length: usize) -> usize {
        usize::try_from(self.next() % u64::try_from(length).unwrap()).unwrap()
    }
}

fn generated_spec(seed: u64) -> GraphSpec {
    let mut rng = ReplayRng::new(seed);
    let mut edges = vec![
        // Equal-cost alternatives and stable parallel-edge tie.
        EdgeSpec {
            source: 0,
            destination: 1,
            relation: 0,
            base_weight: 0.25,
        },
        // One binary32 ULP above the winning parallel edge.
        EdgeSpec {
            source: 0,
            destination: 1,
            relation: 0,
            base_weight: f32::from_bits(0.25_f32.to_bits() + 1),
        },
        EdgeSpec {
            source: 0,
            destination: 2,
            relation: 0,
            base_weight: 0.25,
        },
        EdgeSpec {
            source: 1,
            destination: 3,
            relation: 0,
            base_weight: 0.25,
        },
        EdgeSpec {
            source: 2,
            destination: 3,
            relation: 0,
            base_weight: 0.25,
        },
        EdgeSpec {
            source: 0,
            destination: 1,
            relation: 0,
            base_weight: 0.5,
        },
        EdgeSpec {
            source: 0,
            destination: 1,
            relation: 0,
            base_weight: 0.25,
        },
        // Self edge, disabled zero shortcut, and a zero-weight SCC.
        EdgeSpec {
            source: 1,
            destination: 1,
            relation: 0,
            base_weight: 0.0,
        },
        EdgeSpec {
            source: 0,
            destination: 3,
            relation: 1,
            base_weight: 0.0,
        },
        EdgeSpec {
            source: 0,
            destination: 4,
            relation: 0,
            base_weight: 1.0,
        },
        EdgeSpec {
            source: 4,
            destination: 5,
            relation: 2,
            base_weight: 1.0,
        },
        EdgeSpec {
            source: 5,
            destination: 4,
            relation: 2,
            base_weight: 1.0,
        },
        EdgeSpec {
            source: 5,
            destination: 3,
            relation: 0,
            base_weight: 0.0,
        },
        // Node 6 is an unreachable region; node 7 is completely isolated.
        EdgeSpec {
            source: 6,
            destination: 3,
            relation: 0,
            base_weight: 0.125,
        },
        // Boundary multiplier and high-degree/dense region.
        EdgeSpec {
            source: 0,
            destination: 9,
            relation: 3,
            base_weight: f32::MIN_POSITIVE,
        },
        EdgeSpec {
            source: 0,
            destination: 10,
            relation: 3,
            base_weight: f32::from_bits(1),
        },
        EdgeSpec {
            source: 0,
            destination: 8,
            relation: 0,
            base_weight: 0.75,
        },
    ];
    for destination in [0, 1, 2, 3, 4, 5, 8, 9, 10, 11] {
        edges.push(EdgeSpec {
            source: 8,
            destination,
            relation: rng.index(4),
            base_weight: [0.0, f32::MIN_POSITIVE, 0.25, 0.5, 1.0][rng.index(5)],
        });
    }
    for _ in 0..12 {
        let dense = [8, 9, 10, 11];
        edges.push(EdgeSpec {
            source: dense[rng.index(dense.len())],
            destination: dense[rng.index(dense.len())],
            relation: rng.index(4),
            base_weight: [0.0, f32::MIN_POSITIVE, 0.125, 0.5, 1.0][rng.index(5)],
        });
    }
    GraphSpec {
        seed,
        node_count: 12,
        edges,
    }
}

fn confirm_node(engine: &GraphEngine, name: String) -> NodeRecord {
    let candidate = engine.insert_node_candidate(name).unwrap();
    let ConfirmedRecord::Node(node) = engine
        .confirm_validated_candidate(candidate)
        .unwrap()
        .into_parts()
        .0
    else {
        panic!("node candidate changed record kind")
    };
    node
}

fn confirm_relation(engine: &GraphEngine, name: String) -> RelationRecord {
    let candidate = engine.insert_relation_candidate(name).unwrap();
    let ConfirmedRecord::Relation(relation) = engine
        .confirm_validated_candidate(candidate)
        .unwrap()
        .into_parts()
        .0
    else {
        panic!("relation candidate changed record kind")
    };
    relation
}

fn confirm_edge(
    engine: &GraphEngine,
    source: NodeId,
    destination: NodeId,
    relation: RelationId,
    base_weight: f32,
) -> EdgeRecord {
    let candidate = engine
        .insert_edge_candidate_with_base_weight(
            source,
            destination,
            relation,
            BaseWeight::new(base_weight).unwrap(),
        )
        .unwrap();
    let ConfirmedRecord::Edge(edge) = engine
        .confirm_validated_candidate(candidate)
        .unwrap()
        .into_parts()
        .0
    else {
        panic!("edge candidate changed record kind")
    };
    edge
}

fn materialize(engine: &GraphEngine, spec: &GraphSpec) -> MaterializedGraph {
    // Consume then delete IDs so the confirmed topology cannot assume dense stable IDs.
    for tombstone in 0..2 {
        let node = confirm_node(
            engine,
            format!("seed-{:016x}-tombstone-{tombstone}", spec.seed),
        );
        engine.remove_node(node.id()).unwrap();
    }
    let nodes = (0..spec.node_count)
        .map(|index| confirm_node(engine, format!("seed-{:016x}-node-{index}", spec.seed)))
        .collect::<Vec<_>>();
    let relations = (0..4)
        .map(|index| confirm_relation(engine, format!("seed-{:016x}-relation-{index}", spec.seed)))
        .collect::<Vec<_>>();

    let removed = confirm_edge(engine, nodes[0].id(), nodes[0].id(), relations[0].id(), 1.0);
    engine.remove_edge(removed.id()).unwrap();

    let edges = spec
        .edges
        .iter()
        .map(|edge| {
            confirm_edge(
                engine,
                nodes[edge.source].id(),
                nodes[edge.destination].id(),
                relations[edge.relation].id(),
                edge.base_weight,
            )
        })
        .collect::<Vec<_>>();
    assert!(nodes[0].id().as_u64() > 1);
    assert!(edges[0].id().as_u64() > 1);
    MaterializedGraph {
        nodes,
        relations,
        first_equal_edge: edges[0].id(),
        first_equal_tail: edges[3].id(),
        edges,
    }
}

fn profiles(graph: &MaterializedGraph) -> [RelationProfile; 2] {
    [
        RelationProfile::new([
            (
                graph.relations[0].id(),
                RelationUse::Enabled(RelationMultiplier::new(1.0).unwrap()),
            ),
            (graph.relations[1].id(), RelationUse::Disabled),
            (
                graph.relations[2].id(),
                RelationUse::Enabled(RelationMultiplier::new(0.0).unwrap()),
            ),
            (
                graph.relations[3].id(),
                RelationUse::Enabled(RelationMultiplier::new(f32::MAX).unwrap()),
            ),
        ]),
        RelationProfile::new([
            (
                graph.relations[0].id(),
                RelationUse::Enabled(RelationMultiplier::new(0.5).unwrap()),
            ),
            (
                graph.relations[1].id(),
                RelationUse::Enabled(RelationMultiplier::new(1.0).unwrap()),
            ),
            (
                graph.relations[2].id(),
                RelationUse::Enabled(RelationMultiplier::new(1.0).unwrap()),
            ),
            (
                graph.relations[3].id(),
                RelationUse::Enabled(RelationMultiplier::new(f32::MIN_POSITIVE).unwrap()),
            ),
        ]),
    ]
}

fn relation_uses(profile: &RelationProfile) -> BTreeMap<RelationId, RelationUse> {
    profile.entries().iter().copied().collect()
}

fn bellman_ford(
    graph: &MaterializedGraph,
    origin: NodeId,
    profile: &RelationProfile,
) -> BTreeMap<NodeId, f64> {
    let uses = relation_uses(profile);
    let mut distances = BTreeMap::new();
    distances.insert(origin, 0.0);
    for _ in 0..graph.nodes.len().saturating_sub(1) {
        let mut changed = false;
        for edge in &graph.edges {
            let Some(&current) = distances.get(&edge.source()) else {
                continue;
            };
            let Some(RelationUse::Enabled(multiplier)) = uses.get(&edge.relation_kind()) else {
                continue;
            };
            // Deliberately independent of pathhydra-routing helpers: the declared policy is
            // binary32 operands widened before one binary64 multiply and one binary64 add.
            let effective = f64::from(edge.base_weight().get()) * f64::from(multiplier.get());
            let candidate = current + effective;
            assert!(effective.is_finite() && candidate.is_finite());
            let replace = distances
                .get(&edge.destination())
                .is_none_or(|existing| candidate < *existing);
            if replace {
                distances.insert(edge.destination(), candidate);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    distances
}

fn destinations(graph: &MaterializedGraph) -> Vec<NodeId> {
    vec![
        graph.nodes[3].id(),
        graph.nodes[3].id(),
        graph.nodes[1].id(),
        graph.nodes[6].id(),
        graph.nodes[7].id(),
        NodeId::from_u64(u64::MAX),
        graph.nodes[0].id(),
        graph.nodes[5].id(),
    ]
}

fn assert_path(
    graph: &MaterializedGraph,
    profile: &RelationProfile,
    origin: NodeId,
    destination: NodeId,
    expected_bits: u64,
    path: &pathhydra_routing::RoutePath,
    seed: u64,
) {
    assert_eq!(path.origin(), origin, "seed={seed:#018x}");
    assert_eq!(path.destination(), destination, "seed={seed:#018x}");
    assert_eq!(
        path.logical_distance().to_bits(),
        expected_bits,
        "seed={seed:#018x}"
    );
    let uses = relation_uses(profile);
    let edge_by_id = graph
        .edges
        .iter()
        .map(|edge| (edge.id(), edge))
        .collect::<BTreeMap<_, _>>();
    let mut cursor = origin;
    let mut sum = 0.0_f64;
    let mut visited = BTreeSet::from([origin]);
    for step in path.steps() {
        let edge = edge_by_id
            .get(&step.edge_id())
            .expect("path edge must be confirmed");
        assert_eq!(step.source(), cursor, "seed={seed:#018x}");
        assert_eq!(step.source(), edge.source(), "seed={seed:#018x}");
        assert_eq!(step.destination(), edge.destination(), "seed={seed:#018x}");
        assert_eq!(
            step.relation_id(),
            edge.relation_kind(),
            "seed={seed:#018x}"
        );
        assert_eq!(step.base_weight(), edge.base_weight(), "seed={seed:#018x}");
        let RelationUse::Enabled(multiplier) = uses[&edge.relation_kind()] else {
            panic!("disabled relation appeared in path for seed={seed:#018x}")
        };
        assert_eq!(step.multiplier(), multiplier, "seed={seed:#018x}");
        let effective = f64::from(edge.base_weight().get()) * f64::from(multiplier.get());
        assert_eq!(
            step.effective_weight().to_bits(),
            effective.to_bits(),
            "seed={seed:#018x}"
        );
        sum += effective;
        cursor = step.destination();
        assert!(
            visited.insert(cursor),
            "path is not simple for seed={seed:#018x}"
        );
    }
    assert_eq!(cursor, destination, "seed={seed:#018x}");
    assert_eq!(sum.to_bits(), expected_bits, "seed={seed:#018x}");
}

fn assert_conformance(
    engine: &GraphEngine,
    graph: &MaterializedGraph,
    profile: &RelationProfile,
    request_id: u64,
    expected_executor: Executor,
    seed: u64,
) {
    let origin = graph.nodes[0].id();
    let requested = destinations(graph);
    let oracle = bellman_ford(graph, origin, profile);
    let request = RoutingRequest::new(
        origin,
        requested.clone(),
        profile.clone(),
        true,
        SearchBudget::Unlimited,
        TiePolicy::StablePredecessor,
    );
    let health_before = engine.health().unwrap();
    let output = engine.route(RequestId::new(request_id), &request).unwrap();
    assert_eq!(
        output.diagnostics.executor, expected_executor,
        "seed={seed:#018x}"
    );
    assert_eq!(output.response.origin(), origin, "seed={seed:#018x}");
    assert_eq!(output.response.profile(), profile, "seed={seed:#018x}");
    assert_eq!(output.response.tie_policy(), TiePolicy::StablePredecessor);
    assert!(output.response.paths_requested());
    assert_eq!(
        output.response.completion_reason(),
        CompletionReason::FrontierExhausted
    );
    assert_eq!(
        output.diagnostics.completion_reason,
        output.response.completion_reason()
    );
    assert_eq!(output.response.results().len(), requested.len());
    assert!(
        output
            .diagnostics
            .first_destination_duration
            .is_some_and(|duration| duration <= output.diagnostics.execution_duration),
        "first completion must be scoped within execution for seed={seed:#018x}"
    );
    assert!(
        output.diagnostics.reconstruction_duration <= output.diagnostics.execution_duration,
        "reconstruction must be scoped within execution for seed={seed:#018x}"
    );
    assert!(
        !output.diagnostics.reconstruction_duration.is_zero(),
        "path-bearing conformance route must measure reconstruction for seed={seed:#018x}"
    );

    for (result, destination) in output.response.results().iter().zip(&requested) {
        assert_eq!(result.destination(), *destination, "seed={seed:#018x}");
        if !graph.nodes.iter().any(|node| node.id() == *destination) {
            assert!(matches!(result.state(), DestinationState::MissingNode));
        } else if let Some(distance) = oracle.get(destination) {
            let DestinationState::Exact(exact) = result.state() else {
                panic!("oracle exact state disagreed for seed={seed:#018x}")
            };
            assert_eq!(
                exact.logical_distance().to_bits(),
                distance.to_bits(),
                "seed={seed:#018x}"
            );
            assert_path(
                graph,
                profile,
                origin,
                *destination,
                distance.to_bits(),
                exact.path().expect("paths were requested"),
                seed,
            );
        } else {
            assert!(matches!(result.state(), DestinationState::Unreachable));
        }
    }

    let state_counts = output
        .response
        .results()
        .iter()
        .fold([0_usize; 4], |mut counts, result| {
            match result.state() {
                DestinationState::Exact(_) => counts[0] += 1,
                DestinationState::Unreachable => counts[1] += 1,
                DestinationState::MissingNode => counts[2] += 1,
                DestinationState::Incomplete => counts[3] += 1,
            }
            counts
        });
    assert_eq!(
        output.diagnostics.search.exact_destinations,
        state_counts[0]
    );
    assert_eq!(
        output.diagnostics.search.unreachable_destinations,
        state_counts[1]
    );
    assert_eq!(
        output.diagnostics.search.missing_destinations,
        state_counts[2]
    );
    assert_eq!(
        output.diagnostics.search.incomplete_destinations,
        state_counts[3]
    );
    assert_eq!(
        output.response.examined_edges(),
        output.diagnostics.search.examined_edges
    );
    assert_eq!(
        output.response.finalized_nodes(),
        output.diagnostics.search.finalized_nodes
    );
    let unique_present = requested
        .iter()
        .filter(|destination| graph.nodes.iter().any(|node| node.id() == **destination))
        .copied()
        .collect::<BTreeSet<_>>()
        .len();
    assert_eq!(
        output.diagnostics.search.unique_present_destinations,
        unique_present
    );
    assert_eq!(output.response.results()[0], output.response.results()[1]);

    // Stable predecessor ties choose the lower predecessor and then lower edge ID.
    if profile.entries()[0].1 == RelationUse::Enabled(RelationMultiplier::new(1.0).unwrap()) {
        let DestinationState::Exact(exact) = output.response.results()[0].state() else {
            unreachable!()
        };
        // The randomized dense tail may contain a strictly shorter route. When the
        // deliberately equal-cost pair remains optimal, protect the stable tie rule.
        if exact.logical_distance().to_bits() == 0.5_f64.to_bits() {
            let path = exact.path().unwrap();
            assert_eq!(path.steps()[0].edge_id(), graph.first_equal_edge);
            assert_eq!(path.steps()[1].edge_id(), graph.first_equal_tail);
        }
    }
    let health = engine.health().unwrap();
    assert_eq!(health.active_routes, 0);
    assert_eq!(health.reserved_route_bytes, 0);
    assert_eq!(
        health.cumulative_route_admissions,
        health_before.cumulative_route_admissions + 1
    );
    assert_eq!(
        health.cumulative_admission_rejections,
        health_before.cumulative_admission_rejections
    );
    assert_eq!(
        health.cumulative_image_build_failures,
        health_before.cumulative_image_build_failures
    );
    if expected_executor == Executor::NvidiaCuda {
        let cuda = output.diagnostics.cuda.as_ref().unwrap();
        assert!(
            cuda.first_destination_duration
                .is_some_and(|duration| duration <= output.diagnostics.execution_duration)
        );
        assert!(
            cuda.synchronized_execution_duration <= output.diagnostics.execution_duration,
            "CUDA synchronization must be scoped within outer execution"
        );
        assert_eq!(
            cuda.path_reconstruction_duration,
            output.diagnostics.reconstruction_duration
        );
        assert!(output.diagnostics.partitioned_cpu.is_none());
    } else {
        assert!(output.diagnostics.cuda.is_none());
        assert_eq!(
            output.diagnostics.partitioned_cpu.is_some(),
            health.host_partition_cache.is_some()
        );
        assert_cpu_completion_boundaries(engine, graph, profile, request_id + 1, seed);
    }
}

fn assert_cpu_completion_boundaries(
    engine: &GraphEngine,
    graph: &MaterializedGraph,
    profile: &RelationProfile,
    request_id: u64,
    seed: u64,
) {
    let origin = graph.nodes[0].id();
    let completed = engine
        .route(
            RequestId::new(request_id),
            &RoutingRequest::new(
                origin,
                [origin, graph.nodes[1].id(), graph.nodes[1].id()],
                profile.clone(),
                false,
                SearchBudget::Unlimited,
                TiePolicy::StablePredecessor,
            ),
        )
        .unwrap();
    assert_eq!(
        completed.response.completion_reason(),
        CompletionReason::AllDestinationsFinalized,
        "seed={seed:#018x}"
    );
    assert_eq!(
        completed.response.results()[1],
        completed.response.results()[2]
    );

    let budgeted = engine
        .route(
            RequestId::new(request_id + 1),
            &RoutingRequest::new(
                origin,
                [graph.nodes[3].id(), NodeId::from_u64(u64::MAX)],
                profile.clone(),
                false,
                SearchBudget::ExaminedEdges(0),
                TiePolicy::StablePredecessor,
            ),
        )
        .unwrap();
    assert_eq!(
        budgeted.response.completion_reason(),
        CompletionReason::BudgetExhausted,
        "seed={seed:#018x}"
    );
    assert!(matches!(
        budgeted.response.results()[0].state(),
        DestinationState::Incomplete
    ));
    assert!(matches!(
        budgeted.response.results()[1].state(),
        DestinationState::MissingNode
    ));
}

fn resident_config() -> EngineConfig {
    EngineConfig {
        startup_bundle_policy: StartupBundlePolicy::ValidateOrRebuild,
        ..EngineConfig::default()
    }
}

fn partitioned_config(partition_bytes: u64, cache_entries: usize) -> EngineConfig {
    EngineConfig {
        max_active_image_bytes: 8,
        target_partition_topology_bytes: partition_bytes,
        hard_maximum_partition_topology_bytes: partition_bytes,
        host_partition_cache_bytes: 8 * 1024,
        host_partition_cache_entries: cache_entries,
        routing_io_staging_bytes: 2 * 1024,
        ..EngineConfig::default()
    }
}

fn assert_provisional_invisibility(
    engine: &GraphEngine,
    graph: &MaterializedGraph,
    profile: &RelationProfile,
    request_id: u64,
) {
    let before = engine.health().unwrap().current_image_manifest.unwrap();
    let node = engine
        .insert_node_candidate(format!("provisional-node-{request_id}"))
        .unwrap();
    let relation = engine
        .insert_relation_candidate(format!("provisional-relation-{request_id}"))
        .unwrap();
    let edge = engine
        .insert_edge_candidate(
            graph.nodes[0].id(),
            graph.nodes[1].id(),
            graph.relations[0].id(),
            0.5,
        )
        .unwrap();
    let pathhydra_core::Candidate::Node { name, .. } = engine.get_candidate(node).unwrap() else {
        unreachable!()
    };
    assert_eq!(engine.lookup_node_exact(name.as_str()).unwrap(), None);
    assert_eq!(
        engine
            .lookup_relation_exact(format!("provisional-relation-{request_id}").as_str())
            .unwrap(),
        None
    );
    let routed = engine
        .route(
            RequestId::new(request_id),
            &RoutingRequest::new(
                graph.nodes[0].id(),
                [NodeId::from_u64(node.as_u64()), graph.nodes[1].id()],
                profile.clone(),
                false,
                SearchBudget::Unlimited,
                TiePolicy::StablePredecessor,
            ),
        )
        .unwrap();
    assert!(matches!(
        routed.response.results()[0].state(),
        DestinationState::MissingNode
    ));
    let hydrated = engine
        .hydrate(&HydrationRequest::new(
            [NodeId::from_u64(node.as_u64())],
            [pathhydra_core::EdgeId::from_u64(edge.as_u64())],
            Some(profile.clone()),
        ))
        .unwrap();
    assert!(matches!(
        hydrated.nodes[0].state,
        HydratedNodeState::Missing
    ));
    assert!(matches!(
        hydrated.edges[0].state,
        HydratedEdgeState::Missing
    ));
    let after = engine.health().unwrap().current_image_manifest.unwrap();
    assert_eq!(before, after);
    assert!(engine.get_candidate(node).is_ok());
    assert!(engine.get_candidate(relation).is_ok());
    assert!(engine.get_candidate(edge).is_ok());
}

fn run_cpu_seed(seed: u64) {
    let spec = generated_spec(seed);
    for (index, config) in [
        resident_config(),
        partitioned_config(128, 1),
        partitioned_config(256, 2),
    ]
    .into_iter()
    .enumerate()
    {
        let root = tempfile::tempdir().unwrap();
        let engine = GraphEngine::open(root.path().join("catalog"), config).unwrap();
        let graph = materialize(&engine, &spec);
        assert_provisional_invisibility(
            &engine,
            &graph,
            &profiles(&graph)[1],
            9_000 + index as u64,
        );
        for (profile_index, profile) in profiles(&graph).iter().enumerate() {
            assert_conformance(
                &engine,
                &graph,
                profile,
                10_000 + (index * 100 + profile_index) as u64,
                Executor::CpuReference,
                seed,
            );
        }
        if index != 0 {
            assert!(engine.health().unwrap().host_partition_cache.is_some());
        }
    }
}

#[test]
fn fixed_seed_resident_and_partitioned_cpu_match_independent_oracle() {
    for seed in ORDINARY_SEEDS {
        eprintln!(
            "replay: cargo test -p pathhydra-engine --test system_conformance -- --nocapture; seed={seed:#018x}"
        );
        run_cpu_seed(seed);
    }
}

#[test]
fn exact_names_provisional_invisibility_and_restart_are_preserved() {
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("catalog");
    let config = resident_config();
    let engine = GraphEngine::open(&database, config.clone()).unwrap();
    let names = [
        "Case",
        "case",
        " leading and trailing ",
        "punctuation !?[]{}",
        "embedded\0null",
        "Latin e\u{301}",
        "Latin é",
        "Ελληνικά",
        "日本語",
        "long-名字-🙂-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
    ];
    let provisional = engine
        .insert_node_candidate("Provisional Invisible")
        .unwrap();
    assert_eq!(
        engine.lookup_node_exact("Provisional Invisible").unwrap(),
        None
    );
    let provisional_image_nodes = engine
        .health()
        .unwrap()
        .current_image_manifest
        .unwrap()
        .node_count();
    assert_eq!(provisional_image_nodes, 0);
    let confirmed = names
        .iter()
        .map(|name| confirm_node(&engine, (*name).to_owned()))
        .collect::<Vec<_>>();
    assert_eq!(
        engine
            .health()
            .unwrap()
            .current_image_manifest
            .unwrap()
            .node_count(),
        provisional_image_nodes + names.len()
    );
    for (name, node) in names.iter().zip(&confirmed) {
        assert_eq!(engine.lookup_node_exact(name).unwrap(), Some(node.id()));
    }
    let duplicate = engine.insert_node_candidate("Case").unwrap();
    let ConfirmedRecord::Node(existing) = engine
        .confirm_validated_candidate(duplicate)
        .unwrap()
        .into_parts()
        .0
    else {
        panic!()
    };
    assert_eq!(existing.id(), confirmed[0].id());
    assert!(engine.get_candidate(duplicate).is_err());
    assert!(engine.get_candidate(provisional).is_ok());
    let relation_names = ["Relation", "relation", " relation!?\0 "];
    let relations = relation_names
        .iter()
        .map(|name| confirm_relation(&engine, (*name).to_owned()))
        .collect::<Vec<_>>();
    let provisional_relation = engine
        .insert_relation_candidate("Provisional Relation")
        .unwrap();
    assert_eq!(
        engine
            .lookup_relation_exact("Provisional Relation")
            .unwrap(),
        None
    );
    let duplicate_relation = engine.insert_relation_candidate("Relation").unwrap();
    let ConfirmedRecord::Relation(existing_relation) = engine
        .confirm_validated_candidate(duplicate_relation)
        .unwrap()
        .into_parts()
        .0
    else {
        panic!()
    };
    assert_eq!(existing_relation.id(), relations[0].id());
    assert!(engine.get_candidate(duplicate_relation).is_err());

    let edge_candidate = engine
        .insert_edge_candidate(confirmed[0].id(), confirmed[1].id(), relations[0].id(), 0.5)
        .unwrap();
    let disabled_profile = RelationProfile::new(
        relations
            .iter()
            .map(|relation| (relation.id(), RelationUse::Disabled)),
    );
    let route_before_confirmation = RoutingRequest::new(
        confirmed[0].id(),
        [confirmed[1].id()],
        disabled_profile,
        false,
        SearchBudget::Unlimited,
        TiePolicy::StablePredecessor,
    );
    assert!(matches!(
        engine
            .route(RequestId::new(40_000), &route_before_confirmation)
            .unwrap()
            .response
            .results()[0]
            .state(),
        DestinationState::Unreachable
    ));
    engine.confirm_validated_candidate(edge_candidate).unwrap();
    let route_after_confirmation = RoutingRequest::new(
        confirmed[0].id(),
        [confirmed[1].id()],
        RelationProfile::new(relations.iter().enumerate().map(|(index, relation)| {
            (
                relation.id(),
                if index == 0 {
                    RelationUse::Enabled(RelationMultiplier::new(1.0).unwrap())
                } else {
                    RelationUse::Disabled
                },
            )
        })),
        false,
        SearchBudget::Unlimited,
        TiePolicy::StablePredecessor,
    );
    assert!(matches!(
        engine
            .route(RequestId::new(40_001), &route_after_confirmation)
            .unwrap()
            .response
            .results()[0]
            .state(),
        DestinationState::Exact(_)
    ));
    drop(engine);

    let reopened = GraphEngine::open(&database, config).unwrap();
    for (name, node) in names.iter().zip(&confirmed) {
        assert_eq!(reopened.lookup_node_exact(name).unwrap(), Some(node.id()));
    }
    for (name, relation) in relation_names.iter().zip(&relations) {
        assert_eq!(
            reopened.lookup_relation_exact(name).unwrap(),
            Some(relation.id())
        );
    }
    assert_eq!(
        reopened.lookup_node_exact("Provisional Invisible").unwrap(),
        None
    );
    assert!(reopened.get_candidate(provisional).is_ok());
    assert!(reopened.get_candidate(provisional_relation).is_ok());
}

#[test]
fn checkpoint_restore_replays_pre_mutation_snapshot_exactly() {
    checkpoint_restore_equivalence(false);
}

#[cfg(feature = "cuda")]
#[test]
fn checkpoint_restore_under_concurrent_reads_preserves_cuda_equivalence() {
    checkpoint_restore_equivalence(true);
}

fn checkpoint_restore_equivalence(use_cuda: bool) {
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("source");
    let routing = root.path().join("source-routing");
    let checkpoint = root.path().join("checkpoint");
    let restored = root.path().join("restored");
    let restored_routing = root.path().join("restored-routing");
    let mut config = resident_config();
    config.routing_image_root = Some(routing.clone());
    #[cfg(feature = "cuda")]
    if use_cuda {
        config.cuda = CudaConfig {
            enabled: true,
            executor_policy: CudaExecutorPolicy::RequireCuda,
            minimum_free_memory_headroom: 0,
            algorithm: CudaAlgorithmSelection::Frontier,
            ..CudaConfig::default()
        };
    }
    #[cfg(not(feature = "cuda"))]
    assert!(!use_cuda);
    let engine = std::sync::Arc::new(GraphEngine::open(&database, config.clone()).unwrap());
    let graph = materialize(&engine, &generated_spec(ORDINARY_SEEDS[0]));
    let profile = profiles(&graph)[0].clone();
    let request = RoutingRequest::new(
        graph.nodes[0].id(),
        destinations(&graph),
        profile,
        true,
        SearchBudget::Unlimited,
        TiePolicy::StablePredecessor,
    );
    let before = engine
        .route(RequestId::new(50_000), &request)
        .unwrap()
        .response;
    let provisional = engine
        .insert_node_candidate("checkpoint provisional")
        .unwrap();
    let provisional_relation = engine
        .insert_relation_candidate("checkpoint provisional relation")
        .unwrap();
    let provisional_edge = engine
        .insert_edge_candidate(
            graph.nodes[0].id(),
            graph.nodes[1].id(),
            graph.relations[0].id(),
            0.75,
        )
        .unwrap();
    let hydration_request = HydrationRequest::new(
        graph.nodes.iter().map(NodeRecord::id),
        graph.edges.iter().map(EdgeRecord::id),
        Some(request.profile().clone()),
    );
    let hydration_before = std::sync::Arc::new(engine.hydrate(&hydration_request).unwrap());
    let before_evidence = std::sync::Arc::new(response_evidence(&before));
    let readers_started = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let stop_readers = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let readers = (0..3_u64)
        .map(|reader| {
            let engine = std::sync::Arc::clone(&engine);
            let request = request.clone();
            let hydration_request = hydration_request.clone();
            let hydration_before = std::sync::Arc::clone(&hydration_before);
            let before_evidence = std::sync::Arc::clone(&before_evidence);
            let readers_started = std::sync::Arc::clone(&readers_started);
            let stop_readers = std::sync::Arc::clone(&stop_readers);
            std::thread::spawn(move || {
                let mut iteration = 0_u64;
                loop {
                    let routed = engine
                        .route(
                            RequestId::new(51_000 + reader * 1_000_000 + iteration),
                            &request,
                        )
                        .unwrap();
                    assert_eq!(response_evidence(&routed.response), *before_evidence);
                    assert_eq!(
                        engine.hydrate(&hydration_request).unwrap(),
                        *hydration_before
                    );
                    readers_started.fetch_add(1, std::sync::atomic::Ordering::Release);
                    iteration += 1;
                    if stop_readers.load(std::sync::atomic::Ordering::Acquire) {
                        break;
                    }
                }
            })
        })
        .collect::<Vec<_>>();
    while readers_started.load(std::sync::atomic::Ordering::Acquire) < readers.len() {
        std::thread::yield_now();
    }
    engine
        .create_checkpoint(&CheckpointRequest {
            destination_root: root.path().to_path_buf(),
            destination: checkpoint.clone(),
            routing_image_root: Some(routing),
            scratch_path: None,
            available_destination_bytes: u64::MAX,
            minimum_headroom_bytes: 0,
        })
        .unwrap();
    let bundle_before = engine.health().unwrap().current_bundle.unwrap();
    stop_readers.store(true, std::sync::atomic::Ordering::Release);
    for reader in readers {
        reader.join().unwrap();
    }
    engine.remove_edge(graph.first_equal_tail).unwrap();
    let after = engine
        .route(RequestId::new(50_001), &request)
        .unwrap()
        .response;
    assert_ne!(response_evidence(&before), response_evidence(&after));
    assert!(engine.shutdown().unwrap().complete());

    let mut restore_config = resident_config();
    restore_config.routing_image_root = Some(restored_routing.clone());
    #[cfg(feature = "cuda")]
    if use_cuda {
        restore_config.cuda = config.cuda;
    }
    let report = GraphEngine::restore_checkpoint(&EngineRestoreRequest {
        store: RestoreRequest {
            source_root: root.path().to_path_buf(),
            source_checkpoint: checkpoint,
            destination_root: root.path().to_path_buf(),
            destination: restored.clone(),
            routing_image_root: Some(restored_routing.clone()),
            scratch_path: None,
            available_destination_bytes: u64::MAX,
            minimum_headroom_bytes: 0,
            verification_limits: VerificationLimits {
                maximum_records: Some(10_000),
                maximum_duration: Some(std::time::Duration::from_secs(30)),
            },
        },
        engine_config: restore_config.clone(),
    })
    .unwrap();
    assert!(
        report.smoke_catalog_verified
            && report.smoke_route_verified
            && report.smoke_hydration_verified
    );
    let restored_engine = GraphEngine::open(restored, restore_config).unwrap();
    let restored_response = restored_engine
        .route(RequestId::new(50_002), &request)
        .unwrap()
        .response;
    let mut restored_bundle = restored_engine.health().unwrap().current_bundle.unwrap();
    restored_bundle.relative_name = bundle_before.relative_name.clone();
    assert_eq!(
        restored_bundle, bundle_before,
        "identical restored graph/config must rebuild identical bundle contents; publication directory identity is intentionally fresh"
    );
    assert_eq!(
        response_evidence(&restored_response),
        response_evidence(&before)
    );
    assert!(restored_engine.get_candidate(provisional).is_ok());
    assert!(restored_engine.get_candidate(provisional_relation).is_ok());
    assert!(restored_engine.get_candidate(provisional_edge).is_ok());
    assert_eq!(
        restored_engine.hydrate(&hydration_request).unwrap(),
        *hydration_before
    );
    for node in &graph.nodes {
        assert_eq!(
            restored_engine
                .lookup_node_exact(node.name().as_str())
                .unwrap(),
            Some(node.id())
        );
        assert_eq!(&restored_engine.get_node(node.id()).unwrap(), node);
    }
    for relation in &graph.relations {
        assert_eq!(
            &restored_engine.get_relation(relation.id()).unwrap(),
            relation
        );
    }
    for edge in &graph.edges {
        assert_eq!(&restored_engine.get_edge(edge.id()).unwrap(), edge);
    }
}

#[test]
fn concurrent_publication_schedule_preserves_complete_cpu_images_and_owned_resources() {
    concurrent_publication_schedule(false);
}

#[cfg(feature = "cuda")]
#[test]
fn concurrent_publication_schedule_preserves_complete_cuda_images_and_owned_resources() {
    concurrent_publication_schedule(true);
}

fn concurrent_publication_schedule(use_cuda: bool) {
    let root = tempfile::tempdir().unwrap();
    let routing_root = root.path().join("routing");
    let mut config = partitioned_config(128, 16);
    config.routing_image_root = Some(routing_root.clone());
    #[cfg(feature = "cuda")]
    if use_cuda {
        config.cuda = CudaConfig {
            enabled: true,
            executor_policy: CudaExecutorPolicy::RequireCuda,
            minimum_free_memory_headroom: 0,
            maximum_partitioned_topology_cache_bytes: 8 * 1024,
            maximum_partitioned_topology_cache_slots: 1,
            maximum_partitioned_host_staging_bytes: 2 * 1024,
            maximum_concurrent_searches: 8,
            algorithm: CudaAlgorithmSelection::Frontier,
            ..CudaConfig::default()
        };
    }
    #[cfg(not(feature = "cuda"))]
    assert!(!use_cuda);
    let engine =
        std::sync::Arc::new(GraphEngine::open(root.path().join("catalog"), config).unwrap());
    let graph = materialize(&engine, &generated_spec(ORDINARY_SEEDS[2]));
    let profile = profiles(&graph)[1].clone();
    let destination = graph.nodes[3].id();
    let hub_candidate = engine
        .insert_node_candidate("schedule cascade hub")
        .unwrap();
    let ConfirmedRecord::Node(hub) = engine
        .confirm_validated_candidate(hub_candidate)
        .unwrap()
        .into_parts()
        .0
    else {
        unreachable!()
    };
    for index in 0..8 {
        let leaf_candidate = engine
            .insert_node_candidate(format!("schedule cascade leaf {index}"))
            .unwrap();
        let ConfirmedRecord::Node(leaf) = engine
            .confirm_validated_candidate(leaf_candidate)
            .unwrap()
            .into_parts()
            .0
        else {
            unreachable!()
        };
        for (source, target) in [(leaf.id(), hub.id()), (hub.id(), leaf.id())] {
            let candidate = engine
                .insert_edge_candidate(source, target, graph.relations[0].id(), 0.8)
                .unwrap();
            assert!(matches!(
                engine
                    .confirm_validated_candidate(candidate)
                    .unwrap()
                    .into_parts()
                    .0,
                ConfirmedRecord::Edge(_)
            ));
        }
    }
    let request = std::sync::Arc::new(RoutingRequest::new(
        graph.nodes[0].id(),
        [destination, destination, NodeId::from_u64(u64::MAX)],
        profile.clone(),
        true,
        SearchBudget::Unlimited,
        TiePolicy::StablePredecessor,
    ));
    let before = engine.route(RequestId::new(80_000), &request).unwrap();
    let mut subgraph = pathhydra_subgraph::Subgraph::new();
    let path = before
        .response
        .results()
        .iter()
        .find_map(|result| match result.state() {
            DestinationState::Exact(exact) => exact.path(),
            _ => None,
        })
        .expect("generated route has path evidence");
    let subgraph_edge_to_delete = path
        .steps()
        .first()
        .expect("non-origin destination path has an edge")
        .edge_id();
    subgraph.add_path(path).unwrap();
    assert!(
        engine
            .hydrate_subgraph(&subgraph, Some(profile.clone()))
            .unwrap()
            .complete
    );
    let hydration_request = std::sync::Arc::new(HydrationRequest::new(
        graph.nodes.iter().map(NodeRecord::id),
        graph.edges.iter().map(EdgeRecord::id),
        Some(profile.clone()),
    ));
    let shared_subgraph = std::sync::Arc::new(subgraph.clone());
    let shared_profile = std::sync::Arc::new(profile.clone());

    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let started = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let workers = (0..4_u64)
        .map(|worker| {
            let engine = std::sync::Arc::clone(&engine);
            let request = std::sync::Arc::clone(&request);
            let hydration_request = std::sync::Arc::clone(&hydration_request);
            let subgraph = std::sync::Arc::clone(&shared_subgraph);
            let profile = std::sync::Arc::clone(&shared_profile);
            let stop = std::sync::Arc::clone(&stop);
            let started = std::sync::Arc::clone(&started);
            std::thread::spawn(move || {
                let mut evidence = Vec::new();
                for iteration in 0..64_u64 {
                    match engine.route(RequestId::new(81_000 + worker * 100 + iteration), &request)
                    {
                        Ok(routed) => {
                            evidence.push(response_evidence(&routed.response));
                            assert!(engine.health().unwrap().current_image_manifest.is_some());
                        }
                        Err(pathhydra_engine::EngineError::Lifecycle(_)) => break,
                        Err(error) => panic!("route worker failed unexpectedly: {error}"),
                    }
                    match engine.hydrate(&hydration_request) {
                        Ok(hydrated) => {
                            assert_eq!(hydrated.nodes.len(), hydration_request.node_ids().len());
                            assert_eq!(hydrated.edges.len(), hydration_request.edge_ids().len());
                        }
                        Err(pathhydra_engine::EngineError::Lifecycle(_)) => break,
                        Err(error) => panic!("hydration worker failed unexpectedly: {error}"),
                    }
                    match engine.hydrate_subgraph(&subgraph, Some((*profile).clone())) {
                        Ok(hydrated) => {
                            assert_eq!(
                                hydrated.nodes.len() + hydrated.missing_node_ids.len(),
                                subgraph.node_count()
                            );
                            assert_eq!(
                                hydrated.edges.len() + hydrated.missing_edge_ids.len(),
                                subgraph.edge_count()
                            );
                        }
                        Err(pathhydra_engine::EngineError::Lifecycle(_)) => break,
                        Err(error) => {
                            panic!("subgraph hydration worker failed unexpectedly: {error}")
                        }
                    }
                    started.fetch_add(1, std::sync::atomic::Ordering::Release);
                    if stop.load(std::sync::atomic::Ordering::Acquire) {
                        break;
                    }
                }
                evidence
            })
        })
        .collect::<Vec<_>>();
    while started.load(std::sync::atomic::Ordering::Acquire) < workers.len() {
        std::thread::yield_now();
    }

    let checkpoint_engine = std::sync::Arc::clone(&engine);
    let checkpoint_root = root.path().to_path_buf();
    let checkpoint_routing = routing_root.clone();
    let checkpoint = std::thread::spawn(move || {
        checkpoint_engine.create_checkpoint(&CheckpointRequest {
            destination_root: checkpoint_root.clone(),
            destination: checkpoint_root.join("concurrent-checkpoint"),
            routing_image_root: Some(checkpoint_routing),
            scratch_path: None,
            available_destination_bytes: u64::MAX,
            minimum_headroom_bytes: 0,
        })
    });

    let provisional = engine
        .insert_node_candidate("schedule provisional")
        .unwrap();
    let provisional_relation = engine
        .insert_relation_candidate("schedule provisional relation")
        .unwrap();
    let duplicate = engine
        .insert_node_candidate(graph.nodes[0].name().as_str())
        .unwrap();
    let ConfirmedRecord::Node(duplicate_node) = engine
        .confirm_validated_candidate(duplicate)
        .unwrap()
        .into_parts()
        .0
    else {
        unreachable!()
    };
    assert_eq!(duplicate_node.id(), graph.nodes[0].id());
    assert!(engine.get_candidate(duplicate).is_err());
    assert!(engine.get_candidate(provisional).is_ok());
    assert!(engine.get_candidate(provisional_relation).is_ok());

    let direct_candidate = engine
        .insert_edge_candidate(
            graph.nodes[0].id(),
            destination,
            graph.relations[0].id(),
            0.125,
        )
        .unwrap();
    let ConfirmedRecord::Edge(direct) = engine
        .confirm_validated_candidate(direct_candidate)
        .unwrap()
        .into_parts()
        .0
    else {
        unreachable!()
    };
    let middle = engine.route(RequestId::new(89_000), &request).unwrap();
    engine.remove_edge(direct.id()).unwrap();
    engine.remove_node(hub.id()).unwrap();
    assert!(engine.get_node(hub.id()).is_err());
    engine.remove_edge(subgraph_edge_to_delete).unwrap();
    engine.rebuild_routing_image().unwrap();
    let after = engine.route(RequestId::new(89_001), &request).unwrap();
    let valid = [
        response_evidence(&before.response),
        response_evidence(&middle.response),
        response_evidence(&after.response),
    ];
    assert!(checkpoint.join().unwrap().unwrap().file_count > 0);
    let hydrated_after = engine.hydrate_subgraph(&subgraph, Some(profile)).unwrap();
    assert!(!hydrated_after.complete && !hydrated_after.missing_edge_ids.is_empty());
    stop.store(true, std::sync::atomic::Ordering::Release);
    let observed = workers
        .into_iter()
        .flat_map(|worker| worker.join().unwrap())
        .collect::<Vec<_>>();
    let shutdown = engine.shutdown().unwrap();
    assert!(shutdown.complete());
    assert!(shutdown.drain.drained);
    assert_eq!(shutdown.drain.remaining, Default::default());
    assert!(!observed.is_empty());
    assert!(observed.iter().all(|response| {
        valid.iter().any(|allowed| response == allowed)
            || response
                .iter()
                .any(|state| matches!(state, ResponseEvidence::Incomplete))
    }));
}

#[derive(Debug, Eq, PartialEq)]
enum ResponseEvidence {
    Exact {
        distance_bits: u64,
        path: Option<Vec<(pathhydra_core::EdgeId, u64)>>,
    },
    Unreachable,
    Missing,
    Incomplete,
}

fn response_evidence(response: &pathhydra_routing::RoutingResponse) -> Vec<ResponseEvidence> {
    response
        .results()
        .iter()
        .map(|result| match result.state() {
            DestinationState::Exact(exact) => ResponseEvidence::Exact {
                distance_bits: exact.logical_distance().to_bits(),
                path: exact.path().map(|path| {
                    path.steps()
                        .iter()
                        .map(|step| (step.edge_id(), step.effective_weight().to_bits()))
                        .collect()
                }),
            },
            DestinationState::Unreachable => ResponseEvidence::Unreachable,
            DestinationState::MissingNode => ResponseEvidence::Missing,
            DestinationState::Incomplete => ResponseEvidence::Incomplete,
        })
        .collect()
}

/// Extended deterministic agreement gate:
/// `cargo test -p pathhydra-engine --test system_conformance extended_generated_conformance -- --ignored --nocapture`
#[test]
#[ignore = "extended deterministic generated graph matrix"]
fn extended_generated_conformance() {
    let mut seed = 0x082e_fa98_ec4e_6c89;
    for _case in 0..32 {
        seed = ReplayRng::new(seed).next();
        eprintln!("extended conformance seed={seed:#018x}");
        run_cpu_seed(seed);
        #[cfg(feature = "cuda")]
        run_cuda_seed(seed);
    }
}

#[cfg(feature = "cuda")]
fn cuda_config(algorithm: CudaAlgorithmSelection, partitioned: bool) -> EngineConfig {
    let mut config = if partitioned {
        partitioned_config(256, 2)
    } else {
        resident_config()
    };
    config.cuda = CudaConfig {
        enabled: true,
        executor_policy: CudaExecutorPolicy::RequireCuda,
        minimum_free_memory_headroom: 0,
        maximum_reserved_search_bytes: 256 * 1024 * 1024,
        maximum_partitioned_topology_cache_bytes: 8 * 1024,
        maximum_partitioned_topology_cache_slots: 2,
        maximum_partitioned_host_staging_bytes: 2 * 1024,
        batch_collection_delay: std::time::Duration::ZERO,
        algorithm,
        ..CudaConfig::default()
    };
    config
}

#[cfg(feature = "cuda")]
#[test]
fn resident_and_partitioned_cuda_algorithms_match_independent_oracle() {
    let seed = ORDINARY_SEEDS[1];
    run_cuda_seed(seed);
}

#[cfg(feature = "cuda")]
fn run_cuda_seed(seed: u64) {
    let spec = generated_spec(seed);
    for (algorithm_index, algorithm) in [
        CudaAlgorithmSelection::Frontier,
        CudaAlgorithmSelection::DeltaStepping { delta: 0.125 },
    ]
    .into_iter()
    .enumerate()
    {
        for partitioned in [false, true] {
            eprintln!(
                "CUDA conformance seed={seed:#018x} algorithm={algorithm:?} partitioned={partitioned}"
            );
            let root = tempfile::tempdir().unwrap();
            let engine = GraphEngine::open(
                root.path().join("catalog"),
                cuda_config(algorithm, partitioned),
            )
            .unwrap();
            let graph = materialize(&engine, &spec);
            assert_provisional_invisibility(
                &engine,
                &graph,
                &profiles(&graph)[1],
                68_000 + (algorithm_index * 10 + usize::from(partitioned)) as u64,
            );
            for (profile_index, profile) in profiles(&graph).iter().enumerate() {
                if profile_index == 0
                    && matches!(algorithm, CudaAlgorithmSelection::DeltaStepping { .. })
                {
                    let request = RoutingRequest::new(
                        graph.nodes[0].id(),
                        destinations(&graph),
                        profile.clone(),
                        true,
                        SearchBudget::Unlimited,
                        TiePolicy::StablePredecessor,
                    );
                    let error = engine
                        .route(
                            RequestId::new(
                                69_000
                                    + (algorithm_index * 100
                                        + usize::from(partitioned) * 10
                                        + profile_index)
                                        as u64,
                            ),
                            &request,
                        )
                        .expect_err("unrepresentable delta bucket must not yield a response");
                    assert!(
                        matches!(error, pathhydra_engine::EngineError::CudaFailure(ref reason) if reason.contains("status 6")),
                        "seed={seed:#018x} partitioned={partitioned}: {error}"
                    );
                    continue;
                }
                assert_conformance(
                    &engine,
                    &graph,
                    profile,
                    70_000
                        + (algorithm_index * 100 + usize::from(partitioned) * 10 + profile_index)
                            as u64,
                    Executor::NvidiaCuda,
                    seed,
                );
            }
            assert_eq!(
                engine.health().unwrap().cuda.partitioned_topology,
                partitioned
            );
        }
    }
}
