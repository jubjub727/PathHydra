use pathhydra_api::{
    BatchNodeReferenceDto, BatchRelationReferenceDto, Binary32Dto, CandidateBatchEntryDto,
    ConfirmCandidateBatchRequestDto, ConfirmedRecordDto, DecimalU64Dto,
    InsertCandidateBatchRequestDto, MutationDurableResultDto, PathHydra, PayloadDto,
    PublicationOutcomeDto, decode_strict_canonical, encode,
};
use tempfile::TempDir;

#[test]
fn canonical_mixed_batch_has_aligned_results_one_publication_and_usage_cleanup() {
    let directory = TempDir::new().unwrap();
    let api = PathHydra::open(directory.path().join("catalog")).unwrap();
    let request = InsertCandidateBatchRequestDto {
        entries: vec![
            CandidateBatchEntryDto::Edge {
                source: BatchNodeReferenceDto::BatchNode {
                    entry_index: DecimalU64Dto::from_u64(2),
                },
                destination: BatchNodeReferenceDto::BatchNode {
                    entry_index: DecimalU64Dto::from_u64(1),
                },
                relation_kind: BatchRelationReferenceDto::BatchRelationKind {
                    entry_index: DecimalU64Dto::from_u64(3),
                },
                base_weight: Binary32Dto::from_bits(0.25_f32.to_bits()),
            },
            CandidateBatchEntryDto::Node {
                exact_name: "destination".to_owned(),
                payload: PayloadDto::from_bytes(&[0xff]),
            },
            CandidateBatchEntryDto::Node {
                exact_name: "source".to_owned(),
                payload: PayloadDto::from_bytes(&[]),
            },
            CandidateBatchEntryDto::RelationKind {
                exact_name: "kind".to_owned(),
            },
        ],
    };
    let encoded = encode(&request, &api.limits()).unwrap();
    assert_eq!(
        decode_strict_canonical::<InsertCandidateBatchRequestDto>(&encoded, &api.limits()).unwrap(),
        request
    );
    let inserted = api.insert_candidate_batch(&request).unwrap();
    assert_eq!(
        inserted
            .candidate_ids
            .iter()
            .map(|id| id.as_str())
            .collect::<Vec<_>>(),
        ["1", "2", "3", "4"]
    );
    assert!(api.lookup_node_exact("source").unwrap().is_none());

    let confirmed = api
        .confirm_candidate_batch(&ConfirmCandidateBatchRequestDto {
            candidate_ids: inserted.candidate_ids.clone(),
        })
        .unwrap();
    assert!(confirmed.graph_changed);
    assert!(matches!(
        confirmed.publication,
        PublicationOutcomeDto::Published { .. } | PublicationOutcomeDto::RoutingUnavailable { .. }
    ));
    let ConfirmedRecordDto::Edge(edge) = &confirmed.entries[0].record else {
        panic!("first aligned result must be the edge")
    };
    let ConfirmedRecordDto::RelationKind(relation) = &confirmed.entries[3].record else {
        panic!("fourth aligned result must be the relation kind")
    };
    let usage = api.most_used_relation_kinds(10).unwrap();
    assert_eq!(usage.relation_kinds.len(), 1);
    assert_eq!(usage.relation_kinds[0].relation_kind.id, relation.id);
    assert_eq!(usage.relation_kinds[0].confirmed_edge_count.as_str(), "1");
    assert_eq!(
        usage.relation_kinds[0].provisional_reference_count.as_str(),
        "0"
    );

    let deletion = api.remove_confirmed_edge(&edge.id).unwrap();
    assert!(matches!(
        deletion.durable_result,
        MutationDurableResultDto::EdgeRemoved {
            removed_relation_kinds,
            ..
        } if removed_relation_kinds == [relation.id.clone()]
    ));
    assert!(
        api.most_used_relation_kinds(10)
            .unwrap()
            .relation_kinds
            .is_empty()
    );
}

#[test]
fn duplicate_name_only_batch_consumes_candidates_without_publication() {
    let directory = TempDir::new().unwrap();
    let api = PathHydra::open(directory.path().join("catalog")).unwrap();
    let first = api
        .insert_node_candidate("same".to_owned(), PayloadDto::from_bytes(&[]))
        .unwrap();
    api.confirm_candidate(&first).unwrap();
    let inserted = api
        .insert_candidate_batch(&InsertCandidateBatchRequestDto {
            entries: vec![
                CandidateBatchEntryDto::Node {
                    exact_name: "same".to_owned(),
                    payload: PayloadDto::from_bytes(&[1]),
                },
                CandidateBatchEntryDto::Node {
                    exact_name: "same".to_owned(),
                    payload: PayloadDto::from_bytes(&[2]),
                },
            ],
        })
        .unwrap();
    let confirmed = api
        .confirm_candidate_batch(&ConfirmCandidateBatchRequestDto {
            candidate_ids: inserted.candidate_ids,
        })
        .unwrap();
    assert!(!confirmed.graph_changed);
    assert_eq!(confirmed.created_nodes.as_str(), "0");
    assert_eq!(confirmed.candidates_consumed.as_str(), "2");
    assert_eq!(confirmed.publication, PublicationOutcomeDto::NotRequired);
}
