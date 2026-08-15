# PathHydra System Shape

This document describes the parts the system needs and the contracts between them. It is not an implementation sequence. A choice is fixed only where the required behaviour or available evidence already narrows it sufficiently.

Two implementation choices are fixed. The graph engine is written in Rust and exposed as a library/API consumed by BAML. The design and eventual use of the BAML side are not defined here.

## Fixed behaviour

PathHydra operates on a directed, weighted graph.

- A vertex represents a stored subject and has a stable identity. Its payload is opaque to routing.
- A relation kind has a stable numeric identity and a text label.
- An edge points from one vertex to another, names one relation kind, and has a stored base weight.
- A request supplies one multiplier for every usable relation kind.
- The effective edge weight is `base weight * request multiplier`.
- Path weight is the sum of its effective edge weights.
- Shortest-path distance is used to select the relevant portion of the graph.
- The inference result is the selected graph, not a path. It may branch and reconnect any number of times.
- Newly proposed graph material is stored as provisional candidate data. It cannot affect lookup, routing, or hydration as confirmed fact until an external validation decision promotes it.

There is no rule system after search. The selected graph does not need approval from an ontology or another semantic layer. Context changes the arithmetic, and the arithmetic changes the selection.

All effective weights must be non-negative. Missing multipliers, disabled relation kinds, zero weights, infinities, overflow, and invalid numeric values need declared behaviour. A resource limit produces an incomplete result, not an unreachable result.

The main query shape is one origin and any number of destinations. Work can be shared across those destinations. Separate origins remain separate searches even when they run together.

## System boundary

BAML sits above the graph engine as its consumer. Its prompts, models, workflows, application structure, and use of graph results are outside this document.

The Rust layer is a deterministic graph engine. It accepts provisional candidates and graph-selection requests, enforces record and lifecycle invariants, persists provisional and confirmed data, runs selection, and returns structured results. It does not depend on how candidates were produced, how they are validated, or how results will be used.

```text
input and application state
            |
            v
+-----------------------------+
| BAML consumer               |
| details not yet specified   |
+-----------------------------+
            |
      typed Rust calls
            |
            v
+-----------------------------+
| Rust graph library/API      |
| - record checks/lifecycle   |
| - lookup and persistence    |
| - snapshot compilation      |
| - CPU/GPU graph selection   |
| - reconstruction/hydration  |
+-----------------------------+
            |
            v
+-----------------------------+
| RocksDB and routing images  |
+-----------------------------+
            |
     structured results
            |
            v
    structured result returned
```

The BAML side does not read or write RocksDB directly and does not manipulate routing images. The Rust API is the sole owner of those resources and their invariants. No other responsibility is assigned to BAML here.

No binding, transport, or process-model decision is made for the BAML side here.

## Identity and records

Three identity domains are needed:

- **External vertex ID:** stable across database rebuilds and suitable for callers.
- **Dense vertex ID:** compact and specific to one routing snapshot.
- **Relation ID:** stable key for a relation label and for indexing the request profile.

The minimum durable records are:

```text
Vertex
  external_id
  payload

RelationKind
  relation_id
  name

Edge
  source_external_id
  destination_external_id
  relation_id
  base_weight
```

Provisional candidates also need durable identity and lifecycle state. They may be stored in separate key spaces or marked records, but the physical choice must make accidental inclusion in the confirmed graph impossible. The Rust layer records the promotion decision; it does not define or perform the factual validation that precedes it.

An edge also needs an unambiguous handle if duplicate edges with the same endpoints and relation kind are allowed. Whether that is a standalone edge ID or a compound key remains open until duplicate-edge behaviour is fixed. Routing must never merge parallel edges merely because their endpoints match.

Relation labels are descriptive data, not executable definitions. The relation ID, edge direction, base weight, and request multiplier contain everything search needs.

Names and aliases used to find vertices belong in a lookup index. Their normalization rules are stored format rules: Unicode normalization, case handling, punctuation, alias collisions, and normalization version cannot depend on the process locale.

## Numeric contract

The stored weight type and calculation type remain open. The selected representation must define:

- valid ranges for base weights and multipliers;
- multiplication and path-sum overflow behaviour;
- whether zero is allowed;
- how a relation kind is disabled;
- the default for an omitted multiplier, if omission is allowed at all;
- comparison and tie behaviour;
- reproducibility expectations across CPU and GPU;
- the representation of unreachable distance.

Floating point saves space and is directly supported by existing GPU graph libraries. Integer or fixed-point arithmetic offers stronger reproducibility. Neither should be selected without measuring range, precision, memory traffic, and CPU/GPU agreement on representative data.

Equal shortest-path candidates are equally correct for selection. A deterministic tie policy is still useful for stable tests and cacheable results. Zero-weight cycles require selection state that terminates and cannot corrupt the returned graph.

## Durable graph store

RocksDB is the durable engine. This is an early choice because the graph needs an embedded store with ordered byte keys, range iteration, atomic multi-key updates, snapshots, recovery, and checkpoints. RocksDB supplies those facilities and is available under Apache 2.0 or GPLv2; the project should use it under the selected compatible license.

RocksDB is not expected to understand the graph. PathHydra owns record encoding, identifiers, adjacency, graph invariants, and versioning.

The durable layout needs logical key spaces for:

- format and schema metadata;
- vertex records;
- relation ID-to-name records;
- canonical edges;
- provisional candidates and their lifecycle state;
- outgoing adjacency;
- incoming adjacency or an equivalent incident-edge index for complete node deletion;
- external-to-dense mappings for published snapshots;
- normalized name and alias lookup;
- routing snapshot manifests.

These may become column families or prefixed regions. The number of column families is not fixed early because RocksDB gives each one separate memtables and table files, which affects memory and tuning.

Outgoing neighbours must be retrievable with a bounded prefix or range read. The physical adjacency form remains open:

- one entry per key favours simple mutations;
- packed adjacency favours dense reads but rewrites more data;
- chunked blocks provide a middle ground and avoid single huge values for high-degree vertices.

The choice belongs to workload measurements covering ingest rate, random mutation, snapshot construction, high-degree vertices, compaction, and space amplification.

One logical mutation may touch a canonical edge and multiple indexes. Those writes must become visible atomically. RocksDB `WriteBatch` provides atomic multi-key and cross-column-family writes. If adjacency updates require concurrent read-modify-write conflict detection, the graph layer must either serialize them or use RocksDB's transaction support; atomic batches alone do not detect application-level conflicts.

Deletion is part of the graph contract. Removing one directed relation deletes its canonical record and every adjacency/index entry for that relation. Removing a vertex atomically deletes the vertex, its lookup records, and every relation whose source or destination is that vertex. An incoming adjacency or equivalent incident-edge index is therefore required so the engine can find all affected relations without a graph-wide scan. Tombstones or deferred cleanup may support the implementation, but they do not satisfy deletion while incident relations remain part of the confirmed graph.

Record encodings require a magic value, schema version, fixed byte order, length checks, and migration policy. Corrupt or unknown records must fail visibly.

## Name resolution

Routing uses IDs. A separate resolver maps normalized text and aliases to external vertex IDs without scanning vertex payloads.

The resolver returns one of:

- one match;
- an ambiguity set;
- no match;
- invalid input.

Choosing among ambiguous matches is a caller concern unless a deterministic selection rule is added later. Name lookup is not fuzzy search, embedding search, or query interpretation.

## Routing snapshot compiler

The compiler turns one consistent view of the confirmed durable graph into a query-independent routing image. Provisional candidates are excluded regardless of whether their proposed records are structurally complete.

Its output contains only what expansion and reconstruction require:

- dense vertex numbering;
- outgoing adjacency boundaries;
- destination dense IDs;
- relation IDs;
- base weights;
- edge handles needed to hydrate the selected graph precisely;
- maps between external and dense vertex IDs.

The compiler checks every confirmed endpoint, relation ID, weight, array bound, and count. These are structural checks, not factual validation. It emits a manifest containing the graph version, record-format version, relation-dictionary version, numeric policy, element widths, counts, byte ranges, and checksums.

The image is a rebuildable index, never a second source of truth. A serialized copy is useful because loading a validated contiguous file is different from rebuilding the image through a full database scan.

Shortest-path state is selection machinery. Its predecessor chains or frontier history are not themselves the public result. The selection output is a set of vertex and edge handles forming a subgraph whose original branching and shared connections are retained.

A compressed sparse row layout with separate arrays is the baseline shape, not yet a locked file format:

```text
offsets[vertex_count + 1]
destinations[adjacency_count]
relation_ids[adjacency_count]
base_weights[adjacency_count]
edge_handles[adjacency_count]       optional
```

This matches the sparse, outgoing-neighbour access pattern and maps naturally to contiguous device arrays. Existing GPU graph systems expose CSR graphs and weighted SSSP, while GPU guidance favours memory layouts that allow adjacent threads to make coalesced accesses. Field widths still depend on measured graph size.

Approximate topology memory is calculated before implementation from actual widths:

```text
offset bytes
+ adjacency_count * (destination bytes
                    + relation ID bytes
                    + base-weight bytes
                    + optional edge-handle bytes)
```

Search state, frontiers, request profiles, outputs, and allocator headroom are separate from topology size.

## Published graph versions

Durable writes and device arrays cannot change atomically together. Routing therefore uses immutable published epochs:

1. A durable graph version is selected.
2. A routing image is built and checked against that version.
3. The completed image is published in one operation.
4. New requests pin it.
5. Existing requests keep their previous image until they finish.

The implementation may rebuild whole images or apply verified deltas, but a request cannot observe a mixture of versions.

Hydration must read records compatible with the pinned routing version. A RocksDB runtime snapshot provides a consistent view inside a running database process; a RocksDB checkpoint provides a standalone point-in-time database. Long-lived epochs and restart recovery therefore require either retained checkpoints, application-versioned records, or another explicit retention scheme. This choice cannot be left implicit.

The active manifest and enough prior state to recover safely must be durable. Loss of the GPU image should cause a reload or rebuild, not graph loss.

## Request contract

An inference request contains:

```text
origin external ID
destination external IDs[]
relation multiplier vector
requested graph version or "current"
include selection diagnostics? boolean
resource budget
```

Selection diagnostics are optional and do not shape the normal inference response. The normal response returns a selected graph. The Rust request contract still needs a precise graph-inclusion rule that turns shortest-distance results into selected vertices and edges. That rule cannot be inferred from one predecessor chain.

The multiplier vector is immutable for the request. It is validated and converted to a dense array indexed by relation ID. A missing relation entry must have one documented meaning; silently inheriting process state is not acceptable.

Duplicate destinations are collapsed for search and mapped back to the caller's output positions. The origin may also be a destination and should complete at distance zero without traversal.

Per-destination selection state distinguishes:

- exact distance found and included in selection;
- exact distance found but excluded by the graph boundary;
- unreachable after complete search;
- missing vertex;
- incomplete because a budget or cancellation stopped the search;
- invalid request.

Every response identifies the graph version and numeric policy used. The response graph contains stable vertex and edge handles plus enough weight information to reproduce the distances that selected it.

## Rust query runtime

The Rust query runtime owns work around the search algorithm:

- resolve external IDs against the selected epoch;
- validate and pack the context profile;
- canonicalize destinations;
- select CPU or GPU execution according to capability and admission rules;
- reserve worst-case working memory before admitting GPU work;
- track cancellation and budgets;
- keep independent searches isolated when they are batched;
- assemble and hydrate the selected graph;
- report exactness separately for every destination.

Destinations within one request share a search. Separate requests retain separate state, stopping conditions, budgets, and results even when their origin and profile happen to match. The scheduler may run them in the same device batch but does not turn them into one search.

## CPU reference engine

A CPU implementation over the routing image is part of the product, not disposable prototype code. It provides:

- a correctness oracle for accelerator results;
- operation without a supported GPU;
- a useful path for small searches where device dispatch costs more than the work;
- deterministic diagnosis and fixture testing.

For non-negative effective weights, a conventional distance-ordered single-source shortest-path implementation is the reference baseline. One frontier serves all destinations and stops after every requested destination is final or the reachable component is exhausted.

The CPU and GPU implementations consume the same snapshot, multiplier vector, numeric policy, destination set, and tie policy. Agreement is checked on distances, completion states, selection membership, and the assembled result graph.

## GPU routing engine

The accelerator runs exact weighted shortest-path search. It does not read RocksDB or full vertex payloads during edge relaxation.

Each edge relaxation computes or reads:

```text
effective = base_weights[edge] * profile[relation_ids[edge]]
candidate = current_distance + effective
```

Two cost modes are legitimate:

- compute the effective value during relaxation;
- materialize one temporary effective-weight array and reuse it across requests with an identical profile.

They must return the same result. Materialization spends a full edge pass and more device memory, so profile reuse and benchmark results decide whether it is worthwhile. Profiles are never blended or approximated for batching.

The GPU algorithm is not fixed yet. Delta-stepping is the leading candidate because it was designed as a parallel single-source shortest-path algorithm for directed graphs with non-negative weights. Its bucket width affects parallel work and repeated relaxations, so it needs workload-specific evidence. Other exact candidates should remain available until they are compared on PathHydra's graph shapes and context profiles.

An off-the-shelf GPU library can serve as a reference or a materialized-weight backend. cuGraph currently supports directed weighted graphs, SSSP, predecessor output, and CSR conversion. Its public SSSP interface accepts a concrete edge-weight view; PathHydra's inline relation multiplier and target-aware multi-request scheduler may therefore require custom kernels. This is a reason to evaluate cuGraph, not a reason to commit the core design to it.

Per active search, the accelerator needs independent:

- origin and destination membership;
- context profile reference;
- tentative distances;
- frontier or bucket state;
- unresolved destination count;
- completion and budget state;
- predecessor or other selection state when the graph-inclusion rule requires it;
- generation or reset state for reused buffers.

One search cannot finalize a destination because another search reached it. Batching is only a scheduling optimization.

A destination is complete only when the algorithm proves its distance final. First discovery is not sufficient. Zero-weight edges and repeated relaxations must close correctly before a bucket or equivalent frontier is retired.

Predecessors can be omitted when graph selection depends only on finalized distances and explicit edge criteria. If the inclusion rule needs predecessor or traversal state, that state may be retained or reproduced in a second pass against the same snapshot and profile. The memory-versus-recomputation tradeoff is a measured policy.

The GPU API, kernel language, and supported vendors remain open. The decision depends on target hardware, operating systems, required atomics, compiler support, profiling quality, library compatibility, and measured performance. No cross-vendor abstraction should be added before there are at least two real backends to abstract.

## Admission and concurrency

Topology residency is reserved first. Each admitted search then reserves enough space for its configured distance type, optional predecessors, frontier growth, destination tracking, and output.

The runtime refuses or queues work that cannot fit safely. It must not depend on an optimistic average frontier size and then fail mid-search without a controlled incomplete result.

Completed lanes can be reused only after their state is logically reset. Generation-stamped arrays may avoid clearing an entire vertex-sized allocation for short searches, but generation wrap and stale entries need tests.

Concurrency targets throughput. Once a single broad search saturates the device, adding searches does not promise lower latency for that search.

## Topology larger than device memory

The preferred hot path keeps the routing image resident on the accelerator. Payload size does not count toward this requirement because payloads stay in RocksDB or snapshot files.

If the routing image does not fit:

- partition it by source-vertex ranges;
- keep global search state in the most suitable memory tier;
- group active vertices by required partition;
- stage immutable partitions through a bounded cache;
- track pending I/O as part of frontier completion;
- permit eviction only when data can be reloaded without changing the answer.

If the image fits in host RAM, pinned-memory staging is the baseline candidate. If it exceeds host RAM, a serialized chunked image is preferable to tiny RocksDB reads from the relaxation loop.

DirectStorage is optional, Windows-specific transport research. Microsoft's implementation supports reads and demonstrates file-to-GPU loading, but it does not choose graph partitions or maintain shortest-path correctness. Conventional asynchronous file reads and explicit device copies remain the comparison baseline. No transport is selected before topology size and staging traces show that I/O is actually limiting search.

## Selected-graph assembly and hydration

Search produces distance and selection state rather than a public path. The assembler collects the selected vertex and edge handles, preserving every included branch and shared connection. Predecessors may still be retained internally when the selection rule needs them, but a predecessor chain is not the hydrated result.

Hydration resolves the selected handles into:

- external vertex IDs;
- requested vertex payloads;
- directed edges with relation IDs and labels;
- stored and effective weights for each edge;
- selection distances and boundaries;
- graph version and context-profile identity.

Reads are deduplicated and batched. Relation labels may be cached because they are small and versioned. Vertex payloads are fetched only for the returned graph.

Hydration adds records to the selected graph; it does not reconsider graph membership.

## Provisional candidates

Newly proposed graph material enters the Rust layer as provisional candidate data. It remains excluded from confirmed lookup, snapshot compilation, graph selection, and hydration. After validation occurs outside this system, an explicit confirmation promotes it into the confirmed graph in one atomic mutation.

How candidates are produced, validated, grouped, revised, rejected, or reviewed is outside scope. The core contract covers only provisional insertion, exclusion before confirmation, and atomic promotion after validation.

## Confirmed graph deletion

The Rust API supports direct removal of a confirmed directed relation and removal of a confirmed node. Node removal cascades across every confirmed incoming and outgoing relation in the same atomic mutation. It also removes aliases, lookup entries, and other confirmed indexes owned by that node.

Deletion advances the durable graph version and is reflected in the next published routing epoch. A request already pinned to an older immutable epoch may finish against that version; no newly admitted request may select a deleted node or relation after the deletion epoch is published.

A provisional candidate that refers to an endpoint removed before promotion cannot be confirmed without a new valid endpoint state.

## Rust public API

The stable boundary is a narrow typed graph API rather than a general graph query language. It needs calls equivalent to:

- resolve a name or alias;
- insert provisional candidates;
- confirm candidates after external validation;
- remove a confirmed directed relation;
- remove a confirmed node together with all incoming and outgoing relations;
- submit one-origin/many-destination inference;
- retrieve hydrated result graphs;
- inspect available graph versions and capabilities;
- cancel work and read health information.

The binding or local transport, streaming shape, process model, and remote-access policy remain open. The Rust request and response types should not depend on any one transport.

## Failure and recovery

Expected failure classes include:

- invalid records or request numbers;
- missing IDs and relation profile entries;
- invalid provisional-to-confirmed transitions;
- incomplete or inconsistent deletion cascades;
- database write or recovery errors;
- routing image checksum or version mismatch;
- accelerator allocation, launch, or device loss;
- cancellation and resource exhaustion;
- hydration data unavailable for a pinned epoch.

Each failure has a typed outcome. Device failure must not damage durable graph state. A corrupt routing image is discarded and rebuilt. A routing image is published only after its technical checks pass. Startup either exposes a fully valid epoch or reports that routing is unavailable.

Backups use a documented RocksDB checkpoint or backup procedure and include the application metadata needed to interpret record formats. Rebuildable device images may be omitted from backups if startup can regenerate them.

## Verification surface

Correctness fixtures cover:

- a single edge and a multi-hop winner;
- context profiles that change the selected graph;
- directed edges that cannot be traversed backward;
- parallel edges;
- self-edges and zero-weight cycles;
- equal-cost selection candidates;
- missing, duplicate, and unreachable destinations;
- disabled or missing relation multipliers;
- numeric boundary values and overflow;
- early stopping with several destinations;
- cancellation and budget exhaustion;
- concurrent searches with different origins and profiles;
- mutation atomicity and crash recovery;
- provisional candidates remaining absent from lookup, routing images, and hydrated results;
- atomic promotion after external validation;
- relation deletion removing every durable and routing representation;
- node deletion removing every incoming and outgoing relation;
- old and new routing epochs active together;
- hydration against the correct version.

Property tests generate graphs and profiles, compare CPU and GPU outcomes, and verify that selection membership follows the reported distances and boundary. Adversarial tests concentrate high degree, long chains, dense components, repeated relaxations, and unreachable regions.

Performance reports include:

- vertex and adjacency counts;
- durable and routing-image bytes;
- snapshot build and load time;
- profile packing or weight-materialization time;
- edges examined and relaxation attempts;
- frontier or bucket statistics;
- time to first completed destination and full request completion;
- concurrent search count and aggregate throughput;
- device-memory high-water mark;
- reconstruction and hydration time;
- near, far, unreachable, narrow, broad, and high-degree workloads.

Benchmarks report correctness failures before timing results. Performance targets are measurements on named hardware and datasets, not API guarantees.

## Observability

Every request should expose enough structured diagnostics to explain its execution without logging payload contents:

- request and graph-version IDs;
- profile identity or hash;
- executor used;
- queue and execution duration;
- completion reason;
- destinations completed and unresolved;
- examined edges and relaxations;
- frontier high-water mark;
- device-memory reservation and peak use;
- reconstruction and hydration duration.

Store-level metrics cover write failures, compaction pressure, cache behaviour, snapshot age, image build failures, and active epoch references.

## Software and licensing boundary

The project must build and run without a required licence payment, subscription, hosted database, or paid cloud service. Core development, tests, benchmarks, database inspection, and recovery all run locally.

Dependencies need recorded versions and licences. Prefer permissive open-source components when they meet the requirement. A free-to-use proprietary GPU driver or SDK may be necessary for particular hardware, but PathHydra must not depend on a paid edition or evaluation licence. Optional integrations cannot become mandatory for correctness or data access.

Rust and its standard tooling are available under MIT or Apache 2.0 terms. BAML is Apache 2.0 and can run locally. RocksDB has an Apache 2.0 licensing option. cuGraph is also Apache 2.0 but is NVIDIA-specific and is only an evaluation candidate. DirectStorage samples are MIT-licensed, but the API is platform-specific and optional.

PathHydra-owned engine code is Rust. RocksDB itself is implemented in C++ and will enter the build through a Rust binding; no PathHydra core logic is authored in C++.

## Decisions already justified

| Decision | Reason |
|---|---|
| BAML consumes the Rust graph API. | This boundary is fixed; BAML's internal design and use of the API are not. |
| Rust owns the graph library/API. | The graph engine needs deterministic systems code, explicit resource control, and a stable typed caller boundary. This is a fixed project constraint. |
| Candidate data is provisional until promoted. | Unvalidated proposals must not affect the confirmed graph or any inference result. The validation method remains outside the engine. |
| RocksDB is the durable source of truth. | It is embedded, ordered, persistent, supports atomic batches and consistent views, and has a no-fee open-source licence. |
| Routing uses a separate compact snapshot. | Durable payload storage and accelerator traversal have different access patterns; device memory is finite and distinct from host storage. |
| The reference result is exact. | Graph membership is driven by minimum context-adjusted distance, so approximation could change the selected graph. |
| Effective weights cannot be negative. | The intended distance model and practical exact SSSP candidates rely on non-negative edge weights. |
| A CPU engine remains available. | Accelerator correctness needs an independent comparator, and GPU availability is not universal. |
| Published routing images are immutable per request. | RocksDB batches cannot atomically modify already-resident device arrays. |
| Full payloads stay out of the routing loop. | Expansion needs endpoints, relation IDs, and weights; reading unrelated text adds storage traffic without affecting the calculation. |

## Decisions deliberately left open

- Rust workspace and crate boundaries;
- GPU vendor and programming API;
- exact accelerator algorithm and tuning parameters;
- stored and accumulated numeric types;
- edge identity and duplicate-edge policy;
- RocksDB column-family split and key encoding;
- adjacency packing and high-degree representation;
- full rebuilds, incremental images, or overlay publication;
- snapshot retention method for hydration;
- distance-only versus additional selection-state capture policy;
- profile materialization threshold;
- target-set representation;
- batch width and lane scheduling;
- out-of-core partitioning and I/O transport;
- in-process Rust library or separately hosted Rust API packaging;
- request and response encoding.

Each becomes fixed only after its required workload, correctness constraint, target platform, and benchmark evidence are recorded.

## Evidence for the early choices

- [The Rust project](https://github.com/rust-lang/rust) provides the compiler, standard library, Cargo tooling, and records its MIT and Apache 2.0 licensing.
- [BAML's repository](https://github.com/BoundaryML/baml) records its Apache 2.0 licence.
- [RocksDB overview](https://rocksdb.org/) describes an embedded persistent store using arbitrary byte keys and values and optimized for fast storage.
- [RocksDB column families](https://github.com/facebook/rocksdb/wiki/Column-Families) documents logical partitioning, consistent cross-family views, and atomic writes across column families.
- [RocksDB basic operations](https://github.com/facebook/rocksdb/wiki/Basic-Operations) documents `WriteBatch` atomicity and reads pinned to a snapshot.
- [RocksDB transactions](https://github.com/facebook/rocksdb/wiki/Transactions) distinguishes atomic write batches from transactions with conflict checking.
- [RocksDB checkpoints](https://github.com/facebook/rocksdb/wiki/Checkpoints) documents standalone consistent point-in-time database copies.
- [RocksDB's repository](https://github.com/facebook/rocksdb) records its Apache 2.0 and GPLv2 licensing options.
- Meyer and Sanders' original [delta-stepping paper](https://doi.org/10.1016/S0196-6774(03)00076-2) defines a parallelizable SSSP algorithm for directed graphs with non-negative weights.
- [cuGraph's supported algorithms](https://docs.rapids.ai/api/cugraph/stable/graph_support/algorithms/) and [C++ API](https://docs.rapids.ai/api/cugraph/stable/api_docs/cugraph_cpp/full_api/) show support for directed weighted graphs, CSR conversion, SSSP distances, and predecessors.
- NVIDIA's [CUDA programming model](https://docs.nvidia.com/cuda/cuda-programming-guide/01-introduction/programming-model.html) describes separate host and device memory and the cost of moving data between them.
- Microsoft's [DirectStorage samples](https://github.com/microsoft/DirectStorage) describe read-only file transfer, including file-to-GPU examples, and are MIT-licensed.
