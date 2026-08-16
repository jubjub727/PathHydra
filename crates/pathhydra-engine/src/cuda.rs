use std::{fmt, time::Duration};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CudaExecutorPolicy {
    #[default]
    CpuOnly,
    PreferCuda,
    RequireCuda,
    Auto,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum CudaAlgorithmSelection {
    Frontier,
    DeltaStepping {
        delta: f64,
    },
    #[default]
    Automatic,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CudaConfig {
    pub enabled: bool,
    pub device_ordinal: usize,
    pub executor_policy: CudaExecutorPolicy,
    pub maximum_topology_bytes: usize,
    pub minimum_free_memory_headroom: usize,
    pub maximum_concurrent_searches: usize,
    pub maximum_batch_lanes: usize,
    pub maximum_reserved_search_bytes: usize,
    pub batch_collection_delay: Duration,
    pub algorithm: CudaAlgorithmSelection,
    pub delta_candidates: [f64; 4],
    pub delta_candidate_count: usize,
}

impl Default for CudaConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            device_ordinal: 0,
            executor_policy: CudaExecutorPolicy::CpuOnly,
            maximum_topology_bytes: 6 * 1024 * 1024 * 1024,
            minimum_free_memory_headroom: 1024 * 1024 * 1024,
            maximum_concurrent_searches: 4,
            maximum_batch_lanes: 4,
            maximum_reserved_search_bytes: 2 * 1024 * 1024 * 1024,
            batch_collection_delay: Duration::from_micros(100),
            algorithm: CudaAlgorithmSelection::Automatic,
            delta_candidates: [0.01, 0.1, 1.0, 10.0],
            delta_candidate_count: 4,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CudaIneligibility {
    Disabled,
    SupportNotCompiled,
    PathsUnsupportedByCuda,
    FiniteEdgeBudgetUnsupportedByCuda,
    NoResidentImage(String),
    ResourceRefusal(String),
    AutomaticPolicySelectedCpu,
    Unhealthy(String),
}

impl fmt::Display for CudaIneligibility {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disabled => formatter.write_str("CUDA is disabled"),
            Self::SupportNotCompiled => formatter.write_str("CUDA support is not compiled in"),
            Self::PathsUnsupportedByCuda => formatter.write_str("paths are unsupported by CUDA"),
            Self::FiniteEdgeBudgetUnsupportedByCuda => {
                formatter.write_str("finite examined-edge budgets are unsupported by CUDA")
            }
            Self::NoResidentImage(reason) => {
                write!(formatter, "no matching CUDA resident image: {reason}")
            }
            Self::ResourceRefusal(reason) => {
                write!(formatter, "CUDA resource admission refused: {reason}")
            }
            Self::AutomaticPolicySelectedCpu => {
                formatter.write_str("the measured automatic policy selected CPU")
            }
            Self::Unhealthy(reason) => write!(formatter, "CUDA is unhealthy: {reason}"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CudaAvailability {
    Disabled,
    SupportNotCompiled,
    Available,
    Degraded(String),
    Unavailable(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutorSelectionReason {
    CpuOnlyPolicy,
    PreferredCudaEligible,
    RequiredCudaEligible,
    AutomaticPolicySelectedCpu,
    CpuFallback,
}

#[derive(Clone, Debug)]
pub struct CudaRequestDiagnostics {
    pub algorithm: &'static str,
    pub delta: Option<f64>,
    pub device_ordinal: usize,
    pub device_name: String,
    pub queue_duration: Duration,
    pub batch_collection_duration: Duration,
    pub batch_width: usize,
    pub lane_index: usize,
    pub topology_bytes: usize,
    pub search_bytes: usize,
    pub host_to_device_bytes: usize,
    pub device_to_host_bytes: usize,
    pub kernel_launches: u64,
    pub synchronized_execution_duration: Duration,
    pub examined_edges: u64,
    pub relaxation_attempts: u64,
    pub relaxation_updates: u64,
    pub phases: u64,
    pub frontier_high_water: u32,
}
