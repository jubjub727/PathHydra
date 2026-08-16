# Immutable routing image and CPU routing

The low-level `RoutingImage` remains a caller-owned point-in-time value for deterministic tests and backend comparison. `GraphEngine` separately owns and publishes the one current CPU image used by its admitted routes; callers cannot replace that image or mutate its catalog outside publication coordination.

PathHydra compiles a self-contained read of the confirmed durable graph into
an immutable, in-memory routing image. The catalog holds its mutation mutex
across the complete node, relation-kind, and canonical-edge scan. The returned
records are sorted by stable numeric ID and contain no provisional candidates.
Once copied, that aggregate and every image compiled from it remain unchanged
when the catalog is mutated.

This image is not a durable file format. Its arrays have no serialized byte
order, schema marker, checksum, migration path, or compatibility promise.
There is also no active-image publisher: callers explicitly own or borrow an
image, and a catalog mutation does not rebuild or invalidate it.

## Topology

The image is a compressed sparse row (CSR) representation with private boxed
slices:

| Array or mapping | Contents |
| --- | --- |
| external-to-dense | sorted `(NodeId, DenseNodeId)` entries |
| dense-to-external | one `NodeId` for each dense node |
| offsets | one `u64` start offset per node plus a final sentinel |
| destinations | `u32` destination dense node per adjacency entry |
| relation IDs | stable relation-kind ID per adjacency entry |
| relation indexes | `u32` dense profile index per adjacency entry |
| base-weight bits | canonical binary32 bits per adjacency entry |
| edge IDs | stable edge ID per adjacency entry |
| confirmed relation IDs | sorted relation kinds used to pack profiles |

Dense node IDs are `u32` values assigned in ascending external `NodeId` order.
Compilation rejects a node count that cannot be represented, duplicate stable
IDs, dangling endpoints, unconfirmed relation kinds, noncanonical weights, and
any checked count or conversion overflow. Each source range is sorted by
ascending `EdgeId`. Parallel edges and self-edges remain separate. Names,
payloads, relation labels, and incoming adjacency are not copied.

`RoutingImageArrays` exposes only read-only fixed-width topology slices plus
host-only node/relation identity mappings. The dense relation index is
rebuildable routing state, never relation identity. CPU path evidence continues
to use stable relation and edge IDs.

The manifest identifies the numeric and tie policies, node, relation-kind, and
adjacency counts, element widths, exact allocated bytes represented by every
boxed topology/mapping slice, and the presence of predecessor-capable edge
IDs. It deliberately has no checksum because there are no canonical serialized
bytes.

The GPU topology manifest separately counts only offsets plus destination,
relation-index, and base-weight arrays. Stable IDs, edge IDs, payloads, search
state, packed profiles, queues, counters, destinations, and allocator headroom
are not included. CUDA residency and search accounting report those categories
separately.

## Profiles and arithmetic

A relation profile supplies exactly one explicit entry for every confirmed
relation kind. An entry is disabled or enabled with a validated canonical
binary32 multiplier. Packing rejects unknown, duplicate, and missing entries
and creates an immutable lookup in the image's sorted relation-ID order. Zero
is an enabled multiplier; it is not the disabled state.

The CPU engine converts the stored binary32 base weight and request multiplier
separately to binary64, multiplies them, and then adds the effective weight to
the current binary64 distance. Multiplication and addition are separate,
checked-finite operations with exact comparisons. The full contract is in
[Decision 0002](decisions/0002-cpu-routing-arithmetic.md).

## Deterministic search

One distance-ordered binary heap serves all unique present destinations in a
request. Frontier ties use dense node ID. Equal tentative distances use the
stable predecessor tuple described in Decision 0002, and finalized nodes are
never reopened. Disabled edges are unavailable; zero-effective-weight edges,
self-edges, parallel edges, and cycles follow the ordinary relaxation path.

The explicit search budget is either unlimited or a maximum examined-edge
count. An edge is counted immediately before checking its relation state, so a
disabled edge consumes budget. When no budget remains, search stops before the
next edge. Depth and fan-out are explicitly unlimited; cycles are bounded by
node finalization. Deadlines and cancellation are not part of this reference
engine because they would introduce nondeterministic stopping points.

Search ends when all unique present destinations finalize, the frontier is
exhausted, or the budget stops edge examination. Search state is local to one
invocation. A distance-only request does not allocate predecessor storage.

## Results and paths

The response contains one result in each original destination position, so
duplicate destinations share work without losing order. States are:

- `Exact`: the destination finalized with a finite logical distance;
- `Unreachable`: the complete reachable region was exhausted;
- `MissingNode`: the destination is absent from the image; or
- `Incomplete`: the edge budget stopped search before proof.

A missing origin is a whole-request error. An origin requested as a destination
has exact distance zero. Empty destination lists are valid after origin and
profile validation. Already finalized destinations remain exact after budget
exhaustion; unresolved present destinations are incomplete, never unreachable.

When requested, each exact result includes an independently reconstructed
directed path. Every step carries the edge ID, source and destination node IDs,
relation ID, stored base weight, request multiplier, and effective weight.
Reconstruction checks predecessor cycles, chain length, endpoint continuity,
and edge identity, then re-sums the ordered steps and requires exact equality
with the reported distance. It does not hydrate records or compose a subgraph.
