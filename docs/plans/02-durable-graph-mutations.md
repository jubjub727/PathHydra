# Plan 02: Durable Graph Mutations

## Outcome

Extend the exact identity catalog into the first complete durable graph store.
At completion, PathHydra can persist opaque node payloads and typed, directed,
weighted edges; keep proposed edges provisional; promote them atomically; and
remove confirmed edges or nodes without leaving stale adjacency or lookup
records.

This is the store boundary required by the routing snapshot compiler. Routing
does not begin until the confirmed graph is a structurally valid source of
truth.

## Why this is the next slice

Plan 01 established stable node and relation-kind identities, exact-name
resolution and provisional-to-confirmed promotion.
The nearest missing dependency in `PATHHYDRA_SYSTEM_SHAPE.md` is the confirmed
graph itself:

- there is no canonical edge record or unambiguous edge handle;
- there is no outgoing or incoming adjacency index;
- node records do not yet carry their opaque payload;
- confirmed edge and cascading node deletion cannot be implemented correctly;
- a snapshot compiler therefore has no safe graph representation to compile.

Snapshot compilation, CPU routing, and GPU work remain later slices.

## Explicit non-goals

Do not implement in this slice:

- routing images, dense node IDs, shortest-path search, or path reconstruction;
- request multipliers, effective-weight arithmetic, or a numeric policy for
  accumulated path distance;
- hydration or caller-owned subgraphs;
- GPU code, GPU dependencies, or performance claims;
- BAML code, bindings, a network service, or a wire protocol;
- deletion of a relation kind;
- updates that silently change an existing confirmed node or edge in place;
- factual validation of provisional candidates.

## 1. Restore and freeze the current baseline

Before changing code, make the existing workspace checks pass. On the current
Windows environment, `cargo test --workspace` exits while starting the
`librocksdb-sys` build helper with `STATUS_DLL_NOT_FOUND`. Identify and document
the missing native runtime or toolchain path, then rerun:

```powershell
cargo fmt --all --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Do not treat that environment failure as a graph-storage failure, and do not
replace RocksDB to work around it. Add no GitHub Actions workflow.

## 2. Fix the base-weight contract before encoding edges

The system shape deliberately leaves the stored numeric representation open.
Do not select a Rust primitive merely because it is convenient for the first
codec.

Add a short decision record that fixes only the stored base-weight type and its
durable encoding. Base the decision on representative expected ranges and the
CPU/GPU comparison requirements. Record:

- the valid minimum and maximum;
- that negative, non-finite, and otherwise invalid values are rejected;
- whether negative zero has a canonical representation if floating point is
  selected;
- that zero is allowed, because later routing must support zero-weight edges
  and cycles;
- byte order and canonical byte encoding;
- how exact equality is evaluated for stored records;
- why the representation remains usable by a later CPU reference engine and
  GPU backend.

This slice performs no multiplication or path summation, so multiplier,
accumulator, overflow, disabled-relation, and unreachable-distance policies may
remain open. The decision record must clearly separate those later questions
from the base-weight choice made here.

If representative range evidence is unavailable, stop before fixing the new
durable format and obtain it. A temporary numeric choice must not leak into a
RocksDB record.

## 3. Add precise domain records

Extend `pathhydra-core` with dependency-free types:

- `EdgeId(u64)`: a stable, opaque handle for one confirmed directed edge;
- `NodePayload(Box<[u8]>)`: caller-owned opaque bytes, including an empty
  payload;
- `BaseWeight`: the validated representation selected in step 2;
- `EdgeRecord`: edge ID, source node ID, destination node ID, relation-kind ID,
  and stored base weight.

Extend node candidates and confirmed node records with `NodePayload`. The
existing no-payload insertion call may remain as a convenience that supplies
an empty payload; it must not invent a structured payload schema.

Add an edge candidate containing source node ID, destination node ID,
relation-kind ID, and base weight. Keep the current relation candidate as a
relation-kind proposal; do not confuse it with a directed edge.

Use a standalone `EdgeId` and allow parallel edges, including records with the
same endpoints and relation kind. Every promoted edge candidate receives its
own ID. Never deduplicate edges by endpoints, relation kind, weight, or a hash.
Self-edges are valid.

## 4. Extend the current storage layout

Extend the RocksDB layout with:

- `edges`: edge ID to canonical edge record;
- `outgoing_edges`: `(source node ID, edge ID)` to an edge-ID index value;
- `incoming_edges`: `(destination node ID, edge ID)` to an edge-ID index value;
- `next-edge-id` metadata.

The two adjacency keys use fixed-width big-endian components so every incident
edge for one node is available through a bounded prefix read. The canonical
edge record remains authoritative; adjacency entries are indexes and must
identify one exact edge rather than collapse parallel edges.

PathHydra is unreleased. Update the existing record encodings in place and keep
only the current layout; do not add format markers, migration code, or
compatibility fixtures. Initialize `next-edge-id` to 1 for a new catalog.

Document all current keys, values, and index invariants in
`docs/storage-format.md`.

## 5. Implement provisional edge insertion and atomic promotion

Expose calls equivalent to:

```text
Catalog::insert_edge_candidate(source, destination, relation_kind, base_weight)
Catalog::get_candidate(candidate_id)
Catalog::confirm_validated_candidate(candidate_id)
Catalog::get_edge(edge_id)
```

Insertion validates the base-weight representation and stores only a
provisional candidate. It does not require its endpoints to remain confirmed
forever and does not enter either adjacency index.

Edge confirmation runs under the existing store write mutex. It must:

1. load and decode the candidate;
2. verify both endpoint nodes and the relation kind exist as confirmed records;
3. allocate a new edge ID without wrapping;
4. create the canonical edge and both adjacency entries;
5. remove the provisional candidate;
6. advance the next-edge counter;
7. commit all changes in one RocksDB `WriteBatch`.

Missing endpoints or a missing relation kind produce typed errors and leave the
candidate, counters, and indexes unchanged. This is especially
important when a node was removed after the edge candidate was inserted.

No provisional candidate may appear through `get_edge`, adjacency inspection,
or any future confirmed-graph scan.

## 6. Implement exact confirmed deletion

Expose calls equivalent to:

```text
Catalog::remove_edge(edge_id)
Catalog::remove_node(node_id)
```

Removing an edge loads its canonical record and, in one batch, deletes the
canonical record, its outgoing index entry, and its incoming index entry. A
missing edge returns a typed not-found error.

Removing a node runs under the same write mutex and performs bounded prefix
reads of both incident indexes. Deduplicate self-edges by `EdgeId`, verify every
index entry against its canonical record, and build one atomic batch that:

- deletes every incoming and outgoing canonical edge;
- deletes both adjacency entries for every removed edge;
- deletes the confirmed node record;
- deletes its exact-name mapping.

If any canonical/index relationship is missing or inconsistent, fail visibly
before committing. Do not partially clean up a corrupt cascade. Provisional
edge candidates that mention the removed node remain provisional, but later
confirmation must reject them because the endpoint no longer exists.

Do not add relation-kind deletion in this slice. Its policy for confirmed edges
using that kind must be designed explicitly before such an API exists.

## 7. Validate the complete confirmed graph on open

Extend startup validation so the catalog is published only after all durable
invariants hold:

- every canonical edge has confirmed source and destination nodes;
- every canonical edge names a confirmed relation kind;
- every base weight satisfies the selected stored-weight contract;
- every canonical edge has exactly one matching outgoing entry and one matching
  incoming entry;
- every adjacency entry resolves to a canonical edge with the indexed endpoint;
- parallel and self-edges remain distinct by edge ID;
- every confirmed node has one payload, including an explicitly empty payload;
- provisional candidates decode correctly but are never required to have
  currently confirmed endpoints.

Return typed corruption errors rather than repairing, dropping, or merging
records during open.

## 8. Test the mutation contracts first

Add integration fixtures covering:

- node payload bytes, including empty, non-UTF-8, and maximum accepted length;
- exact payload preservation across confirmation and restart;
- directed edges that do not create reverse adjacency;
- parallel edges and identical-looking duplicates receiving distinct IDs;
- self-edges appearing once in a node-deletion cascade;
- zero and numeric-boundary base weights;
- rejection of invalid base weights before any write;
- failed promotion for a missing source, destination, or relation kind;
- failed promotion leaving the candidate and counters unchanged;
- successful promotion atomically creating all three edge representations;
- edge deletion removing the canonical, outgoing, and incoming records;
- node deletion removing all incoming and outgoing edges plus the exact-name
  lookup while preserving unrelated graph material;
- two concurrent promotions and deletion/promotion races serialized without
  dangling indexes;
- restart detection of missing, duplicate, malformed, or mismatched adjacency;

Use small fixtures whose incident edges are obvious, plus one high-degree node
fixture to exercise the cascade without a graph-wide scan. Where practical,
inspect the raw RocksDB column families after closing `Catalog` so tests prove
durable state rather than only public-method behaviour.

## 9. Keep the next compiler boundary narrow

Keep canonical record decoding and bounded adjacency reads inside the store.
Do not expose raw RocksDB handles or make routing code depend on column-family
details, and do not design the final public transport API in this slice.

## 10. Documentation and completion checks

Update the README implementation status only after the slice is implemented;
the README must not link to or enumerate planning documents. Extend rustdoc
examples to show edge promotion and cascading node deletion. Keep relation
direction, relation kind, and edge identity explicit in every public name and
error.

Run the four authoritative checks. Plan 02 is complete only when:

- all existing exact-identity behaviour still passes;
- no provisional edge affects confirmed reads or indexes;
- every successful edge promotion is one durable atomic batch;
- edge and node deletion leave no canonical or adjacency representation behind;
- open rejects every tested structural inconsistency;
- public behaviour and the current storage layout are documented;
- no routing, GPU, hydration, subgraph, BAML, or GitHub Actions code is added.

Suggested commit message:

```text
Implement durable graph mutations
```

## Following slice

Once this plan is complete, Plan 03 can design the CPU-consumable routing data
needed by the reference engine without mixing those concerns into mutation
correctness.
