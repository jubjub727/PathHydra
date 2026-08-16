#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct KernelParameters {
    pub node_count: u32,
    pub relation_count: u32,
    pub adjacency_count: u64,
    pub destination_count: u32,
    pub lane: u32,
    pub generation: u32,
    pub reserved: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct KernelDiagnostics {
    pub examined_edges: u64,
    pub relaxation_attempts: u64,
    pub relaxation_updates: u64,
    pub phases: u64,
    pub frontier_high_water: u32,
    pub status: u32,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum KernelStatus {
    #[default]
    Success = 0,
    Cancelled = 1,
    InvalidIndex = 2,
    InvalidArithmetic = 3,
    CounterOverflow = 4,
    FrontierOverflow = 5,
    BucketUnrepresentable = 6,
}

const _: [(); 32] = [(); core::mem::size_of::<KernelParameters>()];
const _: [(); 40] = [(); core::mem::size_of::<KernelDiagnostics>()];
const _: [(); 8] = [(); core::mem::align_of::<KernelParameters>()];
const _: [(); 8] = [(); core::mem::align_of::<KernelDiagnostics>()];
