# Exact CUDA routing

CUDA residency contains CSR offsets, dense destinations, dense relation
indexes, and canonical base-weight bits. Stable node/relation IDs remain on the
matching CPU image for request mapping and canonical response construction.
Payloads, edge IDs, hydration, paths, and subgraphs never transfer to CUDA.

The frontier reference performs strict label corrections until no distance
improves. The current production kernel assigns one independent search lane to
a device thread; queued lanes may be collected as a batch and remain entirely
separate in profile, origin, destination mapping, status, and counters. Zero
cycles terminate because equal distances do not update.

Delta-stepping scans sparse logical buckets, repeatedly closes the current
bucket with edges whose exact effective weight is at most delta, retains every
removed node, then relaxes heavy edges. Bucket indexes must fit `u64`; there is
no clamp or approximation. Delta changes scheduling only.

Both algorithms use the CPU numeric policy: convert binary32 base weight and
multiplier separately to binary64, multiply, then add. The PTX audit rejects a
fused binary64 instruction. Results compare exact bits against CPU fixtures.

Eligible requests are distance-only, unlimited-budget, complete-profile calls
against a fully resident current image. Missing destinations are mapped on the
CPU and preserve order/duplicates. Cancellation before a launch returns origin
at zero, missing nodes, and incomplete present destinations; a running kernel
is synchronized before its buffers are reused.

Policies are `CpuOnly`, `PreferCuda`, `RequireCuda`, and `Auto`. Permissive
policies rerun the complete request on its acquired CPU image after an early
CUDA failure. `RequireCuda` returns typed shape, residency, admission, or
device failures. Paths never combine GPU distances with CPU evidence.
