# Plan 05: Production CUDA Routing Backend

## Outcome

Add the first real accelerator implementation to PathHydra. At completion, the
Rust engine can keep the current routing topology resident on a supported
NVIDIA GPU, execute exact context-adjusted distance routing there, batch and
isolate concurrent searches, enforce device-memory admission, coordinate GPU
residency with confirmed graph publication, recover or fall back after device
failure, and prove every supported GPU result against the CPU reference engine.

This is not a demonstration kernel. The completed slice includes the kernel
build path, the narrowly audited CUDA launch boundary, exact GPU algorithms,
the host scheduler, engine integration, cancellation, health, diagnostics,
correctness fixtures, adversarial comparison, and named-hardware benchmarks.

The initial production boundary is a fully resident routing image. Graphs whose
topology plus safely reserved search state cannot fit on the selected device
remain on CPU. Partitioned and out-of-core routing is a separate later slice.

## Local evidence behind this plan

The development machine currently provides:

- NVIDIA GeForce RTX 3080;
- 10,240 MiB reported device memory;
- Windows WDDM driver 610.88;
- CUDA driver capability reported as 13.3;
- `nvcuda.dll` from the installed display driver;
- no `nvcc`, CUDA toolkit path, NVRTC, or nvJitLink installation;
- Rust 1.95 with the `nvptx64-nvidia-cuda` target known to `rustc`, but no
  installed NVPTX target library for the active toolchain.

The RTX 3080 is an Ampere `sm_86` target. The CUDA driver can load PTX and JIT
it to device code without the application using the CUDA runtime compiler.
Rust documents `nvptx64-nvidia-cuda` as a supported target for Rust-authored
`no_std` kernels. These facts support a driver-only runtime and a separate
pinned Rust-to-PTX build step.

The plan does not use cuda-oxide for this slice because its current installation
guide explicitly supports Linux rather than Windows. It does not use CUDA C++
or checked-in handwritten PTX because PathHydra-owned engine and kernel logic
remains Rust.

## Why this is one coherent slice

A GPU shortest-path kernel cannot be correct in isolation. Its semantics depend
on all of these connected contracts:

- the exact arrays and numeric policy published by the CPU routing image;
- the lifetime of topology resident on a device while graph mutations publish
  replacements;
- worst-case per-search device allocations;
- request eligibility, executor selection, and CPU fallback;
- separate state for concurrent origins and profiles;
- completion detection, cancellation, and device errors;
- output translation into the existing per-destination result contract;
- continuous comparison with the CPU oracle;
- measurements that determine whether GPU dispatch is beneficial.

Leaving any one to callers would violate the Rust engine boundary established
by Plan 04. They therefore belong in the same accelerator phase even though
the implementation proceeds through independently testable stages.

## Explicit non-goals

Do not implement in this slice:

- AMD, Intel, Apple, Vulkan, DirectX, WebGPU, or another accelerator backend;
- a vendor-neutral GPU trait or cross-vendor abstraction before a second real
  backend exists;
- topology partitioning, host-RAM partition caches, DirectStorage, or routing
  when the complete topology cannot be resident safely;
- a durable routing-image file, memory-mapped image, migration, compatibility
  reader, graph revision counter, or caller-pinned image version;
- GPU payload hydration, GPU subgraph construction, or payload transfer to the
  device;
- approximate routing, blended profiles, negative effective weights, relaxed
  numeric comparison, or a changed CPU reference result;
- GPU path reconstruction in the first supported request set;
- finite examined-edge budgets on GPU unless the implementation can preserve
  their declared accounting exactly;
- preempting a running CUDA kernel or claiming immediate cancellation;
- multi-GPU partitioning, peer-to-peer topology sharing, MIG support, or remote
  devices;
- CUDA Graph capture, unified memory, stream-ordered allocation, or weight
  materialization unless measurements in this plan justify a concrete use;
- BAML, bindings, network transport, hosted telemetry, or cloud execution;
- GitHub Actions workflows.

Unsupported request shapes and device states route to CPU under permissive
executor policies or return a typed refusal under a caller-required CUDA
policy. They never silently change routing semantics.

## 1. Freeze the complete CPU oracle

Before accelerator changes, run:

```powershell
cargo fmt --all --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Record the passing test count and preserve every Plan 00-04 fixture. The CPU
numeric policy, stable predecessor policy, missing/unreachable/incomplete
states, candidate isolation, confirmed mutation publication, hydration, and
subgraph behavior are the semantic oracle.

Add a small checked-in corpus of deterministic routing cases before GPU code:

- input confirmed graph records;
- canonical relation profile;
- origin and destination order;
- CPU response without wall-clock diagnostics;
- optional exact path evidence;
- a human-readable reason the result is interesting.

The corpus is test data in the current API shape, not a versioned compatibility
format. Update it in place before release if the unreleased API changes.

## 2. Make three accelerator decisions explicit

Add three accepted decision records before the production implementation.

### Decision 0004: Rust PTX and CUDA Driver boundary

Record:

- Rust-authored `no_std` kernels compile to PTX through the official
  `nvptx64-nvidia-cuda` target;
- one separately pinned nightly toolchain builds kernels while the host
  workspace remains on its pinned stable toolchain;
- the host uses the dynamically loaded CUDA Driver API rather than CUDA runtime
  or NVRTC compilation;
- runtime deployment requires a compatible NVIDIA driver, not `nvcc`;
- the initial compiled target is `sm_86`, matching the local RTX 3080;
- PTX is embedded in the CUDA crate at build time and loaded from memory;
- no generated PTX is treated as durable graph data or checked in as a source
  of truth;
- unsupported driver, PTX, architecture, or device conditions are typed CUDA
  capability failures and leave CPU routing available.

### Decision 0005: Audited unsafe CUDA boundary

The current workspace forbids unsafe code. CUDA kernel launch and low-level
device intrinsics cannot honestly be implemented without a trusted boundary.
Record the narrow exception:

- existing core, store, routing, subgraph, and engine crates continue to inherit
  `unsafe_code = "forbid"`;
- only `pathhydra-cuda`'s private launch module and the separately compiled
  device-kernel source may contain unsafe Rust;
- the CUDA crate does not inherit the workspace-wide unsafe lint; instead it
  denies unsafe code crate-wide and locally allows it only in named modules;
- `unsafe_op_in_unsafe_fn` and Clippy's undocumented-unsafe-block lint are
  denied;
- every unsafe block states its buffer length, alignment, initialization,
  kernel ABI, context, stream-order, and lifetime obligations;
- the public CUDA API is safe and synchronous with respect to borrowed host
  values unless an owned operation type proves asynchronous lifetimes;
- raw driver handles, device pointers, and unvalidated launch builders are not
  public;
- a kernel ABI mismatch is a correctness defect, not a recoverable request
  condition.

This decision changes no other crate's safety policy and does not permit unsafe
code for optimization outside the audited boundary.

### Decision 0006: Exact CUDA algorithms and eligibility

Record two concrete algorithms:

1. a parallel frontier label-correcting implementation as the simple exact GPU
   reference; and
2. delta-stepping as the optimized exact candidate.

Both use the Plan 03 binary32-operand/separate-binary64 arithmetic and atomic
minimum over non-negative finite binary64 distances. The label-correcting
engine remains available as a GPU diagnostic comparator even if delta-stepping
becomes the selected production algorithm.

The first CUDA-eligible request set is:

- one origin and any number of destinations;
- complete explicit relation profile;
- distance-only (`return_paths == false`);
- unlimited examined-edge budget;
- current numeric and tie policy;
- a fully resident current CUDA topology;
- counts and memory sizes representable by the kernel ABI;
- device and scheduler healthy;
- cancellation accepted at documented kernel boundaries.

Paths and finite deterministic edge budgets remain on CPU in this plan. Equal
path ties cannot change a distance-only response. The engine still identifies
the configured tie policy in the response.

## 3. Add a feature-gated CUDA crate and build path

Add one host crate:

```text
crates/pathhydra-cuda/
  Cargo.toml
  build.rs
  kernel/
    lib.rs
    arithmetic.rs
    frontier.rs
    delta.rs
  src/
    abi.rs
    algorithm.rs
    context.rs
    device.rs
    error.rs
    launch.rs
    lib.rs
    memory.rs
    resident.rs
    scheduler.rs
  tests/
    abi.rs
    ptx.rs
    device_smoke.rs
    agreement.rs
    batching.rs
    recovery.rs
```

Add `pathhydra-cuda` as an optional dependency of `pathhydra-engine` behind a
`cuda` feature. The crate itself is a normal workspace member, but its CUDA
driver dependency and kernel build are feature-gated so ordinary CPU builds and
tests work on machines without NVIDIA software.

Use the maintained `cudarc` driver wrapper only after a minimal spike proves
the selected pinned release can:

- dynamically open the installed CUDA driver on Windows;
- query device 0 and compute capability;
- allocate and copy typed buffers;
- load embedded Rust-generated PTX;
- retrieve a named kernel;
- launch through the audited private unsafe boundary;
- synchronize and return driver errors;
- build without the CUDA toolkit present.

Start from the currently documented `cudarc` 0.19.8 line with dynamic loading
and explicit CUDA 13.3 bindings. Pin the exact proven version and minimal feature
set in `Cargo.lock`. Record its MIT/Apache-2.0 licensing and do not enable NVRTC,
cuBLAS, cuGraph, or other unused libraries.

Add a CUDA toolchain descriptor and a PowerShell verification command. The
kernel build must use an exact nightly release plus only the components it
needs, such as the NVPTX target library or `rust-src`, `llvm-tools`, and
`llvm-bitcode-linker` according to the proven path. Do not change the host
workspace to nightly.

`build.rs` runs the pinned toolchain only when the CUDA feature is enabled. It
compiles `kernel/lib.rs` as a `no_std` PTX module for `sm_86`, writes the result
under Cargo `OUT_DIR`, and makes the host crate embed those exact bytes. Use a
separate target/output directory so nested compilation cannot deadlock or
overwrite host artifacts. Emit precise rerun directives for kernel sources and
toolchain metadata.

The build fails visibly when CUDA was explicitly enabled but the pinned kernel
toolchain or components are absent. It must never download tools implicitly
inside a Cargo build. Installation is an explicit prerequisite command that
requires user approval when implementation begins.

## 4. Prove the toolchain with a non-routing kernel

Before graph kernels, implement one checked smoke kernel that:

- reads a binary32 input array;
- converts each value to binary64;
- performs one separate multiplication and addition;
- writes binary64 output;
- is launched with bounds validated by the host wrapper;
- round-trips through real device memory on the RTX 3080.

Validate the generated PTX text before loading it:

- expected `.target sm_86` or compatible target declaration;
- supported `.version` for the installed driver;
- expected visible kernel entry names;
- binary64 multiply and add instructions remain separate;
- no fused multiply-add instruction replaces the declared arithmetic;
- no unexpected external function or runtime dependency;
- pointer width and parameter sizes match the host ABI.

Run the same numeric boundary table as Decision 0002: positive zero,
subnormal operands, one, maximum finite multiplier, and checked finite results.
Compare result bits, not tolerances.

Stop the routing implementation if the smoke kernel cannot produce bit-exact
agreement or if the host launch cannot satisfy the audited safety contract.
Do not compensate with approximate equality or CUDA C.

## 5. Define one explicit host/device ABI

Create fixed-width `#[repr(C)]` kernel parameter and diagnostic structs with no
Rust references, `usize`, enums of unspecified layout, booleans, or private
newtype layouts crossing the device boundary.

Use only explicit scalars and device addresses represented by the driver
wrapper. At minimum define:

- `u32` dense node IDs;
- `u64` CSR offsets;
- `u32` dense relation indexes;
- canonical `u32` base-weight and multiplier bits;
- `u64` binary64 distance bits;
- `u32` frontier node entries and counts;
- fixed-width lane and generation values;
- fixed-width error/status words;
- checked `u64` edge-examination and relaxation counters.

Duplicate the minimum ABI declarations in host and device modules only when the
direct `rustc` kernel build cannot share a crate safely. If duplicated, add
host compile-time size/alignment assertions, device compile-time assertions,
and a real-kernel echo test for every struct. Generate neither side from C
headers.

Every count conversion is checked before launch. Kernel parameters carry both
pointer and logical length where bounds depend on a slice. Zero-length buffers
use a documented non-dereferenced representation accepted by the wrapper.

## 6. Make the routing image GPU-addressable without changing identity

Extend the current in-memory `RoutingImage` with a dense relation index per
adjacency. The compiler already sorts confirmed relation IDs; map each stable
`RelationId` to one `u32` profile index and store:

```text
relation_indexes[adjacency] -> u32
```

Keep stable relation IDs in the CPU image for returned evidence. The dense
index is a rebuildable routing field, never durable relation identity.

Expose a read-only `RoutingImageArrays` view required by the concrete CUDA
crate:

```text
offsets
destinations
relation indexes
base weights
dense-to-external node IDs (host result mapping only)
confirmed relation IDs (host profile packing only)
```

Do not expose mutable arrays or raw RocksDB state. Update manifest widths and
byte accounting in place; there is no compatibility layer.

Add a GPU-topology manifest computed before device allocation:

```text
offset bytes
+ adjacency_count * (destination bytes
                    + relation-index bytes
                    + base-weight bytes)
```

Edge IDs and payloads are not transferred for distance-only CUDA execution.
Search state, profile, destinations, queues, counters, and allocator headroom
are reported separately.

## 7. Probe concrete CUDA capabilities at runtime

Create a concrete `CudaDeviceCapabilities` value by querying the driver, not by
assuming the local model name. Record at least:

- driver version;
- device ordinal and name;
- compute capability;
- total and currently free memory;
- maximum threads and block dimensions;
- maximum grid dimensions;
- multiprocessor count;
- warp size;
- concurrent-kernel support;
- stream priority range;
- unified addressing status;
- whether required 64-bit global atomic operations are supported;
- the kernel PTX target and numeric policy IDs.

Reject devices below the proven minimum compute capability, devices that cannot
load the embedded PTX, and devices missing required atomic or address-width
behavior. A second display adapter does not become a backend automatically.

Capability probing is read-only and never creates confirmed graph state. Driver
load failure is `CUDA unavailable`, not an engine-open failure under CPU-capable
policies.

## 8. Implement persistent CUDA topology residency

Define `CudaResidentImage` as an immutable owner of:

- the CUDA context/device association;
- device offsets;
- device destinations;
- device relation indexes;
- device base-weight bits;
- the matching CPU `Arc<RoutingImage>`;
- topology counts and exact allocated bytes;
- kernel module/function handles;
- publication and residency diagnostics.

Upload topology once per confirmed CPU image, synchronize the upload, validate
device-side counts with a small checksum/count kernel, and only then make the
resident image eligible for routes.

Replace the engine's published value conceptually with one immutable execution
snapshot:

```text
PublishedExecutionImage
  cpu: Arc<RoutingImage>
  cuda: Option<Arc<CudaResidentImage>>
  cuda_unavailable_reason: Option<...>
```

Confirmed mutation publication remains CPU-authoritative:

1. commit the durable mutation;
2. compile and validate the replacement CPU image;
3. attempt complete CUDA residency while new image acquisition is blocked;
4. publish CPU plus the matching resident CUDA image in one assignment; or
5. publish the CPU image with typed CUDA unavailability.

A CUDA upload failure never makes correct CPU routing unavailable. Never pair
the new CPU image with old device topology. Already-acquired requests may keep
the old execution snapshot and resident buffers alive until their streams are
complete.

Building a replacement may temporarily require both old and new topology. If
safe double residency cannot be reserved, publish the new CPU image without
CUDA rather than evicting buffers still in use. Expose an explicit
`rebuild_cuda_residency` operation after old references drain; do not add a
background retry thread in this slice.

## 9. Reserve device memory before admitting work

Extend `EngineConfig` with a concrete CUDA section:

```text
enabled / disabled
device ordinal
executor policy
maximum topology bytes
minimum free-memory headroom
maximum concurrent CUDA searches
maximum CUDA batch lanes
maximum reserved CUDA search bytes
batch collection delay
algorithm selection or automatic mode
delta-stepping tuning candidates
```

Validate configuration without opening a device where possible. Device-derived
limits are checked during CUDA initialization.

Calculate worst-case per-lane bytes before admission. The frontier reference
must account for:

- binary64 distance for every node;
- two dense-node frontier queues with capacity `node_count` each;
- current/next membership or generation arrays;
- packed relation-use data;
- destination dense IDs and output distances/states;
- cancellation/status/counter storage;
- all scan or compaction scratch actually used.

Delta-stepping additionally accounts for bucket membership, current-bucket
closure, settled-in-bucket nodes, heavy-edge work, minimum-bucket reduction,
generation/reset state, and algorithm diagnostics.

Reserve immutable topology first. Retain configured and driver-reported
headroom for WDDM/display and unrelated device use. Then reserve complete
per-lane state for every admitted search. Never depend on average frontier,
bucket, or destination sizes.

Use a concrete CUDA admission controller, not a vendor-neutral trait. It can
refuse work or cause permissive executor policy to choose CPU. A required-CUDA
request gets a typed resource refusal. Reservations are RAII values released
only after stream completion makes buffers reusable.

## 10. Implement bit-exact atomic distance relaxation

Represent internal tentative distances as canonical non-negative binary64 bit
patterns. Positive finite binary64 bit ordering matches numeric ordering, so a
64-bit integer compare-and-swap loop can implement atomic minimum when inputs
are proven non-negative and non-NaN.

Implement and test device helpers for:

- binary32-bit to binary64 conversion;
- separate effective-weight multiplication;
- separate path-distance addition;
- finite/non-negative validation;
- atomic minimum returning whether the value improved;
- checked counter increment;
- first-error publication into one status word;
- generation-stamped membership update.

Infinity may be used only as internal unvisited search state. It never crosses
the public result boundary as logical distance. Device code encountering NaN,
negative, non-finite candidate arithmetic, counter overflow, out-of-range
index, or impossible relation index records a typed kernel failure and stops
publishing usable output.

Compare the atomic helper under intense collisions against a single-thread CPU
minimum. Include exact-bit ties, descending values, zero, subnormals, and large
finite values. Run memory checking before trusting graph results.

## 11. Build the exact frontier GPU reference engine

Implement a parallel label-correcting search as the first complete CUDA
algorithm. It is the GPU correctness baseline, not the CPU oracle and not
automatically the fastest backend.

Per lane, maintain:

- origin and destination mapping;
- packed immutable relation profile;
- binary64 tentative distances;
- current and next frontier queues;
- generation or membership state that enqueues each node at most once per
  frontier generation;
- current/next counts;
- cancellation and error status;
- examined-edge, relaxation-attempt, successful-update, iteration, and frontier
  high-water counters.

Initialize origin distance to zero and every other distance to internal
infinity. Each iteration:

1. expands every current frontier node;
2. assigns one CUDA block or another measured work unit to its bounded outgoing
   range;
3. loads relation use through the dense relation index;
4. skips disabled edges;
5. computes exact effective and candidate values;
6. atomically lowers the destination distance;
7. enqueues an improved destination into the next frontier once;
8. records errors and counters;
9. swaps frontiers after stream synchronization or an equivalent proven
   completion boundary.

The search is exact only after the frontier becomes empty. First discovery is
never final. Zero-weight cycles terminate because equal candidates do not
enqueue, only strict distance improvements do. Self-edges and parallel edges
are ordinary directed relaxations.

After convergence, copy only requested destination distances and status plus
diagnostic counters to host. Map internal infinity to `Unreachable`; preserve
missing destination positions handled before launch; preserve origin distance
zero. The host validates every returned finite value before constructing the
existing `RoutingResponse`.

Cancellation is observed between kernel iterations. Because the frontier
reference cannot prove arbitrary destination finality before convergence, a
cancelled result reports origin-at-zero as exact, missing destinations as
missing, and other present destinations as incomplete unless a stronger proof
is implemented and tested. Never label a discovered value exact merely because
it currently looks smallest.

## 12. Implement exact delta-stepping as the optimized candidate

After the frontier reference agrees with CPU on the complete fixture corpus,
implement delta-stepping over the same resident topology and arithmetic.

Use a positive finite binary64 delta that affects scheduling only, never edge
cost or result meaning. Separate light and heavy edge processing:

- light edges have `effective_weight <= delta`;
- heavy edges have `effective_weight > delta`;
- repeatedly close the current bucket under light-edge relaxation;
- retain every node removed from that bucket;
- relax heavy edges from the retained set after light closure;
- advance only after the current bucket is technically complete;
- finalize destination distances only at a proof-valid bucket boundary.

Use sparse bucket state rather than allocating an array up to the maximum
possible bucket number. Define and check how binary64 distances map to `u64`
bucket indexes. If a valid candidate cannot be represented for the selected
delta, record a typed algorithm-ineligible condition and rerun through the
frontier GPU reference or CPU according to executor policy. Never clamp,
approximate, or wrap a bucket index.

Generation-stamped arrays may avoid clearing all node state between buckets or
reused lanes. Define wrap behavior: before a generation counter can wrap, fully
clear the affected state and restart from a known generation. Test the
near-wrap path through constructed internal state rather than billions of
requests.

Delta selection begins as an explicit configuration. The benchmark phase may
add an automatic selector from a small validated set based on current topology
and profile weight statistics. It must never select zero, NaN, infinity, or a
delta whose bucket representation is invalid.

Keep the frontier algorithm after delta-stepping exists. It is useful for:

- GPU-to-GPU debugging;
- profiles outside a delta candidate's representable range;
- detecting optimized-kernel regressions;
- establishing whether a failure is algorithmic or driver/runtime related.

## 13. Preserve the existing request and result contract

Do not create a CUDA-specific public routing response. `GraphEngine::route`
continues returning the same exact destination states, canonical profile,
numeric policy, tie policy, and structured diagnostics.

CUDA host preparation performs the same validation order as CPU routing:

1. resolve origin against the matching CPU image;
2. validate and pack the complete profile;
3. map destinations and preserve original positions/duplicates;
4. recognize origin destinations at exact zero;
5. decide CUDA eligibility;
6. reserve a lane;
7. submit only validated fixed-width device input.

Duplicate destinations share GPU output work and expand back to caller order.
Empty destination requests require no kernel launch after validation. Missing
destinations never consume device output slots. Disabled relation kinds remain
distinct from enabled zero multipliers.

CUDA exactness is per destination:

- `Exact` only after algorithmic proof;
- `Unreachable` only after complete reachable search;
- `MissingNode` from the matching CPU image;
- `Incomplete` after cancellation or a controlled resource stop;
- whole-request typed error or CPU fallback after device/kernel failure.

## 14. Add a concrete CUDA worker and synchronous batching scheduler

Add one engine-owned CUDA worker thread when CUDA initializes successfully.
It owns driver coordination, scheduler queues, reusable streams, and buffer
pools. It does not own durable graph state.

The existing synchronous `GraphEngine::route` API remains synchronous. An
eligible call packages a job, sends it through a standard-library channel, and
waits for its own result. No async runtime or public future is introduced.

Each job carries:

- request ID and cancellation flag;
- one acquired `Arc<PublishedExecutionImage>`;
- one matching resident CUDA image;
- packed profile;
- canonical unique destinations and original-position map;
- executor/algorithm policy;
- reserved lane bytes;
- a one-shot response sender.

Batch only jobs that use the exact same resident image, numeric policy, kernel
ABI, and selected algorithm. Origins, destination sets, and profiles remain
independent lane data. Never blend profiles or merge searches merely because
they share an image.

Collect up to the configured lane count for at most the configured batch delay.
Zero delay is valid. Batch delay is an admission/latency policy reported in
diagnostics, not part of graph semantics.

Implement lane-indexed buffers and kernels so every lane has independent:

- distance array;
- two frontiers or bucket state;
- profile reference/range;
- cancellation and error status;
- unresolved destination state;
- counters and completion state.

A completed or failed lane cannot finalize or clear another lane. A short lane
may sit idle while broader lanes complete the submitted batch; recycling a lane
into an already running batch is outside this slice. Later jobs may use other
streams only after measurements show useful concurrent execution and memory
admission remains exact.

On worker shutdown, reject queued jobs with a typed error, synchronize all
submitted streams, return or free buffers, unload modules, and destroy context
resources in a documented order.

## 15. Integrate executor policy into `GraphEngine`

Replace the CPU-only executor fact with concrete policies:

```text
CpuOnly
PreferCuda
RequireCuda
Auto
```

Do not add a generic `GpuBackend` trait. The engine has exactly two concrete
executors: CPU reference and NVIDIA CUDA.

Policy behavior:

- `CpuOnly`: never initializes or submits CUDA work for the request;
- `PreferCuda`: use CUDA when eligible and healthy, otherwise use CPU and record
  the fallback reason;
- `RequireCuda`: return a typed refusal when request shape, residency,
  admission, driver, or algorithm cannot use CUDA;
- `Auto`: use the measured selection table produced later in this plan; choose
  CPU for small or unsupported work and CUDA only where current evidence says
  it is appropriate.

Executor selection occurs after request validation and image acquisition but
before executor-specific allocation. A mutation published after acquisition
does not move the request to a different image.

If CUDA fails before changing any externally visible request state, permissive
policies may rerun the complete request on the acquired matching CPU image.
Diagnostics record the attempted CUDA executor, device/algorithm failure, and
CPU fallback. `RequireCuda` returns the CUDA failure. Never splice partial GPU
distances into a CPU response.

Path-returning and finite-edge-budget requests use CPU under permissive policy
and are typed ineligible under `RequireCuda` in this plan.

## 16. Make cancellation and budgets honest on GPU

Reuse Plan 04 `RequestId` and cancellation registration. Queued CUDA jobs check
cancellation before admission, before batch assembly, before every launch, and
after every synchronization. Running kernels are not claimed to be preemptible.

For the frontier algorithm, cancellation is observed between complete frontier
iterations. For delta-stepping it is observed between closure launches and
bucket phases at points where finalized destinations remain provable.

If cancellation occurs while one kernel is running:

- do not free or reuse its buffers;
- wait for the stream to reach a safe boundary;
- suppress further launches for that lane;
- return only destination states justified at that boundary;
- release admission after synchronization.

The current deterministic `ExaminedEdges` budget remains CPU-only. Parallel
atomic work claiming a global count in scheduler-dependent order would change
its current meaning. Add no CUDA approximation. A later plan may define an
additional GPU work budget with distinct semantics, but it cannot silently
reinterpret the existing request field.

## 17. Keep path reconstruction exact through explicit CPU policy

CUDA distance-only eligibility does not weaken optional paths. For
`return_paths == true`:

- `CpuOnly`, `PreferCuda`, and `Auto` execute the existing CPU reference route;
- `RequireCuda` returns `PathsUnsupportedByCuda` before device allocation.

Do not run CUDA distances and then attach a path from unrelated current graph
state. Do not infer the stable predecessor tree from parallel relaxation order.

The benchmark harness may measure a diagnostic two-pass experiment—CUDA
distance followed by a CPU constrained reconstruction or full CPU rerun—but it
does not enter production selection unless it exactly reproduces the declared
stable predecessor policy on zero cycles and equal paths and demonstrates a
current use.

## 18. Coordinate CUDA residency with mutation and recovery

Extend Plan 04 publication tests to cover device resources:

- provisional insertion does not upload topology;
- every successful confirmed mutation produces a new CPU image and attempts a
  matching new CUDA resident image;
- new requests never acquire CUDA topology from an older confirmed graph;
- already-acquired CUDA requests may finish on their old resident image;
- old device buffers remain alive until all streams using them complete;
- an upload or allocation failure publishes current CPU-only execution state;
- explicit residency rebuild can restore CUDA after memory becomes available;
- CPU routing and durable mutation remain available through every CUDA failure.

Classify CUDA failures:

- unavailable driver or device;
- incompatible compute capability/PTX;
- context creation or module load failure;
- topology allocation/upload/validation failure;
- per-search admission refusal;
- kernel launch or synchronization failure;
- device-side invariant/error status;
- device lost, reset, or context poisoned;
- worker channel or thread failure.

A context-poisoning failure stops new CUDA admission, drains or fails queued
jobs, and marks CUDA unhealthy. Do not repeatedly launch into a failed context.
Expose an explicit `reinitialize_cuda` operation that creates a fresh context,
reloads kernels, and uploads only the current CPU image. It does not reopen or
rewrite RocksDB.

## 19. Extend capabilities, health, and diagnostics

`EngineCapabilities` reports build and runtime facts separately:

- CUDA support compiled in or absent;
- CUDA driver/device available or unavailable;
- device identity and compute capability;
- supported request shapes;
- supported CUDA algorithms;
- paths on CUDA unsupported;
- finite examined-edge budgets on CUDA unsupported;
- batching and cancellation-boundary behavior;
- topology full-residency requirement;
- numeric and tie policy IDs.

`EngineHealth` adds:

- CUDA initialized/healthy/degraded/unavailable;
- typed last CUDA failure;
- resident topology counts and bytes;
- current free/total device memory at health query when available;
- reserved topology and search bytes;
- active/queued/batched CUDA lanes;
- worker status;
- module/PTX target;
- cumulative uploads, upload failures, launches, launch failures, fallbacks,
  cancellations, and context reinitializations.

Per-request diagnostics add:

- selected executor and selection reason;
- attempted CUDA executor and fallback reason;
- device ordinal/name;
- CUDA algorithm and delta when applicable;
- queue and batch collection duration;
- batch width and lane index;
- topology/search reserved bytes;
- host-to-device and device-to-host bytes/durations;
- kernel launch count and total synchronized execution duration;
- frontier iterations or bucket phases;
- examined edges, relaxation attempts/updates, and frontier high-water mark;
- cancellation observation boundary;
- PTX JIT/module load time only when incurred by that lifecycle.

Do not log node payloads, names, complete profiles, or destination contents.
Structured results remain sufficient for local inspection without a telemetry
service.

## 20. Prove CUDA correctness against CPU continuously

Every CUDA correctness test runs the same request against the acquired CPU
image and compares:

- destination order and duplicates;
- exact/missing/unreachable/incomplete state where the executor supports the
  same stopping condition;
- exact binary64 distance bits;
- canonical profile, numeric policy, and tie policy;
- origin-at-zero behavior;
- no path for distance-only requests;
- error classification for unsupported requests.

Run the complete Plan 03 deterministic graph corpus through both frontier and
delta algorithms. Include:

- one edge and multi-hop winners;
- directed reverse-unreachable cases;
- context profiles that change winners;
- enabled zero versus disabled relations;
- parallel edges;
- self-edges and zero-weight cycles;
- equal-cost paths;
- missing and duplicate destinations;
- isolated and unreachable regions;
- maximum and subnormal multipliers;
- arithmetic whose exact distance differs by one binary64 bit under changed
  operation order;
- empty graph and empty destination request.

GPU tests skip with an explicit reason only when the CUDA feature or device is
unavailable. On the named local RTX 3080 environment, the accelerator agreement
suite is an authoritative completion check, not an optional ignored result.

## 21. Add generated and adversarial agreement tests

Generate deterministic small directed graphs without adding a property-test
dependency. Vary:

- node and edge counts;
- sparse and dense connectivity;
- parallel/self/zero edges;
- relation counts and disabled subsets;
- origins and destination multisets;
- binary32 base-weight and multiplier bit patterns from a curated boundary
  table.

For each case, compare CPU, frontier CUDA, and delta CUDA. When a failure occurs,
print a minimal reproducible graph/profile/request fixture without payloads.

Add adversarial fixed graphs:

- a long chain requiring many frontier iterations;
- a wide shallow star;
- one extreme high-degree source;
- a dense strongly connected component;
- repeated late improvements;
- all-zero reachable component;
- alternating light/heavy edges around delta;
- distances exactly on bucket boundaries;
- bucket-index representability failure;
- frontier capacity at node count;
- multiple lanes with radically different completion times;
- repeated generation reuse and forced near-wrap reset;
- cancellation at every host-visible phase;
- topology replacement while old CUDA lanes run.

Run the CUDA tests under NVIDIA Compute Sanitizer before completion. The toolkit
is not a runtime dependency, but installing or otherwise making the sanitizer
available is a development prerequisite requiring explicit approval. Record
the tool and driver versions used. Any invalid access, race, uninitialized read,
or API error fails the accelerator slice even when numerical outputs happen to
match.

## 22. Build a reproducible local benchmark harness

Add a local benchmark binary rather than a hosted service:

```text
crates/pathhydra-bench/
  Cargo.toml
  src/
    main.rs
    fixtures.rs
    report.rs
    routing.rs
```

The harness generates deterministic named workloads or loads current test
fixtures. It validates correctness before recording timing. It outputs a simple
documented CSV or line-oriented record using the standard library unless a
serialization dependency has an immediate justified use.

Every report records:

- CPU model/threads and memory when discoverable;
- GPU name, compute capability, memory, and driver;
- Rust host and CUDA kernel toolchains;
- kernel PTX target and algorithm parameters;
- build profile;
- node, relation, and adjacency counts;
- topology host/device bytes and upload time;
- request destination count and profile characteristics;
- executor and algorithm;
- warm/cold module state;
- queue/batch width;
- iterations/buckets, examined edges, and relaxations;
- time to completion;
- transfer and kernel durations;
- device-memory high-water estimate;
- correctness outcome before timing.

Measure at least:

- near, far, and unreachable destinations;
- one versus many destinations;
- narrow, broad, dense, and high-degree graphs;
- long-chain worst case for frontier iteration;
- zero-weight closure;
- profiles with mostly disabled, zero, light, and heavy relations;
- single request latency;
- concurrent throughput at lane counts 1, 2, 4, 8, and the admitted maximum;
- topology upload/publication;
- cold PTX JIT separately from warm routing;
- CPU reference, CUDA frontier, and CUDA delta on identical inputs.

Warm up explicitly. Do not hide PTX JIT or upload time inside steady-state
routing, and do not exclude it from cold-start reporting.

## 23. Use measurements to select—not invent—the automatic policy

Benchmark the local RTX 3080 first. Sweep:

- block sizes from a small validated set;
- frontier work mapping for low/high-degree nodes;
- CUDA batch widths;
- delta values or selection candidates;
- synchronous versus pinned asynchronous transfers if the driver wrapper
  supports pinned buffers safely;
- frontier versus delta algorithm;
- optional inline effective-weight computation versus temporary materialized
  weights only if repeated-profile measurements justify the extra edge pass and
  memory.

Do not tune against one favorable graph. Report correctness failure before any
timing from that run.

Create `docs/performance/rtx-3080-cuda-baseline.md` with named hardware,
software versions, workload definitions, raw summary tables, and conclusions.
The `Auto` executor policy is derived from conservative measured crossover
regions. It may select CPU for most small requests or even all tested requests
if the first GPU algorithms are slower. Exactness is the product contract;
speedup claims require evidence.

Do not encode hardware benchmark thresholds as universal API guarantees.
Keep them in engine configuration/default policy data that is inspectable and
can be updated in place before release.

## 24. Test batching, admission, failure, and recovery

Add deterministic host tests with controlled barriers rather than sleeps:

- two requests with the same profile remain separate searches;
- two requests with different profiles cannot share packed relation state;
- different origins cannot finalize one another's destinations;
- duplicate request IDs remain rejected before CUDA enqueue;
- lane count and byte limits refuse work without leaking reservations;
- queued cancellation avoids kernel submission;
- running cancellation waits for a safe boundary before buffer reuse;
- one lane failure does not publish another lane's output as failed or exact;
- completed lanes are fully reset before reuse;
- forced generation wrap performs a full clear;
- mutation publishes a new resident image while old lanes finish;
- new jobs cannot batch across old and new images;
- topology upload failure leaves current CPU routing healthy;
- simulated and real driver errors poison CUDA health once and trigger fallback;
- explicit reinitialization restores residency and agreement;
- worker shutdown drains/fails every job and frees reservations;
- engine reopen reconstructs CUDA residency from RocksDB through the current CPU
  image.

Use real driver/device tests for allocation, upload, module load, launch,
synchronization, cancellation boundary, and context reinitialization. Use
pure-host unit tests for scheduler grouping, accounting, selection, and error
state machines so most behavior remains testable without a GPU.

## 25. Document build, runtime, safety, and operations

Add:

- `docs/cuda-build.md`: pinned kernel toolchain, feature flags, driver/toolkit
  distinction, PTX generation, commands, troubleshooting, and clean rebuild;
- `docs/cuda-safety.md`: every unsafe module, ABI, buffer, launch, stream, and
  kernel invariant;
- `docs/cuda-routing.md`: resident arrays, eligibility, frontier and delta
  algorithms, completion proofs, cancellation, batching, and fallback;
- `docs/cuda-operations.md`: capability/health interpretation, device loss,
  reinitialization, memory pressure, and CPU-only operation;
- `docs/performance/rtx-3080-cuda-baseline.md`: measured evidence.

Update `docs/routing-image.md` with relation indexes and device topology bytes.
Update `docs/cpu-engine.md` into an engine-executor reference without obscuring
that CPU remains the oracle and path-capable fallback.

Add rustdoc examples for:

1. opening with CUDA disabled;
2. opening with `PreferCuda` and inspecting capability;
3. an eligible exact distance-only request using CUDA;
4. a path request falling back to CPU;
5. required CUDA returning typed ineligibility;
6. CUDA device loss followed by CPU fallback and explicit reinitialization.

Update README status only after implementation. State the actual supported GPU
vendor, request subset, full-residency requirement, CPU fallback, and local
build prerequisites. Do not say "GPU accelerated" without the benchmark and
agreement evidence.

## 26. Verification commands

Keep the ordinary CPU-capable checks authoritative on every development
machine:

```powershell
cargo fmt --all --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Add documented CUDA-capable checks equivalent to:

```powershell
# Verify the explicitly installed pinned kernel toolchain.
powershell -File Scripts/verify-cuda-toolchain.ps1

# Build host plus Rust PTX kernel with the CUDA feature.
cargo build -p pathhydra-engine --features cuda

# Run real-device agreement and engine integration tests.
cargo test -p pathhydra-cuda --features cuda
cargo test -p pathhydra-engine --features cuda

# Run the local correctness-first benchmark suite.
cargo run -p pathhydra-bench --release --features cuda -- --suite baseline

# Run the documented Compute Sanitizer command once the approved toolkit is available.
powershell -File Scripts/sanitize-cuda-tests.ps1
```

Scripts validate exact targets before invoking tools and never install software,
delete broad directories, upload results, or alter driver settings.

## 27. Completion criteria

Plan 05 is complete only when:

- every Plan 00-04 CPU behavior still passes without CUDA installed or enabled;
- the local RTX 3080 loads and runs Rust-authored PTX through the CUDA driver;
- the unsafe boundary is isolated, documented, linted, reviewed, and sanitizer-
  clean;
- host/device ABI tests cover every kernel parameter structure;
- topology is uploaded once per published image and validated before use;
- new CUDA requests never see topology older than their acquired CPU image;
- worst-case topology and per-lane memory are reserved before admission;
- frontier CUDA agrees bit-for-bit with CPU on all supported complete requests;
- delta CUDA agrees bit-for-bit with both CPU and frontier on its eligible
  domain;
- cancellation returns only provable exact states and never reuses live buffers;
- independent lanes preserve origins, profiles, destinations, counters, and
  failure states;
- path and finite-edge-budget requests follow the explicit CPU/refusal policy;
- driver, allocation, upload, launch, device, worker, and residency failures
  preserve durable graph state and correct CPU fallback;
- explicit CUDA reinitialization recovers the current image after tested failure;
- capabilities, health, and per-request diagnostics expose the actual executor
  and fallback reason;
- named-hardware reports separate correctness, cold start, upload, transfer,
  kernel, and steady-state measurements;
- `Auto` selection is based on recorded evidence rather than intuition;
- no approximation, cross-vendor abstraction, out-of-core path, durable image
  format, graph revision, BAML, hosted service, or GitHub Actions workflow is
  added.

Suggested commit message:

```text
Implement exact CUDA routing
```

## Following slice

After this plan, PathHydra has one complete CPU engine and one real fully
resident NVIDIA accelerator. The next large plan should choose exactly one of:

- serialized chunked routing images plus partitioned out-of-core GPU routing;
- GPU path reconstruction under the stable tie policy and finite GPU work
  budgets;
- a BAML-facing local boundary around the now benchmarked Rust engine.

Do not add another vendor abstraction until a second actual backend is selected
and implemented.

## Primary evidence

- Rust's official [`nvptx64-nvidia-cuda` target documentation](https://doc.rust-lang.org/stable/rustc/platform-support/nvptx64-nvidia-cuda.html)
  describes Rust-authored `no_std` PTX kernels, target components, and PTX
  generation.
- NVIDIA's [CUDA Driver API guide](https://docs.nvidia.com/cuda/cuda-programming-guide/03-advanced/driver-api.html)
  documents that the installed driver library loads PTX modules and JITs them
  for the device.
- NVIDIA identifies Ampere GeForce/RTX devices as
  [`sm_86`](https://developer.nvidia.com/blog/understanding-ptx-the-assembly-language-of-cuda-gpu-computing/).
- NVIDIA's [atomic operation documentation](https://docs.nvidia.com/cuda/cuda-programming-guide/05-appendices/cpp-language-extensions.html)
  defines 64-bit compare-and-swap behavior used by the non-negative binary64
  atomic-min proof.
- NVIDIA's [stream documentation](https://docs.nvidia.com/cuda/cuda-programming-guide/02-basics/asynchronous-execution.html)
  defines ordering, synchronization, concurrent streams, and the limitation
  that priorities do not preempt already running work.
- The maintained [`cudarc` documentation](https://docs.rs/cudarc/latest/cudarc/driver/index.html)
  describes dynamic CUDA Driver loading, typed device buffers, module loading,
  streams, and the unsafe launch obligation that this plan isolates.
- cuda-oxide's current [installation requirements](https://nvlabs.github.io/cuda-oxide/getting-started/installation.html)
  state that Windows is unsupported, which is why it is not the selected local
  build path for this slice.
