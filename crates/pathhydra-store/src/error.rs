use std::{error::Error, fmt};

use pathhydra_core::{NodeId, RelationId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecordKind {
    Candidate,
    Node,
    Relation,
}

impl fmt::Display for RecordKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Candidate => formatter.write_str("candidate"),
            Self::Node => formatter.write_str("node"),
            Self::Relation => formatter.write_str("relation"),
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
            Self::Relation(id) => write!(formatter, "relation {id}"),
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
    CorruptRecord {
        key_space: &'static str,
        record_id: String,
        reason: String,
    },
    IncompatibleFormat {
        key_space: &'static str,
        record_id: String,
        found: u8,
        supported: u8,
    },
    CounterOverflow {
        counter: &'static str,
    },
    NameTooLong {
        byte_length: usize,
    },
    LockPoisoned {
        lock: &'static str,
    },
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
            Self::CorruptRecord {
                key_space,
                record_id,
                reason,
            } => write!(
                formatter,
                "corrupt {key_space} record {record_id}: {reason}"
            ),
            Self::IncompatibleFormat {
                key_space,
                record_id,
                found,
                supported,
            } => write!(
                formatter,
                "incompatible format version {found} in {key_space} record {record_id}; supported version is {supported}"
            ),
            Self::CounterOverflow { counter } => write!(formatter, "{counter} counter overflow"),
            Self::NameTooLong { byte_length } => write!(
                formatter,
                "name is {byte_length} bytes; the durable format supports at most {} bytes",
                u32::MAX
            ),
            Self::LockPoisoned { lock } => write!(formatter, "{lock} lock is poisoned"),
            Self::RocksDb(error) => write!(formatter, "RocksDB failure: {error}"),
        }
    }
}

impl Error for CatalogError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::RocksDb(error) => Some(error),
            _ => None,
        }
    }
}

impl From<rocksdb::Error> for CatalogError {
    fn from(value: rocksdb::Error) -> Self {
        Self::RocksDb(value)
    }
}
