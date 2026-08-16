use pathhydra_core::{BaseWeight, EdgeId, NodeId, RelationId};
use pathhydra_store::ConfirmedGraphRecords;

use crate::{CompileError, DenseNodeId, RoutingImage, image::manifest};

#[derive(Clone, Copy)]
struct CompiledEdge {
    destination: DenseNodeId,
    relation: RelationId,
    base_weight: BaseWeight,
    edge: EdgeId,
}

pub fn compile_routing_image(
    records: &ConfirmedGraphRecords,
) -> Result<RoutingImage, CompileError> {
    compile_routing_image_impl(records, None)
}

pub fn compile_routing_image_with_limit(
    records: &ConfirmedGraphRecords,
    maximum_bytes: usize,
) -> Result<RoutingImage, CompileError> {
    compile_routing_image_impl(records, Some(maximum_bytes))
}

fn compile_routing_image_impl(
    records: &ConfirmedGraphRecords,
    maximum_bytes: Option<usize>,
) -> Result<RoutingImage, CompileError> {
    let node_count = records.nodes().len();
    validate_node_count(node_count)?;
    let topology_manifest = manifest(
        node_count,
        records.relation_kinds().len(),
        records.edges().len(),
    )?;
    let required =
        topology_manifest
            .byte_counts()
            .checked_total()
            .ok_or(CompileError::CountOverflow {
                structure: "byte total",
            })?;
    if let Some(limit) = maximum_bytes.filter(|limit| required > *limit) {
        return Err(CompileError::TopologyLimitExceeded { required, limit });
    }

    let mut node_ids = try_vec(records.nodes().len(), "node IDs")?;
    node_ids.extend(records.nodes().iter().map(|record| record.id()));
    node_ids.sort_unstable();
    reject_duplicate(&node_ids, CompileError::DuplicateNodeId)?;
    let mut external_to_dense = try_vec(node_count, "external node mapping")?;
    for (dense, external) in node_ids.iter().enumerate() {
        let dense =
            u32::try_from(dense).map_err(|_| CompileError::TooManyNodes { count: node_count })?;
        external_to_dense.push((*external, DenseNodeId::from_u32(dense)));
    }

    let mut relation_ids = try_vec(records.relation_kinds().len(), "relation IDs")?;
    relation_ids.extend(records.relation_kinds().iter().map(|record| record.id()));
    relation_ids.sort_unstable();
    reject_duplicate(&relation_ids, CompileError::DuplicateRelationId)?;

    let mut edge_ids = try_vec(records.edges().len(), "edge IDs")?;
    edge_ids.extend(records.edges().iter().map(|record| record.id()));
    edge_ids.sort_unstable();
    reject_duplicate(&edge_ids, CompileError::DuplicateEdgeId)?;

    let mut outgoing = try_vec(node_count, "outgoing compiler buckets")?;
    outgoing.resize_with(node_count, Vec::new);
    let mut counted_edges = 0_u64;
    for edge in records.edges() {
        let source =
            find_dense(&external_to_dense, edge.source()).ok_or(CompileError::MissingEndpoint {
                edge: edge.id(),
                node: edge.source(),
            })?;
        let destination = find_dense(&external_to_dense, edge.destination()).ok_or(
            CompileError::MissingEndpoint {
                edge: edge.id(),
                node: edge.destination(),
            },
        )?;
        if relation_ids.binary_search(&edge.relation_kind()).is_err() {
            return Err(CompileError::MissingRelationKind {
                edge: edge.id(),
                relation: edge.relation_kind(),
            });
        }
        let base_weight = BaseWeight::from_bits(edge.base_weight().to_bits())
            .map_err(|_| CompileError::InvalidBaseWeight { edge: edge.id() })?;
        counted_edges = counted_edges
            .checked_add(1)
            .ok_or(CompileError::CountOverflow {
                structure: "adjacency",
            })?;
        outgoing[source.as_usize()]
            .try_reserve(1)
            .map_err(|_| CompileError::AllocationFailed {
                structure: "outgoing compiler bucket",
            })?;
        outgoing[source.as_usize()].push(CompiledEdge {
            destination,
            relation: edge.relation_kind(),
            base_weight,
            edge: edge.id(),
        });
    }

    for edges in &mut outgoing {
        edges.sort_unstable_by_key(|edge| edge.edge);
    }
    let adjacency_count =
        usize::try_from(counted_edges).map_err(|_| CompileError::CountOverflow {
            structure: "adjacency",
        })?;
    let offset_count = node_count
        .checked_add(1)
        .ok_or(CompileError::CountOverflow {
            structure: "offset",
        })?;
    let mut offsets = try_vec(offset_count, "CSR offsets")?;
    offsets.push(0_u64);
    for edges in &outgoing {
        let next = offsets
            .last()
            .copied()
            .unwrap_or(0)
            .checked_add(
                u64::try_from(edges.len()).map_err(|_| CompileError::CountOverflow {
                    structure: "adjacency",
                })?,
            )
            .ok_or(CompileError::CountOverflow {
                structure: "offset",
            })?;
        offsets.push(next);
    }

    let mut destinations = try_vec(adjacency_count, "adjacency destinations")?;
    let mut adjacency_relations = try_vec(adjacency_count, "adjacency relation IDs")?;
    let mut relation_indexes = try_vec(adjacency_count, "adjacency relation indexes")?;
    let mut base_weight_bits = try_vec(adjacency_count, "adjacency base weights")?;
    let mut adjacency_edge_ids = try_vec(adjacency_count, "adjacency edge IDs")?;
    for edge in outgoing.into_iter().flatten() {
        destinations.push(edge.destination.as_u32());
        adjacency_relations.push(edge.relation);
        let relation_index =
            relation_ids
                .binary_search(&edge.relation)
                .map_err(|_| CompileError::InvalidImage {
                    reason: "compiled relation is absent from the dense relation table",
                })?;
        relation_indexes.push(u32::try_from(relation_index).map_err(|_| {
            CompileError::CountOverflow {
                structure: "relation index",
            }
        })?);
        base_weight_bits.push(edge.base_weight.to_bits());
        adjacency_edge_ids.push(edge.edge);
    }
    if destinations.len() != adjacency_count || offsets.len() != node_count + 1 {
        return Err(CompileError::InvalidImage {
            reason: "compiled array lengths disagree",
        });
    }
    if offsets.last().copied() != Some(counted_edges) {
        return Err(CompileError::InvalidImage {
            reason: "final CSR offset disagrees with adjacency count",
        });
    }

    let image = RoutingImage {
        external_to_dense: external_to_dense.into_boxed_slice(),
        dense_to_external: node_ids.into_boxed_slice(),
        offsets: offsets.into_boxed_slice(),
        destinations: destinations.into_boxed_slice(),
        relation_ids: adjacency_relations.into_boxed_slice(),
        relation_indexes: relation_indexes.into_boxed_slice(),
        base_weight_bits: base_weight_bits.into_boxed_slice(),
        edge_ids: adjacency_edge_ids.into_boxed_slice(),
        confirmed_relation_ids: relation_ids.into_boxed_slice(),
        manifest: topology_manifest,
    };
    validate_completed_image(&image)?;
    Ok(image)
}

fn try_vec<T>(capacity: usize, structure: &'static str) -> Result<Vec<T>, CompileError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(capacity)
        .map_err(|_| CompileError::AllocationFailed { structure })?;
    Ok(values)
}

fn validate_node_count(count: usize) -> Result<(), CompileError> {
    u32::try_from(count)
        .map(|_| ())
        .map_err(|_| CompileError::TooManyNodes { count })
}

fn validate_completed_image(image: &RoutingImage) -> Result<(), CompileError> {
    let node_count = image.dense_to_external.len();
    let adjacency_count = image.edge_ids.len();
    let offset_count = node_count
        .checked_add(1)
        .ok_or(CompileError::CountOverflow {
            structure: "offset",
        })?;
    if image.external_to_dense.len() != node_count
        || image.offsets.len() != offset_count
        || image.destinations.len() != adjacency_count
        || image.relation_ids.len() != adjacency_count
        || image.relation_indexes.len() != adjacency_count
        || image.base_weight_bits.len() != adjacency_count
    {
        return Err(CompileError::InvalidImage {
            reason: "completed array lengths disagree",
        });
    }
    for (index, &(external, dense)) in image.external_to_dense.iter().enumerate() {
        if dense.as_usize() != index || image.dense_to_external[index] != external {
            return Err(CompileError::InvalidImage {
                reason: "external and dense node mappings disagree",
            });
        }
    }
    let final_offset = u64::try_from(adjacency_count).map_err(|_| CompileError::CountOverflow {
        structure: "adjacency",
    })?;
    if image.offsets.first().copied() != Some(0)
        || image.offsets.last().copied() != Some(final_offset)
        || image.offsets.windows(2).any(|pair| pair[0] > pair[1])
    {
        return Err(CompileError::InvalidImage {
            reason: "CSR offsets are not bounded and monotonic",
        });
    }
    for source in 0..node_count {
        let start =
            usize::try_from(image.offsets[source]).map_err(|_| CompileError::InvalidImage {
                reason: "CSR offset does not fit usize",
            })?;
        let end =
            usize::try_from(image.offsets[source + 1]).map_err(|_| CompileError::InvalidImage {
                reason: "CSR offset does not fit usize",
            })?;
        if end > adjacency_count
            || image.edge_ids[start..end]
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
        {
            return Err(CompileError::InvalidImage {
                reason: "outgoing range is invalid or not strictly ordered by edge ID",
            });
        }
    }
    for index in 0..adjacency_count {
        if usize::try_from(image.destinations[index]).map_or(true, |value| value >= node_count)
            || image
                .confirmed_relation_ids
                .binary_search(&image.relation_ids[index])
                .is_err()
            || usize::try_from(image.relation_indexes[index]).map_or(true, |relation_index| {
                image.confirmed_relation_ids.get(relation_index) != Some(&image.relation_ids[index])
            })
            || BaseWeight::from_bits(image.base_weight_bits[index]).is_err()
        {
            return Err(CompileError::InvalidImage {
                reason: "adjacency entry contains an invalid node, relation, or weight",
            });
        }
    }
    Ok(())
}

fn find_dense(mapping: &[(NodeId, DenseNodeId)], id: NodeId) -> Option<DenseNodeId> {
    mapping
        .binary_search_by_key(&id, |(external, _)| *external)
        .ok()
        .map(|index| mapping[index].1)
}

fn reject_duplicate<T: Copy + Eq>(
    values: &[T],
    error: impl Fn(T) -> CompileError,
) -> Result<(), CompileError> {
    if let Some(pair) = values.windows(2).find(|pair| pair[0] == pair[1]) {
        Err(error(pair[0]))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::validate_node_count;
    use crate::CompileError;

    #[test]
    fn rejects_representational_node_count_overflow_without_allocating_it() {
        if let Ok(too_many) = usize::try_from(u64::from(u32::MAX) + 1) {
            assert!(matches!(
                validate_node_count(too_many),
                Err(CompileError::TooManyNodes { count }) if count == too_many
            ));
        }
    }
}
