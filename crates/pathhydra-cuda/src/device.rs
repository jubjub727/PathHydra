use std::sync::Arc;

use cudarc::driver::{CudaContext, sys::CUdevice_attribute};

use pathhydra_routing::NUMERIC_POLICY_ID;

use crate::{CudaError, CudaFailureKind, KERNEL_PTX_TARGET, launch};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CudaDeviceCapabilities {
    pub driver_version: i32,
    pub device_ordinal: usize,
    pub device_name: String,
    pub compute_capability: (i32, i32),
    pub total_memory_bytes: usize,
    pub free_memory_bytes: usize,
    pub maximum_threads_per_block: i32,
    pub maximum_block_dimensions: (i32, i32, i32),
    pub maximum_grid_dimensions: (i32, i32, i32),
    pub multiprocessor_count: i32,
    pub warp_size: i32,
    pub concurrent_kernels: bool,
    pub stream_priority_range: (i32, i32),
    pub unified_addressing: bool,
    pub global_u64_atomics: bool,
    pub kernel_ptx_target: &'static str,
    pub numeric_policy_id: &'static str,
}

impl CudaDeviceCapabilities {
    pub(crate) fn from_context(context: &Arc<CudaContext>) -> Result<Self, CudaError> {
        let attribute = |name| {
            context.attribute(name).map_err(|error| {
                CudaError::new(
                    CudaFailureKind::Context,
                    format!("failed to query CUDA device attribute: {error}"),
                )
            })
        };
        let compute_capability = context.compute_capability().map_err(context_error)?;
        let (free_memory_bytes, _) = context.mem_get_info().map_err(context_error)?;
        Ok(Self {
            driver_version: launch::driver_version()?,
            device_ordinal: context.ordinal(),
            device_name: context.name().map_err(context_error)?,
            compute_capability,
            total_memory_bytes: context.total_mem().map_err(context_error)?,
            free_memory_bytes,
            maximum_threads_per_block: attribute(
                CUdevice_attribute::CU_DEVICE_ATTRIBUTE_MAX_THREADS_PER_BLOCK,
            )?,
            maximum_block_dimensions: (
                attribute(CUdevice_attribute::CU_DEVICE_ATTRIBUTE_MAX_BLOCK_DIM_X)?,
                attribute(CUdevice_attribute::CU_DEVICE_ATTRIBUTE_MAX_BLOCK_DIM_Y)?,
                attribute(CUdevice_attribute::CU_DEVICE_ATTRIBUTE_MAX_BLOCK_DIM_Z)?,
            ),
            maximum_grid_dimensions: (
                attribute(CUdevice_attribute::CU_DEVICE_ATTRIBUTE_MAX_GRID_DIM_X)?,
                attribute(CUdevice_attribute::CU_DEVICE_ATTRIBUTE_MAX_GRID_DIM_Y)?,
                attribute(CUdevice_attribute::CU_DEVICE_ATTRIBUTE_MAX_GRID_DIM_Z)?,
            ),
            multiprocessor_count: attribute(
                CUdevice_attribute::CU_DEVICE_ATTRIBUTE_MULTIPROCESSOR_COUNT,
            )?,
            warp_size: attribute(CUdevice_attribute::CU_DEVICE_ATTRIBUTE_WARP_SIZE)?,
            concurrent_kernels: attribute(
                CUdevice_attribute::CU_DEVICE_ATTRIBUTE_CONCURRENT_KERNELS,
            )? != 0,
            stream_priority_range: launch::stream_priority_range()?,
            unified_addressing: attribute(
                CUdevice_attribute::CU_DEVICE_ATTRIBUTE_UNIFIED_ADDRESSING,
            )? != 0,
            global_u64_atomics: compute_capability >= (6, 0),
            kernel_ptx_target: KERNEL_PTX_TARGET,
            numeric_policy_id: NUMERIC_POLICY_ID,
        })
    }
}

pub fn probe_device(device_ordinal: usize) -> Result<CudaDeviceCapabilities, CudaError> {
    let context = CudaContext::new(device_ordinal).map_err(|error| {
        CudaError::new(
            CudaFailureKind::Unavailable,
            format!("CUDA device {device_ordinal} is unavailable: {error}"),
        )
    })?;
    CudaDeviceCapabilities::from_context(&context)
}

fn context_error(error: cudarc::driver::DriverError) -> CudaError {
    CudaError::new(
        CudaFailureKind::Context,
        format!("CUDA capability query failed: {error}"),
    )
}
