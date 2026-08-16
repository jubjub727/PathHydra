# CPU graph engine

`pathhydra-engine::GraphEngine` is the coherent CPU-side boundary. It owns the durable catalog, current immutable routing image, admission accounting, active request cancellation flags, and process-local health counters. Its API is pre-release and intentionally concrete.

## Publication

Provisional candidates cannot affect confirmed lookup, routing, or hydration, so inserting them does not publish. Every successful confirmed promotion, edge removal, or cascading node removal holds the publication write lock from before durable mutation through complete record capture and image publication. The replacement becomes visible only after compilation, validation, and the topology-byte check succeed.

The mutation result separates its durable result from `Published` or `RoutingUnavailable`. A post-commit failure therefore cannot encourage a caller to retry a consumed candidate. When unavailable, the catalog remains usable for inspection and repair. `rebuild_routing_image` explicitly retries a full build.

Routes clone one image `Arc` under a read lock. In-flight routes may finish on that image while a mutation publishes a replacement; no later acquisition can enter the stale image. See [Decision 0003](decisions/0003-cpu-engine-publication.md) for transitions and lock order.

## Resource admission and cancellation

`EngineConfig` limits active topology payload bytes, simultaneous routes, total reserved route bytes, destinations per route, and handles per hydration batch. Topology compilation preflights checked counts before topology arrays are allocated. Route estimates conservatively include the packed profile, dense destination state, distances, finalization, predecessor/path validation state, the maximum frontier, response entries, and maximum reconstructed simple paths. This is a logical collection-payload reservation, not allocator metadata or a physical RSS prediction.

Admission refuses excess work; it does not queue. RAII returns active slots, byte reservations, and request IDs on every normal error, cancellation, and panic unwind through safe Rust. `RequestId` is opaque process-local coordination, not graph identity. `cancel` only signals an atomic flag. The synchronous route checks it during search and reconstruction; callers create parallelism with their own threads.

## Diagnostics, capabilities, and health

Every engine route returns its response plus executor, policies, image counts, reservation, monotonic durations, completion reason, examined edges, relaxations, finalized nodes, frontier high-water mark, destination state counts, and reconstruction steps. No payload is logged.

Capabilities report CPU routing, paths, budgets, cancellation, hydration, and subgraphs as supported. GPU routing and durable image files are unsupported. Health reports publication state, current manifest and age, the last build, active and peak reservations, and saturating cumulative counters. These values are process-local and expose no mutable state.

## Recovery

- A store failure before commit preserves the current image.
- A build failure after commit makes new routing unavailable.
- Old images live only while previously admitted routes hold their `Arc`.
- Repair mutations and explicit rebuild can restore availability.
- Cancellation, admission refusal, and hydration errors do not mutate storage or publication state.
- Restart validates RocksDB and builds one fresh current image.
