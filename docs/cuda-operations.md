# CUDA operations

Routing bundles are rebuildable and survive CUDA context loss. Reinitializing CUDA never changes confirmed records or bundle bytes. Operators size complete CUDA topology separately from host identity/directory metadata, the host partition cache, and CPU/device search reservations. A corrupt or unreadable partition is an image failure that triggers a controlled RocksDB rebuild, never an unreachable route.

CUDA is disabled by default. Enable `EngineConfig.cuda`, choose device ordinal
and executor policy, retain display/headroom memory, and independently set
resident topology, partition-cache bytes/slots, pageable staging, search,
concurrency, batch, and algorithm limits. Configuration values are validated
before opening a driver where possible.

Capabilities separate compile-time support from runtime availability and list
the distance-only request subset. Health reports device identity and current
free/total memory, resident counts/bytes, worker state, queued/active lanes,
admission high-water values, device-cache hits/misses/copies/evictions/slot
waits/in-use slots, uploads, launches, failures, fallbacks,
cancellations, and context reinitializations. Request diagnostics identify the
selected executor, reason/fallback, algorithm/delta, lane/batch, bytes,
partition/file/staged/transfer bytes, launches, synchronized duration, phases,
and relaxations. No names,
payloads, complete profiles, or destinations are logged.

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

Retired bundle count/bytes and the last cleanup failure are health fields.
Windows sharing violations retain retryable retirement state. Backups may omit
the routing-image root; a restored pointer whose child is absent is cleared and
rebuilt. `StartupBundlePolicy::RequireValidBundle` can disable automatic startup
rebuild until an operator calls `rebuild_routing_image`.
