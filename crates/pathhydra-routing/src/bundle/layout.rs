#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SegmentDescriptor {
    pub source: u32,
    pub partition: u32,
    pub local_segment: u32,
    pub first_edge_ordinal: u64,
    pub edge_count: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PartitionDescriptor {
    pub id: u32,
    pub topology_offset: u64,
    pub topology_length: u64,
    pub topology_checksum: [u8; 32],
    pub evidence_offset: u64,
    pub evidence_length: u64,
    pub evidence_checksum: [u8; 32],
    pub segment_count: u32,
    pub edge_count: u64,
}

pub(crate) const SEGMENT_ENCODED_BYTES: u64 = 24;
pub(crate) const EDGE_TOPOLOGY_BYTES: u64 = 12;
pub(crate) const PARTITION_HEADER_BYTES: u64 = 12;
pub(crate) const MIN_NONEMPTY_PARTITION_BYTES: u64 =
    PARTITION_HEADER_BYTES + SEGMENT_ENCODED_BYTES + EDGE_TOPOLOGY_BYTES;
