#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PartitionPhaseDiagnostics {
    pub partitions_required: u64,
    pub host_cache_hits: u64,
    pub device_cache_hits: u64,
    pub file_bytes: u64,
    pub staged_bytes: u64,
    pub transfer_bytes: u64,
    pub launches: u64,
}
