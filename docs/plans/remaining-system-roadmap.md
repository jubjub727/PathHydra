# Remaining PathHydra System Roadmap

## Purpose

This roadmap is the finite implementation sequence after the complete
definition of done in Plan 06 to the complete Rust-engine boundary described by
`PATHHYDRA_SYSTEM_SHAPE.md`. Plan 06 is a hard prerequisite. Nothing in Plans
07-10 compensates for a partially implemented Plan 06.

Four plans remain:

1. [Plan 07: Parallel CUDA Execution and Request-Shape Closure](07-scalable-cuda-execution.md)
2. [Plan 08: Durable Operations and Store Observability](08-durable-operations-and-store-observability.md)
3. [Plan 09: Consumer-Ready Rust API and Canonical Encoding](09-consumer-ready-rust-api-and-canonical-encoding.md)
4. [Plan 10: System Conformance and Performance Closure](10-system-conformance-and-performance-closure.md)

They are ordered. Plans 07 and 08 may be developed independently after their
shared baseline is frozen, but Plan 09 should consume their final capability,
health, and error shapes. Plan 10 verifies the complete result and does not
serve as a bucket for unfinished implementation from the first three.

## Required Plan 06 handoff

Before Plan 07 starts, Plans 00-06 provide:

- the Rust workspace and precise node/relation terminology;
- durable provisional and confirmed records in RocksDB;
- exact case-sensitive node and relation-kind resolution;
- stable node, relation-kind, edge, and candidate IDs;
- atomic promotion, relation removal, and cascading node removal;
- canonical weights and the complete binary32/binary64 numeric contract;
- immutable resident routing images and exact deterministic CPU routing;
- current-only durable chunked routing bundles built by a consistent scan;
- exact partitioned CPU routing through a bounded host cache;
- exact resident and partitioned CUDA frontier/delta execution;
- bounded host staging and device topology caches with complete phase tracking;
- safe bundle publication, retirement, startup reconciliation, and corruption
  recovery;
- the complete Plan-06 publication/cache/device fault matrix;
- executable resident/out-of-core benchmarks, including the topology larger
  than local device residency;
- complete CPU paths, finite examined-edge budgets, cancellation, and admission;
- hydration against current confirmed records;
- deterministic caller-owned subgraphs;
- CUDA batching, admission, fallback, health, and explicit context recovery;
- one coherent `GraphEngine` publication boundary.

If any item above is absent, finish Plan 06 first. Later plans may rerun its
tests as regression gates but do not own its implementation.

## Remaining boundaries after complete Plan 06

The remaining original-design work is:

- CUDA is exact; Plan 07 must verify or complete graph-parallel execution, then
  select target/profile/batch/reset policies and close path/finite-budget
  specialization from evidence;
- checkpoint backup, restore validation, database inspection, and operator
  maintenance APIs are absent;
- RocksDB compaction, block-cache, write, and space-amplification metrics are
  absent from engine health;
- subgraphs, requests, responses, hydration values, capabilities, health, and
  errors have no canonical boundary encoding;
- the in-process versus hosted API decision is not recorded;
- the verification surface still needs system-wide generated, lifecycle,
  encoding, workload, and traceability evidence beyond Plan 06's own gates;
- all deliberately open original-design decisions need accepted final answers
  and current reference documentation.

## Scope boundary

The roadmap finishes the original Rust graph-engine design. It does not define:

- BAML prompts, models, workflows, or application composition;
- how provisional candidates are produced, factually validated, reviewed,
  revised, grouped, or rejected outside the engine;
- a rule engine after routing;
- a caller policy for which returned paths belong in a final subgraph;
- hosted authentication, tenancy, remote deployment, or a public network
  service;
- approximate routing or synonym/fuzzy name behavior.

Those items are explicitly outside `PATHHYDRA_SYSTEM_SHAPE.md`, not unfinished
PathHydra engine requirements.

## Design-section traceability

| Original design section | State after complete Plan 06 | Closing plan |
| --- | --- | --- |
| Fixed behaviour | Implemented for CPU and current engine API | Plan 10 re-verifies |
| System boundary | Rust engine exists; consumer packaging remains open | Plan 09 |
| Identity and records | Implemented | Plan 10 re-verifies |
| Numeric contract | Implemented and documented | Plans 07 and 10 verify CUDA scale |
| Durable graph store | Core layout implemented; operations/tuning evidence incomplete | Plan 08 |
| Name resolution | Implemented | Plan 10 re-verifies |
| Routing snapshot compiler | Durable streaming bundle path implemented and scale-tested | Plan 10 re-verifies |
| Routing image publication | Complete publication, retirement, and crash recovery | Plan 10 re-verifies |
| Request contract | Implemented | Plan 09 encodes; Plan 10 verifies |
| Rust query runtime | Implemented for CPU and resident/partitioned CUDA | Plan 07 optimizes CUDA; Plan 09 exposes |
| CPU reference engine | Implemented resident and partitioned | Plan 10 property-verifies |
| GPU routing engine | Exact resident/partitioned baseline; tuning/parity decisions remain | Plan 07 |
| Admission and concurrency | CPU and resident/partitioned CUDA implemented | Plan 07 optimizes scheduling |
| Topology larger than device memory | Implemented by Plan 06 | Plan 10 re-verifies |
| Path reconstruction and hydration | CPU/current-state semantics implemented | Plan 09 encodes; Plan 10 verifies |
| Subgraph construction | In-memory operations implemented; encoding absent | Plan 09 |
| Provisional candidates | Implemented | Plan 10 re-verifies |
| Confirmed graph deletion | Implemented | Plan 10 stress-verifies |
| Rust public API | Engine calls exist; packaging/encoding decision open | Plan 09 |
| Failure and recovery | Runtime/image recovery implemented; checkpoint backup/restore absent | Plan 08 |
| Verification surface | Plan 06 scale/fault evidence exists; system-wide generated evidence remains | Plan 10 |
| Observability | Route/CUDA/cache/lifecycle metrics exist; store and consumer/end-to-end metrics remain | Plans 07-09, consolidated by Plan 10 |
| Software and licensing | Current dependencies are local/free; inventory and reproducible audit absent | Plan 10 |

## Closure of deliberately open decisions

Every item listed under “Decisions deliberately left open” must be either
recorded as selected or explicitly rejected with evidence by the end of Plan
10. The intended closure path is:

| Open decision | Closure |
| --- | --- |
| Workspace and crate boundaries | Plan 09 records the selected packaging boundary and adds only crates required by its current implementation; no speculative transport crate |
| Hash map, concurrency, residency threshold | Benchmark current standard maps/locks and explicit limits; record Plan 10 decision |
| GPU vendor/API | Retain concrete NVIDIA CUDA/Rust PTX choice from Decision 0004 |
| Accelerator algorithm/tuning | Plan 07 compares parallel frontier/delta and records selection rules |
| Numeric types | Retain Decisions 0001/0002 |
| Edge identity/duplicates | Retain standalone `EdgeId` and parallel-edge behavior |
| RocksDB families/keys | Plan 08 measures the current layout and selects the first-release layout from the original store workloads; retain it unless evidence justifies changing the one current pre-release layout in place |
| Adjacency/high degree | Retain one-entry durable adjacency and Decision 0008 routing segments |
| Full rebuild/delta/overlay | Retain complete rebuild publication for the first release unless Plan 08 evidence disproves viability; no revision counter or compatibility layer |
| Hydration snapshot retention | Retain documented current-record hydration with typed unavailable evidence; record explicitly in Plan 09 |
| Distance-only/additional GPU state | Plan 07 adds verified path evidence or records CPU-only path dispatch if exact tie parity cannot be proved |
| Profile materialization | Plan 07 measures inline versus materialized weights and selects from evidence |
| Target-set representation | Plan 07 benchmarks dense/sparse representations and fixes one policy |
| Batch width/lane scheduling | Plan 07 measures and fixes bounded scheduling rules |
| Alternative out-of-core transport | Retain the Plan 06 DirectStorage gate result; Plan 10 records the final decision |
| In-process/hosted packaging | Plan 09 compares the original candidates against current consumer, deployment, failure-isolation, and latency requirements; implement the in-process boundary only if that evidence selects it |
| Request/response/subgraph encoding | Plan 09 compares current candidates against losslessness, bounded decoding, deterministic bytes, target-consumer tooling, and measured cost, then defines one current canonical encoding |
| In-memory subgraph representation | Retain ordered sets/maps and document the measured rationale |

If a benchmark does not justify replacing the current implementation, “retain
current behavior” is a real decision and must be recorded with its workload and
limits. It must not remain listed as open.

## Cross-plan invariants

All four plans preserve these invariants:

- confirmed RocksDB records are authoritative;
- provisional candidates never enter confirmed lookup, routing, or hydration;
- a route uses one immutable published image;
- node and relation names are exact, case-sensitive strings;
- hashes accelerate lookup or verify bytes but never replace full identity;
- relation category and direction remain correctness data;
- CPU routing remains the semantic oracle;
- GPU scheduling and I/O never alter numeric or completion semantics;
- subgraphs remain caller-owned and cannot mutate database state;
- no schema marker, migration reader, graph revision counter, or pinned-version
  API is added before the first public release;
- no GitHub Actions workflow is added;
- correctness and local operation require no paid or hosted service.

## Overall completion condition

The original system shape is complete when Plans 07-10 meet their definitions
of done, every design section is marked implemented or explicitly out of scope
in a checked traceability document, every deliberately open decision has a
recorded answer, all public behavior is represented by the consumer boundary,
and the authoritative local correctness, recovery, sanitizer, encoding, scale,
performance, and license checks pass.
