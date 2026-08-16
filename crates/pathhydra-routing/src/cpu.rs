use std::{
    cmp::Ordering,
    collections::BinaryHeap,
    mem::size_of,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering as AtomicOrdering},
    },
};

use pathhydra_core::{BaseWeight, EdgeId};

use crate::{
    ArithmeticError, ArithmeticOperation, CompletionReason, DenseNodeId, DestinationResult,
    DestinationState, ExactRoute, PackedRelationProfile, PathStep, RelationMultiplier, RelationUse,
    RoutePath, RoutingError, RoutingImage, RoutingRequest, RoutingResponse,
};

#[derive(Clone, Copy, Debug)]
struct FrontierEntry {
    distance: f64,
    node: DenseNodeId,
}

impl PartialEq for FrontierEntry {
    fn eq(&self, other: &Self) -> bool {
        self.distance == other.distance && self.node == other.node
    }
}

impl Eq for FrontierEntry {}

impl Ord for FrontierEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .distance
            .total_cmp(&self.distance)
            .then_with(|| other.node.cmp(&self.node))
    }
}

impl PartialOrd for FrontierEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Copy, Debug)]
struct Predecessor {
    source: DenseNodeId,
    edge: EdgeId,
    adjacency: usize,
}

/// A cooperative, read-only cancellation source for controlled CPU routing.
pub trait CancellationSignal {
    fn is_cancelled(&self) -> bool;
}

impl CancellationSignal for AtomicBool {
    fn is_cancelled(&self) -> bool {
        self.load(AtomicOrdering::Acquire)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NeverCancelled;

impl CancellationSignal for NeverCancelled {
    fn is_cancelled(&self) -> bool {
        false
    }
}

/// Conservative logical collection-payload reservation for one route.
///
/// This deliberately excludes allocator metadata and does not predict RSS.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CpuWorkingSetEstimate {
    bytes: usize,
    frontier_entries: usize,
    maximum_path_steps: usize,
}

impl CpuWorkingSetEstimate {
    #[must_use]
    pub const fn bytes(self) -> usize {
        self.bytes
    }
    #[must_use]
    pub const fn frontier_entries(self) -> usize {
        self.frontier_entries
    }
    #[must_use]
    pub const fn maximum_path_steps(self) -> usize {
        self.maximum_path_steps
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CpuSearchDiagnostics {
    pub examined_edges: u64,
    pub relaxation_updates: u64,
    pub finalized_nodes: u64,
    pub frontier_high_water_mark: usize,
    pub unique_present_destinations: usize,
    pub exact_destinations: usize,
    pub unreachable_destinations: usize,
    pub missing_destinations: usize,
    pub incomplete_destinations: usize,
    pub path_reconstruction_steps: u64,
}

pub fn estimate_cpu_working_set(
    image: &RoutingImage,
    request: &RoutingRequest,
) -> Result<CpuWorkingSetEstimate, RoutingError> {
    fn add(total: &mut usize, count: usize, width: usize) -> Result<(), RoutingError> {
        *total = total
            .checked_add(
                count
                    .checked_mul(width)
                    .ok_or(RoutingError::ResourceEstimateOverflow)?,
            )
            .ok_or(RoutingError::ResourceEstimateOverflow)?;
        Ok(())
    }
    let nodes = image.node_count();
    let destinations = request.destinations().len();
    let frontier_entries = image
        .adjacency_count()
        .checked_add(1)
        .ok_or(RoutingError::ResourceEstimateOverflow)?;
    let unique_present = request
        .destinations()
        .iter()
        .filter_map(|id| image.dense_node_id(*id))
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let maximum_path_steps = if request.return_paths() {
        unique_present
            .checked_mul(nodes.saturating_sub(1))
            .ok_or(RoutingError::ResourceEstimateOverflow)?
    } else {
        0
    };
    let mut bytes = 8_usize
        .checked_mul(size_of::<usize>())
        .ok_or(RoutingError::ResourceEstimateOverflow)?;
    add(
        &mut bytes,
        image.relation_kind_count(),
        size_of::<RelationUse>(),
    )?;
    add(
        &mut bytes,
        image.relation_kind_count(),
        size_of::<(pathhydra_core::RelationId, RelationUse)>(),
    )?;
    add(&mut bytes, destinations, size_of::<Option<DenseNodeId>>())?;
    add(&mut bytes, nodes, size_of::<bool>())?;
    add(&mut bytes, nodes, size_of::<Option<f64>>())?;
    add(&mut bytes, nodes, size_of::<bool>())?;
    if request.return_paths() {
        add(&mut bytes, nodes, size_of::<Option<Predecessor>>())?;
        add(&mut bytes, nodes, size_of::<Option<Arc<RoutePath>>>())?;
        add(&mut bytes, nodes, size_of::<bool>())?;
        add(&mut bytes, maximum_path_steps, size_of::<PathStep>())?;
        let shared_path_payload = size_of::<RoutePath>()
            .checked_add(2 * size_of::<usize>())
            .ok_or(RoutingError::ResourceEstimateOverflow)?;
        add(&mut bytes, unique_present, shared_path_payload)?;
    }
    add(&mut bytes, frontier_entries, size_of::<FrontierEntry>())?;
    add(&mut bytes, destinations, size_of::<DestinationResult>())?;
    let allocation_count = 8_usize
        .checked_add(if request.return_paths() {
            3_usize
                .checked_add(unique_present)
                .ok_or(RoutingError::ResourceEstimateOverflow)?
        } else {
            0
        })
        .ok_or(RoutingError::ResourceEstimateOverflow)?;
    add(&mut bytes, allocation_count, 2 * size_of::<usize>())?;
    Ok(CpuWorkingSetEstimate {
        bytes,
        frontier_entries,
        maximum_path_steps,
    })
}

#[must_use = "routing failures must be handled"]
pub fn route(
    image: &RoutingImage,
    request: &RoutingRequest,
) -> Result<RoutingResponse, RoutingError> {
    route_controlled(image, request, &NeverCancelled).map(|(response, _)| response)
}

#[must_use = "routing failures must be handled"]
pub fn route_controlled(
    image: &RoutingImage,
    request: &RoutingRequest,
    cancellation: &impl CancellationSignal,
) -> Result<(RoutingResponse, CpuSearchDiagnostics), RoutingError> {
    let origin = image
        .dense_node_id(request.origin())
        .ok_or(RoutingError::MissingOrigin(request.origin()))?;
    let profile = request.profile().pack(image)?;

    let mut dense_destinations = try_vec(request.destinations().len(), "destination mapping")?;
    dense_destinations.extend(
        request
            .destinations()
            .iter()
            .map(|&id| image.dense_node_id(id)),
    );
    let mut pending = try_filled(image.node_count(), false, "destination membership")?;
    let mut pending_count = 0_usize;
    for dense in dense_destinations.iter().flatten() {
        if !pending[dense.as_usize()] {
            pending[dense.as_usize()] = true;
            pending_count += 1;
        }
    }
    let unique_present_destinations = pending_count;
    if pending_count == 0 {
        let results = request
            .destinations()
            .iter()
            .copied()
            .map(|destination| DestinationResult::new(destination, DestinationState::MissingNode))
            .collect();
        let response = RoutingResponse::new(
            request.origin(),
            results,
            profile.canonical().clone(),
            request.tie_policy(),
            request.return_paths(),
            (0, 0),
            CompletionReason::AllDestinationsFinalized,
        );
        return Ok((
            response,
            CpuSearchDiagnostics {
                missing_destinations: request.destinations().len(),
                ..CpuSearchDiagnostics::default()
            },
        ));
    }

    let node_count = image.node_count();
    let mut distances = try_filled(node_count, None::<f64>, "distance array")?;
    let mut finalized = try_filled(node_count, false, "finalized array")?;
    let mut predecessors = request
        .return_paths()
        .then(|| try_filled(node_count, None::<Predecessor>, "predecessor array"))
        .transpose()?;
    let mut frontier = BinaryHeap::new();
    frontier
        .try_reserve(
            image
                .adjacency_count()
                .checked_add(1)
                .ok_or(RoutingError::ResourceEstimateOverflow)?,
        )
        .map_err(|_| RoutingError::AllocationFailed {
            structure: "frontier",
        })?;
    distances[origin.as_usize()] = Some(0.0);
    frontier.push(FrontierEntry {
        distance: 0.0,
        node: origin,
    });
    let mut examined_edges = 0_u64;
    let mut finalized_nodes = 0_u64;
    let mut relaxation_updates = 0_u64;
    let mut frontier_high_water_mark = 1_usize;
    let mut completion_reason;

    if cancellation.is_cancelled() {
        completion_reason = CompletionReason::Cancelled;
    } else {
        'search: loop {
            let Some(current) = frontier.pop() else {
                completion_reason = CompletionReason::FrontierExhausted;
                break;
            };
            let index = current.node.as_usize();
            if finalized[index] || distances[index] != Some(current.distance) {
                continue;
            }
            if cancellation.is_cancelled() {
                completion_reason = CompletionReason::Cancelled;
                break;
            }
            finalized[index] = true;
            finalized_nodes =
                finalized_nodes
                    .checked_add(1)
                    .ok_or(RoutingError::InternalInvariant {
                        reason: "finalized-node counter overflow",
                    })?;
            if pending[index] {
                pending[index] = false;
                pending_count -= 1;
            }
            if pending_count == 0 {
                completion_reason = CompletionReason::AllDestinationsFinalized;
                break;
            }

            let outgoing =
                image
                    .outgoing_range(current.node)
                    .ok_or(RoutingError::InternalInvariant {
                        reason: "finalized node has no bounded outgoing range",
                    })?;
            for adjacency in outgoing {
                if cancellation.is_cancelled() {
                    completion_reason = CompletionReason::Cancelled;
                    break 'search;
                }
                if !request.budget().permits(examined_edges) {
                    completion_reason = CompletionReason::BudgetExhausted;
                    break 'search;
                }
                examined_edges =
                    examined_edges
                        .checked_add(1)
                        .ok_or(RoutingError::InternalInvariant {
                            reason: "examined-edge counter overflow",
                        })?;
                let edge = image.edge_at(adjacency);
                let relation_use = profile.relation_use(image, edge.relation_id()).ok_or(
                    RoutingError::InternalInvariant {
                        reason: "edge relation is absent from packed profile",
                    },
                )?;
                let RelationUse::Enabled(multiplier) = relation_use else {
                    continue;
                };
                let destination = edge.destination();
                if finalized[destination.as_usize()] {
                    continue;
                }
                let effective = effective_weight(edge.base_weight(), multiplier)?;
                let candidate = accumulate_distance(current.distance, effective)?;
                let destination_index = destination.as_usize();
                match distances[destination_index] {
                    None => {
                        distances[destination_index] = Some(candidate);
                        set_predecessor(
                            &mut predecessors,
                            destination_index,
                            current.node,
                            edge.edge_id(),
                            adjacency,
                        );
                        frontier.push(FrontierEntry {
                            distance: candidate,
                            node: destination,
                        });
                        relaxation_updates = relaxation_updates.saturating_add(1);
                        frontier_high_water_mark = frontier_high_water_mark.max(frontier.len());
                    }
                    Some(existing) if candidate < existing => {
                        distances[destination_index] = Some(candidate);
                        set_predecessor(
                            &mut predecessors,
                            destination_index,
                            current.node,
                            edge.edge_id(),
                            adjacency,
                        );
                        frontier.push(FrontierEntry {
                            distance: candidate,
                            node: destination,
                        });
                        relaxation_updates = relaxation_updates.saturating_add(1);
                        frontier_high_water_mark = frontier_high_water_mark.max(frontier.len());
                    }
                    Some(existing) if candidate == existing => {
                        if let Some(predecessors) = &mut predecessors {
                            let proposed = (current.node, edge.edge_id());
                            let replace = predecessors[destination_index]
                                .map(|old| proposed < (old.source, old.edge))
                                .unwrap_or(true);
                            if replace {
                                predecessors[destination_index] = Some(Predecessor {
                                    source: current.node,
                                    edge: edge.edge_id(),
                                    adjacency,
                                });
                            }
                        }
                    }
                    Some(_) => {}
                }
            }
        }
    }

    let mut results = try_vec(request.destinations().len(), "response entries")?;
    let mut path_reconstruction_steps = 0_u64;
    let mut path_cache = request
        .return_paths()
        .then(|| try_filled(node_count, None::<Arc<RoutePath>>, "shared path cache"))
        .transpose()?;
    for (&destination, dense) in request.destinations().iter().zip(dense_destinations) {
        let state = if let Some(dense) = dense {
            if finalized[dense.as_usize()] || dense == origin {
                let distance =
                    distances[dense.as_usize()].ok_or(RoutingError::InternalInvariant {
                        reason: "finalized destination has no distance",
                    })?;
                let path = if let (Some(predecessors), Some(path_cache)) =
                    (&predecessors, &mut path_cache)
                {
                    if let Some(path) = &path_cache[dense.as_usize()] {
                        Some(Arc::clone(path))
                    } else {
                        let path = reconstruct_path(
                            image,
                            &profile,
                            request.origin(),
                            origin,
                            destination,
                            dense,
                            distance,
                            predecessors,
                            cancellation,
                            &mut path_reconstruction_steps,
                        )?
                        .map(Arc::new);
                        if let Some(path) = &path {
                            path_cache[dense.as_usize()] = Some(Arc::clone(path));
                        } else {
                            completion_reason = CompletionReason::Cancelled;
                        }
                        path
                    }
                } else {
                    None
                };
                DestinationState::Exact(ExactRoute::new(distance, path))
            } else if matches!(
                completion_reason,
                CompletionReason::BudgetExhausted | CompletionReason::Cancelled
            ) {
                DestinationState::Incomplete
            } else {
                DestinationState::Unreachable
            }
        } else {
            DestinationState::MissingNode
        };
        results.push(DestinationResult::new(destination, state));
    }
    let diagnostics = CpuSearchDiagnostics {
        examined_edges,
        relaxation_updates,
        finalized_nodes,
        frontier_high_water_mark,
        unique_present_destinations,
        exact_destinations: results
            .iter()
            .filter(|r| matches!(r.state(), DestinationState::Exact(_)))
            .count(),
        unreachable_destinations: results
            .iter()
            .filter(|r| matches!(r.state(), DestinationState::Unreachable))
            .count(),
        missing_destinations: results
            .iter()
            .filter(|r| matches!(r.state(), DestinationState::MissingNode))
            .count(),
        incomplete_destinations: results
            .iter()
            .filter(|r| matches!(r.state(), DestinationState::Incomplete))
            .count(),
        path_reconstruction_steps,
    };
    let response = RoutingResponse::new(
        request.origin(),
        results,
        profile.canonical().clone(),
        request.tie_policy(),
        request.return_paths(),
        (examined_edges, finalized_nodes),
        completion_reason,
    );
    Ok((response, diagnostics))
}

fn try_vec<T>(capacity: usize, structure: &'static str) -> Result<Vec<T>, RoutingError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(capacity)
        .map_err(|_| RoutingError::AllocationFailed { structure })?;
    Ok(values)
}

fn try_filled<T: Clone>(
    length: usize,
    value: T,
    structure: &'static str,
) -> Result<Vec<T>, RoutingError> {
    let mut values = try_vec(length, structure)?;
    values.resize(length, value);
    Ok(values)
}

fn set_predecessor(
    predecessors: &mut Option<Vec<Option<Predecessor>>>,
    destination: usize,
    source: DenseNodeId,
    edge: EdgeId,
    adjacency: usize,
) {
    if let Some(predecessors) = predecessors {
        predecessors[destination] = Some(Predecessor {
            source,
            edge,
            adjacency,
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn reconstruct_path(
    image: &RoutingImage,
    profile: &PackedRelationProfile,
    external_origin: pathhydra_core::NodeId,
    origin: DenseNodeId,
    external_destination: pathhydra_core::NodeId,
    destination: DenseNodeId,
    distance: f64,
    predecessors: &[Option<Predecessor>],
    cancellation: &impl CancellationSignal,
    reconstruction_steps: &mut u64,
) -> Result<Option<RoutePath>, RoutingError> {
    if origin == destination {
        return Ok(Some(RoutePath::new(
            external_origin,
            external_destination,
            distance,
            Vec::new(),
        )));
    }
    let mut current = destination;
    let mut visited = try_filled(image.node_count(), false, "path validation state")?;
    let mut reversed = try_vec(image.node_count().saturating_sub(1), "path steps")?;
    while current != origin {
        if cancellation.is_cancelled() {
            return Ok(None);
        }
        let current_index = current.as_usize();
        if visited[current_index] || reversed.len() >= image.node_count() {
            return Err(RoutingError::InternalInvariant {
                reason: "path predecessor cycle",
            });
        }
        visited[current_index] = true;
        let predecessor = predecessors.get(current_index).copied().flatten().ok_or(
            RoutingError::InternalInvariant {
                reason: "exact destination has a missing predecessor",
            },
        )?;
        let range =
            image
                .outgoing_range(predecessor.source)
                .ok_or(RoutingError::InternalInvariant {
                    reason: "predecessor source has no outgoing range",
                })?;
        if !range.contains(&predecessor.adjacency) {
            return Err(RoutingError::InternalInvariant {
                reason: "predecessor edge is outside its source range",
            });
        }
        let edge = image.edge_at(predecessor.adjacency);
        if edge.edge_id() != predecessor.edge || edge.destination() != current {
            return Err(RoutingError::InternalInvariant {
                reason: "predecessor edge endpoints or identity disagree",
            });
        }
        let RelationUse::Enabled(multiplier) = profile
            .relation_use(image, edge.relation_id())
            .ok_or(RoutingError::InternalInvariant {
                reason: "path edge relation is absent from packed profile",
            })?
        else {
            return Err(RoutingError::InternalInvariant {
                reason: "path uses a disabled relation",
            });
        };
        let effective = effective_weight(edge.base_weight(), multiplier)?;
        let source =
            image
                .external_node_id(predecessor.source)
                .ok_or(RoutingError::InternalInvariant {
                    reason: "path source dense ID is out of bounds",
                })?;
        let destination =
            image
                .external_node_id(current)
                .ok_or(RoutingError::InternalInvariant {
                    reason: "path destination dense ID is out of bounds",
                })?;
        reversed.push(PathStep::new(
            edge.edge_id(),
            source,
            destination,
            edge.relation_id(),
            edge.base_weight(),
            multiplier,
            effective,
        ));
        *reconstruction_steps = reconstruction_steps.saturating_add(1);
        current = predecessor.source;
    }
    reversed.reverse();
    let summed = reversed.iter().try_fold(0.0, |sum, step| {
        accumulate_distance(sum, step.effective_weight())
    })?;
    if summed != distance {
        return Err(RoutingError::InternalInvariant {
            reason: "path steps do not exactly reproduce distance",
        });
    }
    Ok(Some(RoutePath::new(
        external_origin,
        external_destination,
        distance,
        reversed,
    )))
}

pub fn effective_weight(
    base_weight: BaseWeight,
    multiplier: RelationMultiplier,
) -> Result<f64, ArithmeticError> {
    let effective = f64::from(base_weight.get()) * f64::from(multiplier.get());
    if effective.is_finite() {
        Ok(effective)
    } else {
        Err(ArithmeticError::new(ArithmeticOperation::EffectiveWeight))
    }
}

pub fn accumulate_distance(current: f64, effective: f64) -> Result<f64, ArithmeticError> {
    let candidate = current + effective;
    if candidate.is_finite() {
        Ok(candidate)
    } else {
        Err(ArithmeticError::new(ArithmeticOperation::PathAddition))
    }
}

#[cfg(test)]
mod tests {
    use pathhydra_core::BaseWeight;

    use super::{accumulate_distance, effective_weight};
    use crate::RelationMultiplier;

    #[test]
    fn arithmetic_is_separate_binary64_multiplication_and_addition() {
        let zero =
            effective_weight(BaseWeight::MAX, RelationMultiplier::new(0.0).unwrap()).unwrap();
        assert_eq!(zero.to_bits(), 0.0_f64.to_bits());
        let subnormal = f32::from_bits(1);
        let product = effective_weight(
            BaseWeight::new(subnormal).unwrap(),
            RelationMultiplier::new(f32::MAX).unwrap(),
        )
        .unwrap();
        assert_eq!(product, f64::from(subnormal) * f64::from(f32::MAX));
        assert_eq!(accumulate_distance(1.0, product).unwrap(), 1.0 + product);
        assert!(accumulate_distance(f64::MAX, f64::MAX).is_err());
    }
}
