# Exact CUDA routing

CUDA full residency remains the preferred accelerator path when topology and configured headroom fit. CPU publication is independently resident or source-segment partitioned, and permissive CUDA policy falls back to the matching exact CPU image without mixing partial device distances with CPU evidence. `RequireCuda` returns a typed refusal when no admissible CUDA representation exists.

Resident CUDA contains CSR offsets, dense destinations, dense relation indexes,
and canonical base-weight bits. Partitioned CUDA keeps global distance/profile
state on the device while immutable source segments move through fixed,
byte-accounted device slots. Stable node/relation IDs remain on the matching
CPU bundle lease for request mapping and canonical response construction.
Payloads, edge IDs, hydration, path evidence, and subgraphs never transfer to
CUDA.

The frontier reference performs strict label corrections. Each phase compacts
active dense sources into explicit edge/source tasks, maps their source
segments to partitions, stages every required partition, and closes only after
all task uploads, topology copies, launches, and synchronization complete.
Newly improved nodes form the next active set; a destination with no outgoing
segments schedules no topology. Reversed partition order is an agreement gate.
Queued lanes remain separate in profile, origin, status, and counters. Zero
cycles terminate because equal distances do not update.

Partitioned delta-stepping scans sparse logical buckets, repeatedly processes
the partitions named by the current bucket's source segments until same-bucket
light closure, then processes the partitions named by the bucket's final
removed set for heavy edges. It does not scan unrelated partitions. Host
reads/copies are pending bucket work. Bucket indexes must fit `u64`; there is no
clamp or approximation.

`frontier_compaction_duration` measures host task construction plus compact
task upload, separately from device relation relaxation.
`compacted_task_count` is the cumulative number of edge/source tasks submitted
across frontier, delta-light, and delta-heavy phases; it is not a post-route
count of reached states. Compact task bytes are included in host-to-device
diagnostics, intermediate active/bucket state downloads are included in
device-to-host diagnostics, and admission reserves the worst simultaneous host
and device task-buffer footprint before any CUDA allocation.

Both algorithms use the CPU numeric policy: convert binary32 base weight and
multiplier separately to binary64, multiply, then add. The PTX audit rejects a
fused binary64 instruction. Results compare exact bits against CPU fixtures.

Eligible requests are unlimited-budget, complete-profile calls against a
matching resident or partitioned current image. Distance-only requests return
CUDA selection directly. Path requests use the same CUDA distance selection and
then a cancellation-aware CPU evidence pass on the acquired image or bundle
lease; every destination state and exact logical-distance bit pattern is
verified before edge evidence is returned. Cancellation at that boundary
returns the CPU router's controlled incomplete response rather than unchecked
evidence. Missing destinations are mapped on the CPU and preserve
order/duplicates. Cancellation before a launch returns origin at zero, missing
nodes, and incomplete present destinations; a running kernel is synchronized
before its buffers are reused.

Policies are `CpuOnly`, `PreferCuda`, `RequireCuda`, and `Auto`. Permissive
policies rerun the complete request on its acquired CPU image after an early
CUDA failure. `RequireCuda` returns typed shape, residency, admission, or
device failures. Finite examined-edge budgets use CPU under permissive policies
and are typed refusals under `RequireCuda`; they are not eligible for this
two-stage path-evidence flow.
