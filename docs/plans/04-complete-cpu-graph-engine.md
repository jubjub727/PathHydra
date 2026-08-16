# Plan 04: Complete CPU Graph Engine

## Outcome

Turn the durable catalog and exact CPU router into one coherent CPU-side Rust
engine. At completion, callers use a single typed `GraphEngine` to manage
provisional candidates, promote and remove confirmed graph material, submit
bounded and cancellable routing requests against the current published image,
hydrate caller-specified handles, build and hydrate caller-owned subgraphs,
and inspect capabilities and health.

This plan completes the functional CPU boundary described by
`PATHHYDRA_SYSTEM_SHAPE.md`. It does not make the system a BAML application and
does not begin accelerator work. The result is the reference implementation
against which later GPU execution and bindings can be compared.

## Why these actions belong together

Plan 03 deliberately exposed a point-in-time `RoutingImage` and a pure CPU
`route` function. That was enough to prove routing arithmetic, but it left the
caller responsible for several contracts that the Rust engine is meant to
own:

- choosing which routing image is current;
- preventing new work from entering an image made stale by confirmed deletion
  or promotion;
- distinguishing a durable mutation failure from a post-commit image-build
  failure;
- bounding concurrent CPU work and stopping it deliberately;
- resolving returned handles into complete confirmed records;
- composing path handles without mutating stored graph state;
- reporting whether routing is available and why it is not.

Publication, controlled execution, hydration, and caller-owned composition all
meet at the same public Rust boundary. Implementing only one of them would
leave callers coordinating internal resources or reconstructing graph
invariants themselves. They therefore form one larger vertical slice even
though their internal modules remain separate.

## Explicit non-goals

Do not implement in this slice:

- GPU kernels, GPU dependencies, accelerator selection, GPU admission, or
  CPU/GPU batching;
- a durable or memory-mapped routing-image file, checksums, image-format
  markers, migrations, or compatibility readers;
- incremental or delta image rebuilding;
- graph revision counters, caller-pinned image versions, or historical graph
  query APIs;
- topology partitioning, host/device staging, DirectStorage, or other
  out-of-core work;
- BAML source, generated clients, FFI, a network service, a wire protocol, or
  a serialized subgraph format;
- factual validation of provisional candidates;
- relation-kind deletion or an update-in-place API for confirmed records;
- background worker threads, an async runtime, or a general job scheduler;
- payload logging, hosted telemetry, or performance guarantees.

The engine rebuilds the complete in-memory routing image after every successful
confirmed mutation. This is the deterministic reference policy. Measure it
before designing deltas.

## 1. Restore and freeze the Plan 03 baseline

Run the authoritative checks before changing code:

```powershell
cargo fmt --all --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Keep all exact-identity, durable-mutation, confirmed-read, image-compilation,
numeric-policy, routing, path, and property-style tests passing. Do not weaken
the low-level point-in-time APIs; they remain useful for deterministic tests
and later backend comparison.

Record baseline source lines, test counts, and representative image-build and
CPU-route timings only as implementation context. Do not publish those values
as performance claims.

## 2. Record the CPU runtime ownership decision

Add `docs/decisions/0003-cpu-engine-publication.md` before implementing the
top-level engine. Record these decisions:

- `GraphEngine` exclusively owns one `Catalog` and the current routing state.
- It does not expose a mutable catalog reference or a replaceable active image.
- Provisional insertion does not rebuild the image because provisional
  candidates cannot affect confirmed lookup, routing, or hydration.
- Successful promotion, edge removal, and cascading node removal rebuild the
  whole routing image before the engine admits another new route.
- A request acquires one immutable image exactly once. Once acquired, it may
  finish on that image even if a confirmed mutation publishes a replacement.
- Publication replaces one `Arc<RoutingImage>` under one write lock only after
  the new image is completely built and checked.
- A confirmed mutation that commits durably but cannot produce a valid image
  makes routing unavailable for new requests. It never republishes the known-
  stale prior image.
- Store access remains available while routing is unavailable, including the
  mutations needed to repair graph size or contents and an explicit rebuild
  attempt.
- Hydration reads current confirmed records. It does not silently claim to
  hydrate the historical image used by an earlier route.
- No graph revision number or image version is introduced before release.

The decision record must include the state transitions and lock order used by
the implementation. Lock acquisition order is fixed as:

```text
request-ID registry / admission permit (when routing only)
  -> published-state lock
  -> Catalog public operation and its internal locks
```

Never acquire the request registry or admission controller while holding the
published-state write lock. Document any narrower internal order required by
the final implementation and test it under contention.

## 3. Add the two missing CPU-side crates

Add these workspace members:

```text
crates/pathhydra-subgraph/
  Cargo.toml
  src/
    error.rs
    lib.rs
    subgraph.rs
  tests/
    subgraph.rs

crates/pathhydra-engine/
  Cargo.toml
  src/
    admission.rs
    cancellation.rs
    engine.rs
    error.rs
    health.rs
    hydration.rs
    lib.rs
    publication.rs
  tests/
    publication.rs
    concurrency.rs
    hydration.rs
    engine_api.rs
```

The dependency direction, stated directly, is:

- `pathhydra-subgraph` depends on `pathhydra-core` and
  `pathhydra-routing` so it can consume stable IDs, edge records, and returned
  paths without accessing storage;
- `pathhydra-engine` depends on core, store, routing, and subgraph;
- neither store nor routing depends on engine;
- core remains dependency-free;
- no crate depends on BAML, serialization, logging, async, or GPU libraries.

Keep storage, routing, hydration, publication, and caller-owned composition in
their named modules. Do not create a generic repository, backend, executor, or
graph trait when there is still only one implementation.

## 4. Define the published routing state machine

`GraphEngine` owns a published state equivalent to:

```text
RoutingState
  Available
    image: Arc<RoutingImage>
    published_at: Instant
    last_build: ImageBuildReport
  Unavailable
    reason: RoutingUnavailableReason
    last_build: ImageBuildReport
```

Use process-local time only for health durations; do not persist timestamps or
turn them into graph identity.

`GraphEngine::open(path, config)` performs:

1. validate runtime configuration;
2. open and fully validate the catalog;
3. obtain one consistent confirmed graph read;
4. compile and validate a complete routing image;
5. enforce the configured topology-memory limit;
6. publish the image or record routing as unavailable;
7. return the engine whenever the catalog itself opened successfully.

An image compile or topology-limit failure does not hide the valid durable
catalog behind an open error. `GraphEngine::open` fails only when the catalog
cannot be safely opened or the engine's own synchronization/configuration
cannot be initialized. Routing availability is inspected separately.

Expose `GraphEngine::rebuild_routing_image()`. It holds the publication write
lock across confirmed-record capture, compilation, validation, limit checking,
and publication. If an already-current image exists and a manual rebuild fails
without an intervening confirmed mutation, keep that current image. If routing
was already unavailable, keep it unavailable with the newest failure report.

No partially compiled image enters `Available`. Store the active image only in
an `Arc`; requests clone that `Arc` under the publication read lock and release
the lock before search. In-flight requests therefore do not block confirmed
mutations after image acquisition.

## 5. Put every confirmed mutation through publication

Expose the existing candidate and graph operations through `GraphEngine` with
the same precise terminology:

```text
insert_node_candidate
insert_node_candidate_with_payload
insert_relation_candidate
insert_edge_candidate
get_candidate
confirm_validated_candidate
remove_edge
remove_node
lookup_node_exact
lookup_relation_exact
get_node
get_relation
get_edge
```

Candidate insertion, candidate reads, exact lookup, and confirmed record reads
delegate to the catalog and do not publish an image. Do not expose
`GraphEngine::catalog()` or any other path that allows a caller to mutate the
owned catalog outside publication coordination.

Promotion and removal acquire the publication write lock before touching the
catalog. Their sequence is:

1. perform the durable catalog mutation;
2. if the durable mutation fails, leave the active image unchanged and return
   the typed store failure;
3. if it succeeds, prevent any further new image acquisition;
4. capture the complete confirmed graph;
5. compile and check a replacement image;
6. publish it in one assignment; or
7. mark routing unavailable if any post-commit step fails.

Return a mutation result that makes commit state unambiguous:

```text
ConfirmedMutation<T>
  durable_result: T
  publication:
    Published(ImageBuildReport)
    RoutingUnavailable(RoutingUnavailableReason, ImageBuildReport)
```

A post-commit publication failure is not returned as if the durable mutation
failed. In particular, a caller must never be encouraged to retry promotion of
a candidate that was already consumed. Typed top-level errors are reserved for
failures before durable commit or failures to enter the engine operation.

Every confirmed mutation triggers publication even when it appears not to
affect current adjacency: a new isolated node changes destination membership,
and a new relation kind changes the required profile domain.

## 6. Bound topology and per-request CPU resources

Define a validated `EngineConfig` with at least:

```text
max_active_image_bytes
max_concurrent_routes
max_reserved_route_bytes
max_destinations_per_request
max_hydration_handles_per_request
```

All limits are explicit. Zero concurrent routes is invalid. Limit arithmetic
uses checked operations. Defaults are documented constants chosen for local
development, not performance promises.

Add a `CpuWorkingSetEstimate` in `pathhydra-routing`. Compute a conservative
upper bound from the current image and request before search begins. Account
for:

- packed relation profile;
- dense destination mapping and unique-target tracking;
- distance and finalized arrays;
- predecessor and path-validation state when paths are requested;
- a binary-heap frontier with at most one push for the origin plus one push per
  examined adjacency;
- response entries in original destination order;
- one maximum simple reconstructed path for each unique present destination;
- fixed per-allocation payload needed by the current implementation.

The estimate is a logical collection-payload reservation, not a promise about
allocator metadata or physical RSS. Document that distinction. Do not omit a
data structure because it is usually small, and do not use average frontier or
path lengths.

Refactor search structures where necessary to make their maximum size
inspectable. In particular:

- replace hash-based pending-destination state with dense bounded membership
  state;
- reserve the frontier's checked worst-case capacity before search;
- reserve output and reconstruction vectors before filling them;
- share reconstructed paths for duplicate destination positions rather than
  duplicating their step storage;
- use fallible `try_reserve` operations for request-dependent collections and
  return typed resource exhaustion instead of relying on uncontrolled growth.

The engine uses a small admission controller with RAII permits. It rejects a
request before search when:

- the destination count exceeds its limit;
- the working-set estimate overflows;
- the request alone exceeds the total route-byte limit;
- the maximum concurrent-route count is already reached; or
- admitting it would exceed the configured total reserved bytes.

This slice refuses excess work with a typed outcome; it does not queue. Permit
release must occur on success, request validation failure, cancellation,
arithmetic failure, panic unwinding through safe Rust, and early return.

Topology bytes are accounted separately from per-route bytes. A confirmed
mutation whose replacement image exceeds `max_active_image_bytes` commits to
RocksDB but leaves new routing unavailable, providing a real and testable
post-commit publication failure without a test-only compiler abstraction.

Refactor routing-image compilation to calculate checked counts and byte totals
before allocating topology arrays, enforce the engine's topology limit at that
preflight boundary, and use fallible reservations for confirmed-record and
image-build collections. A configured byte limit must not first allocate the
oversized image it is intended to reject.

## 7. Add request identity and cooperative cancellation

Add a process-local opaque `RequestId(u64)` to the engine API. It is supplied
by the caller, is not durable graph identity, and is used only for active-work
coordination and diagnostics. Two simultaneously active routes may not have
the same request ID.

For each admitted request, register one `Arc<AtomicBool>` cancellation flag in
an active-request map. Use an RAII registration guard so every exit removes the
entry. Expose:

```text
GraphEngine::route(request_id, request)
GraphEngine::cancel(request_id)
```

Cancellation returns a typed outcome distinguishing signalled, already
signalled, and not active. It does not spawn a thread and does not forcefully
terminate one. Callers obtain concurrency by invoking the synchronous engine
from their own threads.

Extend the low-level routing crate with a controlled CPU entry point while
preserving the existing pure `route(image, request)` convenience function. The
controlled route checks cancellation:

- after origin/profile/destination validation and initialization;
- before finalizing the next frontier node;
- before examining each outgoing edge; and
- during reconstruction of each path.

If cancellation and edge-budget exhaustion are both visible at the same edge
boundary, cancellation wins. Already finalized destinations remain exact;
missing destinations remain missing; every unresolved present destination is
incomplete. Add `Cancelled` to the completion reason. An origin requested as a
destination is initialized as exact distance zero before cancellation can stop
traversal.

Cancellation never mutates the routing image, profile, durable graph, another
request's state, or its admission reservation.

## 8. Make CPU execution diagnostics complete and inspectable

Extend controlled routing to collect, without logging payloads:

- examined edges;
- successful relaxation updates;
- finalized nodes;
- frontier high-water mark;
- unique present, exact, unreachable, missing, and incomplete destination
  counts;
- path reconstruction step count;
- completion reason.

The engine wraps the routing response in runtime diagnostics containing:

```text
request ID
executor = CPU reference
numeric policy
tie policy
image node/relation/adjacency counts
reserved working bytes
admission duration
execution duration
completion reason
search counters
```

Durations use a monotonic process-local clock and are diagnostic only. Profile
diagnostics may include a process-local hash for correlation, but the hash is
never treated as profile identity: the complete canonical profile remains in
the routing response and full equality remains authoritative.

Keep diagnostics structured and returned. Do not introduce a logging facade,
metrics exporter, tracing subscriber, or global singleton.

## 9. Add one batched confirmed-record hydration boundary

Hydration operates on current confirmed graph state, not provisional data and
not an implied historical snapshot. Add a store method equivalent to:

```text
Catalog::confirmed_records_by_id(node_ids, edge_ids)
```

The method holds the catalog write mutex for the whole batch, deduplicates
physical reads, and fetches:

- each requested confirmed node record;
- each requested confirmed edge record;
- the confirmed relation-kind record named by every found edge.

It preserves enough mapping information for the engine to restore original
request order and duplicates. A requested ID absent from confirmed storage is
reported as missing, not as a corrupt record. A found edge whose endpoint or
relation kind is absent is structural corruption. Provisional candidates are
never consulted.

Add transport-independent hydration types in `pathhydra-engine`:

```text
HydrationRequest
  node IDs[]
  edge IDs[]
  optional relation profile

HydratedNodeResult
  requested node ID
  Found(NodeRecord) | Missing

HydratedEdgeResult
  requested edge ID
  Found(HydratedEdge) | Missing

HydratedEdge
  EdgeRecord
  exact RelationRecord label
  Disabled | Enabled(multiplier, effective weight)
```

Without a profile, an edge has no effective-weight field; do not invent a
default context. With a profile, validate the complete profile against the
current active image before reading and preserve it in the result. Enabled
effective weights use Decision 0002 arithmetic. Disabled is an explicit state,
not zero.

`GraphEngine::hydrate` acquires the publication read lock for the entire batch,
so an engine-owned confirmed mutation cannot interleave with current-image
profile validation and record reads. Hydration has its own handle-count limit
and fallible allocations, but it does not consume a route admission permit.

Duplicate requested handles are read once and reproduced in original order.
Return partial found/missing results rather than failing the whole request when
ordinary requested data was deleted. Return typed whole-request errors for an
invalid profile, structural corruption, allocation failure, or unavailable
routing state when a profile must be validated.

## 10. Hydrate routing paths without changing their meaning

Add a path-hydration operation that accepts a `RoutingResponse` and one exact
destination-result position. It obtains the path, complete canonical profile,
numeric policy, and tie policy from that one response rather than accepting
independently supplied evidence. It requests every distinct path node and edge
in one batch, then validates current records against the path evidence:

- edge ID;
- source and destination IDs;
- relation ID;
- stored base weight;
- request multiplier or disabled state;
- effective weight;
- ordered step continuity;
- summed logical distance.

Return a `HydratedPath` containing ordered complete node records and ordered
hydrated directed edges, plus the original logical distance, numeric policy,
tie policy, and profile.

An older response profile is validated against its own path steps and fetched
records, not repacked as though it were a new request against the current
image. A relation kind confirmed after the route therefore does not invalidate
an otherwise available old path merely because the old profile lacks that new
entry. Generic current-state hydration with a supplied profile still requires
one complete entry for every relation kind in the current image.

If any node or edge was removed after the route acquired its image, return a
typed `HydrationUnavailable` listing all unavailable stable handles. Do not
silently hydrate a different path, omit a step, or substitute a near-matching
name. This is the explicit current-state hydration policy; historical snapshot
retention remains outside this slice.

Relation and node names are preserved byte-for-byte as their current confirmed
records provide them. No normalization, aliasing, or payload interpretation is
added.

## 11. Implement the caller-owned subgraph container

Implement `Subgraph` in `pathhydra-subgraph` as deterministic sets of stable
handles with endpoint evidence:

```text
nodes: ordered set of NodeId
edges: ordered map EdgeId -> (source NodeId, destination NodeId)
```

Use standard-library ordered collections so enumeration is stable without
claiming that order changes graph semantics.

Expose operations equivalent to:

```text
Subgraph::new
add_node
add_edge(edge ID, source, destination)
add_edge_record
add_path
union
remove_edge
remove_node
contains_node
contains_edge
nodes
edges
node_count
edge_count
```

Enforce these invariants:

- adding an edge also inserts both endpoints;
- repeating the same node or the same edge with identical endpoints is
  idempotent;
- reusing one edge ID with different endpoints is a typed conflict;
- adding a path validates origin, destination, step continuity, and endpoint
  evidence before mutating the subgraph;
- union validates every conflicting edge before changing the receiver;
- removing a node removes every currently included incoming and outgoing edge;
- removing an edge does not implicitly remove now-isolated nodes;
- self-edges are stored once and removed once;
- parallel edges remain distinct by `EdgeId`;
- every operation changes only the caller-owned subgraph.

Operations that can fail after inspecting multiple inputs must be atomic from
the caller's perspective. Prevalidate or stage changes before mutating the
receiver. Do not give `Subgraph` a catalog, engine, global registry, or interior
mutation.

Provide a typed `SubgraphHandles` export containing ordered node IDs and edge
handle/endpoint records for the Rust API boundary. Do not select JSON, BAML,
Serde, or another byte encoding in this slice.

## 12. Hydrate subgraphs through the engine

Expose:

```text
GraphEngine::hydrate_subgraph(subgraph, optional profile)
```

The operation uses the same one-batch hydration implementation as ordinary
handles. It validates every found edge's current endpoints against the
subgraph evidence and returns:

```text
HydratedSubgraph
  complete confirmed node records in stable ID order
  complete hydrated directed edges in stable edge-ID order
  missing node IDs
  missing edge IDs
  optional complete canonical profile
  completeness flag
```

Missing current records make the result explicitly incomplete but do not
mutate the input subgraph. A found edge with mismatched endpoint evidence is a
typed integrity error. Adding endpoint nodes during subgraph construction does
not force the hydration layer to invent payloads when those nodes have since
been removed.

Hydration returns records only. It does not decide which paths to union, which
destinations are relevant, or how a final inference graph should be composed.

## 13. Expose one narrow complete CPU Rust API

The primary `pathhydra-engine` surface should contain only concrete typed
operations needed now:

- engine open and explicit routing rebuild;
- exact node and relation-kind lookup;
- provisional insertion and inspection;
- confirmed promotion and deletion with publication outcomes;
- synchronous bounded CPU routing with request ID and cancellation;
- current-record, path, and subgraph hydration;
- caller-owned subgraph types or re-exports;
- capabilities, health, and metrics snapshots.

Add `EngineCapabilities` describing facts rather than aspirations:

```text
CPU reference routing: supported
GPU routing: unsupported
paths: supported
edge budgets: supported
cancellation: supported
hydration: supported
subgraphs: supported
durable routing images: unsupported
numeric and tie policy IDs
configured resource limits
```

Add `EngineHealth` with:

- durable catalog availability;
- routing available/unavailable and typed reason;
- current image manifest when available;
- current image age;
- last image-build duration and outcome;
- active and peak route counts;
- currently and peak reserved route bytes;
- cumulative route admissions, admission rejections, cancellations, and image
  build failures.

Metrics are process-local atomics or lock-protected counters. Counter overflow
saturates and is documented; it must not fail graph operations. Health reads do
not expose payloads, profile contents, raw RocksDB handles, active cancellation
flags, or mutable engine state.

Do not call this API stable before the first public release, and do not add
compatibility shims for Plan 03 constructors. Update unreleased call sites and
docs directly when types need to change.

## 14. Define failure and recovery behavior explicitly

Use typed errors and outcomes for at least:

- invalid engine configuration;
- catalog open or durable mutation failure;
- routing unavailable;
- image compilation or topology-limit failure;
- durable mutation committed but publication unavailable;
- duplicate active request ID;
- route concurrency or byte admission refusal;
- request allocation failure;
- invalid origin, profile, number, or resource estimate;
- cancellation and deterministic budget exhaustion as response completion
  reasons rather than generic errors;
- hydration limit, invalid profile, unavailable handle, and integrity failure;
- subgraph edge-identity conflict and invalid path continuity;
- lock poisoning and internal invariant failure.

Recovery rules are:

- a store failure before durable commit leaves the current image unchanged;
- a publication failure after durable commit unpublishes the stale image for
  new requests;
- old `Arc` images survive only while already-acquired routes use them;
- routing-unavailable does not block provisional insertion, confirmed reads,
  repair mutations, health, or explicit rebuild;
- a successful rebuild publishes one complete current image and restores new
  routing admission;
- cancellation and route resource exhaustion never affect durable data or
  publication state;
- hydration failure never mutates the catalog or subgraph;
- process restart always rebuilds from validated RocksDB state in this slice.

Do not catch and reinterpret arbitrary panics as recoverable engine failures.
Write safe code whose expected failure paths return typed results.

## 15. Test publication and mutation races first

Add deterministic fixtures covering:

- startup publishing an empty and a non-empty confirmed graph;
- startup retaining a usable catalog when the configured topology limit makes
  routing unavailable;
- provisional insertion leaving the active image unchanged;
- node, relation-kind, and edge promotion publishing a replacement image;
- edge and cascading node deletion preventing every subsequently admitted
  route from seeing deleted material;
- a request that already acquired the prior image finishing correctly while a
  mutation publishes a replacement;
- no new request acquiring the old image after durable mutation commit;
- durable mutation failure preserving the active image;
- topology-limit failure after durable commit producing an unambiguous
  committed/unavailable mutation result;
- repair mutation plus explicit rebuild restoring routing availability;
- multiple simultaneous promotion/removal operations serializing without a
  stale publication winning;
- publication lock poisoning, where practically constructible, returning a
  typed failure rather than exposing partial state.

Use unit-level image leases or controlled internal barriers for deterministic
publication tests. Do not rely on arbitrary sleeps or a route merely being
"large enough" to create a race.

## 16. Test admission, cancellation, and request isolation

Cover at least:

- invalid zero and overflowing configuration limits;
- conservative working-set estimates at empty, pathless, and path-returning
  boundaries;
- destination-count, topology-byte, per-request-byte, total-byte, and
  concurrent-count refusal;
- permits and request IDs released after success and every typed error;
- duplicate active request IDs rejected without cancelling the first request;
- pre-signalled cancellation;
- cancellation during expansion and during path reconstruction;
- cancellation winning over budget at the same check point;
- finalized destinations remaining exact while unresolved destinations become
  incomplete;
- missing destinations remaining missing after cancellation;
- one request's cancellation not changing another request sharing the image;
- active routes on old images releasing all old image references after exit;
- fallible reservation errors leaving runtime accounting at zero;
- high-water and cumulative diagnostics changing monotonically.

Retain the existing deterministic edge-budget and small-graph oracle tests.
Controlled and uncontrolled routes must agree exactly when cancellation is not
signalled and admission succeeds.

## 17. Test hydration and subgraph contracts

Hydration fixtures cover:

- empty, duplicate, reordered, found, and missing node/edge handle requests;
- exact opaque payload, node name, and relation-kind label preservation;
- directed parallel and self-edge hydration;
- enabled, disabled, zero, subnormal, and maximum multiplier evidence;
- invalid profiles rejected before record results are published;
- provisional nodes, relation kinds, and edges remaining unavailable;
- current-state batch consistency while a confirmed deletion waits;
- routing against an old image followed by deletion and typed path-hydration
  unavailability;
- path edge mismatch and distance mismatch rejected visibly;
- hydration handle limits and allocation failure;
- no normalization or near-match lookup.

Subgraph fixtures cover:

- idempotent node and edge insertion;
- edge insertion adding both endpoints;
- parallel edges and self-edges;
- conflicting endpoint evidence for one edge ID;
- atomic path insertion and atomic union failure;
- shared path prefixes stored once;
- edge removal preserving isolated nodes;
- node removal cascading through only the subgraph's incident edges;
- deterministic enumeration and handle export;
- operations leaving confirmed and provisional catalog state byte-for-byte
  unchanged;
- complete and incomplete subgraph hydration;
- endpoint mismatch detected during hydration.

Prefer small visible graphs, then add one high-degree subgraph and one large
batched hydration fixture to exercise limits without making timing assertions.

## 18. Add concurrency and recovery stress coverage

Add bounded repeated stress tests that run locally and finish predictably:

- concurrent routes with distinct origins, profiles, budgets, request IDs, and
  cancellation outcomes;
- concurrent provisional insertion while routes use the active image;
- repeated confirmed mutations publishing while old routes finish;
- repeated admission refusal and permit reuse;
- health snapshots read during routing, cancellation, mutation, unavailable,
  and recovered states;
- engine close and reopen rebuilding the same current routing behavior from
  RocksDB;
- failed publication followed by process-local recovery without reopening;
- no request result mixing state from two images.

Tests prove safety and result isolation; they do not claim throughput. Avoid
unbounded randomized loops and wall-clock pass/fail thresholds.

## 19. Document the completed CPU boundary

Add:

- `docs/cpu-engine.md` for ownership, state transitions, lock order,
  publication, admission, cancellation, capabilities, health, and recovery;
- `docs/hydration.md` for current-state semantics, batching, profile evidence,
  path validation, missing handles, and provisional exclusion;
- `docs/subgraphs.md` for caller-owned invariants and every construction/edit
  operation.

Update `docs/routing-image.md` to distinguish low-level caller-owned images
from the engine-published current image. Update the storage reference only for
new batched read behavior; do not invent new durable key spaces.

Add rustdoc examples for:

1. opening `GraphEngine` and inspecting capabilities;
2. promoting graph material and observing successful publication;
3. routing with a request ID and cancelling from another caller thread;
4. hydrating an exact returned path;
5. unioning paths into a subgraph and hydrating it;
6. handling a committed mutation whose replacement image cannot be published.

Update the README implementation status only after implementation. It should
state that the complete CPU-side Rust engine works and list GPU acceleration,
durable image files, BAML/bindings, and transport as unimplemented. The README
must not link to or enumerate planning documents.

## 20. Completion checks

Run:

```powershell
cargo fmt --all --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Plan 04 is complete only when:

- every Plan 00-03 behavior still passes;
- the top-level engine is the sole owner of active-image freshness;
- every successful confirmed mutation either publishes its complete current
  replacement or explicitly makes routing unavailable;
- no new route can enter a known-stale image while already-acquired routes may
  finish safely;
- CPU requests are bounded, independently cancellable, and fully diagnosed;
- resource accounting and RAII cleanup survive every tested exit path;
- hydration batches only current confirmed records and reports deleted handles
  without substitution;
- hydrated paths preserve and revalidate all routing evidence;
- subgraph operations are deterministic, idempotent, structurally safe, and
  incapable of mutating database state;
- health and capabilities truthfully describe the available CPU engine;
- routing recovers from tested post-commit publication failure through a full
  rebuild;
- all public behavior, state transitions, limits, and failure classes are
  documented;
- no graph revision counter, serialized image, GPU, BAML, transport, hosted
  telemetry, or GitHub Actions code is added.

Suggested commit message:

```text
Complete the CPU graph engine
```

## Following slice

After this plan, the CPU-side Rust contract is complete enough to serve as the
oracle and fallback for accelerator work. The next plan should choose one
separate evidence-driven direction:

- benchmark graph shapes and implement one real GPU backend against the same
  image, profile, numeric, tie, cancellation, and result contracts; or
- add a BAML-facing local binding/transport around the completed Rust API.

Do not combine those two directions merely because the CPU engine is ready for
both.
