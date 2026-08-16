use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::Path,
    sync::{Mutex, MutexGuard, RwLock, RwLockWriteGuard},
};

use pathhydra_core::{
    BaseWeight, Candidate, CandidateId, ConfirmedRecord, EdgeId, EdgeRecord, NodeId, NodeName,
    NodePayload, NodeRecord, RelationId, RelationName, RelationRecord,
};
use rocksdb::{ColumnFamily, DB, Direction, IteratorMode, Options, WriteBatch};

use crate::{
    codec::{
        CodecError, decode_adjacency_key, decode_adjacency_value, decode_candidate, decode_edge,
        decode_id_key, decode_name_key, decode_node, decode_relation, decode_u64_record,
        encode_adjacency_key, encode_adjacency_value, encode_candidate, encode_edge, encode_id_key,
        encode_name_key, encode_node, encode_relation, encode_u64_record,
    },
    column_families,
    error::{CatalogError, ConfirmedId, EdgeEndpoint, RecordKind},
};

const META_NEXT_CANDIDATE_ID: &[u8] = b"next-candidate-id";
const META_NEXT_NODE_ID: &[u8] = b"next-node-id";
const META_NEXT_RELATION_ID: &[u8] = b"next-relation-id";
const META_NEXT_EDGE_ID: &[u8] = b"next-edge-id";
const INITIAL_ID: u64 = 1;

type NodeNameIndex = HashMap<Box<str>, NodeId>;
type RelationNameIndex = HashMap<Box<str>, RelationId>;

/// A self-contained, point-in-time read of every confirmed graph record.
///
/// Records are sorted by their stable numeric IDs. Provisional candidates and
/// storage implementation details are deliberately absent.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ConfirmedGraphRecords {
    nodes: Vec<NodeRecord>,
    relation_kinds: Vec<RelationRecord>,
    edges: Vec<EdgeRecord>,
}

/// Deduplicated current confirmed records returned by one catalog batch read.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ConfirmedRecordBatch {
    nodes: BTreeMap<NodeId, NodeRecord>,
    edges: BTreeMap<EdgeId, EdgeRecord>,
    relation_kinds: BTreeMap<RelationId, RelationRecord>,
}

impl ConfirmedRecordBatch {
    #[must_use]
    pub fn node(&self, id: NodeId) -> Option<&NodeRecord> {
        self.nodes.get(&id)
    }
    #[must_use]
    pub fn edge(&self, id: EdgeId) -> Option<&EdgeRecord> {
        self.edges.get(&id)
    }
    #[must_use]
    pub fn relation_kind(&self, id: RelationId) -> Option<&RelationRecord> {
        self.relation_kinds.get(&id)
    }
    pub fn nodes(&self) -> impl ExactSizeIterator<Item = &NodeRecord> {
        self.nodes.values()
    }
    pub fn edges(&self) -> impl ExactSizeIterator<Item = &EdgeRecord> {
        self.edges.values()
    }
    pub fn relation_kinds(&self) -> impl ExactSizeIterator<Item = &RelationRecord> {
        self.relation_kinds.values()
    }
}

impl ConfirmedGraphRecords {
    /// Creates an aggregate from records supplied by a caller.
    ///
    /// The routing compiler revalidates ordering, identity, and references, so
    /// this constructor intentionally does not claim the records are valid.
    #[must_use]
    pub fn new(
        nodes: Vec<NodeRecord>,
        relation_kinds: Vec<RelationRecord>,
        edges: Vec<EdgeRecord>,
    ) -> Self {
        Self {
            nodes,
            relation_kinds,
            edges,
        }
    }

    #[must_use]
    pub fn nodes(&self) -> &[NodeRecord] {
        &self.nodes
    }

    #[must_use]
    pub fn relation_kinds(&self) -> &[RelationRecord] {
        &self.relation_kinds
    }

    #[must_use]
    pub fn edges(&self) -> &[EdgeRecord] {
        &self.edges
    }
}

/// A durable store for exact identities and confirmed directed graph records.
///
/// Candidates do not affect confirmed lookup or adjacency until the caller
/// validates and explicitly promotes them. Edge promotion and cascading node
/// deletion are each one atomic RocksDB batch:
///
/// ```no_run
/// use pathhydra_store::{Catalog, ConfirmedRecord};
///
/// # fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let catalog = Catalog::open("pathhydra.db")?;
/// let source = catalog.insert_node_candidate("source")?;
/// let destination = catalog.insert_node_candidate("destination")?;
/// let kind = catalog.insert_relation_candidate("depends on")?;
/// let ConfirmedRecord::Node(source) = catalog.confirm_validated_candidate(source)? else {
///     unreachable!()
/// };
/// let ConfirmedRecord::Node(destination) = catalog.confirm_validated_candidate(destination)? else {
///     unreachable!()
/// };
/// let ConfirmedRecord::Relation(kind) = catalog.confirm_validated_candidate(kind)? else {
///     unreachable!()
/// };
/// let edge = catalog.insert_edge_candidate(
///     source.id(),
///     destination.id(),
///     kind.id(),
///     0.25,
/// )?;
/// let ConfirmedRecord::Edge(edge) = catalog.confirm_validated_candidate(edge)? else {
///     unreachable!()
/// };
/// assert_eq!(catalog.outgoing_edges(source.id())?, vec![edge]);
/// catalog.remove_node(source.id())?;
/// assert!(catalog.outgoing_edges(destination.id())?.is_empty());
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
    /// Opens and validates a complete catalog before publishing it.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, CatalogError> {
        let mut options = Options::default();
        options.create_if_missing(true);
        options.create_missing_column_families(true);

        let db = DB::open_cf_descriptors(&options, path, column_families::descriptors())?;
        initialize_metadata(&db)?;
        let (node_names, relation_names) = rebuild_indexes_and_validate(&db)?;

        Ok(Self {
            db,
            write_mutex: Mutex::new(()),
            node_names: RwLock::new(node_names),
            relation_names: RwLock::new(relation_names),
        })
    }

    /// Inserts a provisional node candidate with an explicitly empty payload.
    pub fn insert_node_candidate(
        &self,
        name: impl Into<NodeName>,
    ) -> Result<CandidateId, CatalogError> {
        self.insert_node_candidate_with_payload(name, NodePayload::default())
    }

    /// Inserts a provisional node candidate and preserves its opaque payload.
    pub fn insert_node_candidate_with_payload(
        &self,
        name: impl Into<NodeName>,
        payload: impl Into<NodePayload>,
    ) -> Result<CandidateId, CatalogError> {
        self.insert_candidate(CandidateInput::Node {
            name: name.into(),
            payload: payload.into(),
        })
    }

    /// Inserts a provisional relation-kind candidate.
    pub fn insert_relation_candidate(
        &self,
        name: impl Into<RelationName>,
    ) -> Result<CandidateId, CatalogError> {
        self.insert_candidate(CandidateInput::Relation(name.into()))
    }

    /// Inserts a provisional directed edge candidate.
    ///
    /// Endpoint and relation-kind existence is checked only at promotion. The
    /// base weight is validated before any durable write.
    pub fn insert_edge_candidate(
        &self,
        source: NodeId,
        destination: NodeId,
        relation_kind: RelationId,
        base_weight: f32,
    ) -> Result<CandidateId, CatalogError> {
        self.insert_edge_candidate_with_base_weight(
            source,
            destination,
            relation_kind,
            BaseWeight::new(base_weight)?,
        )
    }

    /// Inserts an edge candidate whose base weight was already validated.
    pub fn insert_edge_candidate_with_base_weight(
        &self,
        source: NodeId,
        destination: NodeId,
        relation_kind: RelationId,
        base_weight: BaseWeight,
    ) -> Result<CandidateId, CatalogError> {
        self.insert_candidate(CandidateInput::Edge {
            source,
            destination,
            relation_kind,
            base_weight,
        })
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

    /// Promotes one externally validated candidate in one durable batch.
    pub fn confirm_validated_candidate(
        &self,
        id: CandidateId,
    ) -> Result<ConfirmedRecord, CatalogError> {
        let _write = self.write_guard()?;
        match self.get_candidate(id)? {
            Candidate::Node { id, name, payload } => self.confirm_node(id, name, payload),
            Candidate::Relation { id, name } => self.confirm_relation(id, name),
            Candidate::Edge {
                id,
                source,
                destination,
                relation_kind,
                base_weight,
            } => self.confirm_edge(id, source, destination, relation_kind, base_weight),
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
        get_node_from_db(&self.db, id)
    }

    pub fn get_relation(&self, id: RelationId) -> Result<RelationRecord, CatalogError> {
        get_relation_from_db(&self.db, id)
    }

    pub fn get_edge(&self, id: EdgeId) -> Result<EdgeRecord, CatalogError> {
        get_edge_from_db(&self.db, id)
    }

    /// Copies one consistent view of the complete confirmed graph.
    ///
    /// The catalog write mutex covers all three scans, preventing promotion or
    /// deletion from interleaving with this read. The returned aggregate owns
    /// its records and does not retain a RocksDB iterator or snapshot.
    pub fn confirmed_graph_records(&self) -> Result<ConfirmedGraphRecords, CatalogError> {
        let _write = self.write_guard()?;
        let mut nodes = all_nodes(&self.db)?;
        let mut relation_kinds = all_relation_kinds(&self.db)?;
        let mut edges = all_edges(&self.db)?;
        nodes.sort_unstable_by_key(NodeRecord::id);
        relation_kinds.sort_unstable_by_key(RelationRecord::id);
        edges.sort_unstable_by_key(EdgeRecord::id);
        Ok(ConfirmedGraphRecords::new(nodes, relation_kinds, edges))
    }

    /// Reads requested confirmed records once while preventing graph mutation.
    ///
    /// Ordinary absent IDs are omitted. Every found edge is structurally
    /// checked against its current endpoint and relation-kind records.
    pub fn confirmed_records_by_id(
        &self,
        node_ids: &[NodeId],
        edge_ids: &[EdgeId],
    ) -> Result<ConfirmedRecordBatch, CatalogError> {
        let _write = self.write_guard()?;
        let mut batch = ConfirmedRecordBatch::default();
        for id in node_ids.iter().copied().collect::<BTreeSet<_>>() {
            match get_node_from_db(&self.db, id) {
                Ok(record) => {
                    batch.nodes.insert(id, record);
                }
                Err(CatalogError::NotFound {
                    kind: RecordKind::Node,
                    ..
                }) => {}
                Err(error) => return Err(error),
            }
        }
        let mut relation_ids = BTreeSet::new();
        for id in edge_ids.iter().copied().collect::<BTreeSet<_>>() {
            let edge = match get_edge_from_db(&self.db, id) {
                Ok(record) => record,
                Err(CatalogError::NotFound {
                    kind: RecordKind::Edge,
                    ..
                }) => continue,
                Err(error) => return Err(error),
            };
            for endpoint in [edge.source(), edge.destination()] {
                if let Err(error) = get_node_from_db(&self.db, endpoint) {
                    return Err(match error {
                        CatalogError::NotFound { .. } => corrupt(
                            column_families::EDGES,
                            id.to_string(),
                            format!("endpoint node {endpoint} is absent"),
                        ),
                        other => other,
                    });
                }
            }
            relation_ids.insert(edge.relation_kind());
            batch.edges.insert(id, edge);
        }
        for id in relation_ids {
            let relation = get_relation_from_db(&self.db, id).map_err(|error| match error {
                CatalogError::NotFound { .. } => corrupt(
                    column_families::EDGES,
                    id.to_string(),
                    "edge relation kind is absent",
                ),
                other => other,
            })?;
            batch.relation_kinds.insert(id, relation);
        }
        Ok(batch)
    }

    /// Reads only confirmed outgoing edges through the source-prefix index.
    pub fn outgoing_edges(&self, source: NodeId) -> Result<Vec<EdgeRecord>, CatalogError> {
        let _write = self.write_guard()?;
        self.get_node(source)?;
        incident_edges(&self.db, column_families::OUTGOING_EDGES, source, true)
    }

    /// Reads only confirmed incoming edges through the destination-prefix index.
    pub fn incoming_edges(&self, destination: NodeId) -> Result<Vec<EdgeRecord>, CatalogError> {
        let _write = self.write_guard()?;
        self.get_node(destination)?;
        incident_edges(
            &self.db,
            column_families::INCOMING_EDGES,
            destination,
            false,
        )
    }

    /// Removes one exact confirmed edge and both adjacency entries atomically.
    pub fn remove_edge(&self, id: EdgeId) -> Result<(), CatalogError> {
        let _write = self.write_guard()?;
        let edge = self.get_edge(id)?;
        validate_exact_adjacency(&self.db, &edge)?;

        let edges = column_family(&self.db, column_families::EDGES)?;
        let outgoing = column_family(&self.db, column_families::OUTGOING_EDGES)?;
        let incoming = column_family(&self.db, column_families::INCOMING_EDGES)?;
        let mut batch = WriteBatch::default();
        batch.delete_cf(edges, encode_id_key(id.as_u64()));
        batch.delete_cf(outgoing, encode_adjacency_key(edge.source(), id));
        batch.delete_cf(incoming, encode_adjacency_key(edge.destination(), id));
        self.db.write(batch)?;
        Ok(())
    }

    /// Removes a confirmed node, every incident confirmed edge, and the exact
    /// node-name mapping in one atomic batch.
    pub fn remove_node(&self, id: NodeId) -> Result<(), CatalogError> {
        let _write = self.write_guard()?;
        let node = self.get_node(id)?;
        let outgoing_edges = incident_edges(&self.db, column_families::OUTGOING_EDGES, id, true)?;
        let incoming_edges = incident_edges(&self.db, column_families::INCOMING_EDGES, id, false)?;
        let mut incident = HashMap::new();
        for edge in outgoing_edges.into_iter().chain(incoming_edges) {
            if let Some(existing) = incident.insert(edge.id(), edge.clone())
                && existing != edge
            {
                return Err(corrupt(
                    column_families::EDGES,
                    edge.id().to_string(),
                    "incident indexes resolve to different canonical edge records",
                ));
            }
        }
        for edge in incident.values() {
            validate_exact_adjacency(&self.db, edge)?;
        }

        let name_key = encode_name_key(node.name().as_str()).map_err(codec_input_error)?;
        let nodes = column_family(&self.db, column_families::NODES)?;
        let names = column_family(&self.db, column_families::NODE_NAMES)?;
        let edges = column_family(&self.db, column_families::EDGES)?;
        let outgoing = column_family(&self.db, column_families::OUTGOING_EDGES)?;
        let incoming = column_family(&self.db, column_families::INCOMING_EDGES)?;
        let mut index = self.node_index_write()?;
        if index.get(node.name().as_str()) != Some(&id) {
            return Err(corrupt(
                column_families::NODE_NAMES,
                node.name().as_str(),
                "in-memory exact-name mapping disagrees with the confirmed node",
            ));
        }

        let mut batch = WriteBatch::default();
        for edge in incident.values() {
            batch.delete_cf(edges, encode_id_key(edge.id().as_u64()));
            batch.delete_cf(outgoing, encode_adjacency_key(edge.source(), edge.id()));
            batch.delete_cf(
                incoming,
                encode_adjacency_key(edge.destination(), edge.id()),
            );
        }
        batch.delete_cf(nodes, encode_id_key(id.as_u64()));
        batch.delete_cf(names, name_key);
        self.db.write(batch)?;
        index.remove(node.name().as_str());
        Ok(())
    }

    fn insert_candidate(&self, input: CandidateInput) -> Result<CandidateId, CatalogError> {
        let _write = self.write_guard()?;
        let next_id = read_metadata(&self.db, META_NEXT_CANDIDATE_ID, "next-candidate-id")?;
        let following_id = next_id
            .checked_add(1)
            .ok_or(CatalogError::CounterOverflow {
                counter: "candidate ID",
            })?;
        let id = CandidateId::from_u64(next_id);
        let candidate = match input {
            CandidateInput::Node { name, payload } => Candidate::Node { id, name, payload },
            CandidateInput::Relation(name) => Candidate::Relation { id, name },
            CandidateInput::Edge {
                source,
                destination,
                relation_kind,
                base_weight,
            } => Candidate::Edge {
                id,
                source,
                destination,
                relation_kind,
                base_weight,
            },
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
        payload: NodePayload,
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
        let record = NodeRecord::new(NodeId::from_u64(next_id), name, payload);
        let encoded_record = encode_node(&record).map_err(codec_input_error)?;
        let mut index = self.node_index_write()?;

        let candidates = column_family(&self.db, column_families::CANDIDATES)?;
        let nodes = column_family(&self.db, column_families::NODES)?;
        let mut batch = WriteBatch::default();
        batch.put_cf(nodes, encode_id_key(next_id), encoded_record);
        batch.put_cf(names_cf, &name_key, encode_u64_record(next_id));
        batch.delete_cf(candidates, encode_id_key(candidate_id.as_u64()));
        batch.put(META_NEXT_NODE_ID, encode_u64_record(following_id));
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
        self.db.write(batch)?;

        index.insert(record.name().as_str().into(), record.id());
        Ok(ConfirmedRecord::Relation(record))
    }

    fn confirm_edge(
        &self,
        candidate_id: CandidateId,
        source: NodeId,
        destination: NodeId,
        relation_kind: RelationId,
        base_weight: BaseWeight,
    ) -> Result<ConfirmedRecord, CatalogError> {
        require_node(&self.db, source, EdgeEndpoint::Source)?;
        require_node(&self.db, destination, EdgeEndpoint::Destination)?;
        require_relation_kind(&self.db, relation_kind)?;

        let next_id = read_metadata(&self.db, META_NEXT_EDGE_ID, "next-edge-id")?;
        let following_id = next_id
            .checked_add(1)
            .ok_or(CatalogError::CounterOverflow { counter: "edge ID" })?;
        let record = EdgeRecord::new(
            EdgeId::from_u64(next_id),
            source,
            destination,
            relation_kind,
            base_weight,
        );

        let candidates = column_family(&self.db, column_families::CANDIDATES)?;
        let edges = column_family(&self.db, column_families::EDGES)?;
        let outgoing = column_family(&self.db, column_families::OUTGOING_EDGES)?;
        let incoming = column_family(&self.db, column_families::INCOMING_EDGES)?;
        let mut batch = WriteBatch::default();
        batch.put_cf(edges, encode_id_key(next_id), encode_edge(&record));
        batch.put_cf(
            outgoing,
            encode_adjacency_key(source, record.id()),
            encode_adjacency_value(record.id()),
        );
        batch.put_cf(
            incoming,
            encode_adjacency_key(destination, record.id()),
            encode_adjacency_value(record.id()),
        );
        batch.delete_cf(candidates, encode_id_key(candidate_id.as_u64()));
        batch.put(META_NEXT_EDGE_ID, encode_u64_record(following_id));
        self.db.write(batch)?;
        Ok(ConfirmedRecord::Edge(record))
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

enum CandidateInput {
    Node {
        name: NodeName,
        payload: NodePayload,
    },
    Relation(RelationName),
    Edge {
        source: NodeId,
        destination: NodeId,
        relation_kind: RelationId,
        base_weight: BaseWeight,
    },
}

fn initialize_metadata(db: &DB) -> Result<(), CatalogError> {
    let metadata = [
        (META_NEXT_CANDIDATE_ID, "next-candidate-id"),
        (META_NEXT_NODE_ID, "next-node-id"),
        (META_NEXT_RELATION_ID, "next-relation-id"),
        (META_NEXT_EDGE_ID, "next-edge-id"),
    ];
    let present = metadata
        .iter()
        .map(|(key, _)| db.get(key))
        .collect::<Result<Vec<_>, _>>()?;
    if present.iter().all(Option::is_none) {
        ensure_database_empty(db)?;
        let mut batch = WriteBatch::default();
        for (key, _) in metadata {
            batch.put(key, encode_u64_record(INITIAL_ID));
        }
        db.write(batch)?;
        return Ok(());
    }
    for (key, id) in metadata {
        read_metadata(db, key, id)?;
    }
    Ok(())
}

fn ensure_database_empty(db: &DB) -> Result<(), CatalogError> {
    if db
        .iterator(IteratorMode::Start)
        .next()
        .transpose()?
        .is_some()
    {
        return Err(corrupt(
            "default",
            "metadata",
            "required metadata is missing from a non-empty database",
        ));
    }
    for name in column_families::ALL {
        let cf = column_family(db, name)?;
        if db
            .iterator_cf(cf, IteratorMode::Start)
            .next()
            .transpose()?
            .is_some()
        {
            return Err(corrupt(
                name,
                "first record",
                "required metadata is missing from a non-empty database",
            ));
        }
    }
    Ok(())
}

fn rebuild_indexes_and_validate(
    db: &DB,
) -> Result<(NodeNameIndex, RelationNameIndex), CatalogError> {
    validate_candidates(db)?;
    let nodes = rebuild_node_index(db)?;
    let relations = rebuild_relation_index(db)?;
    validate_edges_and_adjacency(db)?;
    validate_next_id_counters(db)?;
    Ok((nodes, relations))
}

fn validate_next_id_counters(db: &DB) -> Result<(), CatalogError> {
    for (metadata_key, metadata_name, family) in [
        (
            META_NEXT_CANDIDATE_ID,
            "next-candidate-id",
            column_families::CANDIDATES,
        ),
        (META_NEXT_NODE_ID, "next-node-id", column_families::NODES),
        (
            META_NEXT_RELATION_ID,
            "next-relation-id",
            column_families::RELATION_KINDS,
        ),
        (META_NEXT_EDGE_ID, "next-edge-id", column_families::EDGES),
    ] {
        let next = read_metadata(db, metadata_key, metadata_name)?;
        if next == 0 {
            return Err(corrupt(
                "default",
                metadata_name,
                "next ID must be at least 1",
            ));
        }
        let maximum = collect_ids(db, family)?.into_iter().max().unwrap_or(0);
        if next <= maximum {
            return Err(corrupt(
                "default",
                metadata_name,
                format!("next ID {next} does not follow existing ID {maximum}"),
            ));
        }
    }
    Ok(())
}

fn validate_candidates(db: &DB) -> Result<(), CatalogError> {
    let cf = column_family(db, column_families::CANDIDATES)?;
    for entry in db.iterator_cf(cf, IteratorMode::Start) {
        let (key, value) = entry?;
        let id = decode_id_key(&key)
            .map_err(|error| record_error(column_families::CANDIDATES, bytes_id(&key), error))?;
        decode_candidate(&value, id)
            .map_err(|error| record_error(column_families::CANDIDATES, id.to_string(), error))?;
    }
    Ok(())
}

fn rebuild_node_index(db: &DB) -> Result<NodeNameIndex, CatalogError> {
    let mut names = HashMap::new();
    let names_cf = column_family(db, column_families::NODE_NAMES)?;
    let records_cf = column_family(db, column_families::NODES)?;
    for entry in db.iterator_cf(names_cf, IteratorMode::Start) {
        let (key, value) = entry?;
        let name = decode_name_key(&key)
            .map_err(|error| record_error(column_families::NODE_NAMES, bytes_id(&key), error))?;
        let id = decode_u64_record(&value)
            .map_err(|error| record_error(column_families::NODE_NAMES, name.to_string(), error))?;
        let record_value = db.get_cf(records_cf, encode_id_key(id))?.ok_or_else(|| {
            corrupt(
                column_families::NODE_NAMES,
                name.to_string(),
                format!("mapped node record {id} is missing"),
            )
        })?;
        let record = decode_node(&record_value, id)
            .map_err(|error| record_error(column_families::NODES, id.to_string(), error))?;
        if record.name().as_str() != name.as_ref() {
            return Err(corrupt(
                column_families::NODE_NAMES,
                name.to_string(),
                format!("mapped node record {id} contains a different exact name"),
            ));
        }
        if names.insert(name.clone(), record.id()).is_some() {
            return Err(corrupt(
                column_families::NODE_NAMES,
                name.to_string(),
                "duplicate exact-name mapping",
            ));
        }
    }
    for entry in db.iterator_cf(records_cf, IteratorMode::Start) {
        let (key, value) = entry?;
        let id = decode_id_key(&key)
            .map_err(|error| record_error(column_families::NODES, bytes_id(&key), error))?;
        let record = decode_node(&value, id)
            .map_err(|error| record_error(column_families::NODES, id.to_string(), error))?;
        if names.get(record.name().as_str()) != Some(&record.id()) {
            return Err(corrupt(
                column_families::NODES,
                id.to_string(),
                "confirmed node has no matching exact-name mapping",
            ));
        }
    }
    Ok(names)
}

fn rebuild_relation_index(db: &DB) -> Result<RelationNameIndex, CatalogError> {
    let mut names = HashMap::new();
    let names_cf = column_family(db, column_families::RELATION_NAMES)?;
    let records_cf = column_family(db, column_families::RELATION_KINDS)?;
    for entry in db.iterator_cf(names_cf, IteratorMode::Start) {
        let (key, value) = entry?;
        let name = decode_name_key(&key).map_err(|error| {
            record_error(column_families::RELATION_NAMES, bytes_id(&key), error)
        })?;
        let id = decode_u64_record(&value).map_err(|error| {
            record_error(column_families::RELATION_NAMES, name.to_string(), error)
        })?;
        let record_value = db.get_cf(records_cf, encode_id_key(id))?.ok_or_else(|| {
            corrupt(
                column_families::RELATION_NAMES,
                name.to_string(),
                format!("mapped relation-kind record {id} is missing"),
            )
        })?;
        let record = decode_relation(&record_value, id).map_err(|error| {
            record_error(column_families::RELATION_KINDS, id.to_string(), error)
        })?;
        if record.name().as_str() != name.as_ref() {
            return Err(corrupt(
                column_families::RELATION_NAMES,
                name.to_string(),
                format!("mapped relation-kind record {id} contains a different exact name"),
            ));
        }
        if names.insert(name.clone(), record.id()).is_some() {
            return Err(corrupt(
                column_families::RELATION_NAMES,
                name.to_string(),
                "duplicate exact-name mapping",
            ));
        }
    }
    for entry in db.iterator_cf(records_cf, IteratorMode::Start) {
        let (key, value) = entry?;
        let id = decode_id_key(&key).map_err(|error| {
            record_error(column_families::RELATION_KINDS, bytes_id(&key), error)
        })?;
        let record = decode_relation(&value, id).map_err(|error| {
            record_error(column_families::RELATION_KINDS, id.to_string(), error)
        })?;
        if names.get(record.name().as_str()) != Some(&record.id()) {
            return Err(corrupt(
                column_families::RELATION_KINDS,
                id.to_string(),
                "confirmed relation kind has no matching exact-name mapping",
            ));
        }
    }
    Ok(names)
}

fn validate_edges_and_adjacency(db: &DB) -> Result<(), CatalogError> {
    let mut edges = HashMap::new();
    for edge in all_edges(db)? {
        require_node(db, edge.source(), EdgeEndpoint::Source).map_err(|_| {
            corrupt(
                column_families::EDGES,
                edge.id().to_string(),
                format!("source node {} is not confirmed", edge.source()),
            )
        })?;
        require_node(db, edge.destination(), EdgeEndpoint::Destination).map_err(|_| {
            corrupt(
                column_families::EDGES,
                edge.id().to_string(),
                format!("destination node {} is not confirmed", edge.destination()),
            )
        })?;
        require_relation_kind(db, edge.relation_kind()).map_err(|_| {
            corrupt(
                column_families::EDGES,
                edge.id().to_string(),
                format!("relation kind {} is not confirmed", edge.relation_kind()),
            )
        })?;
        edges.insert(edge.id(), edge);
    }

    let mut outgoing_count = HashMap::new();
    validate_adjacency_family(
        db,
        column_families::OUTGOING_EDGES,
        &edges,
        true,
        &mut outgoing_count,
    )?;
    let mut incoming_count = HashMap::new();
    validate_adjacency_family(
        db,
        column_families::INCOMING_EDGES,
        &edges,
        false,
        &mut incoming_count,
    )?;
    for edge in edges.values() {
        if outgoing_count.get(&edge.id()) != Some(&1) {
            return Err(corrupt(
                column_families::EDGES,
                edge.id().to_string(),
                "canonical edge does not have exactly one matching outgoing entry",
            ));
        }
        if incoming_count.get(&edge.id()) != Some(&1) {
            return Err(corrupt(
                column_families::EDGES,
                edge.id().to_string(),
                "canonical edge does not have exactly one matching incoming entry",
            ));
        }
    }
    Ok(())
}

fn validate_adjacency_family(
    db: &DB,
    name: &'static str,
    edges: &HashMap<EdgeId, EdgeRecord>,
    outgoing: bool,
    counts: &mut HashMap<EdgeId, usize>,
) -> Result<(), CatalogError> {
    let cf = column_family(db, name)?;
    for entry in db.iterator_cf(cf, IteratorMode::Start) {
        let (key, value) = entry?;
        let (node, edge_id) = decode_adjacency_key(&key)
            .map_err(|error| record_error(name, bytes_id(&key), error))?;
        decode_adjacency_value(&value, edge_id)
            .map_err(|error| record_error(name, bytes_id(&key), error))?;
        let edge = edges.get(&edge_id).ok_or_else(|| {
            corrupt(
                name,
                bytes_id(&key),
                format!("canonical edge {edge_id} is missing"),
            )
        })?;
        let expected = if outgoing {
            edge.source()
        } else {
            edge.destination()
        };
        if node != expected {
            return Err(corrupt(
                name,
                bytes_id(&key),
                format!("indexed endpoint {node} does not match canonical endpoint {expected}"),
            ));
        }
        *counts.entry(edge_id).or_default() += 1;
    }
    Ok(())
}

fn incident_edges(
    db: &DB,
    index_name: &'static str,
    node: NodeId,
    outgoing: bool,
) -> Result<Vec<EdgeRecord>, CatalogError> {
    let cf = column_family(db, index_name)?;
    let prefix = node.as_u64().to_be_bytes();
    let mut edges = Vec::new();
    for entry in db.iterator_cf(cf, IteratorMode::From(&prefix, Direction::Forward)) {
        let (key, value) = entry?;
        if key.get(..8) != Some(prefix.as_slice()) {
            break;
        }
        let (indexed_node, edge_id) = decode_adjacency_key(&key)
            .map_err(|error| record_error(index_name, bytes_id(&key), error))?;
        if indexed_node != node {
            return Err(corrupt(
                index_name,
                bytes_id(&key),
                "adjacency prefix does not match decoded endpoint",
            ));
        }
        decode_adjacency_value(&value, edge_id)
            .map_err(|error| record_error(index_name, bytes_id(&key), error))?;
        let edge = get_edge_from_db(db, edge_id).map_err(|error| match error {
            CatalogError::NotFound { .. } => corrupt(
                index_name,
                bytes_id(&key),
                format!("canonical edge {edge_id} is missing"),
            ),
            other => other,
        })?;
        let endpoint = if outgoing {
            edge.source()
        } else {
            edge.destination()
        };
        if endpoint != node {
            return Err(corrupt(
                index_name,
                bytes_id(&key),
                format!("canonical edge {edge_id} has endpoint {endpoint}"),
            ));
        }
        edges.push(edge);
    }
    Ok(edges)
}

fn validate_exact_adjacency(db: &DB, edge: &EdgeRecord) -> Result<(), CatalogError> {
    for (name, node) in [
        (column_families::OUTGOING_EDGES, edge.source()),
        (column_families::INCOMING_EDGES, edge.destination()),
    ] {
        let cf = column_family(db, name)?;
        let key = encode_adjacency_key(node, edge.id());
        let value = db.get_cf(cf, key)?.ok_or_else(|| {
            corrupt(
                name,
                edge.id().to_string(),
                "required adjacency entry is missing",
            )
        })?;
        decode_adjacency_value(&value, edge.id())
            .map_err(|error| record_error(name, edge.id().to_string(), error))?;
    }
    Ok(())
}

fn get_node_from_db(db: &DB, id: NodeId) -> Result<NodeRecord, CatalogError> {
    let cf = column_family(db, column_families::NODES)?;
    let value = db
        .get_cf(cf, encode_id_key(id.as_u64()))?
        .ok_or(CatalogError::NotFound {
            kind: RecordKind::Node,
            id: id.as_u64(),
        })?;
    decode_node(&value, id.as_u64())
        .map_err(|error| record_error(column_families::NODES, id.to_string(), error))
}

fn get_relation_from_db(db: &DB, id: RelationId) -> Result<RelationRecord, CatalogError> {
    let cf = column_family(db, column_families::RELATION_KINDS)?;
    let value = db
        .get_cf(cf, encode_id_key(id.as_u64()))?
        .ok_or(CatalogError::NotFound {
            kind: RecordKind::Relation,
            id: id.as_u64(),
        })?;
    decode_relation(&value, id.as_u64())
        .map_err(|error| record_error(column_families::RELATION_KINDS, id.to_string(), error))
}

fn get_edge_from_db(db: &DB, id: EdgeId) -> Result<EdgeRecord, CatalogError> {
    let cf = column_family(db, column_families::EDGES)?;
    let value = db
        .get_cf(cf, encode_id_key(id.as_u64()))?
        .ok_or(CatalogError::NotFound {
            kind: RecordKind::Edge,
            id: id.as_u64(),
        })?;
    decode_edge(&value, id.as_u64())
        .map_err(|error| record_error(column_families::EDGES, id.to_string(), error))
}

fn require_node(db: &DB, id: NodeId, endpoint: EdgeEndpoint) -> Result<(), CatalogError> {
    match get_node_from_db(db, id) {
        Ok(_) => Ok(()),
        Err(CatalogError::NotFound { .. }) => Err(CatalogError::MissingEdgeEndpoint {
            endpoint,
            node_id: id,
        }),
        Err(error) => Err(error),
    }
}

fn require_relation_kind(db: &DB, id: RelationId) -> Result<(), CatalogError> {
    match get_relation_from_db(db, id) {
        Ok(_) => Ok(()),
        Err(CatalogError::NotFound { .. }) => Err(CatalogError::MissingEdgeRelationKind {
            relation_kind_id: id,
        }),
        Err(error) => Err(error),
    }
}

fn collect_ids(db: &DB, name: &'static str) -> Result<Vec<u64>, CatalogError> {
    let cf = column_family(db, name)?;
    let mut ids = Vec::new();
    for entry in db.iterator_cf(cf, IteratorMode::Start) {
        let (key, _) = entry?;
        ids.push(decode_id_key(&key).map_err(|error| record_error(name, bytes_id(&key), error))?);
    }
    Ok(ids)
}

fn all_nodes(db: &DB) -> Result<Vec<NodeRecord>, CatalogError> {
    let cf = column_family(db, column_families::NODES)?;
    let mut nodes = Vec::new();
    for entry in db.iterator_cf(cf, IteratorMode::Start) {
        let (key, value) = entry?;
        let id = decode_id_key(&key)
            .map_err(|error| record_error(column_families::NODES, bytes_id(&key), error))?;
        nodes.push(
            decode_node(&value, id)
                .map_err(|error| record_error(column_families::NODES, id.to_string(), error))?,
        );
    }
    Ok(nodes)
}

fn all_relation_kinds(db: &DB) -> Result<Vec<RelationRecord>, CatalogError> {
    let cf = column_family(db, column_families::RELATION_KINDS)?;
    let mut relations = Vec::new();
    for entry in db.iterator_cf(cf, IteratorMode::Start) {
        let (key, value) = entry?;
        let id = decode_id_key(&key).map_err(|error| {
            record_error(column_families::RELATION_KINDS, bytes_id(&key), error)
        })?;
        relations.push(decode_relation(&value, id).map_err(|error| {
            record_error(column_families::RELATION_KINDS, id.to_string(), error)
        })?);
    }
    Ok(relations)
}

fn all_edges(db: &DB) -> Result<Vec<EdgeRecord>, CatalogError> {
    let cf = column_family(db, column_families::EDGES)?;
    let mut edges = Vec::new();
    for entry in db.iterator_cf(cf, IteratorMode::Start) {
        let (key, value) = entry?;
        let id = decode_id_key(&key)
            .map_err(|error| record_error(column_families::EDGES, bytes_id(&key), error))?;
        edges.push(
            decode_edge(&value, id)
                .map_err(|error| record_error(column_families::EDGES, id.to_string(), error))?,
        );
    }
    Ok(edges)
}

fn read_metadata(db: &DB, key: &[u8], record_id: &'static str) -> Result<u64, CatalogError> {
    let value = db
        .get(key)?
        .ok_or_else(|| corrupt("default", record_id, "required metadata record is missing"))?;
    decode_u64_record(&value).map_err(|error| record_error("default", record_id, error))
}

fn column_family<'a>(db: &'a DB, name: &'static str) -> Result<&'a ColumnFamily, CatalogError> {
    db.cf_handle(name)
        .ok_or_else(|| corrupt("database", name, "required column family is missing"))
}

fn record_error(
    key_space: &'static str,
    record_id: impl Into<String>,
    error: CodecError,
) -> CatalogError {
    CatalogError::CorruptRecord {
        key_space,
        record_id: record_id.into(),
        reason: error.to_string(),
    }
}

fn codec_input_error(error: CodecError) -> CatalogError {
    match error {
        CodecError::NameTooLong(byte_length) => CatalogError::NameTooLong { byte_length },
        CodecError::PayloadTooLong(byte_length) => CatalogError::PayloadTooLong { byte_length },
        other => CatalogError::CorruptRecord {
            key_space: "input",
            record_id: "candidate".to_owned(),
            reason: other.to_string(),
        },
    }
}

fn corrupt(
    key_space: &'static str,
    record_id: impl Into<String>,
    reason: impl Into<String>,
) -> CatalogError {
    CatalogError::CorruptRecord {
        key_space,
        record_id: record_id.into(),
        reason: reason.into(),
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
