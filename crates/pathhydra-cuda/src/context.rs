use std::{sync::Arc, time::Duration, time::Instant};

use cudarc::{
    driver::{CudaContext, CudaFunction, CudaStream},
    nvrtc::Ptx,
};

use crate::{
    CudaDeviceCapabilities, CudaError, CudaFailureKind, embedded_ptx, launch, validate_embedded_ptx,
};

#[derive(Debug)]
pub struct CudaContextOwner {
    pub(crate) context: Arc<CudaContext>,
    pub(crate) stream: Arc<CudaStream>,
    pub(crate) smoke_function: CudaFunction,
    pub(crate) validate_topology_function: CudaFunction,
    pub(crate) frontier_function: CudaFunction,
    pub(crate) delta_function: CudaFunction,
    pub(crate) partition_frontier_function: CudaFunction,
    pub(crate) partition_delta_function: CudaFunction,
    pub(crate) benchmark_reset_function: CudaFunction,
    pub(crate) benchmark_target_function: CudaFunction,
    pub(crate) benchmark_profile_inline_function: CudaFunction,
    pub(crate) benchmark_profile_consume_function: CudaFunction,
    capabilities: CudaDeviceCapabilities,
    module_load_duration: Duration,
}

impl CudaContextOwner {
    pub fn initialize(device_ordinal: usize) -> Result<Arc<Self>, CudaError> {
        validate_embedded_ptx()?;
        let started = Instant::now();
        let context = CudaContext::new(device_ordinal).map_err(|error| {
            CudaError::new(
                CudaFailureKind::Unavailable,
                format!("CUDA device {device_ordinal} is unavailable: {error}"),
            )
        })?;
        let capabilities = CudaDeviceCapabilities::from_context(&context)?;
        if capabilities.compute_capability < (6, 0) {
            return Err(CudaError::new(
                CudaFailureKind::Incompatible,
                format!(
                    "CUDA compute capability {}.{} lacks required global 64-bit atomics",
                    capabilities.compute_capability.0, capabilities.compute_capability.1
                ),
            ));
        }
        let stream = context.default_stream();
        let module = context
            .load_module(Ptx::from_src(embedded_ptx()))
            .map_err(|error| {
                CudaError::new(
                    CudaFailureKind::Module,
                    format!("failed to JIT/load embedded Rust PTX: {error}"),
                )
            })?;
        let smoke_function =
            module
                .load_function("pathhydra_smoke_arithmetic")
                .map_err(|error| {
                    CudaError::new(
                        CudaFailureKind::Module,
                        format!("embedded PTX lacks smoke kernel: {error}"),
                    )
                })?;
        let frontier_function =
            module
                .load_function("pathhydra_frontier_phase")
                .map_err(|error| {
                    CudaError::new(
                        CudaFailureKind::Module,
                        format!("embedded PTX lacks parallel frontier-phase kernel: {error}"),
                    )
                })?;
        let validate_topology_function = module
            .load_function("pathhydra_validate_topology")
            .map_err(|error| {
                CudaError::new(
                    CudaFailureKind::Module,
                    format!("embedded PTX lacks topology validation kernel: {error}"),
                )
            })?;
        let delta_function = module
            .load_function("pathhydra_delta_phase")
            .map_err(|error| {
                CudaError::new(
                    CudaFailureKind::Module,
                    format!("embedded PTX lacks parallel delta-phase kernel: {error}"),
                )
            })?;
        let partition_frontier_function = module
            .load_function("pathhydra_partition_frontier")
            .map_err(|error| {
                CudaError::new(
                    CudaFailureKind::Module,
                    format!("embedded PTX lacks partition-frontier kernel: {error}"),
                )
            })?;
        let partition_delta_function =
            module
                .load_function("pathhydra_partition_delta")
                .map_err(|error| {
                    CudaError::new(
                        CudaFailureKind::Module,
                        format!("embedded PTX lacks partition-delta kernel: {error}"),
                    )
                })?;
        let benchmark_reset_function =
            module
                .load_function("pathhydra_benchmark_reset")
                .map_err(|error| {
                    CudaError::new(
                        CudaFailureKind::Module,
                        format!("embedded PTX lacks benchmark reset kernel: {error}"),
                    )
                })?;
        let benchmark_target_function = module
            .load_function("pathhydra_benchmark_target")
            .map_err(|error| {
                CudaError::new(
                    CudaFailureKind::Module,
                    format!("embedded PTX lacks benchmark target kernel: {error}"),
                )
            })?;
        let benchmark_profile_inline_function = module
            .load_function("pathhydra_benchmark_profile_inline")
            .map_err(|error| {
                CudaError::new(
                    CudaFailureKind::Module,
                    format!("embedded PTX lacks benchmark inline-profile kernel: {error}"),
                )
            })?;
        let benchmark_profile_consume_function = module
            .load_function("pathhydra_benchmark_profile_consume")
            .map_err(|error| {
                CudaError::new(
                    CudaFailureKind::Module,
                    format!("embedded PTX lacks benchmark materialized-profile kernel: {error}"),
                )
            })?;
        Ok(Arc::new(Self {
            context,
            stream,
            smoke_function,
            validate_topology_function,
            frontier_function,
            delta_function,
            partition_frontier_function,
            partition_delta_function,
            benchmark_reset_function,
            benchmark_target_function,
            benchmark_profile_inline_function,
            benchmark_profile_consume_function,
            capabilities,
            module_load_duration: started.elapsed(),
        }))
    }

    #[must_use]
    pub const fn capabilities(&self) -> &CudaDeviceCapabilities {
        &self.capabilities
    }

    #[must_use]
    pub const fn module_load_duration(&self) -> Duration {
        self.module_load_duration
    }

    pub fn smoke_arithmetic(
        &self,
        input: &[f32],
        multipliers: &[f32],
        addends: &[f64],
    ) -> Result<Vec<f64>, CudaError> {
        launch::smoke_arithmetic(self, input, multipliers, addends)
    }

    pub fn benchmark_reset_strategy(
        &self,
        node_count: usize,
        strategy: crate::CudaBenchmarkResetStrategy,
    ) -> Result<(), CudaError> {
        launch::benchmark_reset_strategy(self, node_count, strategy)
    }

    pub fn benchmark_target_strategy(
        &self,
        node_count: usize,
        destinations: &[u32],
        strategy: crate::CudaBenchmarkTargetStrategy,
    ) -> Result<(), CudaError> {
        launch::benchmark_target_strategy(self, node_count, destinations, strategy)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn benchmark_profile_strategy(
        &self,
        base_weight_bits: &[u32],
        relation_indexes: &[u32],
        multiplier_bits: &[u32],
        reuse_count: usize,
        maximum_chunk_edges: usize,
        strategy: crate::CudaBenchmarkProfileStrategy,
    ) -> Result<(), CudaError> {
        launch::benchmark_profile_strategy(
            self,
            base_weight_bits,
            relation_indexes,
            multiplier_bits,
            reuse_count,
            maximum_chunk_edges,
            strategy,
        )
    }

    pub fn current_memory(&self) -> Result<(usize, usize), CudaError> {
        self.context.mem_get_info().map_err(|error| {
            CudaError::new(
                CudaFailureKind::Context,
                format!("failed to query CUDA memory: {error}"),
            )
        })
    }
}
