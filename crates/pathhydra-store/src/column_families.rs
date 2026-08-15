use rocksdb::{ColumnFamilyDescriptor, Options};

pub(crate) const CANDIDATES: &str = "candidates";
pub(crate) const NODES: &str = "nodes";
pub(crate) const NODE_NAMES: &str = "node_names";
pub(crate) const RELATION_KINDS: &str = "relation_kinds";
pub(crate) const RELATION_NAMES: &str = "relation_names";

pub(crate) const ALL: [&str; 5] = [
    CANDIDATES,
    NODES,
    NODE_NAMES,
    RELATION_KINDS,
    RELATION_NAMES,
];

pub(crate) fn descriptors() -> impl Iterator<Item = ColumnFamilyDescriptor> {
    ALL.into_iter()
        .map(|name| ColumnFamilyDescriptor::new(name, Options::default()))
}
