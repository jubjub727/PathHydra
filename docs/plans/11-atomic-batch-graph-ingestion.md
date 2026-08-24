# Plan 11: Atomic Batch Graph Ingestion and Relation-Usage Maintenance

## Outcome

Add bounded batch mutation support to the in-process `pathhydra-api` facade so
one request can contain many node candidates, many relation-kind candidates,
many directed-relation (edge) candidates, or a dependency-linked mix of all
three. Preserve PathHydra's externally validated candidate lifecycle:

1. one atomic call stores the complete mixed batch as provisional candidates;
2. after external validation, one atomic call confirms the selected batch;
3. one confirmed batch produces one durable graph commit and at most one
   routing rebuild/publication, never one rebuild per record.

The mixed form must allow an edge candidate to refer to node and relation-kind
candidates in the same insertion request, including entries that appear later
in that request. Homogeneous node-only, relation-kind-only, and edge-only
batches use the same implementation rather than separate partial APIs.

This plan is complete batch ingestion, not a loop hidden behind a batch-shaped
facade. Atomicity, dependency resolution, exact-name behavior, bounded memory,
crash recovery, publication semantics, canonical encoding, diagnostics, and
scale evidence are all part of the feature.

The same mutations must maintain two durable usage counts for every relation
kind: provisional edge references and confirmed directed edges. Existing edge
and cascading node-deletion calls automatically remove an affected confirmed
relation kind only when both counts are zero after the atomic deletion commit.
A bounded public query returns the most-used confirmed relation kinds first and
reports both counts.

## Authority and prerequisite

Read and preserve:

- repository `AGENTS.md`;
- `PATHHYDRA_SYSTEM_SHAPE.md`;
- Decisions 0001 through 0013;
- `docs/storage-format.md`, `docs/cpu-engine.md`, `docs/consumer-api.md`, and
  `docs/system-conformance.md`;
- the current catalog, engine publication, API DTO/codec, recovery, and
  generated-conformance implementations.

Plans 00-10 and the current ordinary/CUDA verification matrix must pass before
implementation. Batch support extends the current pre-release model in place;
do not add schema versions, migrations, compatibility readers, graph revision
counters, or a second publication mechanism.

The original invariants remain authoritative:

- candidate material is invisible to confirmed lookup, routing, and hydration;
- factual validation remains external to Rust;
- exact names remain byte-for-byte, case-sensitive identities;
- confirmed graph mutation is atomic and serialized;
- every request uses one complete immutable routing image;
- duplicate-looking parallel edges remain independent;
- confirmed relation-kind usage is derived from confirmed canonical edges;
  provisional usage is derived from durable provisional edge candidates; both
  stay coherent with candidate, edge, and adjacency mutation;
- CPU correctness remains available without CUDA.

## Current gap

The current facade inserts and confirms one candidate at a time. Candidate
insertion is provisional and does not publish routing, but each
`confirm_candidate` call executes a separate confirmed mutation and routing
rebuild/publication. RocksDB `WriteBatch` currently makes one logical mutation
atomic; it is not caller-visible bulk ingestion.

The current edge-candidate record also refers only to already confirmed
`NodeId` and `RelationId` values. It cannot express a mixed provisional batch
whose edges depend on node or relation-kind candidates in that same request.

Relation-kind records currently have neither a durable provisional-reference
count nor a confirmed-edge count or ordered popularity index. Candidate
insertion/confirmation, edge deletion, and cascading node deletion therefore
cannot cheaply determine that an affected relation kind has truly become
unused, and the facade cannot retrieve the most-used relation kinds without a
complete catalog scan.

## Explicit non-goals

- bypassing external candidate validation or automatically confirming inserted
  material;
- partial success, best-effort import, or per-entry durable errors within an
  otherwise accepted batch;
- unbounded requests, streaming transactions, or a transaction kept open
  across API calls;
- updates/upserts to existing confirmed payloads, names, weights, or edges;
- batch deletion, arbitrary transaction scripting, or a generic command list;
- treating provisional edge candidates, routing-profile entries, historical
  usage, or query frequency as confirmed relation-kind usage;
- a background/global garbage-collection sweep of unrelated zero-use relation
  kinds;
- fuzzy, normalized, aliased, or case-insensitive name resolution;
- a remote service, HTTP endpoint, BAML workflow, or hosted ingestion queue;
- changing routing, numeric, tie, CUDA, hydration, or subgraph semantics;
- retaining obsolete pre-release candidate encodings.

## 1. Fix the public batch semantics

Add two finite facade operations:

```text
insert_candidate_batch(request) -> provisional batch result
confirm_candidate_batch(request) -> confirmed batch result + one publication outcome
```

`insert_candidate_batch` is all-or-nothing. If it succeeds, every entry has a
durable `CandidateId`; if it fails, no candidate or counter change is visible.
It never changes confirmed lookup indexes or the active routing pointer.

`confirm_candidate_batch` means the caller has externally validated every
listed candidate. It is also all-or-nothing for authoritative graph state. If
any candidate, dependency, endpoint, relation kind, weight, name mapping,
counter, resource limit, or durable record is invalid, no candidate is consumed
and no confirmed record, index, adjacency entry, counter, or routing pointer is
changed.

After a successful confirmed durable commit, routing compilation/publication
retains the current `ConfirmedMutation` contract. A later routing build failure
does not roll back authoritative graph state: return every durable per-entry
result plus one typed `RoutingUnavailable` publication outcome. Never attach a
different publication outcome to each entry.

Empty batches are rejected as invalid input. Request order is meaningful for
deterministic candidate/stable-ID allocation and response order, not graph
semantics. Duplicate candidate IDs in a confirmation request are rejected
before mutation.

## 2. Define mixed request-local references

Represent a batch insertion as one ordered entry array. Its zero-based entry
index is its request-local identity; no arbitrary string key or map-order
semantics are needed.

Entry variants are:

```text
NodeCandidate {
  exact_name,
  payload
}

RelationKindCandidate {
  exact_name
}

EdgeCandidate {
  source: ConfirmedNode(NodeId) | BatchNode(entry_index),
  destination: ConfirmedNode(NodeId) | BatchNode(entry_index),
  relation_kind: ConfirmedRelationKind(RelationId) |
                 BatchRelationKind(entry_index),
  base_weight
}
```

A local node reference must identify a node entry in the same request; a local
relation-kind reference must identify a relation-kind entry. Forward and
backward references are both valid. An edge entry cannot be used as an endpoint
or relation kind. Self-edges and multiple parallel edge entries are valid.

Resolve and type-check the complete reference graph before assigning durable
state. A malformed index, wrong entry kind, missing confirmed identity, invalid
weight, invalid name/payload, or arithmetic overflow rejects the whole request.

The insertion response returns one `CandidateId` aligned with each input entry
and aggregate node/relation-kind/edge counts. It must not expose RocksDB keys or
internal candidate-reference representations.

## 3. Extend provisional edge identity without weakening it

Add explicit core candidate references:

```text
CandidateNodeReference = Confirmed(NodeId) | Candidate(CandidateId)
CandidateRelationReference = Confirmed(RelationId) | Candidate(CandidateId)
```

An edge candidate stores these stable references, not endpoint names. This
prevents an edge candidate from silently attaching to a later node that happens
to reuse an exact name after deletion.

A provisional edge that directly references a confirmed `RelationId`
increments that relation kind's provisional-reference count. A provisional
edge that references a relation-kind `CandidateId` increments a durable
incoming-reference count owned by that relation-kind candidate. This candidate
count prevents consuming/promoting the relation-kind candidate separately from
the dependent edge candidates. A dependency-complete confirmation transfers
the selected provisional references to confirmed usage atomically; it never
passes through an observable zero-reference state.

Update the one current candidate record encoding in place and update all
fixtures/docs. Do not retain a legacy decoder. Existing single-edge insertion
continues accepting confirmed IDs and creates `Confirmed` references through
the shared batch primitive.

`get_candidate` and its canonical DTO expose whether each dependency is a
confirmed stable ID or a candidate ID. A candidate reference is provisional
identity only; it must never resolve through confirmed lookup, routing, or
hydration.

## 4. Implement atomic provisional batch insertion in the catalog

Add one catalog primitive that:

1. acquires the catalog mutation lock once;
2. validates the complete batch and all aggregate limits;
3. reads the next candidate ID once;
4. checks the complete contiguous ID range for overflow;
5. translates request-local references to the allocated candidate IDs;
6. encodes every candidate using fallible, pre-reserved buffers;
7. aggregates provisional edge references per confirmed relation kind and per
   relation-kind candidate;
8. plans the old/new provisional counts and popularity-index replacements under
   catalog mutation ownership, rejecting overflow before writing;
9. writes every candidate, usage update, popularity replacement, and the final
   next-ID value in one RocksDB `WriteBatch`;
10. records one batch write attempt/success/failure plus entry and byte counts;
11. returns results in request order.

No write may occur during validation or encoding. A RocksDB failure leaves the
old candidate-ID counter, both usage-count domains, popularity indexes, and all
old records intact. Batch insertion must not clear the active routing image
because all inserted graph material remains provisional; changing provisional
popularity metadata is not a routing-topology change.

Refactor the existing single node/relation-kind/edge insertion methods to call
the same primitive with one entry. Do not maintain two implementations of
candidate encoding, counter allocation, or write semantics.

## 5. Plan a complete confirmation before writing

Add a catalog preflight/planning phase for a unique ordered set of candidate
IDs. It must load every selected candidate and resolve all dependencies against
either:

- confirmed records present at the start of the mutation; or
- correctly typed node/relation-kind candidates included in the same
  confirmation batch.

An edge whose candidate dependency is not included in that confirmation batch
is rejected, even if the dependency candidate still exists. This prevents a
partly promoted batch and avoids persistent promotion-history mappings. A
previously confirmed stable reference remains valid; a consumed/deleted
candidate reference is a typed invalid dependency.

Conversely, a selected node or relation-kind candidate cannot be consumed while
an unselected provisional edge still refers to it. Validate this through
durable reverse-dependency evidence/counts rather than a racy best-effort scan.
The caller must include the complete dependent closure or leave the dependency
candidate provisional.

Preflight simulates exact-name resolution for the whole batch. Multiple node or
relation-kind candidates with the same exact name consume independently but
resolve to the same existing or newly allocated stable record, matching current
single-confirmation semantics. Case, punctuation, whitespace, and Unicode
sequence differences remain distinct. Edge stable IDs are never deduplicated.

Stable IDs are allocated deterministically:

- new nodes in their original request order;
- new relation kinds in their original request order;
- edges in their original request order.

Check the final node, relation-kind, and edge counters for overflow before any
write. Validate canonical records, both adjacency keys, exact-name mappings,
relation-kind provisional-to-confirmed transfers, confirmed increments,
popularity-index replacements, and the final active-routing-pointer invalidation
as part of the plan. Aggregate all selected edges by relation kind and reject
the complete batch if either durable usage count or their checked total would
overflow `u64`.

## 6. Commit a confirmed batch atomically

Turn the complete preflight plan into one RocksDB `WriteBatch` containing:

- all new confirmed node records and exact-name entries;
- all new relation-kind records and exact-name entries;
- every independent canonical edge;
- outgoing and incoming adjacency entries for every edge;
- one aggregated provisional decrement/confirmed increment and popularity-index
  replacement for each confirmed relation kind used by selected edges;
- transfer of relation-kind-candidate incoming provisional references when that
  dependency is promoted with its complete selected edge closure;
- deletion of every selected candidate;
- final next-node, next-relation-kind, and next-edge counters;
- one deletion of the active routing pointer when the confirmed graph changes.

Acquire the node/relation exact-name write locks in one documented order.
Fallibly reserve their required capacity before the durable commit, hold them
through commit, and perform only infallible cache updates afterward. A process
crash after the durable commit is recovered by rebuilding the confirmed maps
and routing image from RocksDB exactly as today.

If every selected node/relation-kind candidate resolves to an already confirmed
exact name and the batch contains no edges, consume those candidates atomically
without invalidating or rebuilding the unchanged routing graph. Report an
explicit `graph_changed` value in diagnostics so this optimization is
observable and tested.

Refactor single-candidate confirmation through the batch confirmation primitive
where doing so preserves its current public return shape. There must be one
implementation of promotion rules and exact-name behavior.

## 7. Maintain provisional and confirmed relation-kind usage

Maintain two independent `u64` counts for every confirmed relation kind:

```text
provisional_reference_count
confirmed_edge_count
```

`provisional_reference_count` is the number of durable provisional edge
candidates that directly reference that confirmed `RelationId`.
`confirmed_edge_count` is the number of confirmed canonical directed edges
whose `relation_kind` equals it. For relation-kind candidates, retain a durable
incoming provisional-reference count so their dependent edge candidates cannot
be orphaned by an incomplete confirmation.

Counting rules are exact:

- every provisional edge candidate counts once in the provisional domain;
- every confirmed parallel edge counts once in the confirmed domain;
- a provisional or confirmed self-edge counts once;
- edge direction does not change either count;
- confirming an edge transfers one use from provisional to confirmed in the
  same atomic commit;
- an edge that depends on a relation-kind candidate transfers that candidate's
  incoming provisional use directly to the resolved new/existing confirmed
  kind's confirmed count when the dependency-complete batch is promoted;
- routing-profile presence, disabled/enabled state, route frequency, and
  historical/deleted edges count in neither domain.

Store both confirmed-kind counts durably as part of the one current
relation-kind record shape or an equally direct current key. Store the incoming
count for a relation-kind candidate in its current candidate representation.
Update these formats in place and do not retain a compatibility decoder.

Add one ordered durable popularity index for confirmed relation kinds. Define
popularity as the checked total:

```text
total_reference_count = provisional_reference_count + confirmed_edge_count
```

A bounded forward range read yields:

```text
total_reference_count descending,
confirmed_edge_count descending,
RelationId ascending
```

The secondary confirmed-count order makes a tie prefer proven confirmed use;
the stable ID makes the remaining tie deterministic. For example, an index key
may contain the big-endian complements of total and confirmed counts followed
by `RelationId`. The complete encoded key/value form must be fixed,
length-checked, documented, and verified against candidate and canonical edge
records. Add only the current key space needed for this bounded query.

### Counter locking and atomic read-modify-write

Treat the existing catalog mutation lock as the exclusive relation-usage
counter lock. Every provisional increment/decrement, confirmed
increment/decrement, transfer, candidate incoming-count change, and popularity
replacement must:

1. acquire `Catalog::write_guard()` before reading any old count;
2. keep that guard for the complete preflight, checked arithmetic, RocksDB
   `WriteBatch` construction, durable commit, and required in-memory index
   update;
3. delete the old popularity key and insert the new key in the same RocksDB
   commit as the candidate/edge mutation;
4. release the guard only after the mutation is fully coherent.

No path may read a count, release ownership, and later write a derived value.
Do not use independent relaxed atomics or one mutex per relation kind: they
would not coordinate the counter with candidates, canonical edges, adjacency,
name lookup, and the popularity index. If an in-memory popularity cache is
actually justified, protect it with an `RwLock`, document one global lock
order, reserve capacity before durable commit, and keep RocksDB as the verified
current representation.

The top-K reader uses a RocksDB snapshot (and a shared cache lock if such a
cache exists) so it observes one complete pre-commit or post-commit index.
Concurrent mutation threads serialize at the catalog mutation lock; therefore
two simultaneous increments/decrements cannot lose an update even when they
target the same relation kind.

### Centralize every usage-changing mutation path

Do not rely on every caller remembering to edit counters beside an arbitrary
`WriteBatch`. Introduce one private catalog mutation planner, for example
`CatalogMutation`, created only while `Catalog::write_guard()` is held. It owns
the raw RocksDB `WriteBatch`, staged record/index changes, per-relation
provisional and confirmed deltas, relation-kind-candidate incoming deltas, and
the final automatically deleted-kind set.

Its typed staging operations must include at least:

```text
stage_insert_candidate
stage_consume_candidate
stage_insert_confirmed_edge
stage_remove_confirmed_edge
stage_remove_node_with_incident_edges
finalize_usage_and_commit
```

The usage delta is a consequence of the typed operation, not a second argument
the caller may omit. For example, `stage_remove_confirmed_edge` accepts or loads
the complete `EdgeRecord`, stages canonical/outgoing/incoming deletion, and
stages exactly one confirmed decrement itself. Node cascade must call that same
edge-removal operation for each deduplicated incident edge rather than deleting
edge keys directly. Candidate consumption inspects the candidate variant and
stages the provisional decrement/transfer automatically.

Insertion is equally inseparable from usage maintenance:

- staging a new confirmed relation kind initializes both counts to zero and
  creates its zero-use popularity entry;
- staging a provisional edge candidate with a confirmed `RelationId` always
  stages one provisional increment for that kind;
- staging a provisional edge candidate with a relation-kind `CandidateId`
  always stages one incoming provisional increment on that candidate;
- staging a confirmed edge always stages one confirmed increment for its kind;
- confirming an existing provisional edge uses a single transfer operation so
  it cannot decrement provisional usage without incrementing confirmed usage,
  or increment confirmed usage twice;
- homogeneous, mixed, singleton, batch, test-fixture, benchmark, and admin
  insertion all use these same staging operations.

There must be no separate "insert edge record" helper that writes the canonical
edge/adjacency keys without the confirmed increment, and no separate "insert
edge candidate" helper that writes the candidate without the provisional
increment. If a future import/repair tool constructs confirmed records, it must
either use this planner or build a fresh database and pass complete usage/index
verification before that database can be opened as authoritative state.

Make the underlying `WriteBatch` and relevant column-family mutation helpers
private to this planner. Normal catalog mutation code must not be able to
directly put/delete candidate-edge, canonical-edge, relation-kind usage,
popularity, outgoing-adjacency, or incoming-adjacency records. The planner's
commit step aggregates deltas, reads old values under the held guard, validates
underflow/overflow/dependencies, applies popularity replacements and
both-counts-zero cleanup, and commits once. It returns the complete usage
effects, including automatically removed relation kinds, for the engine/API
outcome.

Audit and cover the complete current transition inventory:

1. singleton or batch relation-kind confirmation initializes two zero counts and
   the popularity entry when it creates a new confirmed kind;
2. singleton or batch provisional edge insertion increments provisional usage
   on either the confirmed kind or relation-kind candidate it references;
3. singleton or batch edge confirmation consumes the candidate and atomically
   transfers provisional usage to confirmed usage;
4. confirmation/consumption of a relation-kind candidate refuses when
   unselected dependent edge candidates would be orphaned;
5. direct confirmed-edge deletion decrements confirmed usage;
6. confirmed-node deletion deduplicates all incoming/outgoing incident edges
   and routes every one through the same confirmed-edge removal staging path;
7. automatic relation-kind deletion is performed only by finalization after
   both resulting counts are zero;
8. any future candidate rejection/removal uses the candidate-consumption path
   and decrements provisional usage;
9. catalog initialization/restore never applies deltas to an existing graph;
   it accepts a complete database only after recomputing and validating both
   count domains and the popularity index.

`pathhydra-admin` must continue calling catalog operations rather than writing
records directly. Caller-owned subgraph node/edge removal, routing-bundle
retirement, cache eviction, checkpoint file cleanup, and failed scratch restore
do not mutate canonical/candidate graph records and therefore must not alter
usage. Document these exclusions so similarly named deletion operations are not
mistaken for graph deletion paths.

Add a local structural audit test or check that enumerates all production
functions permitted to mutate the candidates, canonical edges, adjacency,
relation-kind usage, and popularity key spaces. It must fail when a new direct
put/delete site appears outside the centralized planner. The check supplements,
not replaces, behavioral counter-oracle tests.

### Candidate insertion and confirmation

Provisional edge insertion aggregates references per relation kind and changes
the provisional counts/popularity keys in the same `WriteBatch` as candidate
creation. Relation-kind-candidate dependencies update that candidate's incoming
count in the same batch.

Batch confirmation aggregates all selected edges by resolved relation kind.
For direct confirmed references it decrements provisional and increments
confirmed counts by equal amounts. For selected relation-kind-candidate
dependencies it consumes the candidate incoming count and adds the selected
edges to the resolved confirmed count. Any underflow, overflow, mismatched
incoming count, missing dependent closure, or checked-total overflow rejects
the complete confirmation before writing. Singleton insertion/confirmation
delegates to the same logic.

### Deletion and automatic relation-kind cleanup

Extend the existing confirmed deletion calls:

- `remove_confirmed_edge`/engine `remove_edge` decrements only the confirmed
  count for the edge's kind;
- `remove_confirmed_node`/engine `remove_node` aggregates all incident confirmed
  edges by kind, counting a self-edge only once, then applies one confirmed
  decrement per kind;
- a kind whose confirmed count reaches zero is retained while its provisional
  count is nonzero;
- an affected kind is automatically deleted only when both its new confirmed
  count and its provisional count are zero;
- automatic kind deletion removes its confirmed record, exact-name lookup,
  popularity-index entry, and every other durable lookup representation in the
  same deletion `WriteBatch`;
- kinds with either remaining confirmed or provisional references receive their
  new counts and replacement popularity key;
- an old count smaller than the aggregated decrement is typed catalog
  corruption and leaves the complete deletion uncommitted;
- a confirmed zero-use kind created independently is not swept by an unrelated
  deletion. Automatic deletion is transition-triggered for kinds affected by
  the current edge/node deletion call.

If candidate rejection/removal is added now or already exists by implementation
time, it must decrement provisional usage atomically and apply the same
both-counts-zero cleanup rule. Do not add candidate rejection merely to satisfy
this plan, but do not leave a future candidate-removal path outside the counter
invariant.

Return the automatically deleted `RelationId` values in deterministic ascending
order as part of the deletion outcome. Do not silently hide that a node or edge
deletion also removed relation kinds. One deletion still produces one durable
commit and at most one routing rebuild/publication.

### Bounded popularity query and verification

Add a bounded facade operation such as:

```text
most_used_relation_kinds(max_results) -> ordered relation-kind usage records
```

Each result contains the complete confirmed relation-kind record, provisional
count, confirmed count, and checked total. Reject zero or over-limit requests.
The call returns a current, internally consistent top-K snapshot; it is not a
historical statistic or pinned pagination API. Include kinds whose two counts
are zero only after every used kind and only when the requested limit reaches
them.

Strict catalog open, `verify_catalog`, checkpoint validation, and restore must
independently recompute provisional usage from edge candidates, relation-kind-
candidate incoming usage from candidate dependencies, and confirmed usage from
canonical edges. Require exact equality with every stored count and popularity
key. Missing, extra, duplicate, stale-count, wrong-order-key, overflowed, and
orphaned entries are corruption, not metrics unavailability. Routing
compilation may consume canonical relation kinds and edges as before; neither
usage count is routing authority.

## 8. Publish routing exactly once per changed batch

Add an engine-level `confirmed_batch_mutation` path that owns one lifecycle
mutation permit, one routing publication write boundary, and one retirement
capacity decision. It invokes the catalog batch commit once and compiles the
complete confirmed graph at most once.

Requirements:

- provisional batch insertion performs no compilation/publication;
- a confirmed batch that changes graph topology performs one complete
  compilation and one publication attempt;
- an unchanged duplicate-name-only confirmation performs no rebuild;
- requests admitted before publication may finish on the old immutable image;
- requests admitted after publication use the complete new image;
- no request can observe a subset of the batch;
- resident/partitioned CPU and CUDA residency selection remains unchanged;
- routing failure after durable success has the existing typed repair path;
- retirement, cancellation, shutdown, device-loss, and recovery ownership stay
  with the existing engine mechanisms.

Expose batch diagnostics containing input counts, newly created versus
existing-name record counts, candidates consumed, graph-changed status, durable
commit bytes/duration, and the single publication outcome. Do not report
unavailable measurements as zero.

## 9. Add bounded canonical DTOs and facade limits

Add owned canonical DTOs for:

- mixed insertion entries and local references;
- batch insertion request/result;
- batch confirmation request/result;
- aligned per-entry provisional and confirmed outcomes;
- aggregate batch diagnostics;
- relation-kind usage records and deletion outcomes containing automatically
  removed relation-kind IDs.

Extend `ApiLimits`/`ApiLimitsDto` with explicit nonzero limits for at least:

- entries per batch;
- node entries, relation-kind entries, and edge entries per batch;
- aggregate exact-name bytes;
- aggregate decoded payload bytes;
- aggregate candidate-reference count;
- estimated request, response, and RocksDB batch bytes;
- results returned by the most-used-relation-kinds query.

Validate byte/depth/value limits in the existing canonical JSON pre-scan, then
validate semantic aggregate limits before engine mutation. Use checked
arithmetic and fallible reservation. Do not allocate the configured maximum in
advance. Ensure an oversized response is refused before committing a batch
whose required success response cannot be represented under the same limits.

All batch DTOs must reject unknown/duplicate fields, noncanonical IDs/float
bits, invalid local indices, wrong reference variants, excessive nesting,
trailing bytes, and malformed UTF-8. Error values may identify an entry index
and stable error code but must not echo exact names, payloads, filesystem paths,
or internal database reasons.

## 10. Keep singleton and batch behavior coherent

Existing singleton facade calls remain the convenient one-entry surface:

- `insert_node_candidate`;
- `insert_relation_kind_candidate`;
- `insert_edge_candidate`;
- `confirm_candidate`.

They must share catalog/engine primitives with the new batch operations and
retain their current return types. Document the only intentional distinction:
single edge insertion accepts confirmed endpoint/relation-kind IDs, while a
mixed batch may additionally use request-local candidate references.

For a one-entry request, singleton and batch behavior must agree on IDs,
encoded records, exact-name resolution, errors, graph change, and publication.
Add equivalence tests rather than assuming delegation proves the public DTO
surface.

## 11. Define concurrency, shutdown, and failure behavior

One batch is one mutation for lifecycle admission and serialization. Concurrent
batches may execute in either lock-acquisition order, but each batch remains
internally ordered and atomic. Reads continue under the existing catalog/image
contracts.

Cover these boundaries explicitly:

- shutdown before admission rejects the whole batch;
- shutdown after durable confirmation waits for the one publication attempt or
  reports through the current shutdown contract;
- a competing singleton mutation cannot interleave inside a batch;
- concurrent duplicate-name batches create at most one stable node/relation
  identity and consume each successful candidate exactly once;
- concurrent mixed batches never create an edge with a missing endpoint or
  relation kind;
- concurrent candidate insertion/confirmation and confirmed deletion cannot
  lose a provisional or confirmed increment/decrement/transfer, retain an
  affected kind after both counts reach zero, or delete a kind that still has
  either kind of reference;
- lock poisoning, allocation refusal, counter overflow, storage-full, and
  RocksDB errors are typed whole-batch failures;
- publication/build/device failure cannot roll back or partially expose the
  authoritative durable batch.

Do not add an open transaction handle, background ingestion queue, or detached
publication worker.

## 12. Extend crash, corruption, checkpoint, and restore evidence

Extend the existing subprocess publication fault campaign with a deterministic
mixed confirmed batch and termination points:

- before the batch write;
- immediately after the atomic durable commit;
- during routing-bundle construction;
- before and after routing-pointer publication;
- while the old image is still leased.

On reopen, authoritative state must be entirely before-batch or entirely
after-batch—never partial. If durable state is after-batch but routing
publication was interrupted, startup rebuilds the complete batch.

Add malformed provisional-reference tests for wrong candidate type, missing or
consumed dependencies, truncated reference encoding, dangling confirmed IDs,
and edge/reference corruption. Strict open/verification must reject corruption
without exposing partial confirmed data.

Checkpoint and restore tests must preserve an unconfirmed mixed batch with
candidate-to-candidate references, then allow its complete confirmation after
restore. Also checkpoint after confirmation and prove exact confirmed counts,
names, payloads, parallel/self edges, adjacency indexes, routing results, and
candidate consumption survive restore.

Add deletion crash boundaries around the atomic edge/node cascade that updates
usage and removes newly unused kinds. Reopen must observe either the complete
pre-deletion graph/count/index state or the complete post-deletion state; it may
never observe removed edges with old counts, retained edges with decremented
counts, provisional candidates missing their count, or a deleted kind that
still has a provisional or confirmed reference.

## 13. Add comprehensive correctness tests

At minimum cover:

- node-only, relation-kind-only, edge-only, and fully mixed batches;
- one entry, configured maxima, and every just-over-limit case;
- forward/backward local references and multiple edges sharing dependencies;
- self-edges and duplicate-looking parallel edges;
- existing confirmed references mixed with local candidate references;
- exact-name duplicates already in the store and repeated within one batch;
- exact Unicode/case/punctuation/whitespace distinctions;
- empty/non-UTF-8/maximum payloads and aggregate payload refusal;
- minimum/maximum valid base-weight bits and invalid numeric evidence;
- deterministic candidate/node/relation-kind/edge ID allocation;
- every validation failure leaving bytes, counters, candidates, indexes, and
  routing pointer unchanged;
- batch confirmation consuming all and only selected candidates;
- missing dependency from the confirmation set refusing the whole batch;
- singleton/batch-of-one equivalence;
- no provisional visibility through lookup, routing, hydration, health image
  counts, or confirmed catalog counts;
- authoritative catalog summary counts before insertion, after provisional
  insertion, after confirmation, after deletion, restart, checkpoint, and
  restore;
- both relation usage domains for zero, one, maximum, parallel, self-edge, and
  multiple-kind fixtures;
- insertion-path coverage through singleton facade, batch facade, engine,
  catalog, admin workload, benchmark fixture, and generated test helpers,
  proving every durable edge-candidate put has one provisional increment and
  every canonical-edge put has one confirmed increment/transfer;
- aggregated provisional increments during candidate insertion, atomic
  provisional-to-confirmed transfers during promotion, and confirmed
  decrements during node cascade without double-counting self-edges;
- edge and node deletion automatically removing exactly the affected kinds that
  reach zero in both domains, while retaining kinds with provisional
  references, remaining confirmed edges, and unrelated pre-existing zero-use
  kinds;
- atomic removal of relation-kind record, exact-name entry, and popularity key;
- deterministic total-descending/confirmed-descending/ID-ascending top-K
  ordering before and after provisional insertion, confirmation, deletion,
  restart, checkpoint, and restore;
- usage overflow/refusal and corruption of every counter/index representation;
- provisional edges incrementing provisional usage, affecting popularity, and
  preventing automatic cleanup until transferred or removed;
- simultaneous threads targeting the same kind without any lost counter update;
- injected failure before each insertion commit leaving candidates, canonical
  edges, both counters, and popularity entries entirely unchanged;
- the structural mutation-site audit rejecting a deliberately introduced raw
  candidate/edge/adjacency/usage put or delete outside the planner;
- old/new image isolation during one large publication;
- resident/partitioned CPU and CUDA agreement after mixed confirmation;
- cancellation, device loss, routing rebuild failure, repair, and shutdown;
- concurrent batches and singleton/batch races;
- canonical JSON byte stability and decode/re-encode equality;
- malformed, truncated, duplicate-field, oversized, and response-limit cases.

Use small fixtures whose expected identities and paths are obvious, plus fixed
seed generated mixed batches compared with an independent in-memory oracle.

## 14. Add ingestion and usage-index performance evidence

Extend the benchmark harness with named batch-ingestion workloads:

- 10,000 node candidates inserted and confirmed;
- many relation-kind candidates, including exact duplicates;
- 100,000 directed edge candidates over confirmed nodes;
- a mixed batch with 10,000 nodes, multiple relation kinds, 100,000 relations,
  parallel edges, self-edges, and nontrivial routing paths;
- bounded concurrent batch submissions;
- many threads inserting, confirming, and deleting edges against the same small
  set of relation kinds;
- duplicate-name-only confirmation that skips publication;
- high-degree edge deletion and node-cascade deletion across few and many
  relation kinds;
- top-K relation-kind popularity lookup at small and large catalog sizes.

Record complete distributions, not a single timing, for validation, encoding,
durable commit, catalog/index update, routing compilation, publication, total
completion, committed bytes, entries/second, and peak process memory. Record
candidate and confirmed counts and assert correctness before accepting timing.
For relation usage, also record affected-kind count, provisional and confirmed
counter/index update time, catalog-lock wait/hold duration, automatic kind
deletions, top-K lookup time, and evidence that both counter domains and the
returned ordering match a candidate-plus-canonical-edge oracle.

Compare batch and repeated singleton behavior on a bounded workload where both
are practical. The batch implementation must demonstrate one candidate write
for insertion, one confirmed write for promotion, and one routing publication,
not merely a faster loop. Do not impose an invented speedup target, but explain
any phase that scales unexpectedly or makes the configured maximum unsafe.

Use the measurements to select and document default batch limits. The defaults
must accommodate the named large mixed workload on the reference machine
without making an individual API call an unbounded memory or disk commitment.

## 15. Reconcile documentation and conformance

Add Decision 0014 recording:

- the two-stage provisional/confirmed batch boundary;
- request-local reference representation;
- atomic all-or-nothing policy;
- deterministic allocation order;
- one confirmed commit and one publication;
- selected default limits and benchmark evidence;
- rejection of partial success, streaming transactions, and promotion-history
  mappings;
- the separate provisional-reference and confirmed-edge usage definitions,
  catalog-lock ownership, atomic transfer, durable total-usage popularity
  index, both-counts-zero deletion rule, and bounded top-K policy.

Update:

- `PATHHYDRA_SYSTEM_SHAPE.md` with batch candidate and publication semantics;
- `docs/consumer-api.md` with complete DTO examples and lifecycle guidance;
- `docs/storage-format.md` with the one current candidate-reference encoding;
- storage format/operations documentation with relation usage and popularity
  index encodings plus deletion invariants;
- `docs/cpu-engine.md` and CUDA operations docs with one-publication behavior;
- backup/restore, administration, dependency, performance, and safety docs
  where affected;
- `docs/system-conformance.md` with new requirement rows and executable evidence;
- the conformance checker minimum reviewed-row count.

Remove stale statements that the facade confirms only one candidate. Do not
describe provisional insertion as confirmed graph mutation, and do not call a
loop of singleton calls atomic.

## 16. Verification matrix

Run all new focused suites and the complete regression floor:

```powershell
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo doc --workspace --no-deps

cargo check --workspace --all-targets --features cuda
cargo clippy --workspace --all-targets --features cuda -- -D warnings
cargo test --workspace --features cuda
```

Also run:

- fixed-seed generated batch/oracle agreement;
- mixed-batch subprocess crash/publication campaign;
- checkpoint/restore rehearsal containing provisional references;
- canonical batch malformed/round-trip corpus;
- named release batch-ingestion benchmark suites;
- system-conformance/dependency stale checks;
- CUDA memcheck/racecheck if the implementation changes CUDA-facing ownership,
  admission, publication, or test coverage.

Record exact commands, date, commit, toolchain, feature flags, workload sizes,
correctness results, resource peaks, and publication counts in the designated
evidence documents. A skipped required gate is not a pass.

## Definition of done

Plan 11 is complete only when:

- one bounded call atomically inserts many provisional node, relation-kind,
  edge, or mixed candidates;
- mixed edges can safely reference node/relation-kind candidates from the same
  insertion request;
- one bounded confirmation call atomically promotes a dependency-complete
  selection and returns aligned results;
- a changed confirmed batch performs exactly one durable graph commit and at
  most one routing rebuild/publication;
- duplicate exact names, stable IDs, parallel/self edges, counters, indexes,
  and candidate consumption preserve singleton semantics;
- every provisional edge candidate changes its relation kind's durable
  provisional-reference count exactly once, including batched, parallel, and
  self-edge cases;
- every insertion surface is routed through typed planner operations that make
  the corresponding counter increment inseparable from candidate/canonical
  edge and adjacency creation;
- candidate confirmation atomically transfers each selected edge from
  provisional to confirmed usage, and every confirmed edge changes the durable
  confirmed count exactly once;
- existing edge and cascading node deletions atomically remove every affected
  relation kind whose provisional and confirmed counts are both zero, retain
  kinds with either reference type, and return the removed kind IDs;
- stored usage counters and popularity-index entries are fully verified against
  provisional candidates and canonical edges on open, verification, checkpoint,
  and restore;
- the bounded public popularity query returns current relation kinds by
  total use descending, confirmed use descending, and `RelationId` ascending;
- all counter read-modify-write paths hold catalog mutation ownership from the
  initial read through the atomic durable commit, with concurrency tests proving
  no lost increments, decrements, or transfers;
- every precommit failure leaves the complete store and routing pointer
  unchanged;
- every postcommit publication failure exposes complete durable state plus a
  typed routing-unavailable/repair outcome;
- provisional batches remain invisible to confirmed lookup, routing, and
  hydration while their aggregate usage counts remain accurate, and they
  survive restart/checkpoint/restore;
- requests observe only complete old or complete new routing images;
- singleton APIs share the batch implementation and batch-of-one equivalence is
  tested;
- all request/response allocations and durable batch sizes are explicitly
  bounded with typed refusal;
- canonical DTOs, errors, health/diagnostics, and documentation accurately
  expose batch behavior without leaking implementation handles;
- named large batch workloads and repeated-singleton comparisons have current
  correctness-backed evidence;
- ordinary, CUDA, crash/recovery, restore, encoding, conformance, benchmark,
  Clippy, rustdoc, and relevant sanitizer gates pass;
- the current pre-release tree contains one candidate format and no deferred
  partial-success, migration, or compatibility implementation.
