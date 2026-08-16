use std::sync::{Arc, atomic::AtomicBool};

use cudarc::driver::CudaSlice;

use pathhydra_routing::RoutingImage;

use crate::{CudaAlgorithm, CudaContextOwner, CudaError, CudaFailureKind, CudaRouteOutput, launch};

#[derive(Debug)]
pub struct CudaResidentImage {
    pub(crate) context: Arc<CudaContextOwner>,
    pub(crate) offsets: CudaSlice<u64>,
    pub(crate) destinations: CudaSlice<u32>,
    pub(crate) relation_indexes: CudaSlice<u32>,
    pub(crate) base_weight_bits: CudaSlice<u32>,
    cpu_image: Arc<RoutingImage>,
    allocated_bytes: usize,
}

impl CudaResidentImage {
    pub fn upload(
        context: Arc<CudaContextOwner>,
        cpu_image: Arc<RoutingImage>,
        maximum_topology_bytes: usize,
        minimum_free_headroom: usize,
    ) -> Result<Arc<Self>, CudaError> {
        let allocated_bytes = cpu_image
            .gpu_topology_manifest()
            .checked_total()
            .ok_or_else(|| {
                CudaError::new(
                    CudaFailureKind::Allocation,
                    "CUDA topology byte count overflow",
                )
            })?;
        if allocated_bytes > maximum_topology_bytes {
            return Err(CudaError::new(
                CudaFailureKind::Admission,
                format!(
                    "CUDA topology requires {allocated_bytes} bytes; configured maximum is {maximum_topology_bytes}"
                ),
            ));
        }
        let (free, _) = context.current_memory()?;
        let required = allocated_bytes
            .checked_add(minimum_free_headroom)
            .ok_or_else(|| {
                CudaError::new(
                    CudaFailureKind::Allocation,
                    "CUDA headroom accounting overflow",
                )
            })?;
        if free < required {
            return Err(CudaError::new(
                CudaFailureKind::Admission,
                format!(
                    "CUDA topology plus headroom requires {required} free bytes; driver reports {free}"
                ),
            ));
        }
        let arrays = cpu_image.arrays();
        let offsets = context
            .stream
            .clone_htod(arrays.offsets)
            .map_err(upload_error)?;
        // CUDA allocations cannot portably have zero bytes. The one-word
        // sentinel is never dereferenced because the logical adjacency count is
        // zero and is deliberately excluded from topology byte accounting.
        let adjacency_sentinel = [0_u32];
        let destinations = context
            .stream
            .clone_htod(if arrays.destinations.is_empty() {
                &adjacency_sentinel[..]
            } else {
                arrays.destinations
            })
            .map_err(upload_error)?;
        let relation_indexes = context
            .stream
            .clone_htod(if arrays.relation_indexes.is_empty() {
                &adjacency_sentinel[..]
            } else {
                arrays.relation_indexes
            })
            .map_err(upload_error)?;
        let base_weight_bits = context
            .stream
            .clone_htod(if arrays.base_weight_bits.is_empty() {
                &adjacency_sentinel[..]
            } else {
                arrays.base_weight_bits
            })
            .map_err(upload_error)?;
        context.stream.synchronize().map_err(upload_error)?;
        let destination_xor = arrays
            .destinations
            .iter()
            .fold(0_u64, |checksum, &value| checksum ^ u64::from(value));
        let relation_weight_xor = arrays
            .relation_indexes
            .iter()
            .copied()
            .zip(arrays.base_weight_bits.iter().copied())
            .fold(0_u64, |checksum, (relation, weight)| {
                checksum ^ ((u64::from(relation) << 32) | u64::from(weight))
            });
        launch::validate_topology(
            &context,
            &offsets,
            &destinations,
            &relation_indexes,
            &base_weight_bits,
            cpu_image.node_count(),
            cpu_image.relation_kind_count(),
            cpu_image.adjacency_count(),
            destination_xor,
            relation_weight_xor,
        )?;
        Ok(Arc::new(Self {
            context,
            offsets,
            destinations,
            relation_indexes,
            base_weight_bits,
            cpu_image,
            allocated_bytes,
        }))
    }

    #[must_use]
    pub const fn allocated_bytes(&self) -> usize {
        self.allocated_bytes
    }

    #[must_use]
    pub const fn cpu_image(&self) -> &Arc<RoutingImage> {
        &self.cpu_image
    }

    #[must_use]
    pub fn node_count(&self) -> usize {
        self.cpu_image.node_count()
    }

    #[must_use]
    pub fn adjacency_count(&self) -> usize {
        self.cpu_image.adjacency_count()
    }

    #[must_use]
    pub fn device_array_lengths(&self) -> (usize, usize, usize, usize) {
        (
            self.offsets.len(),
            self.destinations.len(),
            self.relation_indexes.len(),
            self.base_weight_bits.len(),
        )
    }

    #[must_use]
    pub const fn context(&self) -> &Arc<CudaContextOwner> {
        &self.context
    }

    pub fn route(
        &self,
        request: &pathhydra_routing::RoutingRequest,
        algorithm: CudaAlgorithm,
        cancellation: &AtomicBool,
        reserved_search_bytes: usize,
    ) -> Result<CudaRouteOutput, CudaError> {
        launch::route(
            self,
            request,
            algorithm,
            cancellation,
            reserved_search_bytes,
        )
    }
}

fn upload_error(error: cudarc::driver::DriverError) -> CudaError {
    CudaError::new(
        CudaFailureKind::Upload,
        format!("CUDA topology upload failed: {error}"),
    )
}
