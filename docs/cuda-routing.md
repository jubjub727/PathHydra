# Exact CUDA routing

CUDA full residency remains the preferred accelerator path when topology and configured headroom fit. CPU publication is independently resident or source-segment partitioned, and permissive CUDA policy falls back to the matching exact CPU image without mixing partial device distances with CPU evidence. `RequireCuda` returns a typed refusal when no admissible CUDA representation exists.

Resident CUDA contains CSR offsets, dense destinations, dense relation indexes,
and canonical base-weight bits. Partitioned CUDA keeps global distance/profile
state on the device while immutable source segments move through fixed,
byte-accounted device slots. Stable node/relation IDs remain on the matching
CPU bundle lease for request mapping and canonical response construction.
Payloads, edge IDs, hydration, paths, and subgraphs never transfer to CUDA.

The frontier reference performs strict label corrections. Each phase compacts
active dense sources, maps all source segments to partitions, stages every
required partition, and closes only after all copies and launches complete.
Newly improved nodes form the next active set; a destination with no outgoing
segments schedules no topology. Reversed partition order is an agreement gate.
Queued lanes remain separate in profile, origin, status, and counters. Zero
cycles terminate because equal distances do not update.

Partitioned delta-stepping scans sparse logical buckets, repeatedly processes
all light-edge partitions until same-bucket closure, then processes every heavy
partition for the bucket's final removed set. Host reads/copies are pending
bucket work. Bucket indexes must fit `u64`; there is no clamp or approximation.

Both algorithms use the CPU numeric policy: convert binary32 base weight and
multiplier separately to binary64, multiply, then add. The PTX audit rejects a
fused binary64 instruction. Results compare exact bits against CPU fixtures.

Eligible requests are distance-only, unlimited-budget, complete-profile calls
against a matching resident or partitioned current image. Missing destinations are mapped on the
CPU and preserve order/duplicates. Cancellation before a launch returns origin
at zero, missing nodes, and incomplete present destinations; a running kernel
is synchronized before its buffers are reused.

Policies are `CpuOnly`, `PreferCuda`, `RequireCuda`, and `Auto`. Permissive
policies rerun the complete request on its acquired CPU image after an early
CUDA failure. `RequireCuda` returns typed shape, residency, admission, or
device failures. Paths never combine GPU distances with CPU evidence.
