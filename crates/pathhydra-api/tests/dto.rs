use pathhydra_api::{
    ApiError, ApiErrorCategory, ApiErrorCategoryDto, ApiErrorDto, ApiLimits, ApiLimitsDto,
    Binary32Dto, Binary64Dto, CandidateDto, ConfirmedRecordDto, DecimalU64Dto, DtoValidationError,
    EdgeHandleDto, NodeIdDto, PathHydraConfigDto, PayloadDto, RelationIdDto, RelationProfileDto,
    RelationProfileEntryDto, RelationUseDto, RoutingRequestDto, SearchBudgetDto,
    SubgraphHandlesDto, TiePolicyDto, decode, encode,
};
use pathhydra_core::{
    BaseWeight, Candidate, CandidateId, ConfirmedRecord, EdgeId, EdgeRecord, NodeId, NodeName,
    NodePayload, NodeRecord, RelationId, RelationName, RelationRecord,
};
use pathhydra_routing::{RelationUse, RoutingRequest, SearchBudget, TiePolicy};
use pathhydra_subgraph::Subgraph;

#[test]
fn canonical_scalar_wrappers_round_trip_and_reject_aliases() {
    let maximum = DecimalU64Dto::from_u64(u64::MAX);
    assert_eq!(maximum.as_str(), "18446744073709551615");
    assert_eq!(
        serde_json::to_string(&maximum).unwrap(),
        "\"18446744073709551615\""
    );
    assert_eq!(
        serde_json::from_str::<DecimalU64Dto>("\"0\"")
            .unwrap()
            .as_u64()
            .unwrap(),
        0
    );
    for alias in [
        "\"00\"",
        "\"01\"",
        "\"+1\"",
        "1",
        "\"18446744073709551616\"",
    ] {
        assert!(
            serde_json::from_str::<DecimalU64Dto>(alias).is_err(),
            "accepted {alias}"
        );
    }

    assert_eq!(Binary32Dto::from_bits(0x3f80_0000).as_str(), "0x3f800000");
    assert_eq!(
        Binary64Dto::from_bits(0x3ff0_0000_0000_0000).as_str(),
        "0x3ff0000000000000"
    );
    for alias in ["\"3f800000\"", "\"0x3F800000\"", "\"0x0000000\""] {
        assert!(
            serde_json::from_str::<Binary32Dto>(alias).is_err(),
            "accepted {alias}"
        );
    }

    let payload = PayloadDto::from_bytes(&[0, 1, 2, 0xff]);
    assert_eq!(payload.as_str(), "AAEC/w==");
    assert_eq!(payload.decode().unwrap(), vec![0, 1, 2, 0xff]);
    for alias in ["\"AAEC/w\"", "\"AAEC_w==\"", "\"AAEC/w=\""] {
        assert!(
            serde_json::from_str::<PayloadDto>(alias).is_err(),
            "accepted {alias}"
        );
    }

    assert_eq!(DecimalU64Dto::default().as_str(), "0");
    assert_eq!(Binary32Dto::default().as_str(), "0x00000000");
}

#[test]
fn profile_validation_requires_unique_increasing_relation_kinds() {
    let enabled = |id| RelationProfileEntryDto {
        relation_kind: RelationIdDto::from_u64(id),
        relation_use: RelationUseDto::Enabled {
            multiplier: Binary32Dto::from_bits(1.5_f32.to_bits()),
        },
    };
    assert!(
        RelationProfileDto {
            entries: vec![enabled(1), enabled(2)]
        }
        .validate()
        .is_ok()
    );
    assert!(matches!(
        RelationProfileDto {
            entries: vec![enabled(1), enabled(1)]
        }
        .validate(),
        Err(DtoValidationError::DuplicateProfileRelation { .. })
    ));
    assert_eq!(
        RelationProfileDto {
            entries: vec![enabled(2), enabled(1)]
        }
        .validate(),
        Err(DtoValidationError::UnorderedProfile)
    );
}

#[test]
fn routing_request_conversion_preserves_order_profile_policy_and_budget() {
    let dto = RoutingRequestDto {
        origin: NodeIdDto::from_u64(9),
        destinations: vec![NodeIdDto::from_u64(4), NodeIdDto::from_u64(7)],
        profile: RelationProfileDto {
            entries: vec![
                RelationProfileEntryDto {
                    relation_kind: RelationIdDto::from_u64(2),
                    relation_use: RelationUseDto::Disabled,
                },
                RelationProfileEntryDto {
                    relation_kind: RelationIdDto::from_u64(3),
                    relation_use: RelationUseDto::Enabled {
                        multiplier: Binary32Dto::from_bits(2.25_f32.to_bits()),
                    },
                },
            ],
        },
        return_paths: true,
        budget: SearchBudgetDto::ExaminedEdges {
            maximum: DecimalU64Dto::from_u64(u64::MAX),
        },
        tie_policy: TiePolicyDto::StablePredecessor,
    };

    let request = RoutingRequest::try_from(&dto).unwrap();
    assert_eq!(request.origin(), NodeId::from_u64(9));
    assert_eq!(
        request.destinations(),
        &[NodeId::from_u64(4), NodeId::from_u64(7)]
    );
    assert!(request.return_paths());
    assert_eq!(request.budget(), SearchBudget::ExaminedEdges(u64::MAX));
    assert_eq!(request.tie_policy(), TiePolicy::StablePredecessor);
    assert_eq!(
        request.profile().entries()[0],
        (RelationId::from_u64(2), RelationUse::Disabled)
    );
    let RelationUse::Enabled(multiplier) = request.profile().entries()[1].1 else {
        panic!("enabled profile entry expected")
    };
    assert_eq!(multiplier.to_bits(), 2.25_f32.to_bits());
}

#[test]
fn subgraph_validation_rejects_order_and_endpoint_conflicts() {
    let valid = SubgraphHandlesDto {
        nodes: vec![NodeIdDto::from_u64(1), NodeIdDto::from_u64(2)],
        edges: vec![EdgeHandleDto {
            edge: pathhydra_api::EdgeIdDto::from_u64(5),
            source: NodeIdDto::from_u64(1),
            destination: NodeIdDto::from_u64(2),
        }],
    };
    valid.validate().unwrap();
    let subgraph = Subgraph::try_from(&valid).unwrap();
    let handles = subgraph.handles();
    assert_eq!(handles.nodes(), &[NodeId::from_u64(1), NodeId::from_u64(2)]);
    assert_eq!(handles.edges().len(), 1);

    let duplicate_nodes = SubgraphHandlesDto {
        nodes: vec![NodeIdDto::from_u64(1), NodeIdDto::from_u64(1)],
        edges: vec![],
    };
    assert_eq!(
        duplicate_nodes.validate(),
        Err(DtoValidationError::UnorderedSubgraphNodes)
    );
    let missing_endpoint = SubgraphHandlesDto {
        nodes: vec![NodeIdDto::from_u64(1)],
        edges: valid.edges.clone(),
    };
    assert!(matches!(
        missing_endpoint.validate(),
        Err(DtoValidationError::MissingSubgraphEndpoint { .. })
    ));
}

#[test]
fn api_limits_have_an_exact_owned_dto_round_trip() {
    let limits = ApiLimits {
        maximum_encoded_bytes: 1,
        maximum_name_bytes: 2,
        maximum_payload_bytes: 3,
        maximum_destinations: 4,
        maximum_profile_entries: 5,
        maximum_path_steps: 6,
        maximum_hydration_handles: 7,
        maximum_subgraph_nodes: 8,
        maximum_subgraph_edges: 9,
        maximum_diagnostic_text_bytes: 10,
        maximum_nesting_depth: 11,
        maximum_json_values: 12,
    };
    let dto = ApiLimitsDto::from(&limits);
    assert_eq!(dto.maximum_json_values.as_str(), "12");
    assert_eq!(ApiLimits::try_from(&dto).unwrap(), limits);
}

#[test]
fn deny_unknown_fields_is_enforced() {
    let json = r#"{"origin":"1","destinations":[],"profile":{"entries":[]},"return_paths":false,"budget":{"kind":"unlimited"},"tie_policy":"stable_predecessor","surprise":true}"#;
    assert!(serde_json::from_str::<RoutingRequestDto>(json).is_err());
}

#[test]
fn every_candidate_and_confirmed_record_shape_is_owned_and_lossless() {
    let node_candidate = Candidate::Node {
        id: CandidateId::from_u64(u64::MAX),
        name: NodeName::new("Token\u{301} !"),
        payload: NodePayload::from([0, 0xff, 1]),
    };
    let relation_candidate = Candidate::Relation {
        id: CandidateId::from_u64(2),
        name: RelationName::new("RelATION?"),
    };
    let edge_candidate = Candidate::Edge {
        id: CandidateId::from_u64(3),
        source: NodeId::from_u64(4),
        destination: NodeId::from_u64(5),
        relation_kind: RelationId::from_u64(6),
        base_weight: BaseWeight::new(0.25).unwrap(),
    };
    assert!(
        matches!(CandidateDto::from(&node_candidate), CandidateDto::Node { id, name, payload } if id.as_str() == u64::MAX.to_string() && name == "Token\u{301} !" && payload.decode().unwrap() == [0, 0xff, 1])
    );
    assert!(
        matches!(CandidateDto::from(&relation_candidate), CandidateDto::RelationKind { id, name } if id.as_str() == "2" && name == "RelATION?")
    );
    assert!(
        matches!(CandidateDto::from(&edge_candidate), CandidateDto::Edge { id, source, destination, relation_kind, base_weight } if id.as_str() == "3" && source.as_str() == "4" && destination.as_str() == "5" && relation_kind.as_str() == "6" && base_weight.bits().unwrap() == 0.25_f32.to_bits())
    );

    let confirmed = [
        ConfirmedRecord::Node(NodeRecord::new(
            NodeId::from_u64(10),
            NodeName::new("Case"),
            NodePayload::from([0x80]),
        )),
        ConfirmedRecord::Relation(RelationRecord::new(
            RelationId::from_u64(11),
            RelationName::new("case"),
        )),
        ConfirmedRecord::Edge(EdgeRecord::new(
            EdgeId::from_u64(12),
            NodeId::from_u64(10),
            NodeId::from_u64(13),
            RelationId::from_u64(11),
            BaseWeight::new(1.0).unwrap(),
        )),
    ];
    assert!(
        matches!(ConfirmedRecordDto::from(&confirmed[0]), ConfirmedRecordDto::Node(node) if node.name == "Case" && node.payload.decode().unwrap() == [0x80])
    );
    assert!(
        matches!(ConfirmedRecordDto::from(&confirmed[1]), ConfirmedRecordDto::RelationKind(relation) if relation.name == "case")
    );
    assert!(
        matches!(ConfirmedRecordDto::from(&confirmed[2]), ConfirmedRecordDto::Edge(edge) if edge.id.as_str() == "12" && edge.base_weight.bits().unwrap() == 1.0_f32.to_bits())
    );
}

#[test]
fn pathhydra_config_round_trip_preserves_non_path_policy_and_cuda_selection() {
    let config = pathhydra_engine::EngineConfig {
        routing_image_root: None,
        startup_bundle_policy: pathhydra_engine::StartupBundlePolicy::RequireValidBundle,
        cuda: pathhydra_engine::CudaConfig {
            device_ordinal: 17,
            ..pathhydra_engine::CudaConfig::default()
        },
        ..pathhydra_engine::EngineConfig::default()
    };
    let dto = PathHydraConfigDto::from(&config);
    assert_eq!(dto.cuda.device_ordinal.as_u64().unwrap(), 17);
    let restored = pathhydra_engine::EngineConfig::try_from(&dto).unwrap();
    assert_eq!(restored.cuda.device_ordinal, 17);
    assert_eq!(
        restored.startup_bundle_policy,
        pathhydra_engine::StartupBundlePolicy::RequireValidBundle
    );
    assert_eq!(restored.routing_image_root, None);
}

#[test]
fn every_error_category_has_a_safe_owned_encoded_shape() {
    let limits = ApiLimits::default();
    let categories = [
        ApiErrorCategory::InvalidInput,
        ApiErrorCategory::InvalidEncoding,
        ApiErrorCategory::InvalidProfile,
        ApiErrorCategory::InvalidWeight,
        ApiErrorCategory::MissingCandidate,
        ApiErrorCategory::MissingConfirmed,
        ApiErrorCategory::InvalidCandidateTransition,
        ApiErrorCategory::ExactNameConflict,
        ApiErrorCategory::DurableStore,
        ApiErrorCategory::Corruption,
        ApiErrorCategory::Recovery,
        ApiErrorCategory::RoutingUnavailable,
        ApiErrorCategory::RoutingImageCorruption,
        ApiErrorCategory::AdmissionLimit,
        ApiErrorCategory::ResourceLimit,
        ApiErrorCategory::Cancellation,
        ApiErrorCategory::HydrationUnavailable,
        ApiErrorCategory::HydrationIntegrity,
        ApiErrorCategory::SubgraphConflict,
        ApiErrorCategory::CudaUnavailable,
        ApiErrorCategory::CudaIneligible,
        ApiErrorCategory::CudaDeviceFailure,
        ApiErrorCategory::Backup,
        ApiErrorCategory::Restore,
        ApiErrorCategory::Maintenance,
        ApiErrorCategory::Shutdown,
        ApiErrorCategory::InternalInvariant,
    ];
    let expected = [
        ApiErrorCategoryDto::InvalidInput,
        ApiErrorCategoryDto::InvalidEncoding,
        ApiErrorCategoryDto::InvalidProfile,
        ApiErrorCategoryDto::InvalidWeight,
        ApiErrorCategoryDto::MissingCandidate,
        ApiErrorCategoryDto::MissingConfirmed,
        ApiErrorCategoryDto::InvalidCandidateTransition,
        ApiErrorCategoryDto::ExactNameConflict,
        ApiErrorCategoryDto::DurableStore,
        ApiErrorCategoryDto::Corruption,
        ApiErrorCategoryDto::Recovery,
        ApiErrorCategoryDto::RoutingUnavailable,
        ApiErrorCategoryDto::RoutingImageCorruption,
        ApiErrorCategoryDto::AdmissionLimit,
        ApiErrorCategoryDto::ResourceLimit,
        ApiErrorCategoryDto::Cancellation,
        ApiErrorCategoryDto::HydrationUnavailable,
        ApiErrorCategoryDto::HydrationIntegrity,
        ApiErrorCategoryDto::SubgraphConflict,
        ApiErrorCategoryDto::CudaUnavailable,
        ApiErrorCategoryDto::CudaIneligible,
        ApiErrorCategoryDto::CudaDeviceFailure,
        ApiErrorCategoryDto::Backup,
        ApiErrorCategoryDto::Restore,
        ApiErrorCategoryDto::Maintenance,
        ApiErrorCategoryDto::Shutdown,
        ApiErrorCategoryDto::InternalInvariant,
    ];
    let secret_path = r"C:\private\tenant\secret.db";
    let secret_payload = "private-payload-bytes";
    for (category, expected_category) in categories.into_iter().zip(expected) {
        let dto = ApiErrorDto::from(&ApiError::new(
            category,
            "stable_code",
            "safe consumer-facing message",
            false,
        ));
        assert_eq!(dto.category, expected_category);
        let bytes = encode(&dto, &limits).unwrap();
        let round_trip = decode::<ApiErrorDto>(&bytes, &limits).unwrap();
        assert_eq!(round_trip, dto);
        let encoded = String::from_utf8(bytes).unwrap();
        assert!(!encoded.contains(secret_path));
        assert!(!encoded.contains(secret_payload));
    }

    let mapped = [
        ApiError::from(pathhydra_engine::EngineError::CudaFailure(
            secret_path.to_owned(),
        )),
        ApiError::from(pathhydra_engine::EngineError::Hydration(
            pathhydra_engine::HydrationError::Integrity {
                reason: secret_payload.to_owned(),
            },
        )),
    ];
    for error in mapped {
        let encoded =
            String::from_utf8(encode(&ApiErrorDto::from(&error), &limits).unwrap()).unwrap();
        assert!(!encoded.contains(secret_path));
        assert!(!encoded.contains(secret_payload));
    }

    let constrained = ApiLimits {
        maximum_diagnostic_text_bytes: 4,
        ..ApiLimits::default()
    };
    let oversized = ApiErrorDto::from(&ApiError::new(
        ApiErrorCategory::InvalidInput,
        "five!",
        "safe",
        false,
    ));
    assert!(encode(&oversized, &constrained).is_err());
}
