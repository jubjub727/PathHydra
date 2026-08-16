use rocksdb::{ColumnFamilyDescriptor, Options, SliceTransform, WriteOptions};

use crate::column_families;

/// Current first-release durability policy. Every write uses the WAL. Ordinary
/// commits do not force a device sync; explicit checkpoint and shutdown flushes
/// synchronize the WAL before reporting durability assurance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WalSyncPolicy {
    WalEnabledExplicitSync,
}

pub const WAL_SYNC_POLICY: WalSyncPolicy = WalSyncPolicy::WalEnabledExplicitSync;

pub(crate) fn database_options(create: bool) -> Options {
    let mut options = Options::default();
    options.create_if_missing(create);
    options.create_missing_column_families(create);
    // A small fixed background pool avoids inheriting machine-wide defaults
    // while retaining the current leveled layout and compression behavior.
    options.set_max_background_jobs(4);
    options
}

fn family_options(name: &str) -> Options {
    let mut options = Options::default();
    // These two families are always read by an exact eight-byte NodeId prefix.
    if matches!(
        name,
        column_families::OUTGOING_EDGES | column_families::INCOMING_EDGES
    ) {
        options.set_prefix_extractor(SliceTransform::create_fixed_prefix(8));
    }
    options
}

pub(crate) fn descriptors() -> impl Iterator<Item = ColumnFamilyDescriptor> {
    column_families::ALL
        .into_iter()
        .map(|name| ColumnFamilyDescriptor::new(name, family_options(name)))
}

pub(crate) fn write_options() -> WriteOptions {
    let mut options = WriteOptions::default();
    options.disable_wal(false);
    options.set_sync(false);
    options
}
