# Decision 0011: In-process Rust consumer boundary

Status: accepted

PathHydra's current application boundary is one in-process Rust facade. A
`PathHydra` value owns one `GraphEngine` and its worker lifecycle. Calls are
synchronous except that routing coordination begins with an owned
`RequestHandle`, which can be moved or cloned for cancellation. Routing returns
one complete structured response; correctness does not depend on partial
streaming.

This decision selects process packaging only. Decision 0012 selects the
canonical encoding used for persistence, caller-owned logging, fixtures, and
future bindings. Canonical encoding does not imply a network service.

## Current consumer and deployment evidence

The current consumer is Rust application code that may also invoke BAML in the
same process. There is no separately deployed client, tenancy boundary, remote
caller, or independently managed PathHydra service. Both candidates keep
RocksDB and immutable routing bundles behind Rust APIs, but a hosted process
would additionally require a supervisor, endpoint discovery, authentication or
local peer authorization, transport limits, compatibility negotiation, and
deployment/recovery documentation. None has a current user.

The comparison used these named representative owned-result workloads:

- `route-64-path-16`: 64 ordered destination results, with a 16-edge exact path;
- `hydrate-128-128`: 128 requested nodes and 128 requested edges with current
  confirmed records and evaluation evidence;
- `subgraph-4096-8192`: 4,096 ordered node handles and 8,192 ordered directed
  edge handles.

`boundary_workload_evidence` constructs those current DTOs and measures a
direct in-process owned-value pass against plain compact JSON encode plus
decode. The JSON measurement is only a lower bound for a hosted design: it does
not include a socket, framing, scheduling, copies in a server, authentication,
or process dispatch. The exact command, reference machine, byte sizes, and
timings are recorded with the Plan 09 test evidence; no transport latency claim
is inferred from microbenchmarks.

The 2026-08-17 Windows reference run used the current debug test build and
`cargo test -p pathhydra-api --test lifecycle boundary_workload_evidence --
--nocapture --test-threads=1`. Results are the mean per iteration after
constructing the representative value outside the timed loop:

| Workload | JSON bytes | Direct owned clone | JSON encode + decode lower bound |
| --- | ---: | ---: | ---: |
| `route-64-path-16` | 6,302 | 9.275 us | 366.094 us |
| `hydrate-128-128` | 23,392 | 29.933 us | 1.591 ms |
| `subgraph-4096-8192` | 456,227 | 1.628 ms | 33.104 ms |

The direct measurement is deliberately a full clone, which is more work than
moving the already-owned return value across an in-process call. The JSON
measurement uses `serde_json` directly and omits the canonical boundary scan,
IPC, and server work, so it is a favorable lower bound for the hosted
candidate. These debug measurements are selection evidence, not production
throughput claims.

The representative results show a material incremental serialization and copy
cost for routing, hydration, and especially a large subgraph, while the hosted
candidate supplies no measured current benefit. The smaller in-process boundary
therefore wins. If a future deployment needs process failure isolation or a
non-Rust caller, that work requires a separate reviewed transport plan rather
than hiding HTTP, gRPC, authentication, or version negotiation here.

## Failure, cancellation, shutdown, and recovery

In-process failure isolation is the application's failure isolation. Public
input conversion is bounded and panic-free; internal failures are mapped to a
stable, path-free, payload-free `ApiError`. A facade-owned monotonic allocator
makes request IDs process-local coordination only. A request handle owns the
same cancellation flag registered by the engine, closing the race before
admission and covering queued and running CPU/CUDA execution. Complete responses
win cancellation races and cannot be retroactively changed. IDs are not valid
after process restart and are never graph identity.

Explicit shutdown closes admission, signals active work, drains bounded
operations, joins routing/CUDA workers, flushes the durable store, and reports
typed stage failures. Dropping the last owner performs a best-effort close, but
applications use explicit shutdown when they need a report and durability
boundary. Checkpoint validation and restore remain typed facade operations;
restore targets a fresh destination and rebuilds routing through the production
engine path.

The hosted alternative could isolate a process crash, but it would not remove
the need for the same checkpoint, restore, routing rebuild, cancellation, or
shutdown semantics. Without a current independent consumer, that benefit does
not justify the additional operational and transport failure modes.

## Ownership and access control

Application/BAML code receives owned DTOs only. It receives no RocksDB handle,
column family, routing-image reader, dense node ID, raw hash, CUDA context,
device pointer, worker, cache lease, or local resource path. The facade accepts
operator-selected paths only in its process-local open configuration. Canonical
checkpoint and restore commands use opaque single-component names beneath
those configured roots. Paths are never echoed in errors, health, or encoded
reports. In-process module visibility and Rust ownership keep the durable
catalog and routing images solely owned by the Rust layer; a hosted design
would add peer authorization without reducing the engine's ownership
obligations.

## Target platform and typed payloads

The selected boundary has no IPC runtime dependency and works wherever the Rust
engine and its optional CUDA feature work. Large payloads remain bounded before
conversion or mutation. Structured Rust errors and exact numeric evidence do
not pass through a transport representation on ordinary calls. Canonical DTO
encoding remains available for deterministic persistence, tests, caller-owned
logging, and later BAML bindings.

A future hosted adapter must consume and produce these DTOs, enforce the same or
stricter limits before allocation, and preserve graph semantics. It must not
turn request IDs into durable identity or expose engine/store implementation
handles.

## Current-state hydration

Routing evidence is point-in-time evidence from the acquired routing image.
Hydration intentionally resolves current confirmed records when called. If a
node or edge was deleted after routing, hydration reports every unavailable
handle; it does not silently return historical records. No graph revision,
pinned image, historical snapshot retention, or migration API is introduced.
