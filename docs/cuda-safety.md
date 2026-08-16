# CUDA safety boundary

`pathhydra-cuda` has a safe public API. Host unsafe code exists only in private
`src/launch.rs`; device pointer dereferences exist only in the separately
compiled `kernel/` package. See Decision 0005.

Kernel arguments are fixed-width scalars and typed cudarc device allocations.
Counts are checked before launch. Topology arrays are immutable, uploaded on
one context/stream, synchronized, and owned by `CudaResidentImage` together
with the exact `Arc<RoutingImage>` they represent. Zero-adjacency arrays use a
one-word device sentinel that the zero logical count makes non-dereferenceable;
the sentinel is excluded from reported topology bytes.

Distance, profile, status, counter, and delta scratch buffers are initialized
before launch and uniquely borrowed for mutation. A route synchronizes through
device-to-host copies before releasing reservations or buffers. Worker jobs own
their request and cancellation `Arc`; shutdown rejects queued jobs and joins
the worker before its sender is destroyed.

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
