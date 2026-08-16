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
    pub bundle_bytes: u64,
    pub partition_count: usize,
    pub cache_bytes: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub file_bytes: u64,
    pub transfer_bytes: u64,
    pub cold: bool,
}

pub fn header() {
    println!(
        "workload,executor,algorithm,nodes,adjacencies,topology_bytes,bundle_bytes,partitions,cache_bytes,cache_hits,cache_misses,file_bytes,transfer_bytes,cold,upload_us,route_us,correct,gpu,compute_capability,driver,host_rust,kernel_rust,ptx_target"
    );
}

pub fn write(measurement: &Measurement<'_>) {
    println!(
        "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},1.95.0,nightly-2024-02-17,sm_86",
        measurement.workload,
        measurement.executor,
        measurement.algorithm,
        measurement.nodes,
        measurement.adjacencies,
        measurement.topology_bytes,
        measurement.bundle_bytes,
        measurement.partition_count,
        measurement.cache_bytes,
        measurement.cache_hits,
        measurement.cache_misses,
        measurement.file_bytes,
        measurement.transfer_bytes,
        measurement.cold,
        measurement.upload.as_micros(),
        measurement.elapsed.as_micros(),
        measurement.correctness,
        measurement.gpu_name,
        measurement.compute_capability,
        measurement.driver,
    );
}
