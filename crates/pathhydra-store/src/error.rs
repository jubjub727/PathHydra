use std::{error::Error, fmt};

use pathhydra_core::{CandidateId, InvalidBaseWeight, MAX_NODE_PAYLOAD_BYTES, NodeId, RelationId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecordKind {
    Candidate,
    Node,
    Relation,
    Edge,
}

impl fmt::Display for RecordKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Candidate => formatter.write_str("candidate"),
            Self::Node => formatter.write_str("node"),
            Self::Relation => formatter.write_str("relation kind"),
            Self::Edge => formatter.write_str("edge"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfirmedId {
    Node(NodeId),
    Relation(RelationId),
}

impl fmt::Display for ConfirmedId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Node(id) => write!(formatter, "node {id}"),
            Self::Relation(id) => write!(formatter, "relation kind {id}"),
        }
    }
}

#[derive(Debug)]
pub enum CatalogError {
    NotFound {
        kind: RecordKind,
        id: u64,
    },
    NameAlreadyConfirmed {
        name: Box<str>,
        existing_id: ConfirmedId,
    },
    MissingEdgeEndpoint {
        endpoint: EdgeEndpoint,
        node_id: NodeId,
    },
    MissingEdgeRelationKind {
        relation_kind_id: RelationId,
    },
    InvalidBatch {
        reason: String,
    },
    InvalidCandidateDependency {
        candidate_id: CandidateId,
        reason: String,
    },
    CorruptRecord {
        key_space: &'static str,
        record_id: String,
        reason: String,
    },
    CounterOverflow {
        counter: &'static str,
    },
    NameTooLong {
        byte_length: usize,
    },
    PayloadTooLong {
        byte_length: usize,
    },
    InvalidBaseWeight(InvalidBaseWeight),
    LockPoisoned {
        lock: &'static str,
    },
    InvalidConfiguration {
        field: &'static str,
        reason: &'static str,
    },
    ValidationAborted,
    StorageExhausted {
        operation: &'static str,
    },
    Io(std::io::Error),
    RocksDb(rocksdb::Error),
}

impl fmt::Display for CatalogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { kind, id } => write!(formatter, "{kind} {id} was not found"),
            Self::NameAlreadyConfirmed { name, existing_id } => {
                write!(
                    formatter,
                    "exact name {name:?} is already confirmed as {existing_id}"
                )
            }
            Self::MissingEdgeEndpoint { endpoint, node_id } => {
                write!(formatter, "edge {endpoint} node {node_id} is not confirmed")
            }
            Self::MissingEdgeRelationKind { relation_kind_id } => write!(
                formatter,
                "edge relation kind {relation_kind_id} is not confirmed"
            ),
            Self::InvalidBatch { reason } => write!(formatter, "invalid candidate batch: {reason}"),
            Self::InvalidCandidateDependency {
                candidate_id,
                reason,
            } => write!(
                formatter,
                "invalid dependency for candidate {candidate_id}: {reason}"
            ),
            Self::CorruptRecord {
                key_space,
                record_id,
                reason,
            } => write!(
                formatter,
                "corrupt {key_space} record {record_id}: {reason}"
            ),
            Self::CounterOverflow { counter } => write!(formatter, "{counter} counter overflow"),
            Self::NameTooLong { byte_length } => write!(
                formatter,
                "name is {byte_length} bytes; the durable format supports at most {} bytes",
                u32::MAX
            ),
            Self::PayloadTooLong { byte_length } => write!(
                formatter,
                "node payload is {byte_length} bytes; the durable store supports at most {MAX_NODE_PAYLOAD_BYTES} bytes"
            ),
            Self::InvalidBaseWeight(error) => error.fmt(formatter),
            Self::LockPoisoned { lock } => write!(formatter, "{lock} lock is poisoned"),
            Self::InvalidConfiguration { field, reason } => {
                write!(formatter, "invalid store configuration {field}: {reason}")
            }
            Self::ValidationAborted => formatter.write_str("catalog validation was aborted"),
            Self::StorageExhausted { operation } => {
                write!(
                    formatter,
                    "storage capacity was exhausted during {operation}"
                )
            }
            Self::Io(error) => write!(formatter, "store filesystem failure: {error}"),
            Self::RocksDb(error) => write!(formatter, "RocksDB failure: {error}"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EdgeEndpoint {
    Source,
    Destination,
}

impl fmt::Display for EdgeEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source => formatter.write_str("source"),
            Self::Destination => formatter.write_str("destination"),
        }
    }
}

impl Error for CatalogError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::RocksDb(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::InvalidBaseWeight(error) => Some(error),
            _ => None,
        }
    }
}

impl From<InvalidBaseWeight> for CatalogError {
    fn from(value: InvalidBaseWeight) -> Self {
        Self::InvalidBaseWeight(value)
    }
}

impl From<rocksdb::Error> for CatalogError {
    fn from(value: rocksdb::Error) -> Self {
        rocksdb_error("catalog operation", value)
    }
}

pub(crate) fn rocksdb_error(operation: &'static str, error: rocksdb::Error) -> CatalogError {
    if error.kind() == rocksdb::ErrorKind::IOError && is_storage_exhausted_message(error.as_ref()) {
        CatalogError::StorageExhausted { operation }
    } else {
        CatalogError::RocksDb(error)
    }
}

pub(crate) fn is_storage_exhausted_message(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    [
        "no space left",
        "disk full",
        "storage full",
        "not enough space",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

impl From<std::io::Error> for CatalogError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

#[cfg(test)]
mod tests {
    use super::is_storage_exhausted_message;

    #[test]
    fn storage_capacity_messages_are_classified_without_misclassifying_other_io() {
        assert!(is_storage_exhausted_message(
            "IO error: No space left on device"
        ));
        assert!(is_storage_exhausted_message(
            "IO error: There is not enough space on the disk"
        ));
        assert!(!is_storage_exhausted_message("IO error: permission denied"));
    }
}
