use std::{fmt::Write, time::Duration};

use pathhydra_store::{CatalogSummary, MetricValue, StoreMetricsSnapshot, WriteOperationClass};

pub(crate) struct JsonObject {
    output: String,
}

impl JsonObject {
    pub(crate) fn new(command: &str) -> Self {
        let mut value = Self {
            output: String::from("{"),
        };
        value = value.string("command", command);
        value
    }

    pub(crate) fn field(mut self, name: &str, raw_value: &str) -> Self {
        self.separator();
        quote_into(&mut self.output, name);
        self.output.push(':');
        self.output.push_str(raw_value);
        self
    }

    pub(crate) fn raw_field(self, name: &str, value: String) -> Self {
        self.field(name, &value)
    }

    pub(crate) fn number(self, name: &str, value: u64) -> Self {
        self.field(name, &value.to_string())
    }

    pub(crate) fn boolean(self, name: &str, value: bool) -> Self {
        self.field(name, if value { "true" } else { "false" })
    }

    pub(crate) fn string(mut self, name: &str, value: &str) -> Self {
        self.separator();
        quote_into(&mut self.output, name);
        self.output.push(':');
        quote_into(&mut self.output, value);
        self
    }

    pub(crate) fn duration(self, name: &str, value: Duration) -> Self {
        self.raw_field(name, duration_json(value))
    }

    pub(crate) fn finish(mut self) -> String {
        self.output.push('}');
        self.output
    }

    fn separator(&mut self) {
        if self.output.len() > 1 {
            self.output.push(',');
        }
    }
}

pub(crate) fn summary_json(summary: &CatalogSummary) -> String {
    let candidates = format!(
        "{{\"nodes\":{},\"relation_kinds\":{},\"edges\":{},\"total\":{}}}",
        summary.candidates.nodes,
        summary.candidates.relation_kinds,
        summary.candidates.edges,
        summary.candidates.total()
    );
    format!(
        "{{\"candidates\":{candidates},\"confirmed_nodes\":{},\"relation_kinds\":{},\"confirmed_edges\":{},\"node_name_entries\":{},\"relation_name_entries\":{},\"outgoing_entries\":{},\"incoming_entries\":{},\"routing_pointer_present\":{}}}",
        summary.confirmed_nodes,
        summary.relation_kinds,
        summary.confirmed_edges,
        summary.node_name_entries,
        summary.relation_name_entries,
        summary.outgoing_entries,
        summary.incoming_entries,
        summary.routing_pointer_present,
    )
}

pub(crate) fn metric_json(metrics: &StoreMetricsSnapshot) -> String {
    let mut writes = String::from("{");
    for (index, class) in [
        WriteOperationClass::CandidateInsertion,
        WriteOperationClass::ConfirmedPromotion,
        WriteOperationClass::EdgeDeletion,
        WriteOperationClass::NodeDeletion,
        WriteOperationClass::RoutingPointer,
        WriteOperationClass::Maintenance,
    ]
    .into_iter()
    .enumerate()
    {
        if index > 0 {
            writes.push(',');
        }
        quote_into(&mut writes, write_class_name(class));
        writes.push(':');
        if let Some(value) = metrics.writes.get(&class) {
            let _ = write!(
                writes,
                "{{\"attempts\":{},\"failures\":{},\"committed_entries\":{},\"committed_bytes\":{}}}",
                value.attempts, value.failures, value.committed_entries, value.committed_bytes
            );
        } else {
            writes.push_str(
                "{\"attempts\":0,\"failures\":0,\"committed_entries\":0,\"committed_bytes\":0}",
            );
        }
    }
    writes.push('}');

    let mut families = String::from("[");
    for (index, family) in metrics.column_families.iter().enumerate() {
        if index > 0 {
            families.push(',');
        }
        let mut object = JsonObject::new("column-family")
            .string("name", family.name)
            .raw_field(
                "estimated_key_count",
                metric_value_json(&family.estimated_key_count),
            )
            .raw_field(
                "estimated_live_data_bytes",
                metric_value_json(&family.estimated_live_data_bytes),
            )
            .raw_field("live_sst_bytes", metric_value_json(&family.live_sst_bytes))
            .raw_field(
                "total_sst_bytes",
                metric_value_json(&family.total_sst_bytes),
            )
            .raw_field(
                "active_memtable_bytes",
                metric_value_json(&family.active_memtable_bytes),
            )
            .raw_field(
                "all_memtable_bytes",
                metric_value_json(&family.all_memtable_bytes),
            )
            .raw_field(
                "pending_compaction_bytes",
                metric_value_json(&family.pending_compaction_bytes),
            )
            .raw_field(
                "immutable_memtables",
                metric_value_json(&family.immutable_memtables),
            )
            .raw_field("pending_flush", metric_value_json(&family.pending_flush))
            .raw_field(
                "running_flushes",
                metric_value_json(&family.running_flushes),
            )
            .raw_field(
                "pending_compaction",
                metric_value_json(&family.pending_compaction),
            )
            .raw_field(
                "running_compactions",
                metric_value_json(&family.running_compactions),
            )
            .raw_field(
                "background_errors",
                metric_value_json(&family.background_errors),
            )
            .raw_field("write_stopped", metric_value_json(&family.write_stopped))
            .finish();
        object = object.replacen("\"command\":\"column-family\",", "", 1);
        families.push_str(&object);
    }
    families.push(']');

    JsonObject::new("metrics-snapshot")
        .raw_field("writes", writes)
        .raw_field(
            "scans",
            format!(
                "{{\"completed\":{},\"failures\":{},\"records\":{},\"decoded_bytes\":{},\"total_duration\":{}}}",
                metrics.scans.completed_scans,
                metrics.scans.failures,
                metrics.scans.records,
                metrics.scans.decoded_bytes,
                duration_json(metrics.scans.total_duration),
            ),
        )
        .raw_field(
            "maintenance",
            format!(
                "{{\"wal_sync_attempts\":{},\"wal_sync_failures\":{},\"checkpoint_attempts\":{},\"checkpoint_failures\":{},\"checkpoint_bytes\":{},\"checkpoint_duration\":{},\"restore_attempts\":{},\"restore_failures\":{},\"restore_bytes\":{},\"restore_duration\":{},\"flush_attempts\":{},\"flush_failures\":{},\"compaction_attempts\":{},\"compaction_failures\":{},\"compaction_duration\":{},\"last_verification_succeeded\":{},\"last_maintenance_succeeded\":{}}}",
                metrics.maintenance.wal_sync_attempts,
                metrics.maintenance.wal_sync_failures,
                metrics.maintenance.checkpoint_attempts,
                metrics.maintenance.checkpoint_failures,
                metrics.maintenance.checkpoint_bytes,
                duration_json(metrics.maintenance.checkpoint_duration),
                metrics.maintenance.restore_attempts,
                metrics.maintenance.restore_failures,
                metrics.maintenance.restore_bytes,
                duration_json(metrics.maintenance.restore_duration),
                metrics.maintenance.flush_attempts,
                metrics.maintenance.flush_failures,
                metrics.maintenance.compaction_attempts,
                metrics.maintenance.compaction_failures,
                duration_json(metrics.maintenance.compaction_duration),
                optional_bool(metrics.maintenance.last_verification_succeeded),
                optional_bool(metrics.maintenance.last_maintenance_succeeded),
            ),
        )
        .raw_field(
            "standalone_restore",
            format!(
                "{{\"attempts\":{},\"failures\":{},\"restored_bytes\":{},\"total_duration\":{}}}",
                metrics.standalone_restore.attempts,
                metrics.standalone_restore.failures,
                metrics.standalone_restore.restored_bytes,
                duration_json(metrics.standalone_restore.total_duration),
            ),
        )
        .raw_field("column_families", families)
        .raw_field(
            "block_cache_capacity_bytes",
            metric_value_json(&metrics.block_cache_capacity_bytes),
        )
        .raw_field(
            "block_cache_usage_bytes",
            metric_value_json(&metrics.block_cache_usage_bytes),
        )
        .raw_field(
            "block_cache_hits",
            metric_value_json(&metrics.block_cache_hits),
        )
        .raw_field(
            "block_cache_misses",
            metric_value_json(&metrics.block_cache_misses),
        )
        .finish()
}

pub(crate) fn checksum_hex(checksum: &[u8]) -> String {
    let mut output = String::with_capacity(checksum.len() * 2);
    for byte in checksum {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

pub(crate) fn duration_json(duration: Duration) -> String {
    format!(
        "{{\"seconds\":{},\"nanoseconds\":{}}}",
        duration.as_secs(),
        duration.subsec_nanos()
    )
}

fn metric_value_json(value: &MetricValue<u64>) -> String {
    match value {
        MetricValue::Available(value) => value.to_string(),
        MetricValue::Unavailable => String::from("null"),
    }
}

fn write_class_name(value: WriteOperationClass) -> &'static str {
    match value {
        WriteOperationClass::CandidateInsertion => "candidate_insertion",
        WriteOperationClass::ConfirmedPromotion => "confirmed_promotion",
        WriteOperationClass::EdgeDeletion => "edge_deletion",
        WriteOperationClass::NodeDeletion => "node_deletion",
        WriteOperationClass::RoutingPointer => "routing_pointer",
        WriteOperationClass::Maintenance => "maintenance",
    }
}

fn optional_bool(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "true",
        Some(false) => "false",
        None => "null",
    }
}

fn quote_into(output: &mut String, value: &str) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{08}' => output.push_str("\\b"),
            '\u{0c}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character <= '\u{1f}' => {
                let _ = write!(output, "\\u{:04x}", character as u32);
            }
            character => output.push(character),
        }
    }
    output.push('"');
}

#[cfg(test)]
mod tests {
    use super::{JsonObject, duration_json};
    use std::time::Duration;

    #[test]
    fn json_strings_and_durations_are_stable() {
        assert_eq!(
            JsonObject::new("x").string("value", "a\n\"b").finish(),
            "{\"command\":\"x\",\"value\":\"a\\n\\\"b\"}"
        );
        assert_eq!(
            duration_json(Duration::new(2, 3)),
            "{\"seconds\":2,\"nanoseconds\":3}"
        );
    }
}
