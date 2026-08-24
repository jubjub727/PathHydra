# Decision 0014: Atomic batch ingestion and relation-kind usage

Status: accepted

PathHydra exposes two bounded in-process batch operations. An insertion stores
one ordered node, relation-kind, edge, or mixed request as provisional
candidates in one RocksDB `WriteBatch`. A confirmation consumes one unique,
dependency-complete candidate selection and creates all confirmed records and
indexes in one `WriteBatch`. Factual validation remains outside Rust.

An insertion entry's zero-based array position is its only request-local
identity. Edge endpoints are either a confirmed `NodeId` or a node-entry
position; an edge relation kind is either a confirmed `RelationId` or a
relation-kind-entry position. Forward and backward references are equally
valid. Durable edge candidates replace local positions with stable
`CandidateId` references. No name-based rebinding or promotion-history mapping
exists.

The operations are all-or-nothing. Empty, oversized, malformed, mistyped,
missing, overflowed, or response-unrepresentable requests write nothing.
Confirmation requires every candidate dependency and every dependent edge of
a selected node/relation-kind candidate. New node IDs, relation IDs, and edge
IDs are allocated independently in selected request order; exact duplicate
node/relation names resolve to the first existing or newly planned stable ID.
Parallel and self edges are never deduplicated.

A graph-changing confirmation performs one durable commit and at most one
complete routing compilation/publication. A confirmation containing only
duplicate node/relation names consumes its candidates without invalidating or
rebuilding the unchanged topology and reports `not_required`. Publication
failure after commit retains the existing routing-unavailable repair contract.

Every confirmed relation-kind record stores two `u64` counts:

- provisional references from durable edge candidates that directly name its
  `RelationId`;
- confirmed canonical directed edges that name its `RelationId`.

Candidate-to-candidate dependency counts live on provisional node and
relation-kind candidates. Confirmation transfers direct provisional relation
uses to confirmed uses atomically. All count changes are serialized by the
catalog mutation lock and committed with the candidate/edge/adjacency changes.
The ordered `relation_popularity` family uses
`!total || !confirmed || RelationId`, all big-endian, with an eight-byte
relation-ID value. This yields total descending, confirmed descending, then ID
ascending. Strict open, verification, checkpoint validation, and restore
recompute candidates, dependencies, canonical edges, both counts, and every
popularity entry.

Direct edge deletion and node-cascade deletion decrement confirmed usage once
per canonical edge; self edges are deduplicated by `EdgeId`. An affected
relation kind is removed atomically with its exact-name and popularity entries
only when both resulting counts are zero. Unrelated zero-use kinds are not
swept. A confirmed node that still has a provisional edge reference cannot be
removed because normal mutation must not manufacture a dangling durable
candidate reference.

The selected facade defaults are 120,000 total entries, 20,000 nodes, 10,000
relation kinds, 100,000 edges, 64 MiB aggregate names, 512 MiB aggregate
decoded payload, 300,000 references, a 1 GiB estimated durable batch, and
100,000 popularity results. The canonical document cap is 256 MiB. These are
finite admission bounds, not allocation targets or performance guarantees.
The correctness-backed distributions that selected them are recorded in
[batch-ingestion evidence](../performance/batch-ingestion.md).

Partial success, streaming transactions, background ingestion, caller keys,
upserts, and historical promotion mappings were rejected. They complicate
atomic dependency resolution and publication without a current use. Singleton
candidate APIs delegate to the same batch primitives.
