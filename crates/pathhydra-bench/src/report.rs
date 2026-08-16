use std::time::Duration;

pub struct Measurement<'a> {
    pub workload: &'a str,
    pub executor: &'a str,
    pub algorithm: &'a str,
    pub nodes: usize,
    pub adjacencies: usize,
    pub topology_bytes: usize,
    pub upload: Duration,
    pub elapsed: Duration,
    pub correctness: bool,
    pub gpu_name: String,
    pub compute_capability: String,
    pub driver: i32,
}

pub fn header() {
    println!(
        "workload,executor,algorithm,nodes,adjacencies,topology_bytes,upload_us,route_us,correct,gpu,compute_capability,driver,host_rust,kernel_rust,ptx_target"
    );
}

pub fn write(measurement: &Measurement<'_>) {
    println!(
        "{},{},{},{},{},{},{},{},{},{},{},{},1.95.0,nightly-2024-02-17,sm_86",
        measurement.workload,
        measurement.executor,
        measurement.algorithm,
        measurement.nodes,
        measurement.adjacencies,
        measurement.topology_bytes,
        measurement.upload.as_micros(),
        measurement.elapsed.as_micros(),
        measurement.correctness,
        measurement.gpu_name,
        measurement.compute_capability,
        measurement.driver,
    );
}
