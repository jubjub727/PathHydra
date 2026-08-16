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
- Every reachable requested destination has an exact minimum distance and, when requested, one path selected under the configured tie policy.
- The Rust engine returns routing and hydration primitives. It does not prescribe how a final graph is composed from them.
- Newly proposed graph material is stored as provisional candidate data. It cannot affect lookup, routing, or hydration as confirmed fact until an external validation decision promotes it.

There is no rule system after search. Context changes the arithmetic, and the arithmetic changes the exact routing result.

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
| - paths/subgraphs/hydration |
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
  name
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

Node and relation names are preserved exactly as supplied. They are case-sensitive and are never normalized, folded, corrected, stemmed, or treated as synonyms. Different spellings, punctuation, casing, or Unicode sequences identify different names. Within the node namespace, one exact name maps to one node ID. Within the relation namespace, one exact name maps to one relation ID.

Relation labels are descriptive data, not executable definitions. The relation ID, edge direction, base weight, and request multiplier contain everything search needs.

Exact node and relation names belong in durable lookup indexes. The hot in-process baseline is a pair of hash maps:

```text
exact node name     -> external node ID
exact relation name -> relation ID
```

Hashing chooses where to look; complete string equality decides whether the key matches. A raw hash value is therefore not used as a permanent ID, and hash collisions cannot merge different names. The numeric ID returned by the map is then used for direct array indexing in routing snapshots and request profiles.

The confirmed mappings remain durable in RocksDB. The in-memory maps are rebuildable indexes and contain confirmed names only. Provisional candidates cannot enter them before promotion.

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

RocksDB is not expected to understand the graph. PathHydra owns record encoding, identifiers, adjacency, and graph invariants.

The durable layout needs logical key spaces for:

- next-ID metadata;
- vertex records;
- relation ID-to-name records;
- canonical edges;
- provisional candidates and their lifecycle state;
- outgoing adjacency;
- incoming adjacency or an equivalent incident-edge index for complete node deletion;
- external-to-dense mappings for published snapshots;
- exact node-name and relation-name lookup;
- routing snapshot manifests.

These may become column families or prefixed regions. The number of column families is not fixed early because RocksDB gives each one separate memtables and table files, which affects memory and tuning.

Outgoing neighbours must be retrievable with a bounded prefix or range read. The physical adjacency form remains open:

- one entry per key favours simple mutations;
- packed adjacency favours dense reads but rewrites more data;
- chunked blocks provide a middle ground and avoid single huge values for high-degree vertices.

The choice belongs to workload measurements covering ingest rate, random mutation, snapshot construction, high-degree vertices, compaction, and space amplification.

One logical mutation may touch a canonical edge and multiple indexes. Those writes must become visible atomically. RocksDB `WriteBatch` provides atomic multi-key and cross-column-family writes. If adjacency updates require concurrent read-modify-write conflict detection, the graph layer must either serialize them or use RocksDB's transaction support; atomic batches alone do not detect application-level conflicts.

Deletion is part of the graph contract. Removing one directed relation deletes its canonical record and every adjacency/index entry for that relation. Removing a vertex atomically deletes the vertex, its lookup records, and every relation whose source or destination is that vertex. An incoming adjacency or equivalent incident-edge index is therefore required so the engine can find all affected relations without a graph-wide scan. Tombstones or deferred cleanup may support the implementation, but they do not satisfy deletion while incident relations remain part of the confirmed graph.

Record encodings require fixed byte order and length checks. Malformed records must fail visibly.

## Name resolution

Routing uses IDs. A separate resolver maps an exact supplied name to node or relation IDs without scanning payloads. Comparison uses the complete stored string and is case-sensitive. There is no alias, synonym, fuzzy, spelling, punctuation, locale, or Unicode-normalization step.

The resolver returns one of:

- one match;
- no match;
- invalid input.

The same exact name always resolves to the same ID inside its namespace. No near-match lookup is performed. Promotion cannot create a second confirmed ID for an already confirmed exact name.

## Routing snapshot compiler

The compiler turns one consistent view of the confirmed durable graph into a query-independent routing image. Provisional candidates are excluded regardless of whether their proposed records are structurally complete.

Its output contains only what expansion and reconstruction require:

- dense vertex numbering;
- outgoing adjacency boundaries;
- destination dense IDs;
- relation IDs;
- base weights;
- edge handles needed to reconstruct and hydrate requested paths precisely;
- maps between external and dense vertex IDs.

The compiler checks every confirmed endpoint, relation ID, weight, array bound, and count. These are structural checks, not factual validation. It emits a manifest containing the numeric policy, element widths, counts, byte ranges, and checksums.

The image is a rebuildable index, never a second source of truth. A serialized copy is useful because loading a validated contiguous file is different from rebuilding the image through a full database scan.

Shortest-path state is internal search machinery. When paths are requested, predecessor chains provide the vertex and edge handles for each destination independently.

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

## Routing image publication

Durable writes and device arrays cannot change atomically together. A routing image is built completely before it becomes active:

1. Read the confirmed graph needed to build the image.
2. Build and check the routing image.
3. Publish the completed image in one operation.
4. Requests already using the previous image may finish before it is released.

The implementation may rebuild whole images or apply verified deltas, but a request uses one complete image.

Loss of the GPU image should cause a rebuild, not graph loss.

## Request contract

An inference request contains:

```text
origin external ID
destination external IDs[]
relation multiplier vector
return paths? boolean
resource budget
```

Path reconstruction is optional. A distance-only request does not require the engine to retain or return predecessor state.

The multiplier vector is immutable for the request. It is validated and converted to a dense array indexed by relation ID. A missing relation entry must have one documented meaning; silently inheriting process state is not acceptable.

Duplicate destinations are collapsed for search and mapped back to the caller's output positions. The origin may also be a destination and should complete at distance zero without traversal.

Per-destination selection state distinguishes:

- exact distance found, with a path when requested;
- unreachable after complete search;
- missing vertex;
- incomplete because a budget or cancellation stopped the search;
- invalid request.

Every response identifies the numeric policy used. A returned path contains stable vertex and edge handles plus enough weight information to reproduce its distance.

## Rust query runtime

The Rust query runtime owns work around the search algorithm:

- resolve external IDs against the active graph;
- validate and pack the context profile;
- canonicalize destinations;
- select CPU or GPU execution according to capability and admission rules;
- reserve worst-case working memory before admitting GPU work;
- track cancellation and budgets;
- keep independent searches isolated when they are batched;
- reconstruct requested paths and hydrate caller-specified records;
- report exactness separately for every destination.

Destinations within one request share a search. Separate requests retain separate state, stopping conditions, budgets, and results even when their origin and profile happen to match. The scheduler may run them in the same device batch but does not turn them into one search.

## CPU reference engine

A CPU implementation over the routing image is part of the product, not disposable prototype code. It provides:

- a correctness oracle for accelerator results;
- operation without a supported GPU;
- a useful path for small searches where device dispatch costs more than the work;
- deterministic diagnosis and fixture testing.

For non-negative effective weights, a conventional distance-ordered single-source shortest-path implementation is the reference baseline. One frontier serves all destinations and stops after every requested destination is final or the reachable component is exhausted.

The CPU and GPU implementations consume the same snapshot, multiplier vector, numeric policy, destination set, and tie policy. Agreement is checked on distances, completion states, and returned paths where deterministic ties apply.

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
- predecessor state when paths are requested;
- generation or reset state for reused buffers.

One search cannot finalize a destination because another search reached it. Batching is only a scheduling optimization.

A destination is complete only when the algorithm proves its distance final. First discovery is not sufficient. Zero-weight edges and repeated relaxations must close correctly before a bucket or equivalent frontier is retired.

Predecessors may be retained during the first search or reproduced in a second pass against the same snapshot, profile, destinations, and tie policy. The choice trades device memory for recomputation but cannot change the routing contract.

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

## Path reconstruction and hydration

When requested, reconstruction walks predecessor state from a completed destination back to the origin and returns that destination's path handles. Paths remain independent routing results; the Rust engine does not impose a final graph-building strategy.

Hydration accepts caller-specified vertex and edge handles and resolves them into:

- external vertex IDs;
- requested vertex payloads;
- directed edges with relation IDs and labels;
- stored and effective weights for each edge;
- stored and effective path distances when applicable;
- context-profile identity.

Reads are deduplicated and batched. Relation labels may be cached because they are small. Only requested records are fetched.

Hydration returns records; it does not decide how the caller composes them.

## Subgraph construction

The Rust API provides a graph-shaped result container without imposing a policy for what belongs in it. A subgraph stores sets of vertex and edge handles.

The construction surface needs operations equivalent to:

- create an empty subgraph;
- add a vertex handle;
- add an edge handle together with its endpoints;
- add every handle from a reconstructed path;
- union another subgraph;
- remove an edge;
- remove a vertex and every incident edge currently in the subgraph;
- test membership and enumerate vertices and edges;
- hydrate the current contents;
- encode the result for return across the Rust API boundary.

Insertion is idempotent by identity, so shared vertices and edges are stored once. Adding an edge guarantees that both endpoints are present.

Subgraph operations change only the caller-owned result container. They do not mutate confirmed or provisional database state. The caller decides which construction operations to apply; the Rust layer only enforces structural invariants.

## Provisional candidates

Newly proposed graph material enters the Rust layer as provisional candidate data. It remains excluded from confirmed lookup, snapshot compilation, graph selection, and hydration. After validation occurs outside this system, an explicit confirmation promotes it into the confirmed graph in one atomic mutation.

How candidates are produced, validated, grouped, revised, rejected, or reviewed is outside scope. The core contract covers only provisional insertion, exclusion before confirmation, and atomic promotion after validation.

## Confirmed graph deletion

The Rust API supports direct removal of a confirmed directed relation and removal of a confirmed node. Node removal cascades across every confirmed incoming and outgoing relation in the same atomic mutation. It also removes the node's exact-name lookup entry and other confirmed indexes owned by that node.

Deletion must be reflected when the routing image is rebuilt. The routing layer must not admit new work against an image known to contain deleted graph material.

A provisional candidate that refers to an endpoint removed before promotion cannot be confirmed without a new valid endpoint state.

## Rust public API

The stable boundary is a narrow typed graph API rather than a general graph query language. It needs calls equivalent to:

- resolve an exact node or relation name;
- insert provisional candidates;
- confirm candidates after external validation;
- remove a confirmed directed relation;
- remove a confirmed node together with all incoming and outgoing relations;
- submit one-origin/many-destination routing requests;
- reconstruct requested paths;
- hydrate caller-specified node and relation handles;
- construct, combine, edit, and hydrate subgraphs;
- inspect routing capabilities;
- cancel work and read health information.

The binding or local transport, streaming shape, process model, and remote-access policy remain open. The Rust request and response types should not depend on any one transport.

## Failure and recovery

Expected failure classes include:

- invalid records or request numbers;
- missing IDs and relation profile entries;
- invalid provisional-to-confirmed transitions;
- incomplete or inconsistent deletion cascades;
- database write or recovery errors;
- routing image checksum mismatch;
- accelerator allocation, launch, or device loss;
- cancellation and resource exhaustion;
- requested hydration data unavailable.

Each failure has a typed outcome. Device failure must not damage durable graph state. A corrupt routing image is discarded and rebuilt. A routing image is published only after its technical checks pass. Startup either exposes a fully valid routing image or reports that routing is unavailable.

Backups use a documented RocksDB checkpoint or backup procedure. Rebuildable device images may be omitted from backups if startup can regenerate them.

## Verification surface

Correctness fixtures cover:

- a single edge and a multi-hop winner;
- context profiles that change shortest-path results;
- directed edges that cannot be traversed backward;
- exact-name lookup preserving case, spelling, punctuation, and Unicode-sequence differences;
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
- idempotent subgraph insertion and union;
- subgraph node removal cascading through its incident edges;

Property tests generate graphs and profiles, compare CPU and GPU outcomes, and verify every returned path has the reported minimum distance. Adversarial tests concentrate high degree, long chains, dense components, repeated relaxations, and unreachable regions.

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

- request IDs;
- profile identity or hash;
- executor used;
- queue and execution duration;
- completion reason;
- destinations completed and unresolved;
- examined edges and relaxations;
- frontier high-water mark;
- device-memory reservation and peak use;
- reconstruction and hydration duration.

Store-level metrics cover write failures, compaction pressure, cache behaviour, image age, image build failures, and active image references.

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
| Exact-name lookup uses a hash index. | Case-sensitive string hashing followed by full equality provides the required exact-key behaviour, while the resulting numeric IDs support direct array indexing. |
| Rust exposes subgraph construction primitives. | Callers need graph-shaped composition without forcing one composition strategy into the engine. |
| RocksDB is the durable source of truth. | It is embedded, ordered, persistent, supports atomic batches and consistent views, and has a no-fee open-source licence. |
| Routing uses a separate compact snapshot. | Durable payload storage and accelerator traversal have different access patterns; device memory is finite and distinct from host storage. |
| The reference result is exact. | Every reachable destination has an exact minimum context-adjusted distance, so approximation would change the routing contract. |
| Effective weights cannot be negative. | The intended distance model and practical exact SSSP candidates rely on non-negative edge weights. |
| A CPU engine remains available. | Accelerator correctness needs an independent comparator, and GPU availability is not universal. |
| Published routing images are immutable per request. | RocksDB batches cannot atomically modify already-resident device arrays. |
| Full payloads stay out of the routing loop. | Expansion needs endpoints, relation IDs, and weights; reading unrelated text adds storage traffic without affecting the calculation. |

## Decisions deliberately left open

- Rust workspace and crate boundaries;
- concrete hash-map implementation, concurrency strategy, and full-residency threshold;
- GPU vendor and programming API;
- exact accelerator algorithm and tuning parameters;
- stored and accumulated numeric types;
- edge identity and duplicate-edge policy;
- RocksDB column-family split and key encoding;
- adjacency packing and high-degree representation (fixed by Decision 0008);
- full rebuilds, incremental images, or overlay publication;
- snapshot retention method for hydration;
- distance-only versus additional selection-state capture policy;
- profile materialization threshold;
- target-set representation;
- batch width and lane scheduling;
- alternative out-of-core I/O transports beyond the conventional baseline;
- in-process Rust library or separately hosted Rust API packaging;
- request, response, and serialized subgraph encoding;
- in-memory subgraph representation.

Each becomes fixed only after its required workload, correctness constraint, target platform, and benchmark evidence are recorded.

## Evidence for the early choices

- [The Rust project](https://github.com/rust-lang/rust) provides the compiler, standard library, Cargo tooling, and records its MIT and Apache 2.0 licensing.
- Rust's standard-library [HashMap documentation](https://doc.rust-lang.org/stable/std/collections/struct.HashMap.html) documents its hashed key map and full key equality requirements.
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
