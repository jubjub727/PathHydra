/// Allocation and cardinality limits applied at the consumer boundary.
///
/// The generic codec enforces encoded bytes, nesting depth, and JSON value
/// count before DTO deserialization. DTO contextual validators and facade
/// conversion enforce the remaining semantic limits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApiLimits {
    pub maximum_encoded_bytes: usize,
    pub maximum_name_bytes: usize,
    pub maximum_payload_bytes: usize,
    pub maximum_destinations: usize,
    pub maximum_profile_entries: usize,
    pub maximum_path_steps: usize,
    pub maximum_hydration_handles: usize,
    pub maximum_subgraph_nodes: usize,
    pub maximum_subgraph_edges: usize,
    pub maximum_diagnostic_text_bytes: usize,
    pub maximum_nesting_depth: usize,
    pub maximum_json_values: usize,
    pub maximum_batch_entries: usize,
    pub maximum_batch_node_entries: usize,
    pub maximum_batch_relation_kind_entries: usize,
    pub maximum_batch_edge_entries: usize,
    pub maximum_batch_name_bytes: usize,
    pub maximum_batch_payload_bytes: usize,
    pub maximum_batch_references: usize,
    pub maximum_batch_estimated_bytes: usize,
    pub maximum_relation_kind_usage_results: usize,
}

impl ApiLimits {
    pub const DEFAULT: Self = Self {
        maximum_encoded_bytes: 256 * 1024 * 1024,
        maximum_name_bytes: 64 * 1024,
        maximum_payload_bytes: 8 * 1024 * 1024,
        maximum_destinations: 100_000,
        maximum_profile_entries: 65_536,
        maximum_path_steps: 1_000_000,
        maximum_hydration_handles: 1_000_000,
        maximum_subgraph_nodes: 1_000_000,
        maximum_subgraph_edges: 2_000_000,
        maximum_diagnostic_text_bytes: 64 * 1024,
        maximum_nesting_depth: 64,
        maximum_json_values: 4_000_000,
        maximum_batch_entries: 120_000,
        maximum_batch_node_entries: 20_000,
        maximum_batch_relation_kind_entries: 10_000,
        maximum_batch_edge_entries: 100_000,
        maximum_batch_name_bytes: 64 * 1024 * 1024,
        maximum_batch_payload_bytes: 512 * 1024 * 1024,
        maximum_batch_references: 300_000,
        maximum_batch_estimated_bytes: 1024 * 1024 * 1024,
        maximum_relation_kind_usage_results: 100_000,
    };

    /// Replace the three generic syntactic limits while retaining semantic
    /// defaults. Useful for constrained consumers and adversarial tests.
    #[must_use]
    pub const fn constrained(
        maximum_encoded_bytes: usize,
        maximum_nesting_depth: usize,
        maximum_json_values: usize,
    ) -> Self {
        Self {
            maximum_encoded_bytes,
            maximum_nesting_depth,
            maximum_json_values,
            ..Self::DEFAULT
        }
    }
}

impl Default for ApiLimits {
    fn default() -> Self {
        Self::DEFAULT
    }
}
