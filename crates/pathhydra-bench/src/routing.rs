use std::time::{Duration, Instant};

use pathhydra_routing::{DestinationState, RoutingResponse, route};

use crate::{fixtures::Fixture, report::Measurement};

pub fn cpu<'a>(name: &'a str, fixture: &Fixture) -> (RoutingResponse, Measurement<'a>) {
    let expected = route(&fixture.image, &fixture.request).expect("CPU oracle");
    for _ in 0..3 {
        let _ = route(&fixture.image, &fixture.request).expect("CPU warmup");
    }
    let started = Instant::now();
    let actual = route(&fixture.image, &fixture.request).expect("CPU timed route");
    let elapsed = started.elapsed();
    (
        expected.clone(),
        Measurement {
            workload: name,
            executor: "cpu",
            algorithm: "dijkstra",
            nodes: fixture.image.node_count(),
            adjacencies: fixture.image.adjacency_count(),
            topology_bytes: fixture.image.manifest().byte_counts().total(),
            upload: Duration::ZERO,
            elapsed,
            correctness: same_distances(&expected, &actual),
            gpu_name: String::new(),
            compute_capability: String::new(),
            driver: 0,
        },
    )
}

#[cfg(feature = "cuda")]
pub fn cuda<'a>(
    name: &'a str,
    fixture: &Fixture,
    expected: &RoutingResponse,
    context: std::sync::Arc<pathhydra_cuda::CudaContextOwner>,
    algorithm: pathhydra_cuda::CudaAlgorithm,
) -> Measurement<'a> {
    let upload_started = Instant::now();
    let resident = pathhydra_cuda::CudaResidentImage::upload(
        context.clone(),
        fixture.image.clone(),
        usize::MAX,
        0,
    )
    .expect("CUDA residency");
    let upload = upload_started.elapsed();
    let reserved = pathhydra_cuda::estimate_search_bytes(
        fixture.image.node_count(),
        fixture.image.relation_kind_count(),
        fixture.request.destinations().len(),
        algorithm,
    )
    .expect("CUDA memory estimate");
    for _ in 0..3 {
        resident
            .route(
                &fixture.request,
                algorithm,
                &std::sync::atomic::AtomicBool::new(false),
                reserved,
            )
            .expect("CUDA warmup");
    }
    let started = Instant::now();
    let output = resident
        .route(
            &fixture.request,
            algorithm,
            &std::sync::atomic::AtomicBool::new(false),
            reserved,
        )
        .expect("CUDA timed route");
    let elapsed = started.elapsed();
    let capabilities = context.capabilities();
    let compute = format!(
        "{}.{}",
        capabilities.compute_capability.0, capabilities.compute_capability.1
    );
    Measurement {
        workload: name,
        executor: "cuda",
        algorithm: match algorithm {
            pathhydra_cuda::CudaAlgorithm::Frontier => "frontier",
            pathhydra_cuda::CudaAlgorithm::DeltaStepping(_) => "delta-stepping",
        },
        nodes: fixture.image.node_count(),
        adjacencies: fixture.image.adjacency_count(),
        topology_bytes: resident.allocated_bytes(),
        upload,
        elapsed,
        correctness: same_distances(expected, &output.response),
        gpu_name: capabilities.device_name.clone(),
        compute_capability: compute,
        driver: capabilities.driver_version,
    }
}

fn same_distances(expected: &RoutingResponse, actual: &RoutingResponse) -> bool {
    expected
        .results()
        .iter()
        .zip(actual.results())
        .all(|(left, right)| {
            left.destination() == right.destination()
                && match (left.state(), right.state()) {
                    (DestinationState::Exact(left), DestinationState::Exact(right)) => {
                        left.logical_distance().to_bits() == right.logical_distance().to_bits()
                    }
                    (left, right) => left == right,
                }
        })
}
