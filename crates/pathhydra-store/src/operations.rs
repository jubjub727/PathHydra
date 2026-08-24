use std::{
    collections::BTreeSet,
    error::Error,
    fmt,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
    time::{Duration, Instant},
};

use pathhydra_core::Candidate;
use rocksdb::{DB, FlushOptions, IteratorMode, checkpoint::Checkpoint};

use crate::{
    ActiveRoutingImage, Catalog, CatalogError,
    catalog::{
        META_ACTIVE_ROUTING_IMAGE, read_active_routing_image, rebuild_indexes_and_validate_with,
        validate_metadata_records,
    },
    codec::{decode_candidate, decode_id_key},
    column_families,
    metrics::{MetricValue, StoreMetricsSnapshot},
    options,
    paths::{OperationalPathRequest, PathTarget, normalize_canonical, validate_operational_paths},
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ResourceKind {
    Database,
    RoutingImages,
    Checkpoint,
    Restore,
    Scratch,
}

impl fmt::Display for ResourceKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database => formatter.write_str("database"),
            Self::RoutingImages => formatter.write_str("routing-image"),
            Self::Checkpoint => formatter.write_str("checkpoint"),
            Self::Restore => formatter.write_str("restore"),
            Self::Scratch => formatter.write_str("scratch"),
        }
    }
}

#[derive(Debug)]
pub enum OperationError {
    Catalog(CatalogError),
    RocksDb(rocksdb::Error),
    InvalidPath {
        resource: ResourceKind,
        reason: String,
    },
    UnsafePathRelationship {
        first: ResourceKind,
        second: ResourceKind,
    },
    DestinationNotEmpty {
        resource: ResourceKind,
    },
    DestinationAlreadyExists {
        resource: ResourceKind,
    },
    ResourceRefused {
        resource: &'static str,
        required: u64,
        available: u64,
    },
    StorageExhausted {
        operation: &'static str,
    },
    BackgroundFailure {
        operation: &'static str,
    },
    ConcurrentCheckpoint {
        maximum: usize,
    },
    VerificationLimit {
        resource: &'static str,
        maximum: u64,
    },
    Io {
        operation: &'static str,
        reason: String,
    },
}

impl fmt::Display for OperationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Catalog(error) => error.fmt(formatter),
            Self::RocksDb(error) => write!(formatter, "RocksDB operation failed: {error}"),
            Self::InvalidPath { resource, reason } => {
                write!(formatter, "invalid {resource} path: {reason}")
            }
            Self::UnsafePathRelationship { first, second } => write!(
                formatter,
                "unsafe path relationship between {first} and {second} resources"
            ),
            Self::DestinationNotEmpty { resource } => {
                write!(formatter, "{resource} destination is not empty")
            }
            Self::DestinationAlreadyExists { resource } => {
                write!(formatter, "{resource} destination already exists")
            }
            Self::ResourceRefused {
                resource,
                required,
                available,
            } => write!(
                formatter,
                "{resource} requires {required} bytes but only {available} bytes were admitted"
            ),
            Self::StorageExhausted { operation } => {
                write!(
                    formatter,
                    "storage capacity was exhausted during {operation}"
                )
            }
            Self::BackgroundFailure { operation } => {
                write!(
                    formatter,
                    "RocksDB reported a background failure after {operation}"
                )
            }
            Self::ConcurrentCheckpoint { maximum } => {
                write!(
                    formatter,
                    "maximum {maximum} checkpoints are already active"
                )
            }
            Self::VerificationLimit { resource, maximum } => {
                write!(
                    formatter,
                    "catalog verification exceeded {resource} limit {maximum}"
                )
            }
            Self::Io { operation, reason } => write!(formatter, "{operation} failed: {reason}"),
        }
    }
}

impl Error for OperationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Catalog(error) => Some(error),
            Self::RocksDb(error) => Some(error),
            _ => None,
        }
    }
}

impl From<CatalogError> for OperationError {
    fn from(value: CatalogError) -> Self {
        match value {
            CatalogError::StorageExhausted { operation } => Self::StorageExhausted { operation },
            other => Self::Catalog(other),
        }
    }
}

impl From<rocksdb::Error> for OperationError {
    fn from(value: rocksdb::Error) -> Self {
        if value.kind() == rocksdb::ErrorKind::IOError
            && crate::error::is_storage_exhausted_message(value.as_ref())
        {
            Self::StorageExhausted {
                operation: "RocksDB maintenance",
            }
        } else {
            Self::RocksDb(value)
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CandidateCounts {
    pub nodes: u64,
    pub relation_kinds: u64,
    pub edges: u64,
}

impl CandidateCounts {
    #[must_use]
    pub const fn total(self) -> u64 {
        self.nodes
            .saturating_add(self.relation_kinds)
            .saturating_add(self.edges)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CatalogSummary {
    pub candidates: CandidateCounts,
    pub confirmed_nodes: u64,
    pub relation_kinds: u64,
    pub confirmed_edges: u64,
    pub node_name_entries: u64,
    pub relation_name_entries: u64,
    pub outgoing_entries: u64,
    pub incoming_entries: u64,
    pub routing_pointer_present: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct VerificationLimits {
    pub maximum_records: Option<u64>,
    pub maximum_duration: Option<Duration>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerificationReport {
    pub summary: CatalogSummary,
    pub records_examined: u64,
    pub decoded_bytes: u64,
    /// Stable FNV-1a checksum over family names and exact key/value bytes.
    /// It is operational comparison evidence, never graph identity.
    pub catalog_checksum: u64,
    pub duration: Duration,
}

#[derive(Clone, Debug)]
pub struct CheckpointRequest {
    pub destination_root: PathBuf,
    pub destination: PathBuf,
    pub routing_image_root: Option<PathBuf>,
    pub scratch_path: Option<PathBuf>,
    pub available_destination_bytes: u64,
    pub minimum_headroom_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckpointReport {
    pub catalog: VerificationReport,
    pub file_count: u64,
    pub bytes: u64,
    pub content_checksum: u64,
    pub duration: Duration,
}

#[derive(Clone, Debug)]
pub struct RestoreRequest {
    pub source_root: PathBuf,
    pub source_checkpoint: PathBuf,
    pub destination_root: PathBuf,
    pub destination: PathBuf,
    pub routing_image_root: Option<PathBuf>,
    pub scratch_path: Option<PathBuf>,
    pub available_destination_bytes: u64,
    pub minimum_headroom_bytes: u64,
    pub verification_limits: VerificationLimits,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestoreReport {
    pub source_file_count: u64,
    pub source_bytes: u64,
    pub source_checksum: u64,
    pub cleared_routing_pointer: bool,
    pub restored_catalog: VerificationReport,
    pub duration: Duration,
}

/// Process-lifetime counters for restores that are intentionally performed
/// without opening a live catalog owner (the engine/admin offline path).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StandaloneRestoreMetrics {
    pub attempts: u64,
    pub failures: u64,
    pub restored_bytes: u64,
    pub total_duration: Duration,
}

#[derive(Default)]
struct StandaloneRestoreCounters {
    attempts: AtomicU64,
    failures: AtomicU64,
    restored_bytes: AtomicU64,
    duration_nanoseconds: AtomicU64,
}

static STANDALONE_RESTORE_COUNTERS: StandaloneRestoreCounters = StandaloneRestoreCounters {
    attempts: AtomicU64::new(0),
    failures: AtomicU64::new(0),
    restored_bytes: AtomicU64::new(0),
    duration_nanoseconds: AtomicU64::new(0),
};

/// Returns aggregate, path-free process metrics for offline restore attempts.
#[must_use]
pub fn standalone_restore_metrics() -> StandaloneRestoreMetrics {
    StandaloneRestoreMetrics {
        attempts: STANDALONE_RESTORE_COUNTERS.attempts.load(Ordering::Relaxed),
        failures: STANDALONE_RESTORE_COUNTERS.failures.load(Ordering::Relaxed),
        restored_bytes: STANDALONE_RESTORE_COUNTERS
            .restored_bytes
            .load(Ordering::Relaxed),
        total_duration: Duration::from_nanos(
            STANDALONE_RESTORE_COUNTERS
                .duration_nanoseconds
                .load(Ordering::Relaxed),
        ),
    }
}

fn atomic_saturating_add(counter: &AtomicU64, value: u64) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
        Some(current.saturating_add(value))
    });
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FlushMode {
    Wal,
    WalAndMemtables,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FlushReport {
    pub mode: FlushMode,
    pub duration: Duration,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompactionFamilyReport {
    pub name: &'static str,
    pub live_sst_bytes_before: MetricValue<u64>,
    pub live_sst_bytes_after: MetricValue<u64>,
    pub total_sst_bytes_before: MetricValue<u64>,
    pub total_sst_bytes_after: MetricValue<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompactionReport {
    pub families: Vec<CompactionFamilyReport>,
    pub duration: Duration,
}

pub struct ReadOnlyCatalog {
    db: DB,
    path: PathBuf,
    metrics: crate::metrics::StoreMetrics,
}

impl ReadOnlyCatalog {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, OperationError> {
        let path = require_existing_catalog_path(path.as_ref())?;
        validate_column_families(&path)?;
        let db = DB::open_cf_descriptors_read_only(
            &options::database_options(false),
            &path,
            options::descriptors(),
            false,
        )?;
        validate_metadata_records(&db)?;
        Ok(Self {
            db,
            path,
            metrics: crate::metrics::StoreMetrics::default(),
        })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn summary(&self) -> Result<CatalogSummary, OperationError> {
        Ok(verify_db(&self.db, VerificationLimits::default())?.summary)
    }

    pub fn verify(&self, limits: VerificationLimits) -> Result<VerificationReport, OperationError> {
        verify_db(&self.db, limits)
    }

    pub fn active_routing_image(&self) -> Result<Option<ActiveRoutingImage>, OperationError> {
        Ok(read_active_routing_image(&self.db)?)
    }

    /// Returns RocksDB properties for this strictly read-only handle. Its
    /// process-local operation counters are zero because no mutations, scans,
    /// or maintenance are performed through this handle.
    pub fn metrics_snapshot(&self) -> Result<StoreMetricsSnapshot, CatalogError> {
        self.metrics.snapshot(&self.db)
    }
}

impl Catalog {
    pub fn verify(&self, limits: VerificationLimits) -> Result<VerificationReport, OperationError> {
        let _guard = self.write_guard()?;
        let result = verify_db(&self.db, limits);
        self.metrics.record_verification(result.is_ok());
        result
    }

    pub fn summary(&self) -> Result<CatalogSummary, OperationError> {
        Ok(self.verify(VerificationLimits::default())?.summary)
    }

    pub fn metrics_snapshot(&self) -> Result<StoreMetricsSnapshot, CatalogError> {
        self.metrics.snapshot(&self.db)
    }

    pub fn flush(&self, mode: FlushMode) -> Result<FlushReport, OperationError> {
        let started = Instant::now();
        let _guard = self.write_guard()?;
        let result = (|| {
            self.sync_wal()?;
            if mode == FlushMode::WalAndMemtables {
                self.flush_memtables()?;
            }
            Ok::<_, OperationError>(())
        })();
        self.metrics.record_flush(result.is_ok());
        result?;
        Ok(FlushReport {
            mode,
            duration: started.elapsed(),
        })
    }

    /// Flushes authoritative state, waits for RocksDB background work, and
    /// leaves actual handle release to the owner dropping this `Catalog`.
    pub fn prepare_for_close(&self) -> Result<FlushReport, OperationError> {
        let report = self.flush(FlushMode::WalAndMemtables)?;
        self.db.cancel_all_background_work(true);
        Ok(report)
    }

    /// Flushes and compacts the complete, fixed set of catalog column
    /// families. The bounded scope deliberately does not expose raw RocksDB
    /// range or column-family controls.
    pub fn compact_all(&self) -> Result<CompactionReport, OperationError> {
        let started = Instant::now();
        let result = (|| {
            let _guard = self.write_guard()?;
            self.sync_wal()?;
            let mut flush_options = FlushOptions::default();
            flush_options.set_wait(true);
            self.db.flush_opt(&flush_options)?;
            for name in column_families::ALL {
                let family =
                    self.db
                        .cf_handle(name)
                        .ok_or_else(|| CatalogError::CorruptRecord {
                            key_space: "database",
                            record_id: name.to_owned(),
                            reason: "required column family is missing".to_owned(),
                        })?;
                self.db.flush_cf_opt(family, &flush_options)?;
            }

            let before = self.metrics.snapshot(&self.db)?;
            self.db.compact_range(None::<&[u8]>, None::<&[u8]>);
            for name in column_families::ALL {
                let family =
                    self.db
                        .cf_handle(name)
                        .ok_or_else(|| CatalogError::CorruptRecord {
                            key_space: "database",
                            record_id: name.to_owned(),
                            reason: "required column family is missing".to_owned(),
                        })?;
                self.db
                    .compact_range_cf(family, None::<&[u8]>, None::<&[u8]>);
            }
            let after = self.metrics.snapshot(&self.db)?;
            let background_failure = after.column_families.iter().any(|family| {
                matches!(family.background_errors, MetricValue::Available(value) if value > 0)
                    || matches!(family.write_stopped, MetricValue::Available(value) if value > 0)
            });
            // RocksDB's manual-compaction API does not return the background
            // compaction status. A synchronous WAL/memtable durability probe
            // supplies narrow storage-capacity evidence before classification.
            let capacity_observation = self.sync_wal().and_then(|_| self.flush_memtables());
            classify_compaction_observation(background_failure, capacity_observation)?;
            let families = before
                .column_families
                .iter()
                .zip(after.column_families.iter())
                .map(|(before, after)| CompactionFamilyReport {
                    name: before.name,
                    live_sst_bytes_before: before.live_sst_bytes.clone(),
                    live_sst_bytes_after: after.live_sst_bytes.clone(),
                    total_sst_bytes_before: before.total_sst_bytes.clone(),
                    total_sst_bytes_after: after.total_sst_bytes.clone(),
                })
                .collect();
            Ok(CompactionReport {
                families,
                duration: started.elapsed(),
            })
        })();
        self.metrics
            .record_compaction(result.is_ok(), started.elapsed());
        result
    }

    pub fn create_checkpoint(
        &self,
        request: &CheckpointRequest,
    ) -> Result<CheckpointReport, OperationError> {
        let maximum = self.maximum_concurrent_checkpoints;
        let started = Instant::now();
        let _permit = match CheckpointPermit::acquire(&self.active_checkpoints, maximum) {
            Ok(permit) => permit,
            Err(error) => {
                self.metrics.record_checkpoint(false, 0, started.elapsed());
                return Err(error);
            }
        };
        let result = self.create_checkpoint_inner(request);
        let bytes = result.as_ref().map_or(0, |report| report.bytes);
        self.metrics
            .record_checkpoint(result.is_ok(), bytes, started.elapsed());
        result
    }

    fn create_checkpoint_inner(
        &self,
        request: &CheckpointRequest,
    ) -> Result<CheckpointReport, OperationError> {
        let database_root = self
            .path
            .parent()
            .ok_or_else(|| OperationError::InvalidPath {
                resource: ResourceKind::Database,
                reason: "database has no parent".to_owned(),
            })?;
        let resolved = validate_operational_paths(&OperationalPathRequest {
            database: Some(PathTarget {
                root: database_root,
                path: &self.path,
                must_exist: true,
                must_be_fresh: false,
            }),
            routing_images: request.routing_image_root.as_ref().map(|path| PathTarget {
                root: path.parent().unwrap_or(path),
                path,
                must_exist: false,
                must_be_fresh: false,
            }),
            checkpoint: Some(PathTarget {
                root: &request.destination_root,
                path: &request.destination,
                must_exist: false,
                must_be_fresh: true,
            }),
            scratch: request.scratch_path.as_ref().map(|path| PathTarget {
                root: path.parent().unwrap_or(path),
                path,
                must_exist: false,
                must_be_fresh: true,
            }),
            restore: None,
        })?;
        let destination = resolved
            .get(ResourceKind::Checkpoint)
            .expect("checkpoint target was supplied");
        if destination.exists() {
            return Err(OperationError::DestinationAlreadyExists {
                resource: ResourceKind::Checkpoint,
            });
        }
        // Hold the same lock as every catalog mutation while estimating the
        // source and producing the checkpoint so admission and contents refer
        // to one stable catalog state.
        let _guard = self.write_guard()?;
        let database_bytes = directory_size(&self.path)?;
        let required = database_bytes
            .checked_add(request.minimum_headroom_bytes)
            .ok_or(OperationError::ResourceRefused {
                resource: "checkpoint disk",
                required: u64::MAX,
                available: request.available_destination_bytes,
            })?;
        if required > request.available_destination_bytes {
            return Err(OperationError::ResourceRefused {
                resource: "checkpoint disk",
                required,
                available: request.available_destination_bytes,
            });
        }
        let started = Instant::now();
        self.sync_wal()?;
        self.flush_memtables()?;
        let catalog = verify_db(&self.db, VerificationLimits::default())?;
        Checkpoint::new(&self.db)?.create_checkpoint(destination)?;
        let (file_count, bytes, content_checksum) = directory_fingerprint(destination)?;
        Ok(CheckpointReport {
            catalog,
            file_count,
            bytes,
            content_checksum,
            duration: started.elapsed(),
        })
    }

    fn sync_wal(&self) -> Result<(), OperationError> {
        let result = self.db.flush_wal(true).map_err(OperationError::from);
        self.metrics.record_wal_sync(result.is_ok());
        result
    }

    fn flush_memtables(&self) -> Result<(), OperationError> {
        let mut options = FlushOptions::default();
        options.set_wait(true);
        self.db.flush_opt(&options)?;
        for name in column_families::ALL {
            let family = self
                .db
                .cf_handle(name)
                .ok_or_else(|| CatalogError::CorruptRecord {
                    key_space: "database",
                    record_id: name.to_owned(),
                    reason: "required column family is missing".to_owned(),
                })?;
            self.db.flush_cf_opt(family, &options)?;
        }
        Ok(())
    }

    pub fn restore_checkpoint(
        &self,
        request: &RestoreRequest,
    ) -> Result<RestoreReport, OperationError> {
        let started = Instant::now();
        let result = restore_checkpoint_with_live_database(Some(&self.path), request);
        let bytes = result.as_ref().map_or(0, |report| report.source_bytes);
        self.metrics
            .record_restore(result.is_ok(), bytes, started.elapsed());
        result
    }
}

fn classify_compaction_observation(
    background_failure: bool,
    capacity_observation: Result<(), OperationError>,
) -> Result<(), OperationError> {
    match capacity_observation {
        Err(OperationError::StorageExhausted { .. }) => Err(OperationError::StorageExhausted {
            operation: "fixed-scope compaction",
        }),
        Err(error) => Err(error),
        Ok(()) if background_failure => Err(OperationError::BackgroundFailure {
            operation: "fixed-scope compaction",
        }),
        Ok(()) => Ok(()),
    }
}

pub fn restore_checkpoint(request: &RestoreRequest) -> Result<RestoreReport, OperationError> {
    atomic_saturating_add(&STANDALONE_RESTORE_COUNTERS.attempts, 1);
    let started = Instant::now();
    let result = restore_checkpoint_with_live_database(None, request);
    let elapsed = started.elapsed();
    atomic_saturating_add(
        &STANDALONE_RESTORE_COUNTERS.duration_nanoseconds,
        u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX),
    );
    match &result {
        Ok(report) => {
            atomic_saturating_add(
                &STANDALONE_RESTORE_COUNTERS.restored_bytes,
                report.source_bytes,
            );
        }
        Err(_) => {
            atomic_saturating_add(&STANDALONE_RESTORE_COUNTERS.failures, 1);
        }
    }
    result
}

fn restore_checkpoint_with_live_database(
    live_database: Option<&Path>,
    request: &RestoreRequest,
) -> Result<RestoreReport, OperationError> {
    let started = Instant::now();
    let database_target = live_database.map(|path| PathTarget {
        root: path.parent().unwrap_or(path),
        path,
        must_exist: true,
        must_be_fresh: false,
    });
    let resolved = validate_operational_paths(&OperationalPathRequest {
        database: database_target,
        routing_images: request.routing_image_root.as_ref().map(|path| PathTarget {
            root: path.parent().unwrap_or(path),
            path,
            must_exist: false,
            must_be_fresh: false,
        }),
        checkpoint: Some(PathTarget {
            root: &request.source_root,
            path: &request.source_checkpoint,
            must_exist: true,
            must_be_fresh: false,
        }),
        restore: Some(PathTarget {
            root: &request.destination_root,
            path: &request.destination,
            must_exist: false,
            must_be_fresh: true,
        }),
        scratch: request.scratch_path.as_ref().map(|path| PathTarget {
            root: path.parent().unwrap_or(path),
            path,
            must_exist: false,
            must_be_fresh: true,
        }),
    })?;
    let source = resolved
        .get(ResourceKind::Checkpoint)
        .expect("checkpoint source was supplied");
    let destination = resolved
        .get(ResourceKind::Restore)
        .expect("restore target was supplied");
    let (source_file_count, source_bytes, source_checksum) = directory_fingerprint(source)?;
    let required = source_bytes
        .checked_add(request.minimum_headroom_bytes)
        .ok_or(OperationError::ResourceRefused {
            resource: "restore disk",
            required: u64::MAX,
            available: request.available_destination_bytes,
        })?;
    if required > request.available_destination_bytes {
        return Err(OperationError::ResourceRefused {
            resource: "restore disk",
            required,
            available: request.available_destination_bytes,
        });
    }
    copy_directory(source, destination)?;
    let restored = (|| {
        let catalog = ReadOnlyCatalog::open(destination)?;
        let cleared_routing_pointer = catalog.active_routing_image()?.is_some();
        drop(catalog);
        if cleared_routing_pointer {
            clear_verified_routing_pointer(destination)?;
        }
        let catalog = ReadOnlyCatalog::open(destination)?;
        let report = catalog.verify(request.verification_limits)?;
        Ok::<_, OperationError>((cleared_routing_pointer, report))
    })();
    match restored {
        Ok((cleared_routing_pointer, restored_catalog)) => Ok(RestoreReport {
            source_file_count,
            source_bytes,
            source_checksum,
            cleared_routing_pointer,
            restored_catalog,
            duration: started.elapsed(),
        }),
        Err(error) => Err(error),
    }
}

fn clear_verified_routing_pointer(path: &Path) -> Result<(), OperationError> {
    let db = DB::open_cf_descriptors(
        &options::database_options(false),
        path,
        options::descriptors(),
    )?;
    db.delete_opt(META_ACTIVE_ROUTING_IMAGE, &options::write_options())?;
    db.flush_wal(true)?;
    let mut flush_options = FlushOptions::default();
    flush_options.set_wait(true);
    db.flush_opt(&flush_options)?;
    Ok(())
}

fn verify_db(db: &DB, limits: VerificationLimits) -> Result<VerificationReport, OperationError> {
    let started = Instant::now();
    validate_metadata_records(db)?;
    let mut records = 0_u64;
    let mut decoded_bytes = 0_u64;
    let mut checksum = FNV_OFFSET;
    let mut summary = CatalogSummary::default();
    for name in std::iter::once("default").chain(column_families::ALL) {
        let family = db
            .cf_handle(name)
            .ok_or_else(|| CatalogError::CorruptRecord {
                key_space: "database",
                record_id: name.to_owned(),
                reason: "required column family is missing".to_owned(),
            })?;
        for entry in db.iterator_cf(family, IteratorMode::Start) {
            if limits
                .maximum_duration
                .is_some_and(|maximum| started.elapsed() > maximum)
            {
                return Err(OperationError::VerificationLimit {
                    resource: "duration (nanoseconds)",
                    maximum: limits
                        .maximum_duration
                        .unwrap_or_default()
                        .as_nanos()
                        .try_into()
                        .unwrap_or(u64::MAX),
                });
            }
            let (key, value) = entry?;
            records = records.saturating_add(1);
            if limits
                .maximum_records
                .is_some_and(|maximum| records > maximum)
            {
                return Err(OperationError::VerificationLimit {
                    resource: "record count",
                    maximum: limits.maximum_records.unwrap_or_default(),
                });
            }
            decoded_bytes = decoded_bytes
                .saturating_add(u64::try_from(key.len() + value.len()).unwrap_or(u64::MAX));
            checksum = fnv(checksum, name.as_bytes());
            checksum = fnv(checksum, &key);
            checksum = fnv(checksum, &value);
            match name {
                column_families::CANDIDATES => {
                    let id = decode_id_key(&key).map_err(|error| CatalogError::CorruptRecord {
                        key_space: column_families::CANDIDATES,
                        record_id: "key".to_owned(),
                        reason: error.to_string(),
                    })?;
                    match decode_candidate(&value, id).map_err(|error| {
                        CatalogError::CorruptRecord {
                            key_space: column_families::CANDIDATES,
                            record_id: id.to_string(),
                            reason: error.to_string(),
                        }
                    })? {
                        Candidate::Node { .. } => {
                            summary.candidates.nodes = summary.candidates.nodes.saturating_add(1)
                        }
                        Candidate::Relation { .. } => {
                            summary.candidates.relation_kinds =
                                summary.candidates.relation_kinds.saturating_add(1)
                        }
                        Candidate::Edge { .. } => {
                            summary.candidates.edges = summary.candidates.edges.saturating_add(1)
                        }
                    }
                }
                column_families::NODES => {
                    summary.confirmed_nodes = summary.confirmed_nodes.saturating_add(1)
                }
                column_families::RELATION_KINDS => {
                    summary.relation_kinds = summary.relation_kinds.saturating_add(1)
                }
                column_families::EDGES => {
                    summary.confirmed_edges = summary.confirmed_edges.saturating_add(1)
                }
                column_families::NODE_NAMES => {
                    summary.node_name_entries = summary.node_name_entries.saturating_add(1)
                }
                column_families::RELATION_NAMES => {
                    summary.relation_name_entries = summary.relation_name_entries.saturating_add(1)
                }
                column_families::OUTGOING_EDGES => {
                    summary.outgoing_entries = summary.outgoing_entries.saturating_add(1)
                }
                column_families::INCOMING_EDGES => {
                    summary.incoming_entries = summary.incoming_entries.saturating_add(1)
                }
                column_families::RELATION_POPULARITY => {}
                "default" => {}
                _ => unreachable!(),
            }
        }
    }
    let mut remaining_work = limits
        .maximum_records
        .map(|maximum| maximum.saturating_sub(records));
    let mut work_limit_reached = false;
    let mut check = || {
        if limits
            .maximum_duration
            .is_some_and(|maximum| started.elapsed() > maximum)
        {
            Err(CatalogError::ValidationAborted)
        } else if let Some(remaining) = &mut remaining_work {
            if *remaining == 0 {
                work_limit_reached = true;
                Err(CatalogError::ValidationAborted)
            } else {
                *remaining -= 1;
                Ok(())
            }
        } else {
            Ok(())
        }
    };
    if let Err(error) = rebuild_indexes_and_validate_with(db, &mut check) {
        if matches!(error, CatalogError::ValidationAborted) {
            if work_limit_reached {
                return Err(OperationError::VerificationLimit {
                    resource: "record work units",
                    maximum: limits.maximum_records.unwrap_or_default(),
                });
            }
            return Err(verification_duration_limit(limits));
        }
        return Err(error.into());
    }
    summary.routing_pointer_present = read_active_routing_image(db)?.is_some();
    let duration = started.elapsed();
    if limits
        .maximum_duration
        .is_some_and(|maximum| duration > maximum)
    {
        return Err(verification_duration_limit(limits));
    }
    Ok(VerificationReport {
        summary,
        records_examined: records,
        decoded_bytes,
        catalog_checksum: checksum,
        duration,
    })
}

fn verification_duration_limit(limits: VerificationLimits) -> OperationError {
    OperationError::VerificationLimit {
        resource: "duration (nanoseconds)",
        maximum: limits
            .maximum_duration
            .unwrap_or_default()
            .as_nanos()
            .try_into()
            .unwrap_or(u64::MAX),
    }
}

pub(crate) fn require_existing_catalog_path(path: &Path) -> Result<PathBuf, OperationError> {
    if !path.is_dir() {
        return Err(OperationError::InvalidPath {
            resource: ResourceKind::Database,
            reason: "catalog directory does not exist".to_owned(),
        });
    }
    let path = normalize_canonical(fs::canonicalize(path).map_err(|error| OperationError::Io {
        operation: "canonicalize catalog path",
        reason: error.to_string(),
    })?);
    if path.parent().is_none() {
        return Err(OperationError::InvalidPath {
            resource: ResourceKind::Database,
            reason: "catalog must not be a filesystem root".to_owned(),
        });
    }
    Ok(path)
}

pub(crate) fn validate_column_families(path: &Path) -> Result<(), OperationError> {
    let found: BTreeSet<_> = DB::list_cf(&options::database_options(false), path)?
        .into_iter()
        .collect();
    let expected: BTreeSet<_> = std::iter::once("default".to_owned())
        .chain(column_families::ALL.map(str::to_owned))
        .collect();
    if found != expected {
        return Err(OperationError::Catalog(CatalogError::CorruptRecord {
            key_space: "database",
            record_id: "column families".to_owned(),
            reason: "database does not contain exactly the current column-family set".to_owned(),
        }));
    }
    Ok(())
}

fn copy_directory(source: &Path, destination: &Path) -> Result<(), OperationError> {
    if destination.exists() {
        if fs::read_dir(destination)
            .map_err(io("read restore destination"))?
            .next()
            .transpose()
            .map_err(io("read restore destination"))?
            .is_some()
        {
            return Err(OperationError::DestinationNotEmpty {
                resource: ResourceKind::Restore,
            });
        }
    } else {
        fs::create_dir(destination).map_err(io("create restore destination"))?;
    }
    copy_children(source, destination)
}

fn copy_children(source: &Path, destination: &Path) -> Result<(), OperationError> {
    let mut entries = fs::read_dir(source)
        .map_err(io("read checkpoint directory"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(io("read checkpoint directory"))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let file_type = entry.file_type().map_err(io("inspect checkpoint entry"))?;
        let target = destination.join(entry.file_name());
        if file_type.is_dir() {
            fs::create_dir(&target).map_err(io("create restore subdirectory"))?;
            copy_children(&entry.path(), &target)?;
        } else if file_type.is_file() {
            copy_file(&entry.path(), &target)?;
        } else {
            return Err(OperationError::Io {
                operation: "copy checkpoint",
                reason: "checkpoint contains a non-file, non-directory entry".to_owned(),
            });
        }
    }
    Ok(())
}

fn copy_file(source: &Path, destination: &Path) -> Result<(), OperationError> {
    let mut input = File::open(source).map_err(io("open checkpoint file"))?;
    let mut output = File::create_new(destination).map_err(io("create restore file"))?;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = input
            .read(&mut buffer)
            .map_err(io("read checkpoint file"))?;
        if count == 0 {
            break;
        }
        output
            .write_all(&buffer[..count])
            .map_err(io("write restore file"))?;
    }
    output.sync_all().map_err(io("sync restore file"))
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn fnv(mut state: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        state ^= u64::from(*byte);
        state = state.wrapping_mul(FNV_PRIME);
    }
    state
}

fn directory_fingerprint(path: &Path) -> Result<(u64, u64, u64), OperationError> {
    let mut files = Vec::new();
    collect_files(path, path, &mut files)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut count = 0_u64;
    let mut bytes = 0_u64;
    let mut checksum = FNV_OFFSET;
    let mut buffer = [0_u8; 64 * 1024];
    for (relative, file_path) in files {
        checksum = fnv(checksum, relative.as_os_str().to_string_lossy().as_bytes());
        let mut file = File::open(file_path).map_err(io("open file for checksum"))?;
        loop {
            let read = file
                .read(&mut buffer)
                .map_err(io("read file for checksum"))?;
            if read == 0 {
                break;
            }
            bytes = bytes.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
            checksum = fnv(checksum, &buffer[..read]);
        }
        count = count.saturating_add(1);
    }
    Ok((count, bytes, checksum))
}

fn directory_size(path: &Path) -> Result<u64, OperationError> {
    let mut total = 0_u64;
    for entry in fs::read_dir(path).map_err(io("read directory for size"))? {
        let entry = entry.map_err(io("read directory for size"))?;
        let metadata = entry.metadata().map_err(io("inspect directory for size"))?;
        if metadata.is_dir() {
            total = total.saturating_add(directory_size(&entry.path())?);
        } else if metadata.is_file() {
            total = total.saturating_add(metadata.len());
        }
    }
    Ok(total)
}

fn collect_files(
    root: &Path,
    directory: &Path,
    files: &mut Vec<(PathBuf, PathBuf)>,
) -> Result<(), OperationError> {
    for entry in fs::read_dir(directory).map_err(io("read directory for checksum"))? {
        let entry = entry.map_err(io("read directory for checksum"))?;
        let file_type = entry.file_type().map_err(io("inspect directory entry"))?;
        if file_type.is_dir() {
            collect_files(root, &entry.path(), files)?;
        } else if file_type.is_file() {
            files.push((
                entry
                    .path()
                    .strip_prefix(root)
                    .map_err(|error| OperationError::Io {
                        operation: "resolve checksum path",
                        reason: error.to_string(),
                    })?
                    .to_path_buf(),
                entry.path(),
            ));
        } else {
            return Err(OperationError::Io {
                operation: "checksum directory",
                reason: "directory contains a non-file, non-directory entry".to_owned(),
            });
        }
    }
    Ok(())
}

fn io(operation: &'static str) -> impl FnOnce(std::io::Error) -> OperationError {
    move |error| {
        if error.kind() == std::io::ErrorKind::StorageFull {
            OperationError::StorageExhausted { operation }
        } else {
            OperationError::Io {
                operation,
                reason: error.to_string(),
            }
        }
    }
}

struct CheckpointPermit<'a> {
    active: &'a AtomicUsize,
}

impl<'a> CheckpointPermit<'a> {
    fn acquire(active: &'a AtomicUsize, maximum: usize) -> Result<Self, OperationError> {
        loop {
            let current = active.load(Ordering::Acquire);
            if current >= maximum {
                return Err(OperationError::ConcurrentCheckpoint { maximum });
            }
            if active
                .compare_exchange_weak(current, current + 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return Ok(Self { active });
            }
        }
    }
}

impl Drop for CheckpointPermit<'_> {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkpoint_permit_refuses_configured_concurrency_and_releases() {
        let active = AtomicUsize::new(0);
        let first = CheckpointPermit::acquire(&active, 1).unwrap();
        assert!(matches!(
            CheckpointPermit::acquire(&active, 1),
            Err(OperationError::ConcurrentCheckpoint { maximum: 1 })
        ));
        drop(first);
        assert!(CheckpointPermit::acquire(&active, 1).is_ok());
    }

    #[test]
    fn storage_full_io_is_typed() {
        let error = io("restore copy")(std::io::Error::from(std::io::ErrorKind::StorageFull));
        assert!(matches!(
            error,
            OperationError::StorageExhausted {
                operation: "restore copy"
            }
        ));
    }

    #[test]
    fn compaction_capacity_observation_classifies_storage_exhaustion() {
        let error = classify_compaction_observation(
            true,
            Err(OperationError::StorageExhausted {
                operation: "durability probe",
            }),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            OperationError::StorageExhausted {
                operation: "fixed-scope compaction"
            }
        ));
        assert!(matches!(
            classify_compaction_observation(true, Ok(())),
            Err(OperationError::BackgroundFailure {
                operation: "fixed-scope compaction"
            })
        ));
    }
}
