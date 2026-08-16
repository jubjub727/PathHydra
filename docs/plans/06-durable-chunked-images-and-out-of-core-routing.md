# Plan 06: Durable Chunked Images and Out-of-Core Exact Routing

## Outcome

Remove the assumption that PathHydra must materialize the complete confirmed
graph, complete CPU routing topology, and complete CUDA topology at the same
time.

At completion, the Rust engine can compile one consistent confirmed graph
directly into a deterministic, checksummed, chunked routing-image bundle;
publish that bundle crash-safely as a rebuildable index; reopen and validate it
without rescanning every RocksDB record; and route exactly through it with
bounded host and CUDA topology caches. The existing fully resident CPU and CUDA
paths remain the preferred hot paths when they fit. A graph whose adjacency is
larger than the configured host or device topology budget instead uses the same
request, arithmetic, tie, publication, cancellation, and result contracts
through partitioned execution.

This is an intentionally large production slice. It includes the current-only
bundle layout, streaming compiler, durable publication protocol, startup and
recovery, shared host partition cache, partitioned CPU reference routing,
partitioned CUDA frontier and delta-stepping execution, concurrent request
coordination, diagnostics, fault injection, and scale benchmarks. These pieces
belong together because a partitioned kernel is not usable unless the bytes it
consumes are compiled, published, staged, retained, invalidated, and recovered
under one exact image identity.

The hard memory boundary for this plan is adjacency topology. Dense node
identity tables, relation IDs, the source-to-segment directory, and one search's
global distance/finalization state must still fit their explicitly configured
host or device tiers. Refusing a graph or request that exceeds those remaining
bounds is a typed admission result, never an allocation gamble.

## Current baseline and pressure points

Plan 05 is implemented and the ordinary workspace test suite passes. The
current design has the right semantics but three whole-size materializations:

1. `Catalog::confirmed_graph_records` copies every confirmed node, relation,
   and edge into vectors while holding the catalog mutation mutex.
2. `RoutingImage::compile_with_limit` builds every CPU array as an owned boxed
   slice and publication becomes unavailable above `max_active_image_bytes`.
3. `CudaResidentImage::upload` copies all GPU topology arrays and rejects the
   image when topology plus headroom does not fit the selected device.

Every confirmed engine mutation then repeats the full record copy, image
construction, and attempted complete CUDA upload while publication is
exclusive. Startup always rebuilds through RocksDB. The current CUDA request
diagnostics count one complete topology upload but have no vocabulary for
partition reads, cache hits, evictions, or staged bytes.

Those limitations align exactly with the still-unimplemented “routing snapshot
compiler,” “topology larger than device memory,” and serialized-image recovery
sections of `PATHHYDRA_SYSTEM_SHAPE.md`. This plan implements that boundary
without broadening into bindings, graph composition, or a second accelerator.

## Why this is one coherent slice

Out-of-core routing correctness spans more than storage or CUDA in isolation:

- a source's outgoing relations may cross chunk boundaries, but expansion must
  still examine every relation once and in stable `EdgeId` order;
- a frontier phase is not complete while any required chunk is unread,
  unstaged, executing, or awaiting synchronization;
- eviction is safe only for immutable bytes that remain reloadable from the
  exact bundle acquired by that request;
- a confirmed mutation must invalidate the durable current-image pointer in the
  same RocksDB batch as the graph change;
- a crash may leave an orphan bundle, but must never make an old bundle current;
- CPU partitioned routing must remain the semantic oracle and CUDA fallback for
  the same acquired image;
- old requests may finish on old bundle files while new requests use a newly
  published bundle;
- startup validation, runtime corruption, cancellation, resource exhaustion,
  and device loss need distinct outcomes.

Implementing only the file format would leave it unused. Implementing only a
CUDA cache would still require the whole CPU image in RAM. Implementing only a
partitioned CPU iterator would not solve publication or restart. The complete
slice establishes one operationally useful boundary.

## Explicit non-goals

Do not implement in this slice:

- incremental routing-image patching, topology overlays, or partial confirmed
  mutation publication; every published bundle is a complete immutable view;
- schema versions, record-format markers, compatibility readers, migrations,
  graph revision counters, caller-pinned versions, or a public historical-image
  API before the first release;
- treating a checksum or bundle token as node, relation, or edge identity;
  stable IDs remain the only graph identities and complete exact names remain
  the only name identities;
- reading RocksDB from a relaxation loop or using payload storage as an
  adjacency cache;
- storing provisional candidates in any routing file, cache, lookup table, or
  execution snapshot;
- compressed chunks in the first baseline; compression would entangle CPU
  cost, random access, corruption boundaries, and device staging before the
  uncompressed path is measured;
- adjacency topology whose fixed global directory cannot fit the configured
  host metadata budget, or CUDA searches whose global per-lane state cannot fit
  the configured device search budget;
- GPU predecessor reconstruction, GPU paths, or finite examined-edge budgets;
  those request shapes continue to use the exact CPU implementation;
- changing numeric arithmetic, relation-profile semantics, tie policy, result
  states, directionality, or destination ordering;
- unified memory as an implicit pager, CUDA peer-to-peer, multi-GPU routing,
  another accelerator vendor, or a vendor-neutral accelerator abstraction;
- selecting DirectStorage before conventional file reads and explicit CUDA
  copies are measured as the limiting stage;
- payload hydration, caller-owned subgraph composition, BAML, language
  bindings, local or remote transport, authentication, hosted telemetry, or
  cloud services;
- GitHub Actions workflows.

## 1. Preserve and extend the semantic oracle

Before changing representations, run and record the existing gates:

```powershell
cargo fmt --all --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test --workspace --features cuda
```

Retain every Plan 00-05 fixture. Add a representation-independent agreement
helper that compares responses without timing, cache, or executor diagnostics.
It must compare:

- numeric and tie policy;
- original destination order and duplicates;
- `Exact`, `Unreachable`, `MissingNode`, and `Incomplete` states;
- exact binary64 distance bits;
- completion reason and exact examined-edge count on CPU;
- each predecessor edge and complete directed path evidence when requested.

Use that helper for resident CPU versus partitioned CPU on every existing CPU
fixture, and resident/partitioned CUDA versus CPU for every CUDA-eligible
fixture. The existing `RoutingImage` remains a useful low-level test value; it
does not become a durable format.

## 2. Record two decisions before implementation

### Decision 0007: current-only durable routing-image bundles

Record the following:

- RocksDB confirmed records remain the sole durable graph source of truth.
- A routing-image bundle is a rebuildable, immutable index and may always be
  discarded.
- There is exactly one current bundle layout. It has no magic format marker,
  schema version, compatibility reader, or migration path. Before release, a
  layout change updates the writer, reader, tests, and development fixtures in
  place.
- The manifest declares semantic numeric/tie policies, fixed element widths,
  counts, byte ranges, partition descriptors, and checksums. Policy identifiers
  describe routing semantics; they are not file-format versions.
- Every integer and floating-point bit field has an explicit little-endian
  encoding. Decoding is checked and never casts arbitrary file bytes to a Rust
  struct.
- Checksums establish byte integrity only. The loader verifies complete
  manifests, sizes, ranges, and checksums and then validates graph-specific
  bounds. Hash matches are never accepted as graph identity.
- A single RocksDB metadata record identifies the current relative bundle name
  and manifest checksum. Every confirmed graph-changing batch deletes that
  record atomically. Candidate-only writes do not affect it.
- Files are completed and synchronized in a temporary child directory, checked,
  renamed within the configured image root, and only then referenced from
  RocksDB. A crash may create an unreferenced directory but cannot publish
  missing or partially written bytes.
- Startup ignores every unreferenced directory, validates the referenced bundle
  completely, and rebuilds when the pointer is absent or invalid.
- Public requests acquire an `Arc`-owned immutable execution image. Publication
  never changes the bundle underneath an admitted request.

The decision must describe the precise crash matrix in section 9 and why no
graph revision counter is needed: confirmed mutations invalidate the pointer in
their own atomic batch, and publication is serialized with confirmed mutation
and image compilation.

### Decision 0008: source-segment partitioning and bounded execution

Record the following:

- dense nodes retain ascending stable `NodeId` order and adjacency retains
  ascending stable `EdgeId` order per source;
- partitions contain one or more source segments rather than assuming a whole
  high-degree source fits one chunk;
- a source directory maps each dense source to an ordered range of segment
  descriptors; all segments must execute before that source expansion is
  complete;
- each segment stores a consecutive range of one source's outgoing relations;
  concatenating its segments reproduces exactly the resident adjacency order;
- partitions are immutable, independently checksummed, and bounded by a hard
  byte limit except for fixed metadata whose limit is checked separately;
- CPU search state resides in host memory; CUDA global search state resides on
  the device; adjacency partitions move through bounded caches;
- cache presence and processing order may affect performance but never the
  selected distance, stable predecessor, budget accounting, or completion;
- pending file reads, copies, launches, and synchronizations count as unfinished
  work for the current frontier or delta phase;
- eviction requires zero active users and completed device events; discarded
  bytes must be reloadable from the request's acquired bundle;
- conventional bounded worker-thread reads plus explicit device copies are the
  transport baseline.

## 3. Define one deterministic bundle layout

Add current-only bundle support under `pathhydra-routing`, where the routing
semantics and fixed arrays already live:

```text
crates/pathhydra-routing/src/bundle/
  codec.rs
  compiler.rs
  layout.rs
  manifest.rs
  reader.rs
  writer.rs
  mod.rs
```

A completed bundle directory contains exactly:

```text
manifest.bin
identities.bin
source-directory.bin
topology.bin
evidence.bin
```

Do not encode Rust struct padding or platform `usize` values. Define checked
field encoders/decoders over `u32`, `u64`, and length-prefixed UTF-8 policy IDs.
The current layout is:

- `identities.bin`: dense-to-external `NodeId` values followed by confirmed
  `RelationId` values, each sorted as required by routing;
- `source-directory.bin`: `node_count + 1` segment offsets followed by fixed
  segment descriptors mapping a source and ordered edge range to a partition;
- `topology.bin`: independently addressable partition regions containing local
  source boundaries, dense destinations, dense relation indexes, and canonical
  base-weight bits in structure-of-arrays order suitable for CUDA staging;
- `evidence.bin`: partition-aligned stable `EdgeId` values used by CPU
  predecessor reconstruction; a relation's stable ID is recovered exactly from
  the checked global relation-index table;
- `manifest.bin`: policies, counts, widths, configured partition target and hard
  maximum, file lengths/checksums, and every partition byte range/checksum.

The manifest must be deterministic for the same confirmed records, routing
policies, and partition limits. It contains no timestamps, machine paths, GPU
details, or temporary tokens. The manifest checksum stored in RocksDB is
computed over the exact completed manifest bytes. Select one small maintained
checksum implementation after dependency/license review, pin it in the
workspace, and use it consistently for files and chunks; do not hand-roll a
checksum.

Checked validation must reject:

- unknown numeric or tie policy IDs;
- unsupported element widths or noncanonical booleans/counts;
- count multiplication, byte offset, range-end, or platform conversion
  overflow;
- file lengths different from the manifest;
- overlapping, unordered, unaligned, empty-invalid, or out-of-file ranges;
- missing or extra source segments;
- segment source mismatches, gaps, overlaps, or nonmonotonic edge ordinals;
- a destination outside `node_count`;
- a relation index outside `relation_kind_count`;
- noncanonical base-weight bits;
- duplicate/nonascending stable node or relation IDs;
- partition or complete-file checksum mismatch;
- trailing bytes not declared by the current layout.

Empty graphs, isolated nodes, empty partitions, and zero-adjacency files need
canonical encodings and explicit tests. Files are opened read-only after
publication. Use owned buffers and safe positional or seek-based file reads;
do not introduce memory mapping or another unsafe boundary merely to claim
zero-copy loading.

## 4. Add a consistent streaming confirmed-graph scan

Replace the production compiler's dependency on `ConfirmedGraphRecords` with a
store-owned streaming scan. Preserve `ConfirmedGraphRecords` for compact tests
and callers of the low-level in-memory compiler.

Add a catalog API whose guard holds the catalog mutation mutex for the full
scan/build/publication transaction. The store, not the routing crate, owns
RocksDB snapshots and iterators. The API must provide ordered passes for:

1. confirmed nodes by stable `NodeId`;
2. confirmed relation kinds by stable `RelationId`;
3. canonical outgoing adjacency by `(source NodeId, EdgeId)`, resolving the
   complete canonical edge record from the same consistent view.

The scan excludes candidates by construction. It validates the same durable
invariants as catalog open, including exact endpoints, direction, relation
existence, canonical weight, matching incoming/outgoing representations, and
stable IDs. A callback or cursor may borrow one decoded record at a time; it
must not expose RocksDB handles or allow the routing crate to issue arbitrary
database reads.

Compilation may retain the dense `NodeId` table, relation-ID table, partition
directory, and one bounded partition buffer. It must not retain all
`NodeRecord`, `RelationRecord`, or `EdgeRecord` values. Destination IDs are
resolved by binary search in the resident sorted dense node table. Names and
payloads are decoded only to the degree needed for store validation and never
written to the routing bundle.

Instrument scan record counts, decoded bytes, peak compiler buffer bytes,
RocksDB read duration, bundle write duration, validation duration, and total
duration. Tests must force tiny compiler buffers and prove peak adjacency
buffering remains bounded as edge count grows.

## 5. Build bounded source segments without changing order

Implement a streaming partition builder with two configured values:

- a target partition topology size used to close ordinary partitions; and
- a hard maximum partition topology size used by cache admission.

Account for every serialized boundary, destination, relation index, base-weight
word, and evidence ID before accepting an edge. Use checked arithmetic. The
builder may close a partition between sources or split a single high-degree
source into consecutive segments. It must never reorder edges to improve
packing.

For each dense source:

1. consume canonical outgoing entries in ascending `EdgeId` order;
2. translate destination and relation IDs through the checked global tables;
3. append fixed topology words and the matching edge evidence;
4. close a segment before the next entry would exceed the hard maximum;
5. close a partition at or above the target when doing so preserves a valid
   segment boundary;
6. emit an empty directory range for an isolated source;
7. record the exact consecutive edge ordinal covered by every segment.

Reject a configuration whose hard maximum cannot hold the smallest nonempty
segment plus required fixed metadata. A one-edge source must therefore always
fit after configuration validation. Make partition numbering deterministic and
dense from zero.

After writing, reopen the temporary bundle through the production reader and
perform full validation before it is eligible for publication. Add a test-only
adapter that reconstructs a resident `RoutingImage` from a bundle and compares
every logical array to the existing compiler. This is the primary proof that
streaming partitioning preserves topology and evidence.

## 6. Represent resident and partitioned execution explicitly

Refactor engine publication around an immutable execution image whose common
metadata is always resident:

```text
PublishedExecutionImage
  BundleLease
  RoutingIdentityTables
  SourceSegmentDirectory
  CpuTopology
    Resident(RoutingImage)
    Partitioned(ChunkedRoutingImage)
  CudaTopology
    Resident(CudaResidentImage)
    Partitioned(CudaPartitionedImage)
    Unavailable(reason)
```

This is a concrete representation enum, not a speculative public storage trait.
The two CPU and two CUDA representations are real users. Keep representation
details internal to the engine/routing crates; requests and responses do not
gain a storage-mode field that callers must understand.

At publication, choose CPU residency from checked bundle sizes and configured
budgets:

- load a resident `RoutingImage` when its complete CPU arrays fit
  `max_active_image_bytes`;
- otherwise retain the identity tables and directory and open partition files
  through the bounded cache;
- refuse publication with a typed metadata-limit reason if required global
  tables do not fit `max_resident_image_metadata_bytes`;
- separately validate the maximum per-route host search reservation before
  admitting requests.

CUDA selection similarly prefers complete residency, then partitioned residency
when global CUDA search state plus topology-cache slots and headroom fit, then a
typed CPU-only CUDA degradation under permissive policy. `RequireCuda` reports
why neither CUDA mode is admissible.

The existing resident paths must continue to avoid file reads after
publication. Do not route every small graph through the slower partitioned
implementation just to reduce code paths.

## 7. Implement a shared bounded host partition cache

Add a synchronous public routing API backed by an internal bounded I/O
coordinator. Do not add a general async runtime solely for file reads.

The coordinator owns:

- a fixed worker-thread count;
- cloned read-only file handles for current and still-leased retired bundles;
- a byte-accurate cache limit and admission counter;
- per-partition states `Absent`, `Loading`, `Ready`, and `Failed`;
- load coalescing so concurrent requests wait on one read;
- active-use pins preventing eviction;
- deterministic least-recently-released eviction among unpinned entries;
- optional bounded sequential read-ahead driven by measured access, disabled by
  default until benchmarked;
- shutdown and cancellation that wake every waiter without detaching file work.

One cache entry owns the complete decoded CPU partition: local source
boundaries, topology arrays, and edge evidence. Admission includes encoded read
buffers, decoded arrays, and checksum scratch so peak memory cannot silently
double the configured cache. A partition larger than the complete host cache is
a typed configuration/admission error found during bundle open.

Every load checks exact read lengths and the partition checksums before exposing
bytes. A runtime mismatch poisons that bundle for new routes, preserves the
durable graph, and requests a controlled rebuild. It is not treated as an
unreachable graph region. Already finalized destinations remain exact only
where the ordinary CPU result contract permits a controlled incomplete result;
the engine-level response must also carry the typed image failure and must not
silently present the route as complete.

Cache keys include the engine-internal bundle lease identity and partition ID.
They are never public graph identifiers. Publishing a new bundle does not
retag old entries. Old entries remain usable only by requests holding the old
bundle lease and become evictable normally when released.

## 8. Add exact partitioned CPU routing

Implement partitioned CPU routing before partitioned CUDA. Factor the existing
CPU search so queue/finalization/profile/result semantics are shared while
outgoing expansion is supplied by either resident arrays or a partition lease.
Do not duplicate numeric or tie logic.

For a partitioned source expansion:

1. read its ordered segment-descriptor range from resident metadata;
2. acquire each referenced partition in descriptor order;
3. visit only the source's consecutive edge slice;
4. count an examined edge immediately before relation-state checking, exactly
   as the resident CPU implementation does;
5. perform the same separate binary64 multiplication and addition;
6. compare the same stable predecessor tuple;
7. release each partition after its source slice is consumed.

Prefetch may overlap a later segment read, but it cannot change visitation,
budget, or cancellation order. On a finite edge budget, stop before the next
edge exactly as today. On cancellation, do not begin another partition wait;
wake a pending wait and return using existing cancellation precedence. Paths
store stable predecessor evidence at relaxation time, so reconstruction never
depends on a partition remaining cached.

Add forced-partition tests for:

- every existing CPU fixture with a partition hard limit of one or two edges;
- a high-degree source split across many partitions;
- equal-cost predecessors whose candidates arrive from different partitions;
- parallel edges divided at a partition boundary;
- a self-edge and zero-weight cycle spanning cache churn;
- destination-aware early completion before unrelated partitions load;
- a finite budget ending between two segments of one source;
- cancellation before load, while waiting on a coalesced load, and between
  segments;
- a one-entry cache that thrashes on every expansion;
- two requests using different profiles and origins against the same cache;
- injected short read, checksum mismatch, and worker shutdown.

Resident and partitioned responses must match exactly; cache counters may
differ and are compared separately.

## 9. Make filesystem/RocksDB publication crash-safe

Add a routing-image root to engine configuration. Validate it as an explicit
absolute or database-relative directory, reject the RocksDB directory itself,
and ensure every temporary, final, retired, and cleanup target resolves beneath
that root. Cleanup never operates on a glob, unresolved environment variable,
workspace root, drive root, or caller-unvalidated path.

Use this publication sequence while the engine publication lock and catalog
confirmed-scan guard exclude another confirmed mutation:

1. A confirmed graph-changing RocksDB batch deletes `active-routing-image`.
2. Create one uniquely named temporary child of the routing-image root.
3. Stream the consistent confirmed graph into the five bundle files.
4. flush and synchronize each file, complete the manifest last, and synchronize
   the temporary directory where the platform supports it;
5. reopen and fully validate the temporary bundle;
6. atomically rename the directory to its final relative bundle name on the
   same volume;
7. write the relative name and exact manifest checksum to
   `active-routing-image` while the catalog guard is still held;
8. construct resident/partitioned CPU and CUDA representations from that final
   directory;
9. swap the complete execution image into the engine once;
10. retire the old bundle only after all acquired request references release.

If runtime representation construction fails after step 7, clear the pointer
again and report routing unavailable; do not leave a bundle marked current
that the process rejected. If CUDA upload/cache construction alone fails,
publish the valid CPU representation with the existing typed CUDA degradation.

Test every crash boundary:

| Last completed action | Restart behavior |
| --- | --- |
| confirmed mutation committed | pointer absent; rebuild from confirmed graph |
| temporary files partially written | pointer absent; ignore/remove exact orphan |
| temporary bundle validated | pointer absent; ignore/remove exact orphan |
| final rename completed | pointer absent; ignore/remove exact orphan |
| RocksDB pointer committed | validate and open the referenced final bundle |
| engine swap not reached | restart still opens the referenced valid bundle |
| later confirmed mutation committed | pointer deleted atomically; never open old bundle as current |

Fault injection must be deterministic and available in tests without killing
the test runner: return injected errors at named stages, close/reopen the
catalog and engine, and inspect the resulting files and metadata. Add a small
separate process crash harness only for the few ordering properties that an
ordinary returned error cannot simulate.

Provisional insertion never rebuilds or invalidates the current image. A
promotion or deletion that changes confirmed routing material invalidates it in
the same batch. Removing an edge or node therefore cannot leave a durable
pointer to topology containing the removed relation. Duplicate confirmation
that changes only candidate state may conservatively rebuild, but must never
expose the candidate as new confirmed topology.

## 10. Open validated bundles at startup and manage retirement

On `GraphEngine::open`:

1. open and validate the catalog as today;
2. read the current-image pointer under catalog coordination;
3. resolve only its checked relative child path;
4. validate manifest bytes against the pointer;
5. validate all file sizes, complete-file checksums, partition checksums, and
   semantic bounds in sequential passes;
6. construct the selected CPU representation and attempt the selected CUDA
   representation;
7. publish routing only after validation succeeds.

If the pointer is absent, referenced files are missing, or any validation fails,
discard the pointer and rebuild from RocksDB. A failed rebuild opens the catalog
but reports typed routing unavailability, preserving mutation and hydration
where their existing contracts allow it. Never “repair” a bundle in place.

Track retired bundles explicitly. An old `PublishedExecutionImage` holds a
`BundleLease`; publication moves its directory to a retirement set. A reaper
may delete only an exact retired child after the engine has no image, cache
entry, file handle, I/O task, CUDA transfer, or request referencing it. Windows
file-sharing failures remain retryable retirement state rather than routing
failure. Startup removes only positively identified temporary or unreferenced
bundle children after validating their location and preserving the referenced
current bundle.

Document backup behavior: the routing-image directory may be omitted because
it is rebuildable. A restored RocksDB pointer to an omitted file is treated as
absent/corrupt cache state and cleared before rebuild. A backup procedure must
not present bundle bytes as authoritative graph records.

## 11. Add partition-aware CUDA topology residency

Extend `pathhydra-cuda` with concrete partitioned modules:

```text
src/
  partitioned.rs
  topology_cache.rs
  staging.rs
  phase.rs
kernel/
  partition_frontier.rs
  partition_delta.rs
  frontier_compaction.rs
```

Keep the audited unsafe boundary from Decision 0005. File I/O and bundle decode
remain safe host Rust. Any pinned host allocation, asynchronous copy, event, or
kernel ABI operation must live behind the existing narrowly allowed CUDA
modules with documented size, alignment, lifetime, context, and stream-order
obligations.

A `CudaPartitionedImage` owns:

- the exact matching CPU bundle lease and common identity/directory metadata;
- device-global immutable relation metadata needed for profile packing;
- a fixed number of byte-accounted device topology slots;
- a bounded pool of pinned host staging buffers, if the selected CUDA wrapper
  proves them safely usable;
- partition-to-slot state and load coalescing;
- per-slot active launch/event counts;
- independent admitted global search allocations per lane;
- scheduler health and failure state.

Device admission reserves, in order:

1. configured free-memory headroom;
2. global immutable metadata;
3. complete worst-case search state for every admitted lane;
4. topology cache slots and copy scratch;
5. fixed kernel output/counter buffers.

Do not allocate a topology slot after search admission if it was not included
in the reservation. Reject configurations where no nonempty partition fits a
slot or where a high-degree segment exceeds the declared maximum. Full-resident
and partitioned CUDA images remain distinct types so a resident route does not
pay cache orchestration costs.

The device cache state machine is:

```text
Absent -> HostLoading -> Copying -> Ready -> InUse -> Ready -> Evicting -> Absent
                         |                    |
                         +------ Failed <----+
```

Multiple lanes requesting one partition coalesce its host read and device copy.
A slot cannot be evicted until every launch using it has recorded and completed
its event. Cancellation releases a lane's interest but cannot free a buffer
still referenced by CUDA work. Context poisoning fails every pending cache
state, synchronizes or abandons it through the existing recovery boundary, and
never affects bundle files.

## 12. Implement partitioned CUDA frontier routing

Implement the frontier algorithm first as the inspectable out-of-core CUDA
reference. The kernel continues strict label correction with exact atomic
binary64 minimum and independent request lanes.

For each frontier phase:

1. compact or copy the active dense source IDs into a bounded host-visible list;
2. map every source to all of its ordered segment descriptors;
3. group segment work by partition without dropping duplicate source segments;
4. acquire/load/stage the required partitions through the shared caches;
5. launch relaxation work with explicit partition-local bounds and lane IDs;
6. record completion events and retain slots until those events complete;
7. wait for every required read, copy, and launch for the phase;
8. collect counters and the next frontier only after the complete phase closes;
9. terminate only when the next frontier is empty or cancellation/device failure
   takes the documented path.

Grouping changes scheduling, not the relation sequence within a source. A
source split across partitions is complete only after every segment launch.
Zero-effective cycles still terminate because equal distances do not update.
Destination-aware termination may stop only after target distances are proven
under the algorithm's existing exact completion rule; merely finding a target
in an early partition is insufficient.

Batch the same ready partition across independent lanes where profitable, while
keeping distances, profiles, destinations, cancellation, counters, and status
separate. A slow I/O request must not make another lane's result inherit its
failure. Add a diagnostic mode that deliberately processes partition groups in
reverse cache order and still produces identical distance bits.

## 13. Implement partitioned CUDA delta-stepping

After frontier agreement passes, adapt exact delta-stepping without weakening
its light-closure/heavy-edge obligations.

For each logical bucket:

- identify the current bucket's active nodes;
- group all light-edge source segments by partition;
- process every required partition and incorporate newly inserted same-bucket
  nodes;
- repeat light closure until no same-bucket work exists and no read/copy/launch
  remains pending;
- retain the complete removed set for that bucket;
- group and process every heavy-edge segment for the removed set;
- advance to the smallest represented nonempty bucket only after heavy work and
  all pending transport complete.

The partition scheduler must not mark a bucket closed based only on device queue
emptiness while a host read is pending. Bucket-index overflow remains a typed
failure; delta is never clamped. The exact current arithmetic and unlimited
budget eligibility remain unchanged.

Test frontier versus delta versus CPU on forced one-edge partitions, cache
thrash, sparse huge bucket indexes, light edges crossing partitions, newly
discovered same-bucket work requiring an evicted partition, zero-weight closure,
and multiple batched lanes using different profiles.

## 14. Integrate executor selection, fallback, and cancellation

Expand CUDA eligibility from “complete resident topology” to “matching resident
or partitioned CUDA image.” Preserve the policies:

- `CpuOnly`: resident or partitioned CPU according to the published image;
- `PreferCuda`: use either CUDA mode when eligible, otherwise the matching CPU
  mode;
- `RequireCuda`: return typed resident/partitioned shape, metadata, cache,
  search-state, I/O, or device refusal;
- `Auto`: remain CPU until the new benchmark matrix establishes a conservative,
  repeatable crossover for a named CUDA mode.

Permissive CUDA failure reruns the complete request on the CPU representation
from the same acquired bundle. Never combine partial GPU distances with CPU
path evidence. A corrupt/unreadable bundle cannot be “fixed” by falling back to
another executor over those same bytes; mark the image unhealthy and trigger a
rebuild from RocksDB.

Cancellation checkpoints include queue admission, host-cache wait, before file
read assignment, after read validation, before device copy, before each launch,
between phase groups, and before response construction. A file read already in
progress may complete into cache, but the cancelled lane does not launch it.
Running CUDA work is synchronized before lane or cache-slot reuse, as in Plan
05. Report cancellation latency by state without claiming kernel preemption.

## 15. Make limits and health operationally explicit

Extend configuration with independently validated limits for:

- routing-image root and maximum total bundle bytes;
- target and hard-maximum partition topology bytes;
- maximum resident identity/directory bytes;
- host partition-cache bytes and entry count;
- I/O worker count, maximum queued reads, and staging bytes;
- CUDA topology-cache bytes and slot count;
- CUDA pinned-host staging bytes;
- maximum retired bundle bytes or count awaiting safe cleanup;
- startup checksum/rebuild behavior.

Avoid one ambiguous “memory limit.” Health and image-build reports must expose:

- resident versus partitioned CPU/CUDA mode;
- manifest checksum and relative bundle name only, never graph names/payloads;
- total bundle, identity, directory, topology, and evidence bytes;
- partition/segment counts, largest partition, and split-source count;
- startup load versus RocksDB rebuild and their durations;
- host cache capacity/current/high-water bytes, hits, misses, coalesced waits,
  evictions, read bytes, short reads, checksum failures, and queue depth;
- device cache capacity/current/high-water bytes, hits, misses, copies,
  evictions, slot waits, and in-use slots;
- per-request partitions required, host/device hits, file bytes, staged bytes,
  transfer bytes, I/O wait, copy wait, phase wait, launches, and fallback;
- retired bundle count/bytes and last cleanup failure;
- last image corruption, rebuild, CUDA degradation, and recovery outcome.

Diagnostics must not log node/relation names, payloads, complete profiles,
destination lists, paths, or raw file contents. Counter increments use checked
or explicitly saturating arithmetic and never affect results.

## 16. Add corruption, pressure, and concurrency fault injection

Add deterministic test hooks below public APIs for:

- short positional reads and read errors at a selected partition;
- checksum mismatch in each file and in one partition region;
- delayed/coalesced reads and worker shutdown;
- host cache full with all entries pinned;
- CUDA cache full with all slots awaiting events;
- host allocation, pinned allocation, device allocation, copy, launch, event,
  and synchronization failure;
- cancellation at every state transition;
- context loss while one partition is cached and another is copying;
- publication errors at every filesystem/RocksDB boundary;
- retirement deletion failure on Windows-style open handles.

Stress tests run confirmed mutation publication concurrently with resident CPU,
partitioned CPU, resident CUDA, and partitioned CUDA requests. Assert that each
request observes one complete old or new image; no request sees mixed partition
directories, stable-ID tables, or relation indexes. Delete a node with incoming
and outgoing relations during the stress test and prove no newly admitted route
can traverse any removed relation after publication.

Use small files and tiny limits for ordinary CI-independent tests. CUDA tests
remain feature/runtime gated and skip with a precise capability reason when no
supported device is present.

## 17. Build an honest scale and transport benchmark matrix

Extend `pathhydra-bench` with reproducible generated workloads and CSV output.
Do not check large graph databases or routing bundles into the repository.

Add suites for:

### Build and restart

- RocksDB scan plus bundle build;
- compiler peak host memory;
- full checksum validation and reopen;
- resident-array load from bundle;
- comparison of validated reopen against full RocksDB rebuild;
- graph shapes with sparse stable IDs, high-degree hubs, and uniform degree.

### Partitioned CPU

- cold cache, warm cache, and one-entry thrash;
- narrow chain, broad star, dense region, disconnected regions, and mixed
  locality;
- destination-aware early completion;
- path and distance-only working sets;
- varying partition and host-cache sizes.

### Partitioned CUDA

- resident GPU baseline versus forced partitioning of the same graph;
- cold host/device caches, warm caches, and adversarial eviction;
- frontier and delta-stepping;
- one lane, batching, and concurrent unrelated partition sets;
- file-read, host-stage, copy, launch, and synchronization durations;
- varying device-cache slots, partition sizes, and pinned staging counts.

Every timed GPU response is checked against untimed CPU output first. Report
exact hardware, driver, toolchains, storage volume/type when discoverable,
configuration, graph counts, topology bytes, bundle bytes, cache sizes, and
correctness columns. Do not claim speedup from a single run.

Provide three workload scales:

1. tiny forced-partition correctness fixtures;
2. a practical end-to-end RocksDB graph large enough to exceed a deliberately
   constrained host/device topology cache;
3. an opt-in generated bundle larger than the local RTX 3080's usable topology
   residency, with a default target of at least 12 GiB, to prove the runtime no
   longer has a device-topology-size ceiling. The generator may write directly
   through the production bundle writer but must also validate exact analytic
   routing answers.

The 12 GiB suite is local/manual because it consumes material time and disk. It
is nevertheless part of this plan's completion evidence, not a hypothetical
future benchmark. Record peak host memory to prove the topology was not wholly
materialized.

## 18. Keep DirectStorage as an evidence-gated comparison

After conventional I/O benchmarks, calculate for cold partitioned CUDA routes:

- time waiting for file reads;
- achieved sequential/random read throughput;
- time decoding/checksumming;
- host-to-device copy time;
- kernel time;
- cache hit ratios and eviction amplification.

If file transport is not a repeatable dominant component, document that result
and stop. If it is dominant on the supported Windows/NVIDIA environment, write
a short decision proposal and a separate minimal read-only DirectStorage spike
that loads the same immutable partition bytes into an isolated buffer and
compares integrity and timing. Do not merge it into production routing in this
plan, do not let it choose partitions, and do not make Windows-specific
transport required. Conventional reads remain the correctness and portability
baseline.

## 19. Documentation and public API cleanup

Update:

- `README.md` with resident and out-of-core capability boundaries;
- `docs/routing-image.md` to distinguish low-level in-memory images from the
  one current durable bundle;
- `docs/cuda-routing.md` with resident/partitioned eligibility and exact phase
  completion;
- `docs/cuda-operations.md` with cache sizing, rebuild, retirement, and recovery;
- `docs/storage-format.md` with the current metadata pointer and bundle layout,
  explicitly without compatibility/version promises;
- `docs/performance/` with named out-of-core baseline results and no unsupported
  speedup claim;
- accepted decisions 0007 and 0008.

Document public errors and configuration with examples for:

- startup reuse of a valid bundle;
- forced rebuild after an absent/corrupt bundle;
- resident versus partitioned selection;
- host metadata/cache refusal;
- `PreferCuda` fallback and `RequireCuda` refusal;
- manual rebuild and CUDA reinitialization;
- backup/restore with routing images omitted.

Remove obsolete statements that serialized images, active durable publication,
or out-of-core routing are unimplemented. Do not retain alternate development
layouts or compatibility shims.

## Suggested implementation sequence

Keep every stage buildable and reviewable, but finish the complete boundary:

1. baseline gates and representation-independent agreement helper;
2. Decisions 0007 and 0008;
3. bundle layout/codec and corruption tests;
4. store-owned consistent streaming scan;
5. deterministic partition writer and full reader validation;
6. resident reconstruction equivalence and startup reopen benchmark;
7. crash-safe pointer invalidation/publication and fault matrix;
8. published execution-image enum, leases, and retirement;
9. bounded host I/O coordinator and partition cache;
10. exact partitioned CPU routing, paths, budgets, and cancellation;
11. CUDA partitioned admission, staging pool, and topology cache;
12. partitioned frontier kernels/scheduler and CPU agreement;
13. partitioned delta-stepping and three-way agreement;
14. executor policy, fallback, health, and recovery integration;
15. concurrent publication/cache/device stress tests;
16. scale generator and benchmark matrix;
17. DirectStorage evidence gate;
18. documentation, API cleanup, and final verification.

Do not begin CUDA partition kernels before partitioned CPU agreement and bundle
crash recovery pass. Do not enable automatic CUDA selection before benchmark
evidence exists.

## Required test matrix

The completed slice must cover at least:

| Boundary | Required proof |
| --- | --- |
| Canonical bytes | deterministic rebuilds produce byte-identical bundles |
| Candidate isolation | every provisional kind is absent from every file and route |
| Exact identities | sparse stable IDs and exact names retain their existing mappings |
| Direction/category | reverse traversal stays impossible and profiles use exact relation IDs |
| Segmentation | concatenated segments reproduce resident `EdgeId` order exactly |
| High degree | one source safely spans many bounded partitions |
| CPU semantics | resident and partitioned results/paths/budgets match exactly |
| CUDA semantics | resident CUDA, partitioned CUDA, and CPU distance bits agree |
| Frontier completion | pending read/copy/launch prevents premature completion |
| Delta completion | light closure and heavy phase include all partitions |
| Cache safety | pinned/in-flight entries cannot be evicted or reused |
| Concurrency | independent requests share immutable bytes, never search state |
| Publication | each request sees one complete old or new bundle |
| Deletion | removed node relations disappear from every newly published representation |
| Crash recovery | every named crash point opens valid current data or rebuilds |
| Corruption | no checksum/range failure becomes an ordinary route result |
| Cancellation | every wait/transfer/phase releases reservations and wakes callers |
| Device loss | durable graph/bundle survives and permissive CPU routing remains possible |
| Memory bounds | measured compiler/cache/search peaks stay within declared limits |
| Scale | an opt-in >10 GiB topology routes without complete topology residency |

## Final verification

Run the ordinary gates:

```powershell
cargo fmt --all --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Run CUDA compilation and agreement on the supported device:

```powershell
cargo check --workspace --all-targets --features cuda
cargo clippy --workspace --all-targets --features cuda -- -D warnings
cargo test --workspace --features cuda
```

Run the existing CUDA sanitizer procedure over the new partition kernels and
cache stress cases. Run the named resident/out-of-core benchmark matrix in
release mode, including the opt-in topology larger than device residency, and
check every recorded result against CPU before accepting timing output.

Inspect the repository for:

- unsafe code outside the audited CUDA boundary;
- obsolete unchunked durable-image layouts or compatibility readers;
- format/schema version fields, graph revision counters, or pinned-image APIs;
- provisional records in bundle code or fixtures;
- unchecked byte arithmetic, unbounded queues/caches, or reads from RocksDB in
  expansion loops;
- broad or unresolved cleanup paths;
- accidental payload/name logging;
- unsupported performance claims;
- GitHub Actions additions.

## Definition of done

Plan 06 is complete when all of the following are true:

- production image compilation no longer constructs `ConfirmedGraphRecords` or
  all adjacency arrays in memory;
- one current-only checksummed bundle is built deterministically from one
  consistent confirmed scan and remains a rebuildable index;
- confirmed mutations invalidate the durable pointer atomically, publication is
  crash-safe, and startup either validates the exact current bundle or rebuilds;
- resident CPU/CUDA execution remains available and preferred when it fits;
- partitioned CPU routing supports the full current CPU request contract with
  exact agreement, including paths, finite budgets, and cancellation;
- partitioned frontier and delta CUDA routing support the existing CUDA-eligible
  request set with exact CPU agreement;
- host and device topology caches are bounded, concurrent, observable, and safe
  under load, eviction, cancellation, corruption, and device failure;
- old requests can finish on old bundle leases while new requests use one
  complete replacement;
- routing an opt-in generated topology larger than local device residency is
  demonstrated without whole-topology host or device materialization;
- public behavior, current layouts, operations, recovery, and benchmark limits
  are documented;
- CPU/GPU agreement, sanitizer, fault-injection, and workspace quality gates all
  pass.

The next plan after this one should choose a different product boundary rather
than expanding this slice mid-implementation. Plausible later boundaries are
incremental/overlay image publication, a stable Rust-facing transport/binding
surface for BAML, or GPU predecessor/path support, but none is required to call
Plan 06 complete.
