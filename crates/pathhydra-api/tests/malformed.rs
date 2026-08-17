use pathhydra_api::{
    ApiCodecError, ApiErrorCategoryDto, ApiErrorDto, ApiLimits, Binary32Dto, Binary64Dto,
    CanonicalDto, CodecLimit, EdgeHandleDto, EdgeIdDto, HydrationRequestDto, NodeIdDto,
    PathStepDto, RelationIdDto, RelationProfileDto, RelationProfileEntryDto, RelationUseDto,
    RoutePathDto, RoutingRequestDto, SearchBudgetDto, SubgraphHandlesDto, TiePolicyDto, decode,
    encode,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictFixture {
    id: String,
    values: Vec<String>,
}

impl CanonicalDto for StrictFixture {
    fn validate_boundary(&self, _limits: &ApiLimits) -> Result<(), ApiCodecError> {
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(transparent)]
struct JsonDocument(serde_json::Value);

impl CanonicalDto for JsonDocument {
    fn validate_boundary(&self, _limits: &ApiLimits) -> Result<(), ApiCodecError> {
        Ok(())
    }
}

#[test]
fn duplicate_unknown_trailing_bom_and_utf8_are_rejected() {
    let limits = ApiLimits::default();
    for input in [
        br#"{"id":"1","id":"2","values":[]}"#.as_slice(),
        br#"{"id":"1","values":[],"unexpected":true}"#.as_slice(),
        br#"{"id":"1","values":[]} null"#.as_slice(),
        b"\xef\xbb\xbf{\"id\":\"1\",\"values\":[]}".as_slice(),
        b"{\"id\":\"\xff\",\"values\":[]}".as_slice(),
    ] {
        assert!(matches!(
            decode::<StrictFixture>(input, &limits),
            Err(ApiCodecError::Malformed { .. })
        ));
    }
}

#[test]
fn depth_encoded_bytes_and_global_value_count_are_preallocation_limits() {
    let depth_limits = ApiLimits::constrained(1024, 2, 100);
    assert!(matches!(
        decode::<JsonDocument>(br#"[[[0]]]"#, &depth_limits),
        Err(ApiCodecError::LimitExceeded { .. })
    ));

    let byte_limits = ApiLimits::constrained(8, 8, 100);
    assert!(matches!(
        decode::<JsonDocument>(br#"{"long":true}"#, &byte_limits),
        Err(ApiCodecError::LimitExceeded { .. })
    ));

    let value_limits = ApiLimits::constrained(1024, 8, 3);
    assert!(matches!(
        decode::<JsonDocument>(br#"[0,1,2]"#, &value_limits),
        Err(ApiCodecError::LimitExceeded { .. })
    ));
}

#[test]
fn bounded_encoder_reports_limit_without_reserving_the_global_maximum() {
    let fixture = StrictFixture {
        id: "1".to_owned(),
        values: vec!["a long value".to_owned()],
    };
    let limited = ApiLimits::constrained(8, 8, 100);
    assert!(matches!(
        encode(&fixture, &limited),
        Err(ApiCodecError::LimitExceeded { .. })
    ));
}

#[test]
fn every_semantic_collection_limit_refuses_before_boundary_execution() {
    fn assert_limit<T: CanonicalDto>(value: &T, limits: &ApiLimits, expected: CodecLimit) {
        assert!(matches!(
            encode(value, limits),
            Err(ApiCodecError::LimitExceeded { limit, .. }) if limit == expected
        ));
    }

    let routing = RoutingRequestDto {
        origin: NodeIdDto::from_u64(1),
        destinations: vec![NodeIdDto::from_u64(2), NodeIdDto::from_u64(3)],
        profile: RelationProfileDto::default(),
        return_paths: false,
        budget: SearchBudgetDto::Unlimited,
        tie_policy: TiePolicyDto::StablePredecessor,
    };
    assert_limit(
        &routing,
        &ApiLimits {
            maximum_destinations: 1,
            ..ApiLimits::default()
        },
        CodecLimit::Destinations,
    );

    let profile = RelationProfileDto {
        entries: vec![
            RelationProfileEntryDto {
                relation_kind: RelationIdDto::from_u64(1),
                relation_use: RelationUseDto::Disabled,
            },
            RelationProfileEntryDto {
                relation_kind: RelationIdDto::from_u64(2),
                relation_use: RelationUseDto::Disabled,
            },
        ],
    };
    assert_limit(
        &profile,
        &ApiLimits {
            maximum_profile_entries: 1,
            ..ApiLimits::default()
        },
        CodecLimit::ProfileEntries,
    );

    let one = Binary32Dto::from_bits(1.0_f32.to_bits());
    let one64 = Binary64Dto::from_bits(1.0_f64.to_bits());
    let path = RoutePathDto {
        origin: NodeIdDto::from_u64(1),
        destination: NodeIdDto::from_u64(3),
        logical_distance: Binary64Dto::from_bits(2.0_f64.to_bits()),
        steps: vec![
            PathStepDto {
                edge: EdgeIdDto::from_u64(1),
                source: NodeIdDto::from_u64(1),
                destination: NodeIdDto::from_u64(2),
                relation_kind: RelationIdDto::from_u64(1),
                base_weight: one.clone(),
                multiplier: one.clone(),
                effective_weight: one64.clone(),
            },
            PathStepDto {
                edge: EdgeIdDto::from_u64(2),
                source: NodeIdDto::from_u64(2),
                destination: NodeIdDto::from_u64(3),
                relation_kind: RelationIdDto::from_u64(1),
                base_weight: one.clone(),
                multiplier: one,
                effective_weight: one64,
            },
        ],
    };
    assert_limit(
        &path,
        &ApiLimits {
            maximum_path_steps: 1,
            ..ApiLimits::default()
        },
        CodecLimit::PathSteps,
    );

    let hydration = HydrationRequestDto {
        node_ids: vec![NodeIdDto::from_u64(1)],
        edge_ids: vec![EdgeIdDto::from_u64(1)],
        profile: None,
    };
    assert_limit(
        &hydration,
        &ApiLimits {
            maximum_hydration_handles: 1,
            ..ApiLimits::default()
        },
        CodecLimit::HydrationHandles,
    );

    let subgraph = SubgraphHandlesDto {
        nodes: vec![NodeIdDto::from_u64(1), NodeIdDto::from_u64(2)],
        edges: vec![
            EdgeHandleDto {
                edge: EdgeIdDto::from_u64(1),
                source: NodeIdDto::from_u64(1),
                destination: NodeIdDto::from_u64(2),
            },
            EdgeHandleDto {
                edge: EdgeIdDto::from_u64(2),
                source: NodeIdDto::from_u64(1),
                destination: NodeIdDto::from_u64(2),
            },
        ],
    };
    assert_limit(
        &subgraph,
        &ApiLimits {
            maximum_subgraph_nodes: 1,
            ..ApiLimits::default()
        },
        CodecLimit::SubgraphNodes,
    );
    assert_limit(
        &subgraph,
        &ApiLimits {
            maximum_subgraph_edges: 1,
            ..ApiLimits::default()
        },
        CodecLimit::SubgraphEdges,
    );
}

#[test]
fn truncated_strings_and_documents_are_malformed_without_echoing_input() {
    let error = decode::<StrictFixture>(
        br#"{"id":"secret payload that must not appear"#,
        &ApiLimits::default(),
    )
    .unwrap_err();
    let displayed = error.to_string();
    assert!(!displayed.contains("secret"));
    assert!(!displayed.contains('\\'));
}

#[test]
fn noncanonical_ids_hex_and_base64_are_rejected_by_real_dto_wrappers() {
    let limits = ApiLimits::default();
    for input in [
        br#"{"node_id":"01","candidate_id":"1","request_id":"1","binary32":"0x00000000","binary64":"0x0000000000000000","payload":"AA=="}"#.as_slice(),
        br#"{"node_id":"1","candidate_id":"1","request_id":"1","binary32":"0X00000000","binary64":"0x0000000000000000","payload":"AA=="}"#.as_slice(),
        br#"{"node_id":"1","candidate_id":"1","request_id":"1","binary32":"0x00000000","binary64":"0X0000000000000000","payload":"AA=="}"#.as_slice(),
        br#"{"node_id":"1","candidate_id":"1","request_id":"1","binary32":"0x00000000","binary64":"0x0000000000000000","payload":"AA"}"#.as_slice(),
    ] {
        assert!(decode::<WrapperFixture>(input, &limits).is_err());
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WrapperFixture {
    node_id: pathhydra_api::NodeIdDto,
    candidate_id: pathhydra_api::CandidateIdDto,
    request_id: pathhydra_api::RequestIdDto,
    binary32: pathhydra_api::Binary32Dto,
    binary64: pathhydra_api::Binary64Dto,
    payload: pathhydra_api::PayloadDto,
}

impl CanonicalDto for WrapperFixture {
    fn validate_boundary(&self, limits: &ApiLimits) -> Result<(), ApiCodecError> {
        self.payload.validate_boundary(limits)
    }
}

#[test]
fn subgraph_endpoint_conflicts_ordering_and_limits_are_rejected() {
    let conflicting = br#"{"nodes":["1","2"],"edges":[{"edge":"10","source":"1","destination":"2"},{"edge":"10","source":"2","destination":"1"}]}"#;
    assert!(matches!(
        decode::<SubgraphHandlesDto>(conflicting, &ApiLimits::default()),
        Err(ApiCodecError::Malformed { .. })
    ));

    let missing_endpoint =
        br#"{"nodes":["1"],"edges":[{"edge":"10","source":"1","destination":"2"}]}"#;
    assert!(matches!(
        decode::<SubgraphHandlesDto>(missing_endpoint, &ApiLimits::default()),
        Err(ApiCodecError::Malformed { .. })
    ));

    let limits = ApiLimits {
        maximum_subgraph_nodes: 1,
        ..ApiLimits::default()
    };
    assert!(matches!(
        decode::<SubgraphHandlesDto>(br#"{"nodes":["1","2"],"edges":[]}"#, &limits),
        Err(ApiCodecError::LimitExceeded { .. })
    ));
}

#[test]
fn invalid_float_evidence_and_duration_are_rejected_during_decode() {
    let limits = ApiLimits::default();
    assert!(decode::<pathhydra_api::EdgeRecordDto>(
        br#"{"id":"1","source":"1","destination":"2","relation_kind":"1","base_weight":"0x7fc00000"}"#,
        &limits,
    )
    .is_err());
    assert!(
        decode::<pathhydra_api::RelationUseDto>(
            br#"{"state":"enabled","multiplier":"0x7f800000"}"#,
            &limits,
        )
        .is_err()
    );
    assert!(
        decode::<pathhydra_api::DestinationStateDto>(
            br#"{"state":"exact","logical_distance":"0xfff0000000000000","path":null}"#,
            &limits,
        )
        .is_err()
    );
    assert!(
        decode::<pathhydra_api::CudaAlgorithmDto>(
            br#"{"kind":"delta_stepping","delta":"0x0000000000000000"}"#,
            &limits,
        )
        .is_err()
    );
    assert!(
        decode::<pathhydra_api::DurationDto>(
            br#"{"seconds":"0","nanoseconds":1000000000}"#,
            &limits,
        )
        .is_err()
    );
}

#[test]
fn semantic_name_and_payload_limits_are_not_bypassed_by_default_decode() {
    let name_limits = ApiLimits {
        maximum_name_bytes: 1,
        ..ApiLimits::default()
    };
    assert!(matches!(
        decode::<pathhydra_api::NodeRecordDto>(
            br#"{"id":"1","name":"too long","payload":""}"#,
            &name_limits,
        ),
        Err(ApiCodecError::LimitExceeded { .. })
    ));

    let payload_limits = ApiLimits {
        maximum_payload_bytes: 2,
        ..ApiLimits::default()
    };
    assert!(matches!(
        decode::<WrapperFixture>(
            br#"{"node_id":"1","candidate_id":"1","request_id":"1","binary32":"0x00000000","binary64":"0x0000000000000000","payload":"AAEC"}"#,
            &payload_limits,
        ),
        Err(ApiCodecError::LimitExceeded { .. })
    ));
}

#[test]
fn public_error_code_and_message_are_independently_bounded() {
    let limits = ApiLimits {
        maximum_diagnostic_text_bytes: 3,
        ..ApiLimits::default()
    };
    for error in [
        ApiErrorDto {
            category: ApiErrorCategoryDto::InvalidInput,
            code: "long".to_owned(),
            message: "ok".to_owned(),
            retryable: false,
        },
        ApiErrorDto {
            category: ApiErrorCategoryDto::InvalidInput,
            code: "ok".to_owned(),
            message: "long".to_owned(),
            retryable: false,
        },
    ] {
        assert!(matches!(
            encode(&error, &limits),
            Err(ApiCodecError::LimitExceeded { .. })
        ));
    }
}
