use std::{
    cmp::Ordering,
    collections::{BinaryHeap, HashSet},
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

#[must_use = "routing failures must be handled"]
pub fn route(
    image: &RoutingImage,
    request: &RoutingRequest,
) -> Result<RoutingResponse, RoutingError> {
    let origin = image
        .dense_node_id(request.origin())
        .ok_or(RoutingError::MissingOrigin(request.origin()))?;
    let profile = request.profile().pack(image)?;

    let dense_destinations: Vec<_> = request
        .destinations()
        .iter()
        .map(|&id| image.dense_node_id(id))
        .collect();
    let mut pending: HashSet<DenseNodeId> = dense_destinations.iter().flatten().copied().collect();
    if pending.is_empty() {
        let results = request
            .destinations()
            .iter()
            .copied()
            .map(|destination| DestinationResult::new(destination, DestinationState::MissingNode))
            .collect();
        return Ok(RoutingResponse::new(
            request.origin(),
            results,
            profile.canonical().clone(),
            request.tie_policy(),
            request.return_paths(),
            (0, 0),
            CompletionReason::AllDestinationsFinalized,
        ));
    }

    let node_count = image.node_count();
    let mut distances = vec![None::<f64>; node_count];
    let mut finalized = vec![false; node_count];
    let mut predecessors = request
        .return_paths()
        .then(|| vec![None::<Predecessor>; node_count]);
    let mut frontier = BinaryHeap::new();
    distances[origin.as_usize()] = Some(0.0);
    frontier.push(FrontierEntry {
        distance: 0.0,
        node: origin,
    });
    let mut examined_edges = 0_u64;
    let mut finalized_nodes = 0_u64;
    let completion_reason;

    'search: loop {
        let Some(current) = frontier.pop() else {
            completion_reason = CompletionReason::FrontierExhausted;
            break;
        };
        let index = current.node.as_usize();
        if finalized[index] || distances[index] != Some(current.distance) {
            continue;
        }
        finalized[index] = true;
        finalized_nodes =
            finalized_nodes
                .checked_add(1)
                .ok_or(RoutingError::InternalInvariant {
                    reason: "finalized-node counter overflow",
                })?;
        pending.remove(&current.node);
        if pending.is_empty() {
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

    let mut results = Vec::with_capacity(request.destinations().len());
    for (&destination, dense) in request.destinations().iter().zip(dense_destinations) {
        let state = if let Some(dense) = dense {
            if finalized[dense.as_usize()] {
                let distance =
                    distances[dense.as_usize()].ok_or(RoutingError::InternalInvariant {
                        reason: "finalized destination has no distance",
                    })?;
                let path = if let Some(predecessors) = &predecessors {
                    Some(reconstruct_path(
                        image,
                        &profile,
                        request.origin(),
                        origin,
                        destination,
                        dense,
                        distance,
                        predecessors,
                    )?)
                } else {
                    None
                };
                DestinationState::Exact(ExactRoute::new(distance, path))
            } else if completion_reason == CompletionReason::BudgetExhausted {
                DestinationState::Incomplete
            } else {
                DestinationState::Unreachable
            }
        } else {
            DestinationState::MissingNode
        };
        results.push(DestinationResult::new(destination, state));
    }
    Ok(RoutingResponse::new(
        request.origin(),
        results,
        profile.canonical().clone(),
        request.tie_policy(),
        request.return_paths(),
        (examined_edges, finalized_nodes),
        completion_reason,
    ))
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
) -> Result<RoutePath, RoutingError> {
    if origin == destination {
        return Ok(RoutePath::new(
            external_origin,
            external_destination,
            distance,
            Vec::new(),
        ));
    }
    let mut current = destination;
    let mut visited = vec![false; image.node_count()];
    let mut reversed = Vec::new();
    while current != origin {
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
    Ok(RoutePath::new(
        external_origin,
        external_destination,
        distance,
        reversed,
    ))
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
