# Plan 03: Immutable Routing Image and Exact CPU Routing

## Outcome

Turn the confirmed durable graph into the first exact routing implementation.
At completion, PathHydra can compile one consistent point-in-time view of the
confirmed graph into an immutable, CPU-consumable routing image and answer a
one-origin, many-destination request with exact logical distances and optional
path identities.

This is the CPU reference boundary required before GPU algorithm, scheduling,
or publication work begins. It is also the first slice in which request
context changes the selected path through arithmetic rather than through a
post-search rule system.

## Why this is the next slice

Plans 01 and 02 established the durable source of truth:

- confirmed nodes and relation kinds have stable exact identities;
- confirmed typed, directed edges have independent stable identities;
- base weights are validated and encoded canonically;
- outgoing and incoming adjacency are complete;
- provisional candidates are physically excluded from confirmed reads;
- promotion and deletion preserve the durable graph atomically;
- opening a catalog validates every confirmed record and index relationship.

The nearest useful vertical slice in `PATHHYDRA_SYSTEM_SHAPE.md` is therefore
the query-independent routing image followed by the deterministic CPU engine
that consumes it. Implementing both together proves that the stored graph can
produce exact context-adjusted routing results without introducing GPU or
transport decisions.

## Explicit non-goals

Do not implement in this slice:

- a serialized routing-image file, file-format compatibility, or migrations;
- an active-image publisher, automatic rebuilds, mutation invalidation, image
  retention, graph revision counters, or pinned-version APIs;
- GPU code, GPU dependencies, accelerator admission, batching, or benchmarks;
- concurrent query scheduling, asynchronous execution, deadlines, or external
  cancellation tokens;
- topology partitioning, host-to-device staging, or out-of-core routing;
- hydration of caller-specified records;
- caller-owned subgraph construction;
- BAML code, bindings, a network service, or a wire protocol;
- relation-kind deletion;
- performance claims based only on the reference implementation.

The routing image in this slice is an immutable point-in-time value, not an
active database index. A request explicitly borrows or owns that value before
it starts, which is equivalent to a request already using an older published
image. This slice does not expose a long-lived "current image" or claim that
new work is automatically kept fresh after graph mutation. That admission and
publication contract belongs to the next slice and must be enforced there
without adding a pre-release compatibility layer.

## 1. Restore and freeze the current baseline

Before changing code, run the authoritative checks:

```powershell
cargo fmt --all --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Keep the current graph-store fixtures passing throughout the work. Do not
weaken startup validation or expose raw RocksDB handles to make compilation
easier. Add no GitHub Actions workflow.

## 2. Fix the routing numeric contract

Add `docs/decisions/0002-cpu-routing-arithmetic.md` before defining durable or
public routing types. Record the following reference policy.

### Request multiplier

`RelationMultiplier` stores a canonical IEEE-754 binary32 value.

- Every value is finite and greater than or equal to positive zero.
- Negative values, NaN, and infinities are invalid.
- Negative zero is accepted at the API boundary and canonicalized to positive
  zero.
- Zero is a usable multiplier and can create zero-effective-weight edges. It
  does not mean that the relation kind is disabled.
- A disabled relation kind is represented explicitly and its edges are not
  traversable for that request.
- Every confirmed relation kind has exactly one explicit profile entry:
  enabled with a multiplier, or disabled. A missing entry is an invalid
  request, not inherited process state.
- Entries naming an unconfirmed relation ID are invalid.

Binary32 keeps the request profile compact and gives later CPU/GPU work the
same input scalar representation as stored base weights. Profile equality is
equality of the canonical relation IDs, enabled/disabled states, and multiplier
bits, not approximate numeric equality.

### Effective weights and path distance

Convert both binary32 operands to binary64 before multiplication:

```text
effective = f64(base_weight) * f64(relation_multiplier)
candidate = current_distance + effective
```

The product of two finite binary32 values is representable within binary64's
range, and any addressable finite graph has far too few edges for a simple
minimum path accumulated from those products to overflow binary64. Still check
every product and sum for finiteness and return a typed arithmetic failure if
that invariant is violated.

Use ordinary round-to-nearest, ties-to-even binary64 operations. Do not use a
fused multiply-add or approximate comparison. Keep multiplication and addition
as separate operations so the same policy can be reproduced by a later GPU
backend.

Unreachable and incomplete are result states, not floating-point sentinel
values. No public result uses infinity or NaN as a logical distance.

### Equal-distance policy

Define one reference tie policy and identify it in every response. The CPU
reference policy is:

1. Dense node IDs are assigned in ascending external `NodeId` order.
2. Outgoing adjacency is ordered by ascending `EdgeId`.
3. The distance frontier is ordered by logical distance and then dense node ID.
4. For equal tentative distances before a node is finalized, prefer the
   predecessor tuple `(predecessor dense node ID, edge ID)` with the lowest
   lexicographic value.
5. A finalized node is never reopened for an equal-distance alternative.

This selects one stable minimum-distance predecessor tree without claiming
that the path is the globally lexicographically smallest path. Zero-weight
cycles terminate because each node is finalized at most once. A later GPU
backend either implements this tie policy exactly or reports that it cannot
serve requests requiring it and leaves them on the CPU reference engine.

Test the numeric wrapper types and arithmetic helpers directly, including
negative zero, subnormal values, maximum finite multipliers, zero products,
exact equality, and rejected values.

## 3. Add a routing crate at the current dependency boundary

Add one workspace member:

```text
crates/pathhydra-routing/
  Cargo.toml
  src/
    image.rs
    compile.rs
    profile.rs
    request.rs
    cpu.rs
    error.rs
    lib.rs
  tests/
    compile.rs
    cpu_routing.rs
```

`pathhydra-routing` depends on `pathhydra-core` and `pathhydra-store`. It owns
routing-image, profile, request, result, compiler, and CPU-search behaviour.
It does not own durable encodings or payload hydration.

Keep `pathhydra-core` dependency-free. Add routing-neutral numeric wrappers to
core only if both storage and routing genuinely need them; otherwise keep them
inside `pathhydra-routing`. Do not create separate compiler, runtime, or API
crates before those boundaries have independent implementations.

Use the standard library for the image, maps, priority queue, and errors. Add
no serialization, async-runtime, logging, graph-algorithm, or concurrent-map
dependency.

## 4. Expose one consistent confirmed-graph read

Add a narrow store-owned aggregate such as:

```text
ConfirmedGraphRecords
  nodes[]
  relation_kinds[]
  edges[]

Catalog::confirmed_graph_records()
```

The method runs under the catalog's existing write mutex for the complete
scan. This deliberately simple baseline prevents promotion or deletion from
interleaving with the read and avoids exposing RocksDB snapshot lifetimes or
column-family details outside `pathhydra-store`.

The read must:

- iterate only confirmed node, relation-kind, and canonical edge key spaces;
- decode every key and value through the current store codecs;
- verify each key agrees with the ID inside its value;
- return records sorted by stable numeric ID;
- exclude provisional candidates completely;
- return a typed store error on malformed or inconsistent records;
- release the write mutex after producing the self-contained aggregate.

The aggregate contains complete node records because this is the narrowest
existing store record type, but the compiler must not copy names or payloads
into the routing image. Do not expose column-family handles, iterators, encoded
keys, or RocksDB snapshots.

Add store integration tests proving that the aggregate is point-in-time,
sorted, structurally complete, and contains no provisional node, relation-kind,
or edge candidate.

## 5. Compile an immutable CSR routing image

Define an immutable `RoutingImage` whose fields are private and stored as
boxed slices or read-only maps. Its baseline topology contains:

```text
external_to_dense: NodeId -> DenseNodeId
dense_to_external[dense node] -> NodeId
offsets[dense node + 1] -> adjacency offset
destinations[adjacency] -> DenseNodeId
relation_ids[adjacency] -> RelationId
base_weights[adjacency] -> BaseWeight
edge_ids[adjacency] -> EdgeId
confirmed_relation_ids[] -> RelationId
manifest
```

Use `DenseNodeId(u32)` for the initial in-memory image. Reject compilation when
the node count cannot be represented by `u32`; do not truncate or wrap. Use
`u64` offsets in the image and check all conversions to and from Rust `usize`.
Stable node, relation, and edge IDs remain their current `u64` types.

The compiler must:

1. Assign dense node IDs in ascending external `NodeId` order.
2. Verify every node ID is unique.
3. Verify every relation ID is unique.
4. Verify every edge ID is unique.
5. Resolve both endpoints of every edge to dense IDs.
6. Verify every edge names a confirmed relation kind.
7. Revalidate every stored base weight through its canonical representation.
8. Count outgoing edges with checked arithmetic.
9. Build offsets with checked prefix sums.
10. Place edges under their exact directed source.
11. Sort each outgoing range by ascending `EdgeId`.
12. Validate the completed arrays before returning the image.

Parallel edges and self-edges remain separate adjacency entries. No edge is
deduplicated by endpoints, relation kind, weight, or hash. Incoming adjacency
is not copied because CPU expansion and path reconstruction need only outgoing
topology and stable edge handles.

The image manifest records:

- the numeric policy and tie-policy identifiers;
- node, relation-kind, and adjacency counts;
- each element width;
- exact byte counts for topology arrays and mapping tables;
- whether predecessor-capable edge IDs are present.

Do not add a checksum in this slice because there is no serialized or mutable
routing-image representation to validate. The later serialized-image plan must
define canonical bytes and checksums together rather than pretending that an
in-memory Rust layout is a file format.

Expose only methods required by compilation tests and the CPU engine: count
inspection, exact ID mapping, and bounded outgoing-range access. Do not expose
mutable array access.

## 6. Define the exact request and response contract

Define transport-independent request types equivalent to:

```text
RoutingRequest
  origin: NodeId
  destinations: NodeId[]
  profile: RelationProfile
  return_paths: bool
  budget: SearchBudget
  tie_policy: TiePolicy
```

`RelationProfile` is constructed from `(RelationId, RelationUse)` entries,
where `RelationUse` is either disabled or enabled with a validated multiplier.
Packing against a routing image produces a dense immutable lookup indexed by
the image's confirmed relation IDs. Reject duplicate, missing, and unknown
profile entries before search begins.

For this reference slice, `SearchBudget` contains an explicit maximum number
of examined edges. An unlimited value is explicit rather than inferred from a
magic number. Count an edge immediately before examining its relation state or
performing relaxation. If no budget remains, stop before examining that edge.

Depth and fan-out are not independently restricted in the reference engine:
their explicit policy is unlimited. Cycles are allowed and bounded by node
finalization. Deadlines and cancellation are deferred because they introduce
nondeterministic stopping points; later runtime work may add them while
preserving the deterministic edge-examination budget.

The request contract is:

- a missing origin is a whole-request error;
- a missing destination receives a per-destination `MissingNode` result;
- duplicate destinations share search work and are expanded back into the
  original output order;
- an origin that is also a destination completes exactly at distance zero;
- an empty destination list is valid after origin and profile validation;
- a disabled relation kind makes its edges unavailable for that request;
- a zero multiplier keeps its edges available with zero effective weight;
- invalid profile numbers or membership fail before search state is allocated.

Return one result for every supplied destination position. The destination
state is exactly one of:

- `Exact`, containing a finite logical distance and optionally one path;
- `Unreachable`, only after the complete reachable region is exhausted;
- `MissingNode`;
- `Incomplete`, when the deterministic budget stops the search before that
  destination is finalized or proved unreachable.

Every response preserves or identifies:

- the origin;
- the original destination sequence;
- the canonical relation profile;
- the numeric policy;
- the tie policy;
- whether paths were requested;
- examined-edge and finalized-node counts;
- the completion reason.

Use typed request, compilation, arithmetic, and internal-invariant errors. Do
not collapse missing nodes, invalid numbers, budget exhaustion, unreachable
destinations, or corrupt images into one string error.

## 7. Implement the deterministic CPU reference engine

Implement distance-ordered single-source shortest-path search over the routing
image with the standard library's binary heap.

One frontier serves all unique, present destinations. Search stops when:

- every unique present destination is finalized;
- the frontier is exhausted; or
- the edge-examination budget is exhausted.

Maintain per dense node:

- tentative binary64 logical distance;
- finalized state;
- predecessor dense node and edge ID only when paths are requested;
- enough deterministic predecessor rank to apply the tie policy.

Stale heap entries are ignored. A destination becomes exact only when its node
is finalized, never when it is first discovered. Disabled edges are counted as
examined but are not relaxed. Zero-weight edges, self-edges, and parallel edges
follow the same relaxation path as every other edge.

When the frontier is exhausted, every unresolved present destination is
`Unreachable`. When the budget stops search, already finalized destinations
remain `Exact` and every unresolved present destination is `Incomplete`;
none may be reported unreachable.

A distance-only request must not allocate or retain predecessor arrays. Keep
search state local to one invocation. Two requests with identical origins or
profiles do not share tentative distances, budgets, completion state, or
predecessors in this slice.

## 8. Reconstruct inspectable paths

When paths are requested, reconstruct each exact destination independently by
walking predecessor state back to the origin. Detect a missing predecessor,
repeated predecessor node, endpoint mismatch, or excessive chain length as an
internal invariant failure rather than looping or returning a partial path.

A returned path contains:

```text
origin NodeId
destination NodeId
logical distance
ordered steps[]

PathStep
  edge ID
  source NodeId
  destination NodeId
  relation ID
  stored base weight
  request multiplier
  effective weight
```

The path for `origin == destination` contains no steps and has distance zero.
Re-sum every reconstructed path under the declared arithmetic policy in debug
or test validation and require its value to equal the reported logical
distance exactly.

Path reconstruction returns stable handles and weight evidence. It does not
load node payloads or relation labels; that is hydration and remains a later
slice. It also does not merge paths into a subgraph.

## 9. Test compiler and routing contracts first

Use small graphs whose expected routes are visually obvious. Cover at least:

- an empty confirmed graph and a graph containing isolated nodes;
- deterministic dense IDs from sparse external IDs;
- compiler rejection of duplicate IDs, missing endpoints, missing relation
  kinds, invalid weights, and representational overflow;
- provisional candidates absent from the confirmed aggregate and image;
- one directed edge that cannot be traversed backward;
- a direct edge versus a lower-distance multi-hop path;
- two context profiles that choose different exact paths;
- disabled relations versus enabled zero-multiplier relations;
- missing, duplicate, empty, and origin-equal destinations;
- parallel edges remaining independently selectable by edge ID;
- self-edges and zero-weight cycles terminating correctly;
- equal-distance alternatives selecting the declared stable predecessor;
- complete-search unreachable results;
- budget exhaustion before any edge, partway through a search, and after some
  destinations have finalized;
- early stopping after all requested destinations finalize without exploring
  an unrelated region;
- distance-only requests allocating no predecessor state;
- path step direction, relation ID, base weight, multiplier, effective weight,
  and exact re-summed distance;
- reopening the catalog and compiling an equivalent routing image;
- a graph mutation followed by a new compilation reflecting promotion, edge
  deletion, and cascading node deletion while the old image remains immutable;
- independent sequential or threaded requests with different origins,
  profiles, and budgets producing isolated results.

Add property-style tests without a new dependency where practical: enumerate
small generated graphs and compare the target-aware engine against a simple
full-search implementation. For every exact returned path, verify endpoints,
edge continuity, relation use, and exact reported distance.

Do not add a GPU-shaped abstraction or mock GPU backend merely to write future
agreement tests. CPU/GPU comparison starts when a real second implementation
exists.

## 10. Document the implemented boundary

Add rustdoc examples showing:

1. confirmed graph construction;
2. routing-image compilation;
3. two profiles selecting different paths;
4. an optional reconstructed path.

Add `docs/routing-image.md` describing the current in-memory arrays, dense-ID
assignment, manifest fields, arithmetic policy, tie policy, profile packing,
budget accounting, and result states. State clearly that this is not a durable
file format and that no active-image publication contract exists yet.

Update the README implementation status only after the slice is implemented.
The README should describe exact CPU routing as implemented while leaving GPU,
publication, hydration, subgraphs, and BAML integration unimplemented. It must
not link to or enumerate planning documents.

## 11. Completion checks

Run:

```powershell
cargo fmt --all --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Plan 03 is complete only when:

- every current exact-identity and graph-mutation test still passes;
- one consistent confirmed read produces a deterministic immutable image;
- no provisional candidate appears in the confirmed aggregate or image;
- compiler failures are typed and cannot return a partially valid image;
- context multipliers can change the exact selected path;
- every present destination is reported as exact, unreachable, or incomplete
  according to proof rather than discovery;
- every requested path is directed, continuous, stable by identity, and exactly
  reproduces its reported logical distance;
- zero-weight cycles and parallel edges preserve correctness and termination;
- budget accounting is deterministic and never converts incomplete into
  unreachable;
- distance-only search retains no predecessor state;
- the numeric, image, request, result, and test contracts are documented;
- no serialized image, active publisher, GPU, hydration, subgraph, BAML, or
  GitHub Actions code is added.

Suggested commit message:

```text
Implement immutable CPU routing
```

## Following slice

Once this plan is complete, Plan 04 can own routing-image publication and query
admission: build a complete replacement image, publish it atomically, prevent
new work from entering a known-stale image after confirmed mutation, allow
requests already holding the prior image to finish, and expose routing health.
That slice may also define serialized image bytes and checksums if measured
startup rebuild cost justifies them. GPU implementation, hydration, and
caller-owned subgraphs should remain separate later slices.
