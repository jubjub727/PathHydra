use pathhydra_core::{
    Candidate, CandidateId, NodeId, NodeName, NodeRecord, RelationId, RelationName, RelationRecord,
};

pub(crate) const FORMAT_VERSION: u8 = 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CodecError {
    Truncated,
    TrailingGarbage,
    UnknownVersion(u8),
    InvalidLength(u32),
    InvalidUtf8,
    InvalidCandidateKind(u8),
    IdMismatch { expected: u64, found: u64 },
    NameTooLong(usize),
}

impl std::fmt::Display for CodecError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Truncated => formatter.write_str("truncated input"),
            Self::TrailingGarbage => formatter.write_str("trailing garbage"),
            Self::UnknownVersion(version) => write!(formatter, "unknown format version {version}"),
            Self::InvalidLength(length) => write!(formatter, "invalid string byte length {length}"),
            Self::InvalidUtf8 => formatter.write_str("invalid UTF-8"),
            Self::InvalidCandidateKind(kind) => write!(formatter, "unknown candidate kind {kind}"),
            Self::IdMismatch { expected, found } => {
                write!(
                    formatter,
                    "record ID {found} does not match key ID {expected}"
                )
            }
            Self::NameTooLong(length) => write!(formatter, "name is too long: {length} bytes"),
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

    fn string(&mut self) -> Result<Box<str>, CodecError> {
        let length = self.u32()?;
        let length = usize::try_from(length).map_err(|_| CodecError::InvalidLength(length))?;
        let bytes = self.take(length)?;
        let value = std::str::from_utf8(bytes).map_err(|_| CodecError::InvalidUtf8)?;
        Ok(value.into())
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

    fn version(&mut self) -> Result<(), CodecError> {
        let found = self.byte()?;
        if found == FORMAT_VERSION {
            Ok(())
        } else {
            Err(CodecError::UnknownVersion(found))
        }
    }

    fn finish(self) -> Result<(), CodecError> {
        if self.position == self.bytes.len() {
            Ok(())
        } else {
            Err(CodecError::TrailingGarbage)
        }
    }
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

pub(crate) fn encode_format_version() -> [u8; 1] {
    [FORMAT_VERSION]
}

pub(crate) fn decode_format_version(bytes: &[u8]) -> Result<(), CodecError> {
    let mut reader = Reader::new(bytes);
    reader.version()?;
    reader.finish()
}

pub(crate) fn encode_u64_record(value: u64) -> [u8; 9] {
    let mut output = [0; 9];
    output[0] = FORMAT_VERSION;
    output[1..].copy_from_slice(&value.to_be_bytes());
    output
}

pub(crate) fn decode_u64_record(bytes: &[u8]) -> Result<u64, CodecError> {
    let mut reader = Reader::new(bytes);
    reader.version()?;
    let value = reader.u64()?;
    reader.finish()?;
    Ok(value)
}

pub(crate) fn encode_candidate(candidate: &Candidate) -> Result<Vec<u8>, CodecError> {
    let mut output = Vec::new();
    output.push(FORMAT_VERSION);
    match candidate {
        Candidate::Node { id, name } => {
            output.push(1);
            output.extend_from_slice(&id.as_u64().to_be_bytes());
            push_string(&mut output, name.as_str())?;
        }
        Candidate::Relation { id, name } => {
            output.push(2);
            output.extend_from_slice(&id.as_u64().to_be_bytes());
            push_string(&mut output, name.as_str())?;
        }
    }
    Ok(output)
}

pub(crate) fn decode_candidate(bytes: &[u8], expected_id: u64) -> Result<Candidate, CodecError> {
    let mut reader = Reader::new(bytes);
    reader.version()?;
    let kind = reader.byte()?;
    let id = reader.u64()?;
    if id != expected_id {
        return Err(CodecError::IdMismatch {
            expected: expected_id,
            found: id,
        });
    }
    let name = reader.string()?;
    reader.finish()?;
    let id = CandidateId::from_u64(id);
    match kind {
        1 => Ok(Candidate::Node {
            id,
            name: NodeName::new(name),
        }),
        2 => Ok(Candidate::Relation {
            id,
            name: RelationName::new(name),
        }),
        other => Err(CodecError::InvalidCandidateKind(other)),
    }
}

pub(crate) fn encode_node(record: &NodeRecord) -> Result<Vec<u8>, CodecError> {
    encode_confirmed(record.id().as_u64(), record.name().as_str())
}

pub(crate) fn decode_node(bytes: &[u8], expected_id: u64) -> Result<NodeRecord, CodecError> {
    let (id, name) = decode_confirmed(bytes, expected_id)?;
    Ok(NodeRecord::new(NodeId::from_u64(id), NodeName::new(name)))
}

pub(crate) fn encode_relation(record: &RelationRecord) -> Result<Vec<u8>, CodecError> {
    encode_confirmed(record.id().as_u64(), record.name().as_str())
}

pub(crate) fn decode_relation(
    bytes: &[u8],
    expected_id: u64,
) -> Result<RelationRecord, CodecError> {
    let (id, name) = decode_confirmed(bytes, expected_id)?;
    Ok(RelationRecord::new(
        RelationId::from_u64(id),
        RelationName::new(name),
    ))
}

fn encode_confirmed(id: u64, name: &str) -> Result<Vec<u8>, CodecError> {
    let mut output = Vec::new();
    output.push(FORMAT_VERSION);
    output.extend_from_slice(&id.to_be_bytes());
    push_string(&mut output, name)?;
    Ok(output)
}

fn decode_confirmed(bytes: &[u8], expected_id: u64) -> Result<(u64, Box<str>), CodecError> {
    let mut reader = Reader::new(bytes);
    reader.version()?;
    let id = reader.u64()?;
    if id != expected_id {
        return Err(CodecError::IdMismatch {
            expected: expected_id,
            found: id,
        });
    }
    let name = reader.string()?;
    reader.finish()?;
    Ok((id, name))
}

#[cfg(test)]
mod tests {
    use pathhydra_core::{
        Candidate, CandidateId, NodeId, NodeName, NodeRecord, RelationId, RelationName,
        RelationRecord,
    };

    use super::*;

    #[test]
    fn every_codec_round_trips() {
        let node_candidate = Candidate::Node {
            id: CandidateId::from_u64(7),
            name: NodeName::new(" toke\u{301}n "),
        };
        let relation_candidate = Candidate::Relation {
            id: CandidateId::from_u64(8),
            name: RelationName::new("TOKEN!"),
        };
        let node = NodeRecord::new(NodeId::from_u64(11), NodeName::new("tokén"));
        let relation =
            RelationRecord::new(RelationId::from_u64(12), RelationName::new("contains "));

        assert_eq!(decode_id_key(&encode_id_key(42)), Ok(42));
        assert_eq!(
            decode_name_key(&encode_name_key("Exact").unwrap()).as_deref(),
            Ok("Exact")
        );
        assert_eq!(decode_format_version(&encode_format_version()), Ok(()));
        assert_eq!(decode_u64_record(&encode_u64_record(91)), Ok(91));
        assert_eq!(
            decode_candidate(&encode_candidate(&node_candidate).unwrap(), 7),
            Ok(node_candidate)
        );
        assert_eq!(
            decode_candidate(&encode_candidate(&relation_candidate).unwrap(), 8),
            Ok(relation_candidate)
        );
        assert_eq!(decode_node(&encode_node(&node).unwrap(), 11), Ok(node));
        assert_eq!(
            decode_relation(&encode_relation(&relation).unwrap(), 12),
            Ok(relation)
        );
    }

    #[test]
    fn malformed_codecs_reject_truncation_trailing_bytes_versions_and_lengths() {
        assert_eq!(decode_id_key(&[0; 7]), Err(CodecError::Truncated));
        assert_eq!(decode_id_key(&[0; 9]), Err(CodecError::TrailingGarbage));
        assert_eq!(
            decode_format_version(&[2]),
            Err(CodecError::UnknownVersion(2))
        );
        assert_eq!(
            decode_u64_record(&[FORMAT_VERSION; 8]),
            Err(CodecError::Truncated)
        );
        assert_eq!(
            decode_name_key(&[0, 0, 0, 2, b'a']),
            Err(CodecError::Truncated)
        );
        assert_eq!(
            decode_name_key(&[0, 0, 0, 1, 0xff]),
            Err(CodecError::InvalidUtf8)
        );

        let mut candidate = encode_candidate(&Candidate::Node {
            id: CandidateId::from_u64(3),
            name: NodeName::new("x"),
        })
        .unwrap();
        candidate.push(0);
        assert_eq!(
            decode_candidate(&candidate, 3),
            Err(CodecError::TrailingGarbage)
        );

        let mut bad_kind = candidate[..candidate.len() - 1].to_vec();
        bad_kind[1] = 9;
        assert_eq!(
            decode_candidate(&bad_kind, 3),
            Err(CodecError::InvalidCandidateKind(9))
        );
        assert_eq!(
            decode_candidate(&bad_kind, 4),
            Err(CodecError::IdMismatch {
                expected: 4,
                found: 3
            })
        );

        let node = NodeRecord::new(NodeId::from_u64(1), NodeName::new("x"));
        let mut encoded_node = encode_node(&node).unwrap();
        encoded_node[0] = 99;
        assert_eq!(
            decode_node(&encoded_node, 1),
            Err(CodecError::UnknownVersion(99))
        );

        let relation = RelationRecord::new(RelationId::from_u64(1), RelationName::new("x"));
        let mut encoded_relation = encode_relation(&relation).unwrap();
        encoded_relation.truncate(5);
        assert_eq!(
            decode_relation(&encoded_relation, 1),
            Err(CodecError::Truncated)
        );
    }
}
