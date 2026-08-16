mod fixtures;
mod report;
mod routing;

fn main() {
    let arguments: Vec<_> = std::env::args().collect();
    if arguments.len() != 3 || arguments[1] != "--suite" || arguments[2] != "baseline" {
        eprintln!("usage: pathhydra-bench --suite baseline");
        std::process::exit(2);
    }
    report::header();
    #[cfg(feature = "cuda")]
    let context = pathhydra_cuda::CudaContextOwner::initialize(0).expect("CUDA device");
    for workload in fixtures::BASELINE {
        let fixture = fixtures::build(*workload);
        let (cpu_reference, cpu) = routing::cpu(workload.name, &fixture);
        report::write(&cpu);
        #[cfg(not(feature = "cuda"))]
        let _ = &cpu_reference;
        #[cfg(feature = "cuda")]
        for algorithm in [
            pathhydra_cuda::CudaAlgorithm::Frontier,
            pathhydra_cuda::CudaAlgorithm::DeltaStepping(
                pathhydra_cuda::DeltaConfiguration::new(0.1).unwrap(),
            ),
        ] {
            let cuda = routing::cuda(
                workload.name,
                &fixture,
                &cpu_reference,
                context.clone(),
                algorithm,
            );
            report::write(&cuda);
        }
    }
}
