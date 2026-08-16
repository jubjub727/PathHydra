use std::time::Duration;

use pathhydra_routing::{NUMERIC_POLICY_ID, RoutingImageManifest, TIE_POLICY_ID};

use crate::{EngineConfig, ImageBuildReport, RoutingUnavailableReason};

#[derive(Clone, Debug)]
pub struct EngineCapabilities {
    pub cpu_reference_routing: bool,
    pub gpu_routing: bool,
    pub paths: bool,
    pub edge_budgets: bool,
    pub cancellation: bool,
    pub hydration: bool,
    pub subgraphs: bool,
    pub durable_routing_images: bool,
    pub numeric_policy_id: &'static str,
    pub tie_policy_id: &'static str,
    pub resource_limits: EngineConfig,
}

impl EngineCapabilities {
    pub(crate) fn new(config: EngineConfig) -> Self {
        Self {
            cpu_reference_routing: true,
            gpu_routing: false,
            paths: true,
            edge_budgets: true,
            cancellation: true,
            hydration: true,
            subgraphs: true,
            durable_routing_images: false,
            numeric_policy_id: NUMERIC_POLICY_ID,
            tie_policy_id: TIE_POLICY_ID,
            resource_limits: config,
        }
    }
}

#[derive(Clone, Debug)]
pub enum RoutingHealth {
    Available,
    Unavailable(RoutingUnavailableReason),
}

#[derive(Clone, Debug)]
pub struct EngineHealth {
    pub durable_catalog_available: bool,
    pub routing: RoutingHealth,
    pub current_image_manifest: Option<RoutingImageManifest>,
    pub current_image_age: Option<Duration>,
    pub last_image_build: ImageBuildReport,
    pub active_routes: usize,
    pub peak_active_routes: usize,
    pub reserved_route_bytes: usize,
    pub peak_reserved_route_bytes: usize,
    pub cumulative_route_admissions: u64,
    pub cumulative_admission_rejections: u64,
    pub cumulative_cancellations: u64,
    pub cumulative_image_build_failures: u64,
}
