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
