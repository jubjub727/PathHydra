# Immutable routing image and CPU routing

`RoutingImage` is the low-level, caller-owned in-memory representation used by compact fixtures and resident CPU/CUDA paths. It is not a serialized Rust struct. Production publication uses one current durable bundle containing exactly `manifest.bin`, `identities.bin`, `source-directory.bin`, `topology.bin`, and `evidence.bin`.

Fields are explicitly little-endian; the manifest has semantic policy IDs but no format marker or version. BLAKE3 covers each complete data file and every independently addressable topology/evidence partition. The loader checks lengths, ranges, counts, identities, source-segment coverage, dense bounds, canonical weights, checksums, and trailing bytes before publication. Concatenating a source's segments preserves exact stable `EdgeId` order, including parallel and self relations.

The low-level `RoutingImage` remains a caller-owned point-in-time value for deterministic tests and backend comparison. `GraphEngine` separately owns and publishes the one current CPU image used by its admitted routes; callers cannot replace that image or mutate its catalog outside publication coordination.

Production compilation holds the catalog mutation mutex across ordered node,
relation-kind, and canonical outgoing passes. It retains only identity tables,
the source directory, and one bounded partition buffer; it does not construct
`ConfirmedGraphRecords` or complete adjacency arrays. Provisional candidates
are excluded by construction. The completed temporary bundle is synchronized,
reopened through the production reader, renamed, referenced by RocksDB, and
published once.

`RoutingImage` itself remains an in-memory test/resident representation with no
serialized compatibility promise. The durable bundle is the only serialized
representation. Confirmed mutation atomically invalidates its pointer and the
engine republishes a complete replacement.

## Partitioned CPU execution

`ChunkedRoutingImage` keeps identities and the source-segment directory
resident. A fixed worker pool serves a bounded read queue; the shared cache has
byte, entry, and staging limits and coalesces concurrent loads. Loading, ready,
failed, pinned, and least-recently-released eviction states are explicit.
Every load checks exact lengths and checksums. A failure poisons that bundle for
new routes and triggers a controlled rebuild rather than producing an
`Unreachable` result.

Partitioned search uses the same CPU queue, arithmetic, tie, budget,
cancellation, and path reconstruction code as resident search. Stable path
evidence is copied at relaxation time, so cache eviction cannot alter a result.
An execution image owns an immutable bundle lease; old requests can finish
after publication, and the reaper deletes an exact retired child only after all
request, cache, I/O, and CUDA references release it.

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
node finalization. Cancellation is cooperatively checked at admission, cache
waits, between segments, and during reconstruction; its precedence relative to
finite budgets is part of the tested result contract.

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
