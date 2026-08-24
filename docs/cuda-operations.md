# CUDA operations

Atomic confirmed candidate batches retain the existing complete-image CUDA
ownership boundary. One graph-changing batch compiles one CPU/bundle image and
attempts CUDA residency once; it never uploads after each entry. Requests that
already leased the old image may finish, while later admission observes the
complete replacement or the one typed routing-unavailable state. Provisional
and duplicate-name-only batches do not touch CUDA residency.

Routing bundles are rebuildable and survive CUDA context loss. Reinitializing CUDA never changes confirmed records or bundle bytes. Operators size complete CUDA topology separately from host identity/directory metadata, the host partition cache, and CPU/device search reservations. A corrupt or unreadable partition is an image failure that triggers a controlled RocksDB rebuild, never an unreachable route.

CUDA is disabled by default. Enable `EngineConfig.cuda`, choose device ordinal
and executor policy, retain display/headroom memory, and independently set
resident topology, partition-cache bytes/slots, pageable staging, search,
concurrency, batch, and algorithm limits. Configuration values are validated
before opening a driver where possible.

Capabilities separate compile-time support from runtime availability and name
the graph-parallel, reset, target, profile, and same-image path-evidence modes.
Exact path requests are supported; finite examined-edge budgets remain CPU.
Health reports device identity and current
free/total memory, resident counts/bytes, worker state, queued/active lanes,
admission high-water values, device-cache hits/misses/copies/evictions/slot
waits/in-use slots, uploads, launches, failures, fallbacks,
cancellations, and context reinitializations. Request diagnostics identify the
selected executor, reason/fallback, algorithm/delta, lane/batch, bytes,
partition/file/staged/transfer bytes, launches, synchronized duration, phases,
atomic CAS retries, path-evidence work, and relaxations. No names,
payloads, complete profiles, or destinations are logged.

The device cache exposes host-loading, copying, evicting, ready, in-use, and
failed counts. One loader owns a missing partition while concurrent lanes wait
on its state; the cache mutex is not held across the host read, CUDA copy, or
device-allocation release. Copies use a dedicated stream. Every compute launch
records a CUDA completion event, and a slot remains in use until its leases are
released and all recorded events are complete. Slot pressure waits with
cancellation checkpoints instead of reusing an in-flight allocation.

Confirmed mutation publication is CPU-authoritative. A replacement CPU image
is compiled first, then resident upload or partition-cache construction is attempted while publication is
exclusive. Success publishes the matching pair once; failure publishes the
new CPU image with a CUDA degradation reason. Existing requests retain old CPU
and device buffers through their own execution snapshot.

Use `rebuild_cuda_residency` after transient memory pressure. Use
`reinitialize_cuda` after device loss, launch/synchronization poisoning, or a
driver reset; it creates a fresh context/module/worker and uploads only the
current CPU representation. Neither operation rewrites RocksDB. CPU routing
and durable mutation remain available under permissive policies.

Runtime partition corruption is not a CUDA fallback condition: the acquired
bundle is poisoned, the request returns a typed routing-image failure, and the
engine rebuilds from confirmed RocksDB records. Device loss is different: the
bundle remains valid, `PreferCuda` may rerun the full request on matching CPU
bytes, and `reinitialize_cuda` creates a fresh context and cache.

Retired bundle count/bytes, publication-backpressure waits/duration, and the
last cleanup failure are health fields. The configured count and byte values
are enforced limits: while an externally leased replacement would exceed one,
publication holds its exclusive boundary until an older lease expires and its
bundle is reaped. They are not alert-only thresholds.
Windows sharing violations retain retryable retirement state. Backups may omit
the routing-image root; a restored pointer whose child is absent is cleared and
rebuilt. `StartupBundlePolicy::RequireValidBundle` can disable automatic startup
rebuild until an operator calls `rebuild_routing_image`.

Explicit engine shutdown closes admission and cancels/drains routes before it
asks the CUDA worker to stop. The worker rejects queued lanes, completes the
active batch through its normal terminal path, joins, and reports queued,
active, rejected, and joined counts. `Drop` uses the same path and never
intentionally detaches the worker. A validated restore rebuilds the omitted
bundle and initializes a fresh context/worker before its route smoke check.
