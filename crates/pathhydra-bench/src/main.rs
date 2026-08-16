mod fixtures;
mod report;
mod routing;
mod scale;

fn main() {
    let arguments: Vec<_> = std::env::args().collect();
    if arguments.len() < 3 || arguments[1] != "--suite" {
        eprintln!(
            "usage: pathhydra-bench --suite baseline|out-of-core|scale [directory] [target-gib]"
        );
        std::process::exit(2);
    }
    report::header();
    if arguments[2] == "scale" {
        let directory = arguments
            .get(3)
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("pathhydra-scale-bundle"));
        let target_gib = arguments.get(4).map_or(12, |value| {
            value.parse().expect("target GiB must be an integer")
        });
        scale::run(&directory, target_gib);
        return;
    }
    if !matches!(arguments[2].as_str(), "baseline" | "out-of-core") {
        eprintln!("unknown benchmark suite {}", arguments[2]);
        std::process::exit(2);
    }
    #[cfg(feature = "cuda")]
    let context = pathhydra_cuda::CudaContextOwner::initialize(0).expect("CUDA device");
    for workload in fixtures::BASELINE {
        let fixture = fixtures::build(*workload);
        let (cpu_reference, cpu) = routing::cpu(workload.name, &fixture);
        report::write(&cpu);
        if arguments[2] == "out-of-core" {
            report::write(&routing::partitioned_cpu(
                workload.name,
                &fixture,
                &cpu_reference,
                true,
            ));
            report::write(&routing::partitioned_cpu(
                workload.name,
                &fixture,
                &cpu_reference,
                false,
            ));
            report::write(&routing::partitioned_cpu_thrash(
                workload.name,
                &fixture,
                &cpu_reference,
            ));
        }
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
            if arguments[2] == "out-of-core" {
                report::write(&routing::partitioned_cuda(
                    workload.name,
                    &fixture,
                    &cpu_reference,
                    context.clone(),
                    algorithm,
                ));
            }
        }
    }
}
