use std::{collections::BTreeMap, sync::Mutex, time::Duration};

use rocksdb::{DB, properties};

use crate::{CatalogError, column_families};

/// A metric that the linked RocksDB build may not expose.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MetricValue<T> {
    Available(T),
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum WriteOperationClass {
    CandidateInsertion,
    ConfirmedPromotion,
    EdgeDeletion,
    NodeDeletion,
    RoutingPointer,
    Maintenance,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WriteMetrics {
    pub attempts: u64,
    pub failures: u64,
    pub committed_entries: u64,
    pub committed_bytes: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ScanMetrics {
    pub completed_scans: u64,
    pub failures: u64,
    pub records: u64,
    pub decoded_bytes: u64,
    pub total_duration: Duration,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MaintenanceMetrics {
    pub wal_sync_attempts: u64,
    pub wal_sync_failures: u64,
    pub checkpoint_attempts: u64,
    pub checkpoint_failures: u64,
    pub checkpoint_bytes: u64,
    pub checkpoint_duration: Duration,
    pub restore_attempts: u64,
    pub restore_failures: u64,
    pub restore_bytes: u64,
    pub restore_duration: Duration,
    pub flush_attempts: u64,
    pub flush_failures: u64,
    pub compaction_attempts: u64,
    pub compaction_failures: u64,
    pub compaction_duration: Duration,
    pub last_verification_succeeded: Option<bool>,
    pub last_maintenance_succeeded: Option<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ColumnFamilyMetrics {
    pub name: &'static str,
    pub estimated_key_count: MetricValue<u64>,
    pub estimated_live_data_bytes: MetricValue<u64>,
    pub live_sst_bytes: MetricValue<u64>,
    pub total_sst_bytes: MetricValue<u64>,
    pub active_memtable_bytes: MetricValue<u64>,
    pub all_memtable_bytes: MetricValue<u64>,
    pub pending_compaction_bytes: MetricValue<u64>,
    pub immutable_memtables: MetricValue<u64>,
    pub pending_flush: MetricValue<u64>,
    pub running_flushes: MetricValue<u64>,
    pub pending_compaction: MetricValue<u64>,
    pub running_compactions: MetricValue<u64>,
    pub background_errors: MetricValue<u64>,
    pub write_stopped: MetricValue<u64>,
    pub actual_delayed_write_rate: MetricValue<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoreMetricsSnapshot {
    pub wal_enabled: bool,
    pub ordinary_writes_sync: bool,
    pub writes: BTreeMap<WriteOperationClass, WriteMetrics>,
    pub scans: ScanMetrics,
    pub maintenance: MaintenanceMetrics,
    pub standalone_restore: crate::StandaloneRestoreMetrics,
    pub column_families: Vec<ColumnFamilyMetrics>,
    pub block_cache_capacity_bytes: MetricValue<u64>,
    pub block_cache_usage_bytes: MetricValue<u64>,
    pub block_cache_hits: MetricValue<u64>,
    pub block_cache_misses: MetricValue<u64>,
}

#[derive(Default)]
struct ProcessMetrics {
    writes: BTreeMap<WriteOperationClass, WriteMetrics>,
    scans: ScanMetrics,
    maintenance: MaintenanceMetrics,
}

#[derive(Default)]
pub(crate) struct StoreMetrics {
    state: Mutex<ProcessMetrics>,
}

impl StoreMetrics {
    pub(crate) fn record_wal_sync(&self, succeeded: bool) {
        if let Ok(mut state) = self.state.lock() {
            let maintenance = &mut state.maintenance;
            maintenance.wal_sync_attempts = maintenance.wal_sync_attempts.saturating_add(1);
            maintenance.wal_sync_failures = maintenance
                .wal_sync_failures
                .saturating_add(u64::from(!succeeded));
        }
    }

    pub(crate) fn record_write_attempt(&self, class: WriteOperationClass) {
        if let Ok(mut state) = self.state.lock() {
            let value = state.writes.entry(class).or_default();
            value.attempts = value.attempts.saturating_add(1);
        }
    }

    pub(crate) fn record_write_success(
        &self,
        class: WriteOperationClass,
        entries: usize,
        bytes: usize,
    ) {
        if let Ok(mut state) = self.state.lock() {
            let value = state.writes.entry(class).or_default();
            value.committed_entries = value
                .committed_entries
                .saturating_add(u64::try_from(entries).unwrap_or(u64::MAX));
            value.committed_bytes = value
                .committed_bytes
                .saturating_add(u64::try_from(bytes).unwrap_or(u64::MAX));
        }
    }

    pub(crate) fn record_write_failure(&self, class: WriteOperationClass) {
        if let Ok(mut state) = self.state.lock() {
            let value = state.writes.entry(class).or_default();
            value.failures = value.failures.saturating_add(1);
        }
    }

    pub(crate) fn record_scan(
        &self,
        succeeded: bool,
        records: u64,
        decoded_bytes: u64,
        duration: Duration,
    ) {
        if let Ok(mut state) = self.state.lock() {
            let scans = &mut state.scans;
            scans.completed_scans = scans.completed_scans.saturating_add(u64::from(succeeded));
            scans.failures = scans.failures.saturating_add(u64::from(!succeeded));
            scans.records = scans.records.saturating_add(records);
            scans.decoded_bytes = scans.decoded_bytes.saturating_add(decoded_bytes);
            scans.total_duration = scans.total_duration.saturating_add(duration);
        }
    }

    pub(crate) fn record_checkpoint(&self, succeeded: bool, bytes: u64, duration: Duration) {
        if let Ok(mut state) = self.state.lock() {
            let maintenance = &mut state.maintenance;
            maintenance.checkpoint_attempts = maintenance.checkpoint_attempts.saturating_add(1);
            maintenance.checkpoint_failures = maintenance
                .checkpoint_failures
                .saturating_add(u64::from(!succeeded));
            if succeeded {
                maintenance.checkpoint_bytes = maintenance.checkpoint_bytes.saturating_add(bytes);
            }
            maintenance.checkpoint_duration =
                maintenance.checkpoint_duration.saturating_add(duration);
            maintenance.last_maintenance_succeeded = Some(succeeded);
        }
    }

    pub(crate) fn record_restore(&self, succeeded: bool, bytes: u64, duration: Duration) {
        if let Ok(mut state) = self.state.lock() {
            let maintenance = &mut state.maintenance;
            maintenance.restore_attempts = maintenance.restore_attempts.saturating_add(1);
            maintenance.restore_failures = maintenance
                .restore_failures
                .saturating_add(u64::from(!succeeded));
            if succeeded {
                maintenance.restore_bytes = maintenance.restore_bytes.saturating_add(bytes);
            }
            maintenance.restore_duration = maintenance.restore_duration.saturating_add(duration);
            maintenance.last_maintenance_succeeded = Some(succeeded);
        }
    }

    pub(crate) fn record_flush(&self, succeeded: bool) {
        if let Ok(mut state) = self.state.lock() {
            let maintenance = &mut state.maintenance;
            maintenance.flush_attempts = maintenance.flush_attempts.saturating_add(1);
            maintenance.flush_failures = maintenance
                .flush_failures
                .saturating_add(u64::from(!succeeded));
            maintenance.last_maintenance_succeeded = Some(succeeded);
        }
    }

    pub(crate) fn record_compaction(&self, succeeded: bool, duration: Duration) {
        if let Ok(mut state) = self.state.lock() {
            let maintenance = &mut state.maintenance;
            maintenance.compaction_attempts = maintenance.compaction_attempts.saturating_add(1);
            maintenance.compaction_failures = maintenance
                .compaction_failures
                .saturating_add(u64::from(!succeeded));
            maintenance.compaction_duration =
                maintenance.compaction_duration.saturating_add(duration);
            maintenance.last_maintenance_succeeded = Some(succeeded);
        }
    }

    pub(crate) fn record_verification(&self, succeeded: bool) {
        if let Ok(mut state) = self.state.lock() {
            state.maintenance.last_verification_succeeded = Some(succeeded);
        }
    }

    pub(crate) fn snapshot(&self, db: &DB) -> Result<StoreMetricsSnapshot, CatalogError> {
        let state = self.state.lock().map_err(|_| CatalogError::LockPoisoned {
            lock: "store metrics",
        })?;
        let mut families = Vec::with_capacity(column_families::ALL.len() + 1);
        for name in std::iter::once("default").chain(column_families::ALL) {
            let family = db
                .cf_handle(name)
                .ok_or_else(|| CatalogError::CorruptRecord {
                    key_space: "database",
                    record_id: name.to_owned(),
                    reason: "required column family is missing".to_owned(),
                })?;
            let property = |name| match db.property_int_value_cf(family, name) {
                Ok(Some(value)) => MetricValue::Available(value),
                Ok(None) | Err(_) => MetricValue::Unavailable,
            };
            families.push(ColumnFamilyMetrics {
                name,
                estimated_key_count: property(properties::ESTIMATE_NUM_KEYS),
                estimated_live_data_bytes: property(properties::ESTIMATE_LIVE_DATA_SIZE),
                live_sst_bytes: property(properties::LIVE_SST_FILES_SIZE),
                total_sst_bytes: property(properties::TOTAL_SST_FILES_SIZE),
                active_memtable_bytes: property(properties::CUR_SIZE_ACTIVE_MEM_TABLE),
                all_memtable_bytes: property(properties::SIZE_ALL_MEM_TABLES),
                pending_compaction_bytes: property(properties::ESTIMATE_PENDING_COMPACTION_BYTES),
                immutable_memtables: property(properties::NUM_IMMUTABLE_MEM_TABLE),
                pending_flush: property(properties::MEM_TABLE_FLUSH_PENDING),
                running_flushes: property(properties::NUM_RUNNING_FLUSHES),
                pending_compaction: property(properties::COMPACTION_PENDING),
                running_compactions: property(properties::NUM_RUNNING_COMPACTIONS),
                background_errors: property(properties::BACKGROUND_ERRORS),
                write_stopped: property(properties::IS_WRITE_STOPPED),
                actual_delayed_write_rate: property(properties::ACTUAL_DELAYED_WRITE_RATE),
            });
        }
        let db_property = |name| match db.property_int_value(name) {
            Ok(Some(value)) => MetricValue::Available(value),
            Ok(None) | Err(_) => MetricValue::Unavailable,
        };
        Ok(StoreMetricsSnapshot {
            wal_enabled: true,
            ordinary_writes_sync: false,
            writes: state.writes.clone(),
            scans: state.scans,
            maintenance: state.maintenance,
            standalone_restore: crate::standalone_restore_metrics(),
            column_families: families,
            block_cache_capacity_bytes: db_property(properties::BLOCK_CACHE_CAPACITY),
            block_cache_usage_bytes: db_property(properties::BLOCK_CACHE_USAGE),
            // The selected RocksDB options do not enable global ticker statistics.
            // Unsupported counters are deliberately unavailable rather than zero.
            block_cache_hits: MetricValue::Unavailable,
            block_cache_misses: MetricValue::Unavailable,
        })
    }
}
