# Graph engine executors

`pathhydra-engine::GraphEngine` is the coherent storage and execution boundary.
The deterministic CPU executor remains the semantic oracle. The concrete NVIDIA
CUDA executor returns exact distances and, for path requests, verifies every
distance bit before a CPU evidence pass over the same immutable image. Finite
examined-edge budgets remain CPU-only. The API is pre-release and concrete.

## Publication

Provisional candidates cannot affect confirmed lookup, routing, or hydration,
so singleton or batch insertion does not publish. Every graph-changing
singleton or batch confirmation, edge removal, or cascading node removal holds
the publication write lock from before its one durable mutation through one
consistent streaming confirmed scan, complete bundle compilation, validation,
and image publication. A batch never rebuilds once per entry. The replacement
becomes visible only after all configured topology/metadata limits and
technical checks pass.

A duplicate-name-only node/relation confirmation consumes all selected
candidates without invalidating the active routing pointer. It reports
`NotRequired` and performs no bundle build or state swap. A changed batch has
one `Published` or `RoutingUnavailable` outcome for its complete durable result.

The mutation result separates its durable result from `Published` or `RoutingUnavailable`. A post-commit failure therefore cannot encourage a caller to retry a consumed candidate. When unavailable, the catalog remains usable for inspection and repair. `rebuild_routing_image` explicitly retries a full build.

Routes clone one immutable execution snapshot under a read lock. It always has
the CPU image and may have exactly matching CUDA residency. In-flight routes
may finish on an old snapshot while mutation publishes a replacement; no later
acquisition can pair new CPU records with stale device topology. CUDA upload
failure publishes current CPU-only state. See [Decision
0003](decisions/0003-cpu-engine-publication.md) and [CUDA
operations](cuda-operations.md).

## Resource admission and cancellation

`EngineConfig` limits active topology payload bytes, simultaneous routes, total reserved route bytes, destinations per route, and handles per hydration batch. Topology compilation preflights checked counts before topology arrays are allocated. Route estimates conservatively include the packed profile, dense destination state, distances, finalization, predecessor/path validation state, the maximum frontier, response entries, and maximum reconstructed simple paths. This is a logical collection-payload reservation, not allocator metadata or a physical RSS prediction.

CPU admission refuses excess work; CUDA has separate worst-case lane admission
and a bounded collection queue. RAII returns active slots, byte reservations,
and request IDs on every normal error, cancellation, and panic unwind through
safe Rust. `RequestId` is opaque process-local coordination, not graph
identity. `cancel` only signals an atomic flag. CPU checks during search and
reconstruction; CUDA checks at safe host-visible launch boundaries.

## Diagnostics, capabilities, and health

Every engine route returns its response plus executor, policies, image counts,
reservation, monotonic admission/execution timing, the first present
destination's completion timestamp, path-reconstruction duration, completion
reason, examined edges, relaxations, finalized nodes, frontier high-water mark,
destination state counts, and reconstruction steps. Missing destinations do
not manufacture a zero first-completion timestamp. The timestamp starts at
entry into the selected executor and therefore includes its request mapping,
profile packing, and fallible working-state allocation. A path-returning
destination is complete only after its path evidence is reconstructed; an
unreachable present destination completes when frontier exhaustion proves that
state. No payload is logged.

Capabilities report build and runtime CUDA facts independently, device
identity, algorithms, path-evidence and finite-budget policy, and partitioned
execution. Resource limits are a redacted snapshot with no filesystem paths.
Health adds resident bytes/counts, current device memory, worker
and admission state, uploads, launches, failures, fallbacks, cancellations,
and reinitializations. These values are process-local and expose no mutable
state. See [CUDA routing](cuda-routing.md).

## Recovery

- A store failure before commit preserves the current image.
- A build failure after commit makes new routing unavailable.
- Old images live only while previously admitted routes hold their `Arc`.
- Repair mutations and explicit rebuild can restore availability.
- Cancellation, admission refusal, and hydration errors do not mutate storage or publication state.
- Restart validates RocksDB and reuses a complete matching Plan 06 bundle when
  possible; otherwise it clears an unusable pointer and rebuilds from confirmed
  records.
- CUDA residency can be rebuilt without changing the CPU image; a poisoned
  context is replaced explicitly with `reinitialize_cuda`.

## Explicit shutdown

`GraphEngine::shutdown` first closes route, mutation, checkpoint, and
maintenance admission, signals active request cancellation, and waits for the
configured drain bound. A timeout leaves the engine in `Closing`; after work
drains, a repeated call continues safely. A complete shutdown stops partition
I/O, drains and joins CUDA work, synchronizes WAL and memtables, waits for
RocksDB background work, releases the database handle, requests retired-bundle
cleanup, and reports each stage plus active-before and drained counts for every
operation class. `Drop` invokes the same idempotent path as a fallback and never
intentionally detaches workers.

Maintenance uses the selected caller-executed policy: one configurable worker
slot and a zero-length queue, so concurrent excess work is refused rather than
silently accumulated. Checkpoint concurrency is admitted independently.
