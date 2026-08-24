use std::time::Instant;

use pathhydra_core::{BaseWeight, NodePayload};
use pathhydra_store::{
    BatchNodeReference, BatchRelationReference, CandidateBatchEntry, Catalog, ConfirmedRecord,
    WriteOperationClass,
};

pub(crate) fn run(repeats: usize) {
    println!(
        "batch_ingestion_schema,workload,sample,entries,nodes,relation_kinds,edges,insert_ns,confirm_ns,top_k_ns,candidate_writes,confirmed_writes,candidate_committed_entries,confirmed_committed_entries,candidate_committed_bytes,confirmed_committed_bytes,peak_process_bytes,correct"
    );
    for sample in 0..repeats {
        nodes(sample);
        duplicate_relations(sample);
        confirmed_edges(sample);
        mixed(sample);
    }
}

fn nodes(sample: usize) {
    let entries = (0..10_000)
        .map(|index| CandidateBatchEntry::Node {
            name: format!("node-{index}").into(),
            payload: NodePayload::default(),
        })
        .collect::<Vec<_>>();
    run_catalog_case("nodes-10000", sample, entries, (10_000, 0, 0));
}

fn duplicate_relations(sample: usize) {
    let entries = (0..10_000)
        .map(|index| CandidateBatchEntry::RelationKind {
            name: format!("kind-{}", index % 100).into(),
        })
        .collect::<Vec<_>>();
    run_catalog_case(
        "relation-kinds-10000-duplicates",
        sample,
        entries,
        (0, 100, 0),
    );
}

fn confirmed_edges(sample: usize) {
    let root = tempfile::tempdir().unwrap();
    let catalog = Catalog::open(root.path()).unwrap();
    let setup = (0..1_000)
        .map(|index| CandidateBatchEntry::Node {
            name: format!("node-{index}").into(),
            payload: NodePayload::default(),
        })
        .chain(std::iter::once(CandidateBatchEntry::RelationKind {
            name: "kind".into(),
        }))
        .collect::<Vec<_>>();
    let setup = catalog.insert_candidate_batch(&setup).unwrap();
    let confirmed = catalog
        .confirm_candidate_batch(&setup.candidate_ids)
        .unwrap();
    let nodes = confirmed.records[..1_000]
        .iter()
        .map(|record| match record {
            ConfirmedRecord::Node(node) => node.id(),
            _ => unreachable!(),
        })
        .collect::<Vec<_>>();
    let ConfirmedRecord::Relation(relation) = &confirmed.records[1_000] else {
        unreachable!()
    };
    let entries = (0..100_000)
        .map(|index| CandidateBatchEntry::Edge {
            source: BatchNodeReference::Confirmed(nodes[index % nodes.len()]),
            destination: BatchNodeReference::Confirmed(nodes[(index * 17 + 1) % nodes.len()]),
            relation_kind: BatchRelationReference::Confirmed(relation.id()),
            base_weight: BaseWeight::new(0.5).unwrap(),
        })
        .collect::<Vec<_>>();
    run_existing_case(
        &catalog,
        "confirmed-edges-100000",
        sample,
        entries,
        (1_000, 1, 100_000),
    );
}

fn mixed(sample: usize) {
    let mut entries = Vec::with_capacity(110_004);
    entries.extend((0..10_000).map(|index| CandidateBatchEntry::Node {
        name: format!("node-{index}").into(),
        payload: NodePayload::default(),
    }));
    entries.extend((0..4).map(|index| CandidateBatchEntry::RelationKind {
        name: format!("kind-{index}").into(),
    }));
    entries.extend((0..100_000).map(|index| CandidateBatchEntry::Edge {
        source: BatchNodeReference::BatchNode(index % 10_000),
        destination: BatchNodeReference::BatchNode(if index % 1_000 == 0 {
            index % 10_000
        } else {
            (index * 17 + 1) % 10_000
        }),
        relation_kind: BatchRelationReference::BatchRelationKind(10_000 + index % 4),
        base_weight: BaseWeight::new(0.5).unwrap(),
    }));
    run_catalog_case(
        "mixed-10000-nodes-100000-edges",
        sample,
        entries,
        (10_000, 4, 100_000),
    );
}

fn run_catalog_case(
    workload: &str,
    sample: usize,
    entries: Vec<CandidateBatchEntry>,
    expected: (usize, usize, usize),
) {
    let root = tempfile::tempdir().unwrap();
    let catalog = Catalog::open(root.path()).unwrap();
    run_existing_case(&catalog, workload, sample, entries, expected);
}

fn run_existing_case(
    catalog: &Catalog,
    workload: &str,
    sample: usize,
    entries: Vec<CandidateBatchEntry>,
    expected: (usize, usize, usize),
) {
    let before = catalog.metrics_snapshot().unwrap();
    let insert_started = Instant::now();
    let inserted = catalog.insert_candidate_batch(&entries).unwrap();
    let insert = insert_started.elapsed();
    let confirm_started = Instant::now();
    let confirmed = catalog
        .confirm_candidate_batch(&inserted.candidate_ids)
        .unwrap();
    let confirm = confirm_started.elapsed();
    let top_k_started = Instant::now();
    let usage = catalog.most_used_relation_kinds(expected.1.max(1)).unwrap();
    let top_k = top_k_started.elapsed();
    let after = catalog.metrics_snapshot().unwrap();
    let writes = |class| {
        after.writes.get(&class).map_or(0, |value| value.attempts)
            - before.writes.get(&class).map_or(0, |value| value.attempts)
    };
    let committed_bytes = |class| {
        after
            .writes
            .get(&class)
            .map_or(0, |value| value.committed_bytes)
            - before
                .writes
                .get(&class)
                .map_or(0, |value| value.committed_bytes)
    };
    let committed_entries = |class| {
        after
            .writes
            .get(&class)
            .map_or(0, |value| value.committed_entries)
            - before
                .writes
                .get(&class)
                .map_or(0, |value| value.committed_entries)
    };
    let records = catalog.confirmed_graph_records().unwrap();
    let correct = inserted.candidate_ids.len() == entries.len()
        && confirmed.records.len() == entries.len()
        && records.nodes().len() == expected.0
        && records.relation_kinds().len() == expected.1
        && records.edges().len() == expected.2
        && usage.len() == expected.1
        && writes(WriteOperationClass::CandidateInsertion) == 1
        && writes(WriteOperationClass::ConfirmedPromotion) == 1
        && committed_entries(WriteOperationClass::CandidateInsertion) > entries.len() as u64
        && committed_entries(WriteOperationClass::ConfirmedPromotion) >= entries.len() as u64;
    println!(
        "1,{workload},{sample},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{correct}",
        entries.len(),
        expected.0,
        expected.1,
        expected.2,
        insert.as_nanos(),
        confirm.as_nanos(),
        top_k.as_nanos(),
        writes(WriteOperationClass::CandidateInsertion),
        writes(WriteOperationClass::ConfirmedPromotion),
        committed_entries(WriteOperationClass::CandidateInsertion),
        committed_entries(WriteOperationClass::ConfirmedPromotion),
        committed_bytes(WriteOperationClass::CandidateInsertion),
        committed_bytes(WriteOperationClass::ConfirmedPromotion),
        crate::system::peak_memory_bytes()
            .map_or_else(|| "unavailable".to_owned(), |value| value.to_string()),
    );
    assert!(correct, "batch ingestion benchmark correctness failed");
}
