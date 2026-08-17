mod fixtures;
mod operations;
mod report;
mod routing;
mod scale;
mod strategy;
mod system;

fn main() {
    let arguments: Vec<_> = std::env::args().collect();
    if arguments.len() < 3 || arguments[1] != "--suite" {
        eprintln!(
            "usage: pathhydra-bench --suite SUITE [--format human|csv|json] [--repeats N] [--warmup N]\n\
             suites: store-ingest, store-mutation, snapshot-build-load, cpu-routing, cuda-resident, cuda-out-of-core, concurrency, reconstruction-hydration, backup-restore, scale, all\n\
             compatibility: baseline, out-of-core, parallel-strategy [repeat-count], operations, scale [directory] [target-gib]"
        );
        std::process::exit(2);
    }
    if arguments[2] == "operations" {
        operations::run();
        return;
    }
    if arguments[2] == "parallel-strategy" {
        let repeats = arguments.get(3).map_or(5, |value| {
            value
                .parse::<usize>()
                .expect("repeat count must be a positive integer")
        });
        if repeats == 0 {
            eprintln!("repeat count must be a positive integer");
            std::process::exit(2);
        }
        report::strategy_header();
        strategy::run(repeats);
        return;
    }
    const SYSTEM_SUITES: &[&str] = &[
        "store-ingest",
        "store-mutation",
        "snapshot-build-load",
        "cpu-routing",
        "cuda-resident",
        "cuda-out-of-core",
        "concurrency",
        "reconstruction-hydration",
        "backup-restore",
        "all",
    ];
    if SYSTEM_SUITES.contains(&arguments[2].as_str())
        || arguments[2] == "scale"
            && arguments
                .get(3)
                .is_some_and(|value| value.starts_with("--"))
    {
        let mut repeats = 3_usize;
        let mut warmups = 1_usize;
        let mut format = system::OutputFormat::Human;
        let mut index = 3;
        while index < arguments.len() {
            let option = &arguments[index];
            let value = arguments.get(index + 1).unwrap_or_else(|| {
                eprintln!("missing value for {option}");
                std::process::exit(2);
            });
            match option.as_str() {
                "--repeats" => repeats = value.parse().unwrap_or_else(|_| usage_number("repeats")),
                "--warmup" => warmups = value.parse().unwrap_or_else(|_| usage_number("warmup")),
                "--format" => {
                    format = match value.as_str() {
                        "human" => system::OutputFormat::Human,
                        "csv" => system::OutputFormat::Csv,
                        "json" => system::OutputFormat::Json,
                        _ => {
                            eprintln!("format must be human, csv, or json");
                            std::process::exit(2);
                        }
                    }
                }
                _ => {
                    eprintln!("unknown option {option}");
                    std::process::exit(2);
                }
            }
            index += 2;
        }
        if repeats == 0 {
            eprintln!("repeats must be positive");
            std::process::exit(2);
        }
        system::run(
            &arguments[2],
            system::Options {
                repeats,
                warmups,
                format,
            },
        );
        return;
    }
    if arguments[2] == "scale" {
        let directory = arguments
            .get(3)
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("target/pathhydra-scale-bundle"));
        let target_gib = arguments.get(4).map_or(12, |value| {
            value.parse().expect("target GiB must be an integer")
        });
        scale::header();
        scale::run(&directory, target_gib);
        return;
    }
    report::header();
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

fn usage_number(name: &str) -> ! {
    eprintln!("{name} must be a nonnegative integer");
    std::process::exit(2)
}
