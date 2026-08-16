use super::{
    BundleError,
    codec::{Decoder, put_string, put_u32, put_u64},
    layout::PartitionDescriptor,
};
use crate::{NUMERIC_POLICY_ID, TIE_POLICY_ID};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileDescriptor {
    pub length: u64,
    pub checksum: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BundleManifest {
    pub numeric_policy: Box<str>,
    pub tie_policy: Box<str>,
    pub node_count: u64,
    pub relation_kind_count: u64,
    pub adjacency_count: u64,
    pub segment_count: u64,
    pub target_partition_bytes: u64,
    pub hard_maximum_partition_bytes: u64,
    pub identities: FileDescriptor,
    pub source_directory: FileDescriptor,
    pub topology: FileDescriptor,
    pub evidence: FileDescriptor,
    pub partitions: Vec<PartitionDescriptor>,
}

impl BundleManifest {
    pub fn encode(&self) -> Result<Vec<u8>, BundleError> {
        let mut out = Vec::new();
        put_string(&mut out, &self.numeric_policy)?;
        put_string(&mut out, &self.tie_policy)?;
        for value in [
            self.node_count,
            self.relation_kind_count,
            self.adjacency_count,
            self.segment_count,
            self.target_partition_bytes,
            self.hard_maximum_partition_bytes,
        ] {
            put_u64(&mut out, value);
        }
        for width in [8_u32, 8, 8, 4, 4, 4, 8] {
            put_u32(&mut out, width);
        }
        for file in [
            &self.identities,
            &self.source_directory,
            &self.topology,
            &self.evidence,
        ] {
            put_u64(&mut out, file.length);
            out.extend_from_slice(&file.checksum);
        }
        put_u64(
            &mut out,
            u64::try_from(self.partitions.len())
                .map_err(|_| BundleError::Invalid("partition count overflow".into()))?,
        );
        for p in &self.partitions {
            put_u32(&mut out, p.id);
            put_u64(&mut out, p.topology_offset);
            put_u64(&mut out, p.topology_length);
            out.extend_from_slice(&p.topology_checksum);
            put_u64(&mut out, p.evidence_offset);
            put_u64(&mut out, p.evidence_length);
            out.extend_from_slice(&p.evidence_checksum);
            put_u32(&mut out, p.segment_count);
            put_u64(&mut out, p.edge_count);
        }
        Ok(out)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, BundleError> {
        let mut d = Decoder::new(bytes);
        let numeric_policy = d.string()?;
        let tie_policy = d.string()?;
        if &*numeric_policy != NUMERIC_POLICY_ID || &*tie_policy != TIE_POLICY_ID {
            return Err(BundleError::Invalid("unknown numeric or tie policy".into()));
        }
        let node_count = d.u64()?;
        let relation_kind_count = d.u64()?;
        let adjacency_count = d.u64()?;
        let segment_count = d.u64()?;
        let target_partition_bytes = d.u64()?;
        let hard_maximum_partition_bytes = d.u64()?;
        let expected = [8_u32, 8, 8, 4, 4, 4, 8];
        for width in expected {
            if d.u32()? != width {
                return Err(BundleError::Invalid("unsupported element width".into()));
            }
        }
        let mut file = || -> Result<FileDescriptor, BundleError> {
            Ok(FileDescriptor {
                length: d.u64()?,
                checksum: d.checksum()?,
            })
        };
        let identities = file()?;
        let source_directory = file()?;
        let topology = file()?;
        let evidence = file()?;
        let count = usize::try_from(d.u64()?)
            .map_err(|_| BundleError::Invalid("partition count does not fit platform".into()))?;
        let mut partitions = Vec::new();
        partitions
            .try_reserve_exact(count)
            .map_err(|_| BundleError::Invalid("partition allocation failed".into()))?;
        for expected_id in 0..count {
            let id = d.u32()?;
            if usize::try_from(id).ok() != Some(expected_id) {
                return Err(BundleError::Invalid("partition IDs are not dense".into()));
            }
            partitions.push(PartitionDescriptor {
                id,
                topology_offset: d.u64()?,
                topology_length: d.u64()?,
                topology_checksum: d.checksum()?,
                evidence_offset: d.u64()?,
                evidence_length: d.u64()?,
                evidence_checksum: d.checksum()?,
                segment_count: d.u32()?,
                edge_count: d.u64()?,
            });
        }
        d.finish()?;
        Ok(Self {
            numeric_policy,
            tie_policy,
            node_count,
            relation_kind_count,
            adjacency_count,
            segment_count,
            target_partition_bytes,
            hard_maximum_partition_bytes,
            identities,
            source_directory,
            topology,
            evidence,
            partitions,
        })
    }
}
