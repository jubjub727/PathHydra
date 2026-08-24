use pathhydra_core::{
    BaseWeight, Candidate, CandidateId, CandidateNodeReference, CandidateRelationReference, EdgeId,
    EdgeRecord, MAX_NODE_PAYLOAD_BYTES, NodeId, NodeName, NodePayload, NodeRecord, RelationId,
    RelationName, RelationRecord,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CodecError {
    Truncated,
    TrailingGarbage,
    InvalidLength(u32),
    InvalidUtf8,
    InvalidCandidateKind(u8),
    InvalidCandidateReferenceKind(u8),
    InvalidBaseWeight(u32),
    IdMismatch { expected: u64, found: u64 },
    NameTooLong(usize),
    PayloadTooLong(usize),
}

impl std::fmt::Display for CodecError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Truncated => formatter.write_str("truncated input"),
            Self::TrailingGarbage => formatter.write_str("trailing garbage"),
            Self::InvalidLength(length) => write!(formatter, "invalid byte length {length}"),
            Self::InvalidUtf8 => formatter.write_str("invalid UTF-8"),
            Self::InvalidCandidateKind(kind) => write!(formatter, "unknown candidate kind {kind}"),
            Self::InvalidCandidateReferenceKind(kind) => {
                write!(formatter, "unknown candidate reference kind {kind}")
            }
            Self::InvalidBaseWeight(bits) => {
                write!(formatter, "invalid base-weight bits 0x{bits:08x}")
            }
            Self::IdMismatch { expected, found } => {
                write!(
                    formatter,
                    "record ID {found} does not match key ID {expected}"
                )
            }
            Self::NameTooLong(length) => write!(formatter, "name is too long: {length} bytes"),
            Self::PayloadTooLong(length) => {
                write!(formatter, "node payload is too long: {length} bytes")
            }
        }
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn byte(&mut self) -> Result<u8, CodecError> {
        let value = self
            .bytes
            .get(self.position)
            .copied()
            .ok_or(CodecError::Truncated)?;
        self.position += 1;
        Ok(value)
    }

    fn u32(&mut self) -> Result<u32, CodecError> {
        let bytes = self.take(4)?;
        let mut value = [0; 4];
        value.copy_from_slice(bytes);
        Ok(u32::from_be_bytes(value))
    }

    fn u64(&mut self) -> Result<u64, CodecError> {
        let bytes = self.take(8)?;
        let mut value = [0; 8];
        value.copy_from_slice(bytes);
        Ok(u64::from_be_bytes(value))
    }

    fn bytes(&mut self) -> Result<Box<[u8]>, CodecError> {
        let length = self.u32()?;
        let length = usize::try_from(length).map_err(|_| CodecError::InvalidLength(length))?;
        Ok(self.take(length)?.into())
    }

    fn payload(&mut self) -> Result<NodePayload, CodecError> {
        let bytes = self.bytes()?;
        if bytes.len() > MAX_NODE_PAYLOAD_BYTES {
            return Err(CodecError::PayloadTooLong(bytes.len()));
        }
        Ok(NodePayload::new(bytes))
    }

    fn string(&mut self) -> Result<Box<str>, CodecError> {
        let bytes = self.bytes()?;
        let value = std::str::from_utf8(&bytes).map_err(|_| CodecError::InvalidUtf8)?;
        Ok(value.into())
    }

    fn weight(&mut self) -> Result<BaseWeight, CodecError> {
        let bits = self.u32()?;
        BaseWeight::from_bits(bits).map_err(|_| CodecError::InvalidBaseWeight(bits))
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], CodecError> {
        let end = self
            .position
            .checked_add(length)
            .ok_or(CodecError::Truncated)?;
        let value = self
            .bytes
            .get(self.position..end)
            .ok_or(CodecError::Truncated)?;
        self.position = end;
        Ok(value)
    }

    fn finish(self) -> Result<(), CodecError> {
        if self.position == self.bytes.len() {
            Ok(())
        } else {
            Err(CodecError::TrailingGarbage)
        }
    }
}

fn push_bytes(output: &mut Vec<u8>, value: &[u8]) -> Result<(), CodecError> {
    if value.len() > MAX_NODE_PAYLOAD_BYTES {
        return Err(CodecError::PayloadTooLong(value.len()));
    }
    let length = u32::try_from(value.len()).map_err(|_| CodecError::PayloadTooLong(value.len()))?;
    output.extend_from_slice(&length.to_be_bytes());
    output.extend_from_slice(value);
    Ok(())
}

fn push_string(output: &mut Vec<u8>, value: &str) -> Result<(), CodecError> {
    let length = u32::try_from(value.len()).map_err(|_| CodecError::NameTooLong(value.len()))?;
    output.extend_from_slice(&length.to_be_bytes());
    output.extend_from_slice(value.as_bytes());
    Ok(())
}

pub(crate) fn encode_id_key(id: u64) -> [u8; 8] {
    id.to_be_bytes()
}

pub(crate) fn decode_id_key(bytes: &[u8]) -> Result<u64, CodecError> {
    let mut reader = Reader::new(bytes);
    let value = reader.u64()?;
    reader.finish()?;
    Ok(value)
}

pub(crate) fn encode_adjacency_key(node: NodeId, edge: EdgeId) -> [u8; 16] {
    let mut output = [0; 16];
    output[..8].copy_from_slice(&node.as_u64().to_be_bytes());
    output[8..].copy_from_slice(&edge.as_u64().to_be_bytes());
    output
}

pub(crate) fn decode_adjacency_key(bytes: &[u8]) -> Result<(NodeId, EdgeId), CodecError> {
    let mut reader = Reader::new(bytes);
    let node = NodeId::from_u64(reader.u64()?);
    let edge = EdgeId::from_u64(reader.u64()?);
    reader.finish()?;
    Ok((node, edge))
}

pub(crate) fn encode_adjacency_value(edge: EdgeId) -> [u8; 8] {
    encode_u64_record(edge.as_u64())
}

pub(crate) fn decode_adjacency_value(
    bytes: &[u8],
    expected_edge: EdgeId,
) -> Result<(), CodecError> {
    let found = decode_u64_record(bytes)?;
    if found == expected_edge.as_u64() {
        Ok(())
    } else {
        Err(CodecError::IdMismatch {
            expected: expected_edge.as_u64(),
            found,
        })
    }
}

pub(crate) fn encode_name_key(name: &str) -> Result<Vec<u8>, CodecError> {
    let mut output = Vec::with_capacity(4 + name.len());
    push_string(&mut output, name)?;
    Ok(output)
}

pub(crate) fn decode_name_key(bytes: &[u8]) -> Result<Box<str>, CodecError> {
    let mut reader = Reader::new(bytes);
    let name = reader.string()?;
    reader.finish()?;
    Ok(name)
}

pub(crate) fn encode_u64_record(value: u64) -> [u8; 8] {
    value.to_be_bytes()
}

pub(crate) fn decode_u64_record(bytes: &[u8]) -> Result<u64, CodecError> {
    decode_id_key(bytes)
}

pub(crate) fn encode_candidate(candidate: &Candidate) -> Result<Vec<u8>, CodecError> {
    let mut output = Vec::new();
    match candidate {
        Candidate::Node {
            id,
            name,
            payload,
            incoming_reference_count,
        } => {
            output.push(1);
            output.extend_from_slice(&id.as_u64().to_be_bytes());
            push_string(&mut output, name.as_str())?;
            push_bytes(&mut output, payload.as_bytes())?;
            output.extend_from_slice(&incoming_reference_count.to_be_bytes());
        }
        Candidate::Relation {
            id,
            name,
            incoming_reference_count,
        } => {
            output.push(2);
            output.extend_from_slice(&id.as_u64().to_be_bytes());
            push_string(&mut output, name.as_str())?;
            output.extend_from_slice(&incoming_reference_count.to_be_bytes());
        }
        Candidate::Edge {
            id,
            source,
            destination,
            relation_kind,
            base_weight,
        } => {
            output.push(3);
            output.extend_from_slice(&id.as_u64().to_be_bytes());
            encode_node_reference(&mut output, *source);
            encode_node_reference(&mut output, *destination);
            encode_relation_reference(&mut output, *relation_kind);
            output.extend_from_slice(&base_weight.to_bits().to_be_bytes());
        }
    }
    Ok(output)
}

pub(crate) fn decode_candidate(bytes: &[u8], expected_id: u64) -> Result<Candidate, CodecError> {
    let mut reader = Reader::new(bytes);
    let kind = reader.byte()?;
    let id = checked_id(&mut reader, expected_id)?;
    let id = CandidateId::from_u64(id);
    let candidate = match kind {
        1 => Candidate::Node {
            id,
            name: NodeName::new(reader.string()?),
            payload: reader.payload()?,
            incoming_reference_count: reader.u64()?,
        },
        2 => Candidate::Relation {
            id,
            name: RelationName::new(reader.string()?),
            incoming_reference_count: reader.u64()?,
        },
        3 => Candidate::Edge {
            id,
            source: decode_node_reference(&mut reader)?,
            destination: decode_node_reference(&mut reader)?,
            relation_kind: decode_relation_reference(&mut reader)?,
            base_weight: reader.weight()?,
        },
        other => return Err(CodecError::InvalidCandidateKind(other)),
    };
    reader.finish()?;
    Ok(candidate)
}

pub(crate) fn encode_node(record: &NodeRecord) -> Result<Vec<u8>, CodecError> {
    let mut output = Vec::new();
    output.extend_from_slice(&record.id().as_u64().to_be_bytes());
    push_string(&mut output, record.name().as_str())?;
    push_bytes(&mut output, record.payload().as_bytes())?;
    Ok(output)
}

pub(crate) fn decode_node(bytes: &[u8], expected_id: u64) -> Result<NodeRecord, CodecError> {
    let mut reader = Reader::new(bytes);
    let id = checked_id(&mut reader, expected_id)?;
    let name = NodeName::new(reader.string()?);
    let payload = reader.payload()?;
    reader.finish()?;
    Ok(NodeRecord::new(NodeId::from_u64(id), name, payload))
}

pub(crate) fn encode_relation(record: &RelationRecord) -> Result<Vec<u8>, CodecError> {
    let mut output = encode_named_record(record.id().as_u64(), record.name().as_str())?;
    output.extend_from_slice(&record.provisional_reference_count().to_be_bytes());
    output.extend_from_slice(&record.confirmed_edge_count().to_be_bytes());
    Ok(output)
}

pub(crate) fn decode_relation(
    bytes: &[u8],
    expected_id: u64,
) -> Result<RelationRecord, CodecError> {
    let mut reader = Reader::new(bytes);
    let id = checked_id(&mut reader, expected_id)?;
    let name = reader.string()?;
    let provisional_reference_count = reader.u64()?;
    let confirmed_edge_count = reader.u64()?;
    reader.finish()?;
    provisional_reference_count
        .checked_add(confirmed_edge_count)
        .ok_or(CodecError::InvalidLength(u32::MAX))?;
    Ok(RelationRecord::with_usage(
        RelationId::from_u64(id),
        RelationName::new(name),
        provisional_reference_count,
        confirmed_edge_count,
    ))
}

fn encode_node_reference(output: &mut Vec<u8>, reference: CandidateNodeReference) {
    match reference {
        CandidateNodeReference::Confirmed(id) => {
            output.push(1);
            output.extend_from_slice(&id.as_u64().to_be_bytes());
        }
        CandidateNodeReference::Candidate(id) => {
            output.push(2);
            output.extend_from_slice(&id.as_u64().to_be_bytes());
        }
    }
}

fn decode_node_reference(reader: &mut Reader<'_>) -> Result<CandidateNodeReference, CodecError> {
    match reader.byte()? {
        1 => Ok(CandidateNodeReference::Confirmed(NodeId::from_u64(
            reader.u64()?,
        ))),
        2 => Ok(CandidateNodeReference::Candidate(CandidateId::from_u64(
            reader.u64()?,
        ))),
        other => Err(CodecError::InvalidCandidateReferenceKind(other)),
    }
}

fn encode_relation_reference(output: &mut Vec<u8>, reference: CandidateRelationReference) {
    match reference {
        CandidateRelationReference::Confirmed(id) => {
            output.push(1);
            output.extend_from_slice(&id.as_u64().to_be_bytes());
        }
        CandidateRelationReference::Candidate(id) => {
            output.push(2);
            output.extend_from_slice(&id.as_u64().to_be_bytes());
        }
    }
}

fn decode_relation_reference(
    reader: &mut Reader<'_>,
) -> Result<CandidateRelationReference, CodecError> {
    match reader.byte()? {
        1 => Ok(CandidateRelationReference::Confirmed(RelationId::from_u64(
            reader.u64()?,
        ))),
        2 => Ok(CandidateRelationReference::Candidate(
            CandidateId::from_u64(reader.u64()?),
        )),
        other => Err(CodecError::InvalidCandidateReferenceKind(other)),
    }
}

pub(crate) fn encode_popularity_key(record: &RelationRecord) -> Result<[u8; 24], CodecError> {
    let total = record
        .total_reference_count()
        .ok_or(CodecError::InvalidLength(u32::MAX))?;
    let mut output = [0; 24];
    output[..8].copy_from_slice(&(!total).to_be_bytes());
    output[8..16].copy_from_slice(&(!record.confirmed_edge_count()).to_be_bytes());
    output[16..].copy_from_slice(&record.id().as_u64().to_be_bytes());
    Ok(output)
}

pub(crate) fn decode_popularity_key(bytes: &[u8]) -> Result<(u64, u64, RelationId), CodecError> {
    let mut reader = Reader::new(bytes);
    let total = !reader.u64()?;
    let confirmed = !reader.u64()?;
    let id = RelationId::from_u64(reader.u64()?);
    reader.finish()?;
    Ok((total, confirmed, id))
}

fn encode_named_record(id: u64, name: &str) -> Result<Vec<u8>, CodecError> {
    let mut output = Vec::new();
    output.extend_from_slice(&id.to_be_bytes());
    push_string(&mut output, name)?;
    Ok(output)
}

pub(crate) fn encode_edge(record: &EdgeRecord) -> Vec<u8> {
    let mut output = Vec::with_capacity(36);
    output.extend_from_slice(&record.id().as_u64().to_be_bytes());
    output.extend_from_slice(&record.source().as_u64().to_be_bytes());
    output.extend_from_slice(&record.destination().as_u64().to_be_bytes());
    output.extend_from_slice(&record.relation_kind().as_u64().to_be_bytes());
    output.extend_from_slice(&record.base_weight().to_bits().to_be_bytes());
    output
}

pub(crate) fn decode_edge(bytes: &[u8], expected_id: u64) -> Result<EdgeRecord, CodecError> {
    let mut reader = Reader::new(bytes);
    let id = checked_id(&mut reader, expected_id)?;
    let source = NodeId::from_u64(reader.u64()?);
    let destination = NodeId::from_u64(reader.u64()?);
    let relation_kind = RelationId::from_u64(reader.u64()?);
    let base_weight = reader.weight()?;
    reader.finish()?;
    Ok(EdgeRecord::new(
        EdgeId::from_u64(id),
        source,
        destination,
        relation_kind,
        base_weight,
    ))
}

fn checked_id(reader: &mut Reader<'_>, expected: u64) -> Result<u64, CodecError> {
    let found = reader.u64()?;
    if found == expected {
        Ok(found)
    } else {
        Err(CodecError::IdMismatch { expected, found })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_codec_round_trips() {
        let node_candidate = Candidate::Node {
            id: CandidateId::from_u64(7),
            name: NodeName::new(" toke\u{301}n "),
            payload: NodePayload::from([0, 0xff]),
            incoming_reference_count: 0,
        };
        let edge_candidate = Candidate::Edge {
            id: CandidateId::from_u64(8),
            source: CandidateNodeReference::Confirmed(NodeId::from_u64(2)),
            destination: CandidateNodeReference::Confirmed(NodeId::from_u64(3)),
            relation_kind: CandidateRelationReference::Confirmed(RelationId::from_u64(4)),
            base_weight: BaseWeight::new(0.25).unwrap(),
        };
        let node = NodeRecord::new(
            NodeId::from_u64(11),
            NodeName::new("tokén"),
            NodePayload::from([0xff]),
        );
        let relation =
            RelationRecord::new(RelationId::from_u64(12), RelationName::new("contains "));
        let edge = EdgeRecord::new(
            EdgeId::from_u64(13),
            node.id(),
            NodeId::from_u64(14),
            relation.id(),
            BaseWeight::MAX,
        );

        assert_eq!(decode_id_key(&encode_id_key(42)), Ok(42));
        assert_eq!(
            decode_name_key(&encode_name_key("Exact").unwrap()).as_deref(),
            Ok("Exact")
        );
        assert_eq!(decode_u64_record(&encode_u64_record(91)), Ok(91));
        assert_eq!(
            decode_candidate(&encode_candidate(&node_candidate).unwrap(), 7),
            Ok(node_candidate)
        );
        assert_eq!(
            decode_candidate(&encode_candidate(&edge_candidate).unwrap(), 8),
            Ok(edge_candidate)
        );
        assert_eq!(decode_node(&encode_node(&node).unwrap(), 11), Ok(node));
        assert_eq!(
            decode_relation(&encode_relation(&relation).unwrap(), 12),
            Ok(relation)
        );
        assert_eq!(decode_edge(&encode_edge(&edge), 13), Ok(edge.clone()));
        let key = encode_adjacency_key(NodeId::from_u64(11), EdgeId::from_u64(13));
        assert_eq!(
            decode_adjacency_key(&key),
            Ok((NodeId::from_u64(11), EdgeId::from_u64(13)))
        );
        assert_eq!(
            decode_adjacency_value(&encode_adjacency_value(edge.id()), edge.id()),
            Ok(())
        );
    }

    #[test]
    fn malformed_values_are_rejected() {
        assert_eq!(decode_id_key(&[0; 7]), Err(CodecError::Truncated));
        assert_eq!(decode_id_key(&[0; 9]), Err(CodecError::TrailingGarbage));
        assert_eq!(decode_u64_record(&[0; 7]), Err(CodecError::Truncated));
        assert_eq!(
            decode_name_key(&[0, 0, 0, 1, 0xff]),
            Err(CodecError::InvalidUtf8)
        );

        let mut edge = encode_edge(&EdgeRecord::new(
            EdgeId::from_u64(1),
            NodeId::from_u64(1),
            NodeId::from_u64(2),
            RelationId::from_u64(1),
            BaseWeight::MIN,
        ));
        edge[32..36].copy_from_slice(&f32::NAN.to_bits().to_be_bytes());
        assert_eq!(
            decode_edge(&edge, 1),
            Err(CodecError::InvalidBaseWeight(f32::NAN.to_bits()))
        );
    }
}
