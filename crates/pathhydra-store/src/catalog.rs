use std::{
    collections::HashMap,
    path::Path,
    sync::{Mutex, MutexGuard, RwLock, RwLockWriteGuard},
};

use pathhydra_core::{
    Candidate, CandidateId, ConfirmedRecord, NodeId, NodeName, NodeRecord, RelationId,
    RelationName, RelationRecord,
};
use rocksdb::{ColumnFamily, DB, IteratorMode, Options, WriteBatch};

use crate::{
    codec::{
        self, CodecError, decode_candidate, decode_format_version, decode_id_key, decode_name_key,
        decode_node, decode_relation, decode_u64_record, encode_candidate, encode_format_version,
        encode_id_key, encode_name_key, encode_node, encode_relation, encode_u64_record,
    },
    column_families,
    error::{CatalogError, ConfirmedId, RecordKind},
};

const META_FORMAT: &[u8] = b"storage-format";
const META_GRAPH_VERSION: &[u8] = b"graph-version";
const META_NEXT_CANDIDATE_ID: &[u8] = b"next-candidate-id";
const META_NEXT_NODE_ID: &[u8] = b"next-node-id";
const META_NEXT_RELATION_ID: &[u8] = b"next-relation-id";
const INITIAL_ID: u64 = 1;

type NodeNameIndex = HashMap<Box<str>, NodeId>;
type RelationNameIndex = HashMap<Box<str>, RelationId>;

/// A durable exact-name catalog.
///
/// Candidate insertion does not make a name visible. The caller must validate
/// it externally and then cross the explicit confirmation boundary:
///
/// ```no_run
/// use pathhydra_store::Catalog;
///
/// # fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let catalog = Catalog::open("pathhydra.db")?;
/// let candidate = catalog.insert_node_candidate("Exact Name")?;
/// assert_eq!(catalog.lookup_node_exact("Exact Name")?, None);
/// catalog.confirm_validated_candidate(candidate)?;
/// assert!(catalog.lookup_node_exact("Exact Name")?.is_some());
/// # Ok(())
/// # }
/// ```
pub struct Catalog {
    db: DB,
    write_mutex: Mutex<()>,
    node_names: RwLock<NodeNameIndex>,
    relation_names: RwLock<RelationNameIndex>,
}

impl Catalog {
    /// Opens a catalog and rebuilds its confirmed exact-name indexes.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, CatalogError> {
        let mut options = Options::default();
        options.create_if_missing(true);
        options.create_missing_column_families(true);

        let db = DB::open_cf_descriptors(&options, path, column_families::descriptors())?;
        initialize_metadata(&db)?;
        let (node_names, relation_names) = rebuild_indexes(&db)?;

        Ok(Self {
            db,
            write_mutex: Mutex::new(()),
            node_names: RwLock::new(node_names),
            relation_names: RwLock::new(relation_names),
        })
    }

    pub fn insert_node_candidate(
        &self,
        name: impl Into<NodeName>,
    ) -> Result<CandidateId, CatalogError> {
        self.insert_candidate(CandidateName::Node(name.into()))
    }

    pub fn insert_relation_candidate(
        &self,
        name: impl Into<RelationName>,
    ) -> Result<CandidateId, CatalogError> {
        self.insert_candidate(CandidateName::Relation(name.into()))
    }

    pub fn get_candidate(&self, id: CandidateId) -> Result<Candidate, CatalogError> {
        let cf = column_family(&self.db, column_families::CANDIDATES)?;
        let value =
            self.db
                .get_cf(cf, encode_id_key(id.as_u64()))?
                .ok_or(CatalogError::NotFound {
                    kind: RecordKind::Candidate,
                    id: id.as_u64(),
                })?;
        decode_candidate(&value, id.as_u64())
            .map_err(|error| record_error(column_families::CANDIDATES, id.to_string(), error))
    }

    /// Confirms a candidate that the caller has already validated externally.
    ///
    /// The confirmed record, name mapping, candidate removal, counters, and
    /// graph version are committed in one RocksDB write batch.
    pub fn confirm_validated_candidate(
        &self,
        id: CandidateId,
    ) -> Result<ConfirmedRecord, CatalogError> {
        let _write = self.write_guard()?;
        let candidate = self.get_candidate(id)?;

        match candidate {
            Candidate::Node { id, name } => self.confirm_node(id, name),
            Candidate::Relation { id, name } => self.confirm_relation(id, name),
        }
    }

    /// Resolves a confirmed node by complete, case-sensitive string equality.
    pub fn lookup_node_exact(&self, name: &str) -> Result<Option<NodeId>, CatalogError> {
        let names = self
            .node_names
            .read()
            .map_err(|_| CatalogError::LockPoisoned {
                lock: "node-name index",
            })?;
        Ok(names.get(name).copied())
    }

    /// Resolves a confirmed relation kind by complete, case-sensitive equality.
    pub fn lookup_relation_exact(&self, name: &str) -> Result<Option<RelationId>, CatalogError> {
        let names = self
            .relation_names
            .read()
            .map_err(|_| CatalogError::LockPoisoned {
                lock: "relation-name index",
            })?;
        Ok(names.get(name).copied())
    }

    pub fn get_node(&self, id: NodeId) -> Result<NodeRecord, CatalogError> {
        let cf = column_family(&self.db, column_families::NODES)?;
        let value =
            self.db
                .get_cf(cf, encode_id_key(id.as_u64()))?
                .ok_or(CatalogError::NotFound {
                    kind: RecordKind::Node,
                    id: id.as_u64(),
                })?;
        decode_node(&value, id.as_u64())
            .map_err(|error| record_error(column_families::NODES, id.to_string(), error))
    }

    pub fn get_relation(&self, id: RelationId) -> Result<RelationRecord, CatalogError> {
        let cf = column_family(&self.db, column_families::RELATION_KINDS)?;
        let value =
            self.db
                .get_cf(cf, encode_id_key(id.as_u64()))?
                .ok_or(CatalogError::NotFound {
                    kind: RecordKind::Relation,
                    id: id.as_u64(),
                })?;
        decode_relation(&value, id.as_u64())
            .map_err(|error| record_error(column_families::RELATION_KINDS, id.to_string(), error))
    }

    #[must_use = "errors reading durable metadata must be handled"]
    pub fn graph_version(&self) -> Result<u64, CatalogError> {
        read_metadata(&self.db, META_GRAPH_VERSION, "graph-version")
    }

    fn insert_candidate(&self, candidate_name: CandidateName) -> Result<CandidateId, CatalogError> {
        let _write = self.write_guard()?;
        let next_id = read_metadata(&self.db, META_NEXT_CANDIDATE_ID, "next-candidate-id")?;
        let following_id = next_id
            .checked_add(1)
            .ok_or(CatalogError::CounterOverflow {
                counter: "candidate ID",
            })?;
        let id = CandidateId::from_u64(next_id);
        let candidate = match candidate_name {
            CandidateName::Node(name) => Candidate::Node { id, name },
            CandidateName::Relation(name) => Candidate::Relation { id, name },
        };
        let encoded = encode_candidate(&candidate).map_err(codec_input_error)?;

        let candidates = column_family(&self.db, column_families::CANDIDATES)?;
        let mut batch = WriteBatch::default();
        batch.put_cf(candidates, encode_id_key(next_id), encoded);
        batch.put(META_NEXT_CANDIDATE_ID, encode_u64_record(following_id));
        self.db.write(batch)?;
        Ok(id)
    }

    fn confirm_node(
        &self,
        candidate_id: CandidateId,
        name: NodeName,
    ) -> Result<ConfirmedRecord, CatalogError> {
        let name_key = encode_name_key(name.as_str()).map_err(codec_input_error)?;
        let names_cf = column_family(&self.db, column_families::NODE_NAMES)?;
        if let Some(value) = self.db.get_cf(names_cf, &name_key)? {
            let existing = decode_u64_record(&value)
                .map_err(|error| record_error(column_families::NODE_NAMES, name.as_str(), error))?;
            return Err(CatalogError::NameAlreadyConfirmed {
                name: name.into_boxed_str(),
                existing_id: ConfirmedId::Node(NodeId::from_u64(existing)),
            });
        }

        let next_id = read_metadata(&self.db, META_NEXT_NODE_ID, "next-node-id")?;
        let following_id = next_id
            .checked_add(1)
            .ok_or(CatalogError::CounterOverflow { counter: "node ID" })?;
        let graph_version = self.graph_version()?;
        let next_graph_version =
            graph_version
                .checked_add(1)
                .ok_or(CatalogError::CounterOverflow {
                    counter: "graph version",
                })?;
        let record = NodeRecord::new(NodeId::from_u64(next_id), name);
        let encoded_record = encode_node(&record).map_err(codec_input_error)?;
        let mut index = self.node_index_write()?;

        let candidates = column_family(&self.db, column_families::CANDIDATES)?;
        let nodes = column_family(&self.db, column_families::NODES)?;
        let mut batch = WriteBatch::default();
        batch.put_cf(nodes, encode_id_key(next_id), encoded_record);
        batch.put_cf(names_cf, &name_key, encode_u64_record(next_id));
        batch.delete_cf(candidates, encode_id_key(candidate_id.as_u64()));
        batch.put(META_NEXT_NODE_ID, encode_u64_record(following_id));
        batch.put(META_GRAPH_VERSION, encode_u64_record(next_graph_version));
        self.db.write(batch)?;

        index.insert(record.name().as_str().into(), record.id());
        Ok(ConfirmedRecord::Node(record))
    }

    fn confirm_relation(
        &self,
        candidate_id: CandidateId,
        name: RelationName,
    ) -> Result<ConfirmedRecord, CatalogError> {
        let name_key = encode_name_key(name.as_str()).map_err(codec_input_error)?;
        let names_cf = column_family(&self.db, column_families::RELATION_NAMES)?;
        if let Some(value) = self.db.get_cf(names_cf, &name_key)? {
            let existing = decode_u64_record(&value).map_err(|error| {
                record_error(column_families::RELATION_NAMES, name.as_str(), error)
            })?;
            return Err(CatalogError::NameAlreadyConfirmed {
                name: name.into_boxed_str(),
                existing_id: ConfirmedId::Relation(RelationId::from_u64(existing)),
            });
        }

        let next_id = read_metadata(&self.db, META_NEXT_RELATION_ID, "next-relation-id")?;
        let following_id = next_id
            .checked_add(1)
            .ok_or(CatalogError::CounterOverflow {
                counter: "relation ID",
            })?;
        let graph_version = self.graph_version()?;
        let next_graph_version =
            graph_version
                .checked_add(1)
                .ok_or(CatalogError::CounterOverflow {
                    counter: "graph version",
                })?;
        let record = RelationRecord::new(RelationId::from_u64(next_id), name);
        let encoded_record = encode_relation(&record).map_err(codec_input_error)?;
        let mut index = self.relation_index_write()?;

        let candidates = column_family(&self.db, column_families::CANDIDATES)?;
        let relations = column_family(&self.db, column_families::RELATION_KINDS)?;
        let mut batch = WriteBatch::default();
        batch.put_cf(relations, encode_id_key(next_id), encoded_record);
        batch.put_cf(names_cf, &name_key, encode_u64_record(next_id));
        batch.delete_cf(candidates, encode_id_key(candidate_id.as_u64()));
        batch.put(META_NEXT_RELATION_ID, encode_u64_record(following_id));
        batch.put(META_GRAPH_VERSION, encode_u64_record(next_graph_version));
        self.db.write(batch)?;

        index.insert(record.name().as_str().into(), record.id());
        Ok(ConfirmedRecord::Relation(record))
    }

    fn write_guard(&self) -> Result<MutexGuard<'_, ()>, CatalogError> {
        self.write_mutex
            .lock()
            .map_err(|_| CatalogError::LockPoisoned {
                lock: "catalog write",
            })
    }

    fn node_index_write(&self) -> Result<RwLockWriteGuard<'_, NodeNameIndex>, CatalogError> {
        self.node_names
            .write()
            .map_err(|_| CatalogError::LockPoisoned {
                lock: "node-name index",
            })
    }

    fn relation_index_write(
        &self,
    ) -> Result<RwLockWriteGuard<'_, RelationNameIndex>, CatalogError> {
        self.relation_names
            .write()
            .map_err(|_| CatalogError::LockPoisoned {
                lock: "relation-name index",
            })
    }
}

enum CandidateName {
    Node(NodeName),
    Relation(RelationName),
}

fn initialize_metadata(db: &DB) -> Result<(), CatalogError> {
    match db.get(META_FORMAT)? {
        Some(value) => {
            decode_format_version(&value)
                .map_err(|error| record_error("default", "storage-format", error))?;
            for (key, id) in [
                (META_GRAPH_VERSION, "graph-version"),
                (META_NEXT_CANDIDATE_ID, "next-candidate-id"),
                (META_NEXT_NODE_ID, "next-node-id"),
                (META_NEXT_RELATION_ID, "next-relation-id"),
            ] {
                read_metadata(db, key, id)?;
            }
            Ok(())
        }
        None => {
            ensure_database_empty(db)?;
            let mut batch = WriteBatch::default();
            batch.put(META_FORMAT, encode_format_version());
            batch.put(META_GRAPH_VERSION, encode_u64_record(0));
            batch.put(META_NEXT_CANDIDATE_ID, encode_u64_record(INITIAL_ID));
            batch.put(META_NEXT_NODE_ID, encode_u64_record(INITIAL_ID));
            batch.put(META_NEXT_RELATION_ID, encode_u64_record(INITIAL_ID));
            db.write(batch)?;
            Ok(())
        }
    }
}

fn ensure_database_empty(db: &DB) -> Result<(), CatalogError> {
    if db
        .iterator(IteratorMode::Start)
        .next()
        .transpose()?
        .is_some()
    {
        return Err(CatalogError::CorruptRecord {
            key_space: "default",
            record_id: "storage-format".to_owned(),
            reason: "format marker is missing from a non-empty database".to_owned(),
        });
    }
    for name in column_families::ALL {
        let cf = column_family(db, name)?;
        if db
            .iterator_cf(cf, IteratorMode::Start)
            .next()
            .transpose()?
            .is_some()
        {
            return Err(CatalogError::CorruptRecord {
                key_space: name,
                record_id: "first record".to_owned(),
                reason: "format marker is missing from a non-empty database".to_owned(),
            });
        }
    }
    Ok(())
}

fn rebuild_indexes(db: &DB) -> Result<(NodeNameIndex, RelationNameIndex), CatalogError> {
    validate_candidates(db)?;

    let mut nodes = HashMap::new();
    let node_names_cf = column_family(db, column_families::NODE_NAMES)?;
    let nodes_cf = column_family(db, column_families::NODES)?;
    for entry in db.iterator_cf(node_names_cf, IteratorMode::Start) {
        let (key, value) = entry?;
        let key_id = bytes_id(&key);
        let name = decode_name_key(&key)
            .map_err(|error| record_error(column_families::NODE_NAMES, key_id, error))?;
        let id = decode_u64_record(&value)
            .map_err(|error| record_error(column_families::NODE_NAMES, name.to_string(), error))?;
        let record_value =
            db.get_cf(nodes_cf, encode_id_key(id))?
                .ok_or_else(|| CatalogError::CorruptRecord {
                    key_space: column_families::NODE_NAMES,
                    record_id: name.to_string(),
                    reason: format!("mapped node record {id} is missing"),
                })?;
        let record = decode_node(&record_value, id)
            .map_err(|error| record_error(column_families::NODES, id.to_string(), error))?;
        if record.name().as_str() != name.as_ref() {
            return Err(CatalogError::CorruptRecord {
                key_space: column_families::NODE_NAMES,
                record_id: name.to_string(),
                reason: format!("mapped node record {id} contains a different exact name"),
            });
        }
        if nodes.insert(name.clone(), NodeId::from_u64(id)).is_some() {
            return Err(CatalogError::CorruptRecord {
                key_space: column_families::NODE_NAMES,
                record_id: name.to_string(),
                reason: "duplicate exact-name mapping".to_owned(),
            });
        }
    }
    verify_all_nodes_are_mapped(db, &nodes)?;

    let mut relations = HashMap::new();
    let relation_names_cf = column_family(db, column_families::RELATION_NAMES)?;
    let relations_cf = column_family(db, column_families::RELATION_KINDS)?;
    for entry in db.iterator_cf(relation_names_cf, IteratorMode::Start) {
        let (key, value) = entry?;
        let key_id = bytes_id(&key);
        let name = decode_name_key(&key)
            .map_err(|error| record_error(column_families::RELATION_NAMES, key_id, error))?;
        let id = decode_u64_record(&value).map_err(|error| {
            record_error(column_families::RELATION_NAMES, name.to_string(), error)
        })?;
        let record_value = db.get_cf(relations_cf, encode_id_key(id))?.ok_or_else(|| {
            CatalogError::CorruptRecord {
                key_space: column_families::RELATION_NAMES,
                record_id: name.to_string(),
                reason: format!("mapped relation record {id} is missing"),
            }
        })?;
        let record = decode_relation(&record_value, id).map_err(|error| {
            record_error(column_families::RELATION_KINDS, id.to_string(), error)
        })?;
        if record.name().as_str() != name.as_ref() {
            return Err(CatalogError::CorruptRecord {
                key_space: column_families::RELATION_NAMES,
                record_id: name.to_string(),
                reason: format!("mapped relation record {id} contains a different exact name"),
            });
        }
        if relations
            .insert(name.clone(), RelationId::from_u64(id))
            .is_some()
        {
            return Err(CatalogError::CorruptRecord {
                key_space: column_families::RELATION_NAMES,
                record_id: name.to_string(),
                reason: "duplicate exact-name mapping".to_owned(),
            });
        }
    }
    verify_all_relations_are_mapped(db, &relations)?;
    Ok((nodes, relations))
}

fn validate_candidates(db: &DB) -> Result<(), CatalogError> {
    let cf = column_family(db, column_families::CANDIDATES)?;
    for entry in db.iterator_cf(cf, IteratorMode::Start) {
        let (key, value) = entry?;
        let key_id = bytes_id(&key);
        let id = decode_id_key(&key)
            .map_err(|error| record_error(column_families::CANDIDATES, key_id, error))?;
        decode_candidate(&value, id)
            .map_err(|error| record_error(column_families::CANDIDATES, id.to_string(), error))?;
    }
    Ok(())
}

fn verify_all_nodes_are_mapped(db: &DB, names: &NodeNameIndex) -> Result<(), CatalogError> {
    let cf = column_family(db, column_families::NODES)?;
    for entry in db.iterator_cf(cf, IteratorMode::Start) {
        let (key, value) = entry?;
        let key_id = bytes_id(&key);
        let id = decode_id_key(&key)
            .map_err(|error| record_error(column_families::NODES, key_id, error))?;
        let record = decode_node(&value, id)
            .map_err(|error| record_error(column_families::NODES, id.to_string(), error))?;
        if names.get(record.name().as_str()) != Some(&record.id()) {
            return Err(CatalogError::CorruptRecord {
                key_space: column_families::NODES,
                record_id: id.to_string(),
                reason: "confirmed node has no matching exact-name mapping".to_owned(),
            });
        }
    }
    Ok(())
}

fn verify_all_relations_are_mapped(db: &DB, names: &RelationNameIndex) -> Result<(), CatalogError> {
    let cf = column_family(db, column_families::RELATION_KINDS)?;
    for entry in db.iterator_cf(cf, IteratorMode::Start) {
        let (key, value) = entry?;
        let key_id = bytes_id(&key);
        let id = decode_id_key(&key)
            .map_err(|error| record_error(column_families::RELATION_KINDS, key_id, error))?;
        let record = decode_relation(&value, id).map_err(|error| {
            record_error(column_families::RELATION_KINDS, id.to_string(), error)
        })?;
        if names.get(record.name().as_str()) != Some(&record.id()) {
            return Err(CatalogError::CorruptRecord {
                key_space: column_families::RELATION_KINDS,
                record_id: id.to_string(),
                reason: "confirmed relation has no matching exact-name mapping".to_owned(),
            });
        }
    }
    Ok(())
}

fn read_metadata(db: &DB, key: &[u8], record_id: &'static str) -> Result<u64, CatalogError> {
    let value = db.get(key)?.ok_or_else(|| CatalogError::CorruptRecord {
        key_space: "default",
        record_id: record_id.to_owned(),
        reason: "required metadata record is missing".to_owned(),
    })?;
    decode_u64_record(&value).map_err(|error| record_error("default", record_id, error))
}

fn column_family<'a>(db: &'a DB, name: &'static str) -> Result<&'a ColumnFamily, CatalogError> {
    db.cf_handle(name)
        .ok_or_else(|| CatalogError::CorruptRecord {
            key_space: "database",
            record_id: name.to_owned(),
            reason: "required column family is missing".to_owned(),
        })
}

fn record_error(
    key_space: &'static str,
    record_id: impl Into<String>,
    error: CodecError,
) -> CatalogError {
    let record_id = record_id.into();
    match error {
        CodecError::UnknownVersion(found) => CatalogError::IncompatibleFormat {
            key_space,
            record_id,
            found,
            supported: codec::FORMAT_VERSION,
        },
        other => CatalogError::CorruptRecord {
            key_space,
            record_id,
            reason: other.to_string(),
        },
    }
}

fn codec_input_error(error: CodecError) -> CatalogError {
    match error {
        CodecError::NameTooLong(byte_length) => CatalogError::NameTooLong { byte_length },
        other => CatalogError::CorruptRecord {
            key_space: "input",
            record_id: "name".to_owned(),
            reason: other.to_string(),
        },
    }
}

fn bytes_id(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}
