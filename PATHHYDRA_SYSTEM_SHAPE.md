# PathHydra System Shape

This document describes the parts the system needs and the contracts between them. It is not an implementation sequence. A choice is fixed only where the required behaviour or available evidence already narrows it sufficiently.

Two implementation choices are fixed. BAML is the application layer and owns the higher-level control flow. Rust is the systems layer and exposes the graph engine as a library/API used by BAML.

## Fixed behaviour

PathHydra operates on a directed, weighted graph.

- A vertex represents a stored subject and has a stable identity. Its payload is opaque to routing.
- A relation kind has a stable numeric identity and a text label.
- An edge points from one vertex to another, names one relation kind, and has a stored base weight.
- A request supplies one multiplier for every usable relation kind.
- The effective edge weight is `base weight * request multiplier`.
- Route weight is the sum of its effective edge weights.
- The answer for a destination is the directed route with the lowest weight under that request.

There is no rule system after search. The chosen route does not need approval from an ontology or another semantic layer. Context changes the arithmetic, and the arithmetic changes the answer.

All effective weights must be non-negative. Missing multipliers, disabled relation kinds, zero weights, infinities, overflow, and invalid numeric values need declared behaviour. A resource limit produces an incomplete result, not an unreachable result.

The main query shape is one origin and any number of destinations. Work can be shared across those destinations. Separate origins remain separate searches even when they run together.

## System boundary

BAML runs the application. It owns model calls, higher-level decisions, workflow state, and the flow that builds and queries the factual graph. It decides which Rust operations to call and how their results affect the next application action.

The Rust layer is a deterministic graph engine. It accepts concrete records and inference requests, validates them, persists them, runs route search, and returns structured results. It does not call models or control the application.

```text
input and application state
            |
            v
+-----------------------------+
| BAML application            |
| - model-driven workflows    |
| - application decisions     |
| - graph-building flow       |
| - inference orchestration   |
+-----------------------------+
            |
      typed Rust calls
            |
            v
+-----------------------------+
| Rust graph library/API      |
| - validation and mutation   |
| - lookup and persistence    |
| - snapshot compilation      |
| - CPU/GPU route search      |
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
      BAML continues flow
```

The BAML application does not read or write RocksDB directly and does not manipulate routing images. The Rust API is the sole owner of those resources and their invariants. Conversely, Rust does not decide which facts the application should store, which model to call, or what the next workflow step should be.

The exact call mechanism between BAML and Rust remains open. It may be an in-process library bridge or a narrow local API, but the ownership boundary and typed request/response contracts remain the same.

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

Equal-cost routes are equally correct. A deterministic tie policy is still useful for stable tests, repeatable reconstruction, and cacheable results. Zero-weight cycles require a predecessor policy that cannot produce a cyclic reconstructed route.

## Durable graph store

RocksDB is the durable engine. This is an early choice because the graph needs an embedded store with ordered byte keys, range iteration, atomic multi-key updates, snapshots, recovery, and checkpoints. RocksDB supplies those facilities and is available under Apache 2.0 or GPLv2; the project should use it under the selected compatible license.

RocksDB is not expected to understand the graph. PathHydra owns record encoding, identifiers, adjacency, graph invariants, and versioning.

The durable layout needs logical key spaces for:

- format and schema metadata;
- vertex records;
- relation ID-to-name records;
- canonical edges;
- outgoing adjacency;
- optional incoming adjacency for maintenance or future reverse lookup;
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

Deletion needs an explicit contract. Removing a vertex must prevent all incident outgoing edges from appearing in any later published routing snapshot. Incoming adjacency, an incident-edge index, tombstones, or deferred cleanup are possible implementations, but dangling traversable edges are not.

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

The compiler turns one consistent durable graph view into a query-independent routing image.

Its output contains only what expansion and reconstruction require:

- dense vertex numbering;
- outgoing adjacency boundaries;
- destination dense IDs;
- relation IDs;
- base weights;
- edge handles when routes must be hydrated precisely;
- maps between external and dense vertex IDs.

The compiler validates every endpoint, relation ID, weight, array bound, and count. It emits a manifest containing the graph version, record-format version, relation-dictionary version, numeric policy, element widths, counts, byte ranges, and checksums.

The image is a rebuildable index, never a second source of truth. A serialized copy is useful because loading a validated contiguous file is different from rebuilding the image through a full database scan.

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
return routes? boolean
resource budget
```

The multiplier vector is immutable for the request. It is validated and converted to a dense array indexed by relation ID. A missing relation entry must have one documented meaning; silently inheriting process state is not acceptable.

Duplicate destinations are collapsed for search and mapped back to the caller's output positions. The origin may also be a destination and should complete at distance zero without traversal.

Per-destination output distinguishes:

- exact route found;
- exact distance found but route not requested;
- unreachable after complete search;
- missing vertex;
- incomplete because a budget or cancellation stopped the search;
- invalid request.

Every response identifies the graph version and numeric policy used. When a route is returned, it carries enough handles to reproduce the reported total.

## Rust query runtime

The Rust query runtime owns work around the search algorithm:

- resolve external IDs against the selected epoch;
- validate and pack the context profile;
- canonicalize destinations;
- select CPU or GPU execution according to capability and admission rules;
- reserve worst-case working memory before admitting GPU work;
- track cancellation and budgets;
- keep independent searches isolated when they are batched;
- reconstruct and hydrate requested routes;
- report exactness separately for every destination.

Destinations within one request share a search. Separate requests retain separate state, stopping conditions, budgets, and results even when their origin and profile happen to match. The scheduler may run them in the same device batch but does not turn them into one search.

## CPU reference engine

A CPU implementation over the routing image is part of the product, not disposable prototype code. It provides:

- a correctness oracle for accelerator results;
- operation without a supported GPU;
- a useful path for small searches where device dispatch costs more than the work;
- deterministic diagnosis and fixture testing.

For non-negative effective weights, a conventional distance-ordered single-source shortest-path implementation is the reference baseline. One frontier serves all destinations and stops after every requested destination is final or the reachable component is exhausted.

The CPU and GPU implementations consume the same snapshot, multiplier vector, numeric policy, destination set, and tie policy. Agreement is checked on distances, completion states, and reconstructed routes where deterministic ties apply.

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
- predecessor state when routes are requested;
- generation or reset state for reused buffers.

One search cannot finalize a destination because another search reached it. Batching is only a scheduling optimization.

A destination is complete only when the algorithm proves its distance final. First discovery is not sufficient. Zero-weight edges and repeated relaxations must close correctly before a bucket or equivalent frontier is retired.

Predecessors can be omitted for distance-only requests. A distance-first request may be rerun with predecessor capture against the same snapshot and profile when only a few routes are later requested. The memory-versus-recomputation tradeoff is a measured policy.

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

## Route reconstruction and hydration

Search stores parent references, not complete path arrays. Reconstruction walks those references from a completed destination to the origin and reverses the result.

Hydration resolves the reconstructed handles into:

- ordered external vertex IDs;
- requested vertex payloads;
- relation IDs and labels;
- stored and effective weights for each edge;
- total route weight;
- graph version and context-profile identity.

Reads are deduplicated and batched. Relation labels may be cached because they are small and versioned. Vertex payloads are fetched only for returned routes.

Hydration adds information to the route; it does not reconsider which route is correct.

## Mutation and ingestion surfaces

BAML owns the application flow that produces graph changes, including any model work used to decide which records should exist. It submits concrete mutation commands to Rust. Rust validates and applies them without needing to know how BAML arrived at them.

The graph layer needs operations for:

- create or update a vertex;
- create or rename a relation kind;
- create, update, or remove an edge;
- remove a vertex under the declared incident-edge policy;
- atomically apply a group of graph mutations;
- bulk import;
- rebuild indexes;
- validate durable invariants;
- publish and inspect routing epochs.

Imports must use the same validation and record formats as online writes. A fast bulk path may build sorted files or packed adjacency directly, but it cannot create a database that normal mutation code would interpret differently.

Renaming a relation does not alter routing. Reassigning its numeric ID or changing edge weights does and therefore requires a new routing epoch.

## Rust API presented to BAML

The stable boundary is a narrow typed graph API rather than a general graph query language. BAML uses it to perform calls equivalent to:

- resolve a name or alias;
- mutate graph records;
- submit one-origin/many-destination inference;
- retrieve hydrated routes;
- inspect available graph versions and capabilities;
- cancel work and read health information.

The binding or local transport, streaming shape, process model, and remote-access policy remain open. The Rust request and response types should not depend on any one transport.

## Failure and recovery

Expected failure classes include:

- invalid records or request numbers;
- missing IDs and relation profile entries;
- database write or recovery errors;
- routing image checksum or version mismatch;
- accelerator allocation, launch, or device loss;
- cancellation and resource exhaustion;
- hydration data unavailable for a pinned epoch.

Each failure has a typed outcome. Device failure must not damage durable graph state. A corrupt routing image is discarded and rebuilt. Publication occurs only after validation. Startup either exposes a fully valid epoch or reports that routing is unavailable.

Backups use a documented RocksDB checkpoint or backup procedure and include the application metadata needed to interpret record formats. Rebuildable device images may be omitted from backups if startup can regenerate them.

## Verification surface

Correctness fixtures cover:

- a single edge and a multi-hop winner;
- context profiles that change the winning route;
- directed edges that cannot be traversed backward;
- parallel edges;
- self-edges and zero-weight cycles;
- equal-cost routes;
- missing, duplicate, and unreachable destinations;
- disabled or missing relation multipliers;
- numeric boundary values and overflow;
- early stopping with several destinations;
- cancellation and budget exhaustion;
- concurrent searches with different origins and profiles;
- mutation atomicity and crash recovery;
- old and new routing epochs active together;
- hydration against the correct version.

Property tests generate graphs and profiles, compare CPU and GPU outcomes, and verify that returned route weights reproduce the reported totals. Adversarial tests concentrate high degree, long chains, dense components, repeated relaxations, and unreachable regions.

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
| BAML owns the application layer. | Model-driven behaviour, application decisions, graph-building flow, and inference orchestration belong to the controlling application rather than the storage engine. |
| Rust owns the graph library/API. | The graph engine needs deterministic systems code, explicit resource control, and a stable typed boundary for BAML. This is a fixed project constraint. |
| RocksDB is the durable source of truth. | It is embedded, ordered, persistent, supports atomic batches and consistent views, and has a no-fee open-source licence. |
| Routing uses a separate compact snapshot. | Durable payload storage and accelerator traversal have different access patterns; device memory is finite and distinct from host storage. |
| The reference result is exact. | The product definition selects the minimum context-adjusted route, so approximation would change behaviour. |
| Effective weights cannot be negative. | The intended distance model and practical exact SSSP candidates rely on non-negative edge weights. |
| A CPU engine remains available. | Accelerator correctness needs an independent comparator, and GPU availability is not universal. |
| Published routing images are immutable per request. | RocksDB batches cannot atomically modify already-resident device arrays. |
| Full payloads stay out of the routing loop. | Expansion needs endpoints, relation IDs, and weights; reading unrelated text adds storage traffic without affecting the calculation. |

## Decisions deliberately left open

- Rust workspace and crate boundaries;
- BAML application module boundaries;
- the BAML-to-Rust binding or local transport;
- build orchestration across the BAML and Cargo toolchains;
- GPU vendor and programming API;
- exact accelerator algorithm and tuning parameters;
- stored and accumulated numeric types;
- edge identity and duplicate-edge policy;
- RocksDB column-family split and key encoding;
- adjacency packing and high-degree representation;
- full rebuilds, incremental images, or overlay publication;
- snapshot retention method for hydration;
- distance-only versus predecessor-first execution policy;
- profile materialization threshold;
- target-set representation;
- batch width and lane scheduling;
- out-of-core partitioning and I/O transport;
- in-process Rust library or separately hosted Rust API packaging;
- request and response encoding.

Each becomes fixed only after its required workload, correctness constraint, target platform, and benchmark evidence are recorded.

## Evidence for the early choices

- [The Rust project](https://github.com/rust-lang/rust) provides the compiler, standard library, Cargo tooling, and records its MIT and Apache 2.0 licensing.
- [BAML's repository](https://github.com/BoundaryML/baml) describes typed model functions and agent workflows, local operation, language interoperability, and its Apache 2.0 licence.
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
