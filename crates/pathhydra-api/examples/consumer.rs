use pathhydra_api::{
    Binary32Dto, ConfirmedRecordDto, DestinationStateDto, MutationDurableResultDto, NodeRecordDto,
    PathHydra, PayloadDto, RelationKindRecordDto, RelationProfileDto, RelationProfileEntryDto,
    RelationUseDto, RequestIdAllocation, RoutingRequestDto, SearchBudgetDto, SubgraphHandlesDto,
    TiePolicyDto, decode, encode,
};

fn external_decision_placeholder(_candidate: &pathhydra_api::CandidateDto) -> bool {
    // The application owns factual validation. PathHydra only records the
    // explicit confirmation decision made here.
    true
}

fn confirmed_node(value: pathhydra_api::MutationOutcomeDto) -> NodeRecordDto {
    let MutationDurableResultDto::Confirmed(ConfirmedRecordDto::Node(node)) = value.durable_result
    else {
        panic!("node confirmation returned a different record kind")
    };
    node
}

fn confirmed_relation(value: pathhydra_api::MutationOutcomeDto) -> RelationKindRecordDto {
    let MutationDurableResultDto::Confirmed(ConfirmedRecordDto::RelationKind(relation)) =
        value.durable_result
    else {
        panic!("relation-kind confirmation returned a different record kind")
    };
    relation
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let api = PathHydra::open(directory.path().join("catalog"))?;

    let source_candidate = api.insert_node_candidate(
        "Source".to_owned(),
        PayloadDto::from_bytes(b"caller-owned source payload"),
    )?;
    let candidate = api.get_candidate(&source_candidate)?;
    assert!(external_decision_placeholder(&candidate));
    let source = confirmed_node(api.confirm_candidate(&source_candidate)?);

    let destination_candidate = api.insert_node_candidate(
        "Destination".to_owned(),
        PayloadDto::from_bytes(b"caller-owned destination payload"),
    )?;
    let destination = confirmed_node(api.confirm_candidate(&destination_candidate)?);
    let isolated_candidate = api.insert_node_candidate(
        "Isolated".to_owned(),
        PayloadDto::from_bytes(b"unreachable confirmed node"),
    )?;
    let isolated = confirmed_node(api.confirm_candidate(&isolated_candidate)?);
    let relation_candidate = api.insert_relation_kind_candidate("connects exactly".to_owned())?;
    let relation = confirmed_relation(api.confirm_candidate(&relation_candidate)?);
    let edge_candidate = api.insert_edge_candidate(
        &source.id,
        &destination.id,
        &relation.id,
        &Binary32Dto::from_bits(0.5_f32.to_bits()),
    )?;
    api.confirm_candidate(&edge_candidate)?;

    assert_eq!(api.lookup_node_exact("Source")?, Some(source.id.clone()));
    assert_eq!(
        api.lookup_relation_kind_exact("connects exactly")?,
        Some(relation.id.clone())
    );

    let profile = RelationProfileDto {
        entries: vec![RelationProfileEntryDto {
            relation_kind: relation.id,
            relation_use: RelationUseDto::Enabled {
                multiplier: Binary32Dto::from_bits(2.0_f32.to_bits()),
            },
        }],
    };
    let handle = api.request_handle(RequestIdAllocation::Automatic)?;
    let routed = handle.route(&RoutingRequestDto {
        origin: source.id,
        destinations: vec![
            destination.id,
            isolated.id,
            pathhydra_api::NodeIdDto::from_u64(u64::MAX),
        ],
        profile: profile.clone(),
        return_paths: true,
        budget: SearchBudgetDto::Unlimited,
        tie_policy: TiePolicyDto::StablePredecessor,
    })?;

    for result in &routed.response.results {
        let state = match &result.state {
            DestinationStateDto::Exact { .. } => "exact",
            DestinationStateDto::Unreachable => "unreachable",
            DestinationStateDto::MissingNode => "missing",
            DestinationStateDto::Incomplete => "incomplete",
        };
        println!("destination {}: {state}", result.destination);
    }

    let first = pathhydra_api::DecimalU64Dto::from_u64(0);
    let hydrated_path = handle.hydrate_path(&first)?;
    println!(
        "hydrated path: {} nodes, {} edges",
        hydrated_path.nodes.len(),
        hydrated_path.edges.len()
    );

    let mut subgraph = api.new_subgraph();
    handle.add_path_to_subgraph(&first, &mut subgraph)?;
    let bytes = encode(&subgraph, &api.limits())?;
    let decoded: SubgraphHandlesDto = decode(&bytes, &api.limits())?;
    let hydrated_subgraph = api.hydrate_subgraph(&decoded, Some(&profile))?;
    assert!(hydrated_subgraph.complete);

    let health = api.health()?;
    println!(
        "catalog available: {}, active routes: {}",
        health.durable_catalog_available, health.active_routes
    );
    let shutdown = api.shutdown()?;
    println!("shutdown state: {:?}", shutdown.state_after);
    Ok(())
}
