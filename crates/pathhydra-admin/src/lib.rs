//! Safe, finite administration commands for a local PathHydra catalog.
//!
//! Inspection commands use [`pathhydra_store::ReadOnlyCatalog`]. Commands that
//! create a checkpoint, validate a restore, or generate a benchmark database
//! are named explicitly and refuse implicit destinations.

mod output;
mod workload;

use std::{
    ffi::OsString,
    fmt,
    path::{Component, Path, PathBuf},
    time::Duration,
};

use output::{JsonObject, checksum_hex, metric_json, summary_json};
use pathhydra_engine::{EngineConfig, EngineRestoreRequest, GraphEngine, RoutingHealth};
use pathhydra_routing::open_bundle;
use pathhydra_store::{
    Catalog, CheckpointRequest, ReadOnlyCatalog, RestoreRequest, VerificationLimits,
};

/// Runs one finite administration command and returns one JSON document.
pub fn run<I, S>(arguments: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let mut arguments = Arguments::new(arguments);
    let command = arguments.next_command()?;
    let result = match command.as_str() {
        "summary" => summary(&mut arguments),
        "verify" => verify(&mut arguments),
        "active-pointer" => active_pointer(&mut arguments),
        "candidate-counts" => candidate_counts(&mut arguments),
        "checkpoint-create" => checkpoint_create(&mut arguments),
        "restore-validate" => restore_validate(&mut arguments),
        "metrics-snapshot" => metrics_snapshot(&mut arguments),
        "engine-health" => engine_health(&mut arguments),
        "reconcile-routing-root-dry-run" => reconcile_routing_root(&mut arguments),
        "workload" => workload::run(&mut arguments),
        "help" | "--help" | "-h" => Ok(help_json()),
        _ => Err(CliError::Usage("unknown command")),
    }?;
    arguments.finish()?;
    Ok(result)
}

fn summary(arguments: &mut Arguments) -> Result<String, CliError> {
    let database = arguments.required_path("--database")?;
    let catalog = ReadOnlyCatalog::open(&database).map_err(CliError::operation)?;
    let resolved_database = catalog.path().display().to_string();
    let summary = catalog.summary().map_err(CliError::operation)?;
    Ok(JsonObject::new("summary")
        .string("resolved_database_path", &resolved_database)
        .raw_field("catalog", summary_json(&summary))
        .finish())
}

fn verify(arguments: &mut Arguments) -> Result<String, CliError> {
    let database = arguments.required_path("--database")?;
    let maximum_records = arguments.optional_u64("--max-records")?;
    let maximum_duration = arguments
        .optional_u64("--max-duration-ms")?
        .map(Duration::from_millis);
    let catalog = ReadOnlyCatalog::open(&database).map_err(CliError::operation)?;
    let report = catalog
        .verify(VerificationLimits {
            maximum_records,
            maximum_duration,
        })
        .map_err(CliError::operation)?;
    Ok(JsonObject::new("verify")
        .raw_field("catalog", summary_json(&report.summary))
        .number("records_examined", report.records_examined)
        .number("decoded_bytes", report.decoded_bytes)
        .string(
            "catalog_checksum",
            &format!("{:016x}", report.catalog_checksum),
        )
        .duration("duration", report.duration)
        .finish())
}

fn active_pointer(arguments: &mut Arguments) -> Result<String, CliError> {
    let database = arguments.required_path("--database")?;
    let routing_root = arguments.optional_path("--routing-root")?;
    let catalog = ReadOnlyCatalog::open(&database).map_err(CliError::operation)?;
    let pointer = catalog
        .active_routing_image()
        .map_err(CliError::operation)?;
    let mut output = JsonObject::new("active-pointer");
    let Some(pointer) = pointer else {
        return Ok(output
            .field("present", "false")
            .field("bundle_validation", "null")
            .finish());
    };
    output = output.field("present", "true").string(
        "manifest_checksum",
        &checksum_hex(pointer.manifest_checksum()),
    );
    let Some(root) = routing_root else {
        return Ok(output.field("bundle_validation", "null").finish());
    };
    let child = safe_bundle_child(pointer.relative_bundle())?;
    match open_bundle(&root.join(child)) {
        Ok(bundle) => {
            let snapshot = bundle.snapshot();
            let checksum_matches = bundle.manifest_checksum() == pointer.manifest_checksum();
            Ok(output
                .field("bundle_validation", "true")
                .boolean("checksum_matches", checksum_matches)
                .number("bundle_bytes", snapshot.total_bytes)
                .number("partitions", snapshot.partition_count as u64)
                .number("segments", snapshot.segment_count as u64)
                .number("largest_partition_bytes", snapshot.largest_partition_bytes)
                .finish())
        }
        Err(_) => Ok(output
            .field("bundle_validation", "false")
            .boolean("checksum_matches", false)
            .finish()),
    }
}

fn candidate_counts(arguments: &mut Arguments) -> Result<String, CliError> {
    let database = arguments.required_path("--database")?;
    let catalog = ReadOnlyCatalog::open(&database).map_err(CliError::operation)?;
    let counts = catalog.summary().map_err(CliError::operation)?.candidates;
    Ok(JsonObject::new("candidate-counts")
        .number("nodes", counts.nodes)
        .number("relation_kinds", counts.relation_kinds)
        .number("edges", counts.edges)
        .number("total", counts.total())
        .finish())
}

fn checkpoint_create(arguments: &mut Arguments) -> Result<String, CliError> {
    let database = arguments.required_path("--database")?;
    let destination_root = arguments.required_path("--destination-root")?;
    let destination = arguments.required_path("--destination")?;
    let routing_image_root = arguments.optional_path("--routing-root")?;
    let scratch_path = arguments.optional_path("--scratch")?;
    let available_destination_bytes = arguments.required_u64("--available-bytes")?;
    let minimum_headroom_bytes = arguments.optional_u64("--headroom-bytes")?.unwrap_or(0);
    let catalog = Catalog::open_existing(&database).map_err(CliError::operation)?;
    let report = catalog
        .create_checkpoint(&CheckpointRequest {
            destination_root,
            destination,
            routing_image_root,
            scratch_path,
            available_destination_bytes,
            minimum_headroom_bytes,
        })
        .map_err(CliError::operation)?;
    Ok(JsonObject::new("checkpoint-create")
        .number("files", report.file_count)
        .number("bytes", report.bytes)
        .string(
            "content_checksum",
            &format!("{:016x}", report.content_checksum),
        )
        .raw_field("catalog", summary_json(&report.catalog.summary))
        .duration("duration", report.duration)
        .finish())
}

fn restore_validate(arguments: &mut Arguments) -> Result<String, CliError> {
    let source_root = arguments.required_path("--source-root")?;
    let source_checkpoint = arguments.required_path("--source")?;
    let destination_root = arguments.required_path("--destination-root")?;
    let destination = arguments.required_path("--destination")?;
    let routing_image_root = arguments.required_path("--routing-root")?;
    let scratch_path = arguments.optional_path("--scratch")?;
    let available_destination_bytes = arguments.required_u64("--available-bytes")?;
    let minimum_headroom_bytes = arguments.optional_u64("--headroom-bytes")?.unwrap_or(0);
    let maximum_records = arguments.optional_u64("--max-records")?;
    let maximum_duration = arguments
        .optional_u64("--max-duration-ms")?
        .map(Duration::from_millis);
    let report = GraphEngine::restore_checkpoint(&EngineRestoreRequest {
        store: RestoreRequest {
            source_root,
            source_checkpoint,
            destination_root,
            destination,
            routing_image_root: Some(routing_image_root.clone()),
            scratch_path,
            available_destination_bytes,
            minimum_headroom_bytes,
            verification_limits: VerificationLimits {
                maximum_records,
                maximum_duration,
            },
        },
        engine_config: EngineConfig {
            routing_image_root: Some(routing_image_root),
            ..EngineConfig::default()
        },
    })
    .map_err(CliError::operation)?;
    Ok(JsonObject::new("restore-validate")
        .number("source_files", report.store.source_file_count)
        .number("source_bytes", report.store.source_bytes)
        .string(
            "source_checksum",
            &format!("{:016x}", report.store.source_checksum),
        )
        .boolean(
            "cleared_routing_pointer",
            report.store.cleared_routing_pointer,
        )
        .raw_field(
            "catalog",
            summary_json(&report.store.restored_catalog.summary),
        )
        .number(
            "records_examined",
            report.store.restored_catalog.records_examined,
        )
        .boolean("catalog_smoke", report.smoke_catalog_verified)
        .boolean("route_smoke", report.smoke_route_verified)
        .boolean("hydration_smoke", report.smoke_hydration_verified)
        .boolean("cuda_initialized", report.cuda_initialized)
        .boolean("shutdown_complete", report.shutdown.complete())
        .duration("duration", report.store.duration)
        .finish())
}

fn metrics_snapshot(arguments: &mut Arguments) -> Result<String, CliError> {
    let database = arguments.required_path("--database")?;
    let catalog = ReadOnlyCatalog::open(&database).map_err(CliError::operation)?;
    let metrics = catalog.metrics_snapshot().map_err(CliError::operation)?;
    Ok(metric_json(&metrics))
}

fn engine_health(arguments: &mut Arguments) -> Result<String, CliError> {
    let database = arguments.required_path("--database")?;
    let routing_image_root = arguments.required_path("--routing-root")?;
    let engine = GraphEngine::open(
        database,
        EngineConfig {
            routing_image_root: Some(routing_image_root),
            ..EngineConfig::default()
        },
    )
    .map_err(CliError::operation)?;
    let capabilities = engine.capabilities();
    let health = engine.health().map_err(CliError::operation)?;
    let shutdown = engine.shutdown().map_err(CliError::operation)?;
    if !shutdown.complete() {
        return Err(CliError::Operation);
    }
    Ok(JsonObject::new("engine-health")
        .boolean("cpu_reference_routing", capabilities.cpu_reference_routing)
        .boolean("gpu_routing", capabilities.gpu_routing)
        .boolean("cuda_support_compiled", capabilities.cuda_support_compiled)
        .boolean(
            "routing_available",
            matches!(health.routing, RoutingHealth::Available),
        )
        .boolean(
            "durable_catalog_available",
            health.durable_catalog_available,
        )
        .number(
            "active_routes",
            u64::try_from(health.active_routes).unwrap_or(u64::MAX),
        )
        .number(
            "maximum_concurrent_routes",
            u64::try_from(capabilities.resource_limits.maximum_concurrent_routes)
                .unwrap_or(u64::MAX),
        )
        .number(
            "maximum_concurrent_checkpoints",
            u64::try_from(capabilities.resource_limits.maximum_concurrent_checkpoints)
                .unwrap_or(u64::MAX),
        )
        .number(
            "maximum_maintenance_workers",
            u64::try_from(capabilities.resource_limits.maximum_maintenance_workers)
                .unwrap_or(u64::MAX),
        )
        .number(
            "maximum_queued_maintenance",
            u64::try_from(capabilities.resource_limits.maximum_queued_maintenance)
                .unwrap_or(u64::MAX),
        )
        .number(
            "active_routes_before_shutdown",
            u64::try_from(shutdown.active_before.routes).unwrap_or(u64::MAX),
        )
        .number(
            "active_mutations_before_shutdown",
            u64::try_from(shutdown.active_before.mutations).unwrap_or(u64::MAX),
        )
        .number(
            "active_checkpoints_before_shutdown",
            u64::try_from(shutdown.active_before.checkpoints).unwrap_or(u64::MAX),
        )
        .number(
            "active_maintenance_before_shutdown",
            u64::try_from(shutdown.active_before.maintenance).unwrap_or(u64::MAX),
        )
        .number(
            "drained_routes",
            u64::try_from(shutdown.drained.routes).unwrap_or(u64::MAX),
        )
        .number(
            "drained_mutations",
            u64::try_from(shutdown.drained.mutations).unwrap_or(u64::MAX),
        )
        .number(
            "drained_checkpoints",
            u64::try_from(shutdown.drained.checkpoints).unwrap_or(u64::MAX),
        )
        .number(
            "drained_maintenance",
            u64::try_from(shutdown.drained.maintenance).unwrap_or(u64::MAX),
        )
        .boolean("shutdown_complete", shutdown.complete())
        .finish())
}

fn reconcile_routing_root(arguments: &mut Arguments) -> Result<String, CliError> {
    let database = arguments.required_path("--database")?;
    let root = arguments.required_path("--routing-root")?;
    let catalog = ReadOnlyCatalog::open(&database).map_err(CliError::operation)?;
    let pointer = catalog
        .active_routing_image()
        .map_err(CliError::operation)?;
    let current = pointer
        .as_ref()
        .and_then(|value| safe_bundle_child(value.relative_bundle()).ok())
        .map(Path::to_path_buf);
    let entries = std::fs::read_dir(&root).map_err(|_| CliError::Operation)?;
    let mut recognized = 0_u64;
    let mut valid = 0_u64;
    let mut invalid = 0_u64;
    let mut temporary = 0_u64;
    let mut eligible_for_cleanup = 0_u64;
    let mut retained_unknown = 0_u64;
    for entry in entries {
        let entry = entry.map_err(|_| CliError::Operation)?;
        let file_type = entry.file_type().map_err(|_| CliError::Operation)?;
        if !file_type.is_dir() {
            retained_unknown = retained_unknown.saturating_add(1);
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(".tmp-") {
            temporary = temporary.saturating_add(1);
            recognized = recognized.saturating_add(1);
            eligible_for_cleanup = eligible_for_cleanup.saturating_add(1);
        } else if name.starts_with("bundle-") {
            recognized = recognized.saturating_add(1);
            if open_bundle(&entry.path()).is_ok() {
                valid = valid.saturating_add(1);
            } else {
                invalid = invalid.saturating_add(1);
            }
            if current.as_deref() != Some(Path::new(name.as_ref())) {
                eligible_for_cleanup = eligible_for_cleanup.saturating_add(1);
            }
        } else {
            retained_unknown = retained_unknown.saturating_add(1);
        }
    }
    Ok(JsonObject::new("reconcile-routing-root-dry-run")
        .boolean("mutated", false)
        .boolean("active_pointer_present", pointer.is_some())
        .number("recognized_children", recognized)
        .number("valid_bundles", valid)
        .number("invalid_bundles", invalid)
        .number("temporary_children", temporary)
        .number("eligible_for_cleanup", eligible_for_cleanup)
        .number("retained_unknown_entries", retained_unknown)
        .finish())
}

fn safe_bundle_child(value: &str) -> Result<&Path, CliError> {
    let path = Path::new(value);
    let mut components = path.components();
    if matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none() {
        Ok(path)
    } else {
        Err(CliError::Operation)
    }
}

fn help_json() -> String {
    JsonObject::new("help")
        .raw_field(
            "commands",
            "[\"summary\",\"verify\",\"active-pointer\",\"candidate-counts\",\"checkpoint-create\",\"restore-validate\",\"metrics-snapshot\",\"engine-health\",\"reconcile-routing-root-dry-run\",\"workload\"]".to_owned(),
        )
        .finish()
}

#[derive(Debug, Eq, PartialEq)]
pub enum CliError {
    Usage(&'static str),
    InvalidValue(&'static str),
    Operation,
}

impl CliError {
    fn operation<E>(_error: E) -> Self {
        Self::Operation
    }

    #[must_use]
    pub const fn exit_code(&self) -> u8 {
        match self {
            Self::Usage(_) | Self::InvalidValue(_) => 2,
            Self::Operation => 1,
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(reason) => write!(formatter, "usage error: {reason}; run `help`"),
            Self::InvalidValue(option) => write!(formatter, "invalid value for {option}"),
            Self::Operation => formatter.write_str("operation or validation failed"),
        }
    }
}

impl std::error::Error for CliError {}

pub(crate) struct Arguments {
    values: Vec<OsString>,
    consumed: Vec<bool>,
}

impl Arguments {
    fn new<I, S>(arguments: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        let values: Vec<_> = arguments.into_iter().map(Into::into).collect();
        let consumed = vec![false; values.len()];
        Self { values, consumed }
    }

    fn next_command(&mut self) -> Result<String, CliError> {
        let Some(value) = self.values.first() else {
            return Err(CliError::Usage("a command is required"));
        };
        self.consumed[0] = true;
        value
            .to_str()
            .map(str::to_owned)
            .ok_or(CliError::Usage("command is not UTF-8"))
    }

    pub(crate) fn required_path(&mut self, name: &'static str) -> Result<PathBuf, CliError> {
        self.optional_path(name)?
            .ok_or(CliError::Usage("a required path option is missing"))
    }

    pub(crate) fn optional_path(
        &mut self,
        name: &'static str,
    ) -> Result<Option<PathBuf>, CliError> {
        Ok(self.take_value(name)?.map(PathBuf::from))
    }

    pub(crate) fn optional_u64(&mut self, name: &'static str) -> Result<Option<u64>, CliError> {
        self.take_value(name)?
            .map(|value| {
                value
                    .to_str()
                    .ok_or(CliError::InvalidValue(name))?
                    .parse()
                    .map_err(|_| CliError::InvalidValue(name))
            })
            .transpose()
    }

    pub(crate) fn required_u64(&mut self, name: &'static str) -> Result<u64, CliError> {
        self.optional_u64(name)?
            .ok_or(CliError::Usage("a required integer option is missing"))
    }

    fn take_value(&mut self, name: &'static str) -> Result<Option<OsString>, CliError> {
        let mut found = None;
        for index in 1..self.values.len() {
            if self.consumed[index] || self.values[index] != name {
                continue;
            }
            if found.is_some() {
                return Err(CliError::Usage("an option was supplied more than once"));
            }
            let value_index = index + 1;
            if value_index >= self.values.len() || self.consumed[value_index] {
                return Err(CliError::Usage("an option value is missing"));
            }
            self.consumed[index] = true;
            self.consumed[value_index] = true;
            found = Some(self.values[value_index].clone());
        }
        Ok(found)
    }

    fn finish(&self) -> Result<(), CliError> {
        if self.consumed.iter().all(|value| *value) {
            Ok(())
        } else {
            Err(CliError::Usage("unknown option or positional argument"))
        }
    }
}
