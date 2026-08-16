use std::sync::Arc;

use pathhydra_core::ConfirmedRecord;
use pathhydra_routing::{
    RelationMultiplier, RelationProfile, RelationUse, RoutingImage, RoutingRequest, SearchBudget,
    TiePolicy,
};
use pathhydra_store::Catalog;

#[derive(Clone, Copy)]
pub enum Shape {
    Chain,
    Star,
    Dense,
    ZeroClosure,
}

#[derive(Clone, Copy)]
pub struct Workload {
    pub name: &'static str,
    pub nodes: usize,
    pub shape: Shape,
}

pub const BASELINE: &[Workload] = &[
    Workload {
        name: "narrow-chain",
        nodes: 128,
        shape: Shape::Chain,
    },
    Workload {
        name: "broad-star",
        nodes: 512,
        shape: Shape::Star,
    },
    Workload {
        name: "dense-scc",
        nodes: 48,
        shape: Shape::Dense,
    },
    Workload {
        name: "zero-closure",
        nodes: 128,
        shape: Shape::ZeroClosure,
    },
];

pub struct Fixture {
    _directory: tempfile::TempDir,
    pub image: Arc<RoutingImage>,
    pub request: RoutingRequest,
}

pub fn build(workload: Workload) -> Fixture {
    let directory = tempfile::tempdir().expect("temporary benchmark directory");
    let catalog = Catalog::open(directory.path()).expect("benchmark catalog");
    let nodes: Vec<_> = (0..workload.nodes)
        .map(|index| {
            let candidate = catalog
                .insert_node_candidate(format!("node-{index}"))
                .expect("node candidate");
            let ConfirmedRecord::Node(node) = catalog
                .confirm_validated_candidate(candidate)
                .expect("confirmed node")
            else {
                unreachable!()
            };
            node.id()
        })
        .collect();
    let candidate = catalog
        .insert_relation_candidate("route")
        .expect("relation candidate");
    let ConfirmedRecord::Relation(relation) = catalog
        .confirm_validated_candidate(candidate)
        .expect("confirmed relation")
    else {
        unreachable!()
    };
    let mut edges = Vec::new();
    match workload.shape {
        Shape::Chain | Shape::ZeroClosure => {
            for index in 0..nodes.len().saturating_sub(1) {
                edges.push((index, index + 1));
            }
        }
        Shape::Star => {
            for index in 1..nodes.len() {
                edges.push((0, index));
            }
        }
        Shape::Dense => {
            for source in 0..nodes.len() {
                for destination in 0..nodes.len() {
                    if source != destination {
                        edges.push((source, destination));
                    }
                }
            }
        }
    }
    let weight = if matches!(workload.shape, Shape::ZeroClosure) {
        0.0
    } else {
        0.01
    };
    for (source, destination) in edges {
        let candidate = catalog
            .insert_edge_candidate(nodes[source], nodes[destination], relation.id(), weight)
            .expect("edge candidate");
        catalog
            .confirm_validated_candidate(candidate)
            .expect("confirmed edge");
    }
    let image = Arc::new(
        RoutingImage::compile(
            &catalog
                .confirmed_graph_records()
                .expect("confirmed records"),
        )
        .expect("routing image"),
    );
    let destinations = [nodes[nodes.len() - 1], nodes[nodes.len() / 2], nodes[0]];
    let request = RoutingRequest::new(
        nodes[0],
        destinations,
        RelationProfile::new([(
            relation.id(),
            RelationUse::Enabled(RelationMultiplier::new(1.0).unwrap()),
        )]),
        false,
        SearchBudget::Unlimited,
        TiePolicy::StablePredecessor,
    );
    Fixture {
        _directory: directory,
        image,
        request,
    }
}
