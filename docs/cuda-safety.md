# CUDA safety boundary

`pathhydra-cuda` has a safe public API. Host unsafe code exists only in private
`src/launch.rs`; device pointer dereferences exist only in the separately
compiled `kernel/` package. See Decision 0005 and the final [public API and
safety audit](public-api-safety-audit.md).

Kernel arguments are fixed-width scalars and typed cudarc device allocations.
Counts are checked before launch. Topology arrays are immutable, uploaded on
one context/stream, synchronized, and owned by `CudaResidentImage` together
with the exact `Arc<RoutingImage>` they represent. Zero-adjacency arrays use a
one-word device sentinel that the zero logical count makes non-dereferenceable;
the sentinel is excluded from reported topology bytes.

Distance, profile, status, counter, frontier/delta, and compacted source/edge
task buffers are initialized before launch and uniquely borrowed for mutation.
Task construction is fallibly allocated and cancellation-checkpointed. A route
synchronizes through device-to-host copies before releasing reservations,
partition leases, or task buffers. Worker jobs own their request and
cancellation `Arc`; shutdown rejects queued jobs and joins the worker before
its sender is destroyed.

Every safe resident/partitioned route entry recomputes the complete checked
request estimate and refuses an undersized reservation before request-shaped
state allocation or launch. The engine additionally owns the live byte/lane
reservation and releases it exactly once; the lower check prevents diagnostic
or direct evidence callers from understating the amount they claim to hold.

The ABI uses `u32` dense node/relation indexes, `u64` offsets and distance bits,
canonical `u32` operand bits, and fixed-width status/counter structures.
Compile-time size/alignment tests and PTX entry/parameter audits guard it.
Device checks reject invalid indexes, flags, arithmetic, counters, or bucket
indexes. Such output is never translated to a routing response.

Run memory and race checking after explicitly installing NVIDIA Compute
Sanitizer:

```powershell
powershell -File Scripts/sanitize-cuda-tests.ps1
```

The Windows WDDM racecheck pass uses Compute Sanitizer's
`--force-blocking-launches` instrumentation mode to avoid the tool's concurrent
kernel-launch tracking failure. Ordinary agreement/concurrency tests remain
nonblocking; this flag changes only the sanitizer observation mode.

On Windows systems using WDDM, Compute Sanitizer can require NVIDIA's
`EnableDebuggerInterface.bat` to be run explicitly as an administrator before
instrumentation. That toolkit-provided script sets
`HKLM\SOFTWARE\NVIDIA Corporation\GPUDebugger\EnableInterface` to `1`; remove
the value after testing if the debugging interface should not remain enabled.
