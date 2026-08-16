# Plan 07: Parallel CUDA Execution and Request-Shape Closure

## Outcome

Starting from the complete Plan 06 resident and partitioned CUDA implementation,
verify or complete genuinely graph-parallel execution and close the remaining
accelerator-policy decisions. At completion, resident and out-of-core CUDA have
proven parallel relation relaxation, and target membership,
profile handling, state reset, delta selection, and lane batching are selected
from named measurements. CUDA path and finite-budget behavior have explicit,
tested outcomes rather than accidental unsupported states.

The existing resident and partitioned CPU implementations remain the semantic
oracle and permissive fallback. This plan does not build durable bundles,
partitioned CUDA residency, host/device partition caches, or their recovery;
those are prerequisites delivered by Plan 06.

## Prerequisite

Plan 06 must meet its complete definition of done, including:

- exact partitioned CPU routing;
- exact partitioned frontier and delta CUDA routing;
- bounded host/device topology caches and staging;
- split-source and pending-I/O phase correctness;
- publication, retirement, corruption, cancellation, and device-loss tests;
- the executable topology-larger-than-device benchmark.

If partitioned CPU topology still disables CUDA, or the Plan-06 scale/fault
matrix is absent, stop and finish Plan 06 rather than implementing a substitute
here.

## Explicit non-goals

- changing the routing-bundle layout or publication protocol;
- reimplementing partitioned CUDA ownership, staging, or cache state machines;
- another GPU vendor or a vendor-neutral accelerator trait;
- approximate distances, negative weights, relaxed comparison, or blended
  profiles;
- GPU payload hydration or subgraph composition;
- unified-memory paging, multi-GPU routing, or remote devices;
- selecting DirectStorage again after Plan 06's evidence gate;
- weakening CPU fallback or making CUDA required for correctness.

## 1. Freeze six-way semantic agreement

Run the complete Plan-06 gates and retain one reusable harness comparing:

- resident CPU;
- partitioned CPU under forced churn;
- resident CUDA frontier;
- resident CUDA delta;
- partitioned CUDA frontier;
- partitioned CUDA delta.

Compare destination state/order, exact distance bits, numeric/tie policy, and
completion. When a mode is path-capable, compare every stable handle and weight
bit. Diagnostics are checked separately.

Record the completed Plan-06 resident and partitioned kernel work distribution
and performance as the optimization baseline. If Plan 06 already introduced
graph-parallel work, retain it and prove/tune it here; do not replace it merely
to follow an assumed implementation sequence. Correctness and fallback tests
must continue to pass after each change.

## 2. Record the parallel execution decision

Add Decision 0009 covering:

- graph-parallel relation relaxation inside one lane;
- bounded throughput batching across independent lanes;
- reuse of Plan-06 resident/partitioned topology ownership and phase tracking;
- exact atomic binary64 distance updates with separate multiplication/addition;
- deterministic completion independent of partition or block schedule;
- CPU fallback from the same acquired bundle;
- selected path and finite-budget policies from sections 10 and 11.

Do not broaden Decision 0005's unsafe boundary. New kernel ABI and launch code
remain inside its documented modules.

## 3. Profile the reference kernels before replacing them

Instrument resident and partitioned kernels to separate:

- queue and batch collection;
- state initialization/reset;
- frontier or bucket compaction;
- host partition scheduling;
- file read/checksum and device staging already owned by Plan 06;
- relation relaxation;
- atomic contention;
- destination completion checks;
- device-to-host response transfer;
- synchronization.

Use narrow, broad, dense, high-degree, zero-closure, unreachable, hot-cache, and
cold-cache workloads. This establishes where graph parallelism helps and
prevents replacing inspectable reference code with unmeasured complexity.

## 4. Verify or complete graph-parallel frontier relaxation

Audit the Plan-06 frontier kernel. If it already distributes relation work
across threads/blocks, prove the properties below and optimize only from
profiles. Otherwise replace the lane-serial edge loop with bounded
graph-parallel work while retaining Plan-06 phase orchestration:

1. compact active dense sources per lane;
2. map sources to all ordered source-segment tasks;
3. group tasks by already managed resident array or partition cache slot;
4. launch enough threads/blocks to cover relations across tasks;
5. apply exact atomic distance minimum;
6. record improvements without duplicate-state corruption;
7. close the phase only after every Plan-06 read/copy/kernel/event completes;
8. compact the next frontier and repeat.

A source split across partitions is complete only after all segments execute.
Equal distances do not change distance state. Zero cycles terminate. Reversing
partition, block, or lane scheduling must not change response bits.

Keep the serial CUDA frontier kernel as a diagnostic comparator only while it
has a current test use; remove it if the CPU oracle plus parallel kernel make it
redundant.

## 5. Verify or complete graph-parallel delta-stepping

Audit Plan 06's delta work distribution. Retain a correct parallel
implementation if present; otherwise parallelize light and heavy relation work
without changing Plan-06 bucket completion:

- repeatedly process every light segment for current same-bucket sources;
- include sources discovered while other blocks/partitions are active;
- require no pending phase work before light closure;
- retain the complete removed set;
- process every heavy segment for that set;
- advance to the smallest representable nonempty bucket only after completion.

Bucket indexes remain checked `u64`; delta never clamps arithmetic. Test sparse
huge indexes, zero-weight closure across cache eviction, repeated relaxations,
and independent lanes in different bucket phases.

## 6. Select reusable search-state reset strategy

Measure synchronized full clears against generation-stamped distance,
frontier, destination, and predecessor candidates. If generation stamps win
materially, implement independent lane generations, first-touch initialization,
synchronized complete reset on wrap, reduced-width wrap tests, and no lane
reuse before Plan-06 events complete.

Otherwise retain explicit clears and record why. Remove unused generation
prototypes.

## 7. Select target membership representation

Compare sorted sparse destination IDs, a dense destination bitset, and
generation-stamped dense membership. Measure destination counts from one
through the configured maximum and include transfer/reset cost for resident and
partitioned modes. Select one representation or a bounded hybrid threshold.

Missing and duplicate destinations remain CPU-mapped and restored to caller
order. Target-aware stopping occurs only when every requested distance is
proved final; first discovery is insufficient.

## 8. Select inline or materialized effective weights

Build a benchmark candidate that materializes exact effective weights for an
immutable complete profile. Compare against inline `base * multiplier` for
one-use/reused profiles, few/many relation kinds, narrow/broad/dense searches,
and resident/partitioned topology, including materialization memory and copy
cost.

If no repeatable total-request win exists, retain inline computation and remove
the candidate. If selected, bound cached bytes/entries, key by bundle plus full
canonical profile equality, and use hashes only to accelerate verified lookup.

## 9. Tune bounded multi-lane scheduling

Use Plan-06 partition coalescing and cache safety while improving how ready work
from independent lanes shares launches. Keep origins, profiles, destinations,
distances, counters, cancellation, and status separate.

Measure collection delay, block/task packing, fairness, resident/partitioned
mixes, and 1/2/4/maximum lanes. Prevent a cache-thrashing lane from indefinitely
starving a hot/resident lane. Document that concurrency targets throughput once
one broad route saturates the device.

## 10. Close CUDA path-evidence behavior

Prototype one exact strategy under the current stable predecessor policy:

- device predecessor evidence updated under a proven deterministic rule; or
- exact CUDA distances followed by a CPU evidence pass over the same acquired
  bundle that reproduces CPU finalization/tie order and verifies every distance
  equation.

The prototype must match resident and partitioned CPU paths for zero-weight
cycles, parallel relations, equal-cost candidates, partition reorder, and split
sources. Account for all state and reconstruction memory before admission.

If exact parity passes, enable path requests and report accelerator/evidence
stages honestly. If it does not, remove the prototype and record CPU dispatch
for path requests as the selected policy. Never combine unchecked GPU distances
with CPU evidence.

## 11. Close finite examined-edge budget behavior

Determine whether parallel CUDA can honor the existing deterministic
`ExaminedEdges` stopping point independently of warp/block schedule. Do not
redefine it to mean relaxation attempts, launches, or elapsed time.

If exact agreement cannot be proved, retain CPU dispatch for finite budgets and
record executor specialization. Unlimited CUDA work remains bounded by
admission, cancellation, and exact search completion.

## 12. Select algorithms and automatic dispatch from evidence

Use distributions rather than one fastest run to choose frontier/delta, delta
candidates, CPU/CUDA and resident/partitioned crossovers, batch widths,
collection delay, and any target/profile/reset thresholds.

`Auto` remains CPU unless a conservative repeatable rule wins including queue,
reset, staging, and response costs. Encode no universal performance promise in
the API. Keep `PreferCuda` and `RequireCuda` explicit.

## 13. Regress Plan-06 failure, cancellation, and recovery

Kernel/scheduler changes must pass Plan 06's existing tests for cache pressure,
read/checksum/copy/event/launch/synchronization failures, cancellation in every
state, publication with old events/leases, device loss/reinitialization,
same-bundle CPU fallback, and corruption-triggered rebuild.

Add fault injection only for new compaction, parallel scratch, predecessor,
profile-cache, and target-state resources.

## 14. Extend capabilities, health, and diagnostics

Add selected parallel strategy, target/profile/reset mode, effective batch
width, path-evidence mode, and finite-budget capability. Report graph-parallel
work, contention indicators, compaction sizes, and strategy-specific
reservations without duplicating Plan-06 cache/staging counters.

Never log payloads, exact names, destinations, complete profiles, or paths.

## 15. Correctness and sanitizer tests

Run every resident/partitioned Plan-06 fixture plus extreme block boundaries,
high-degree split sources, deliberately reordered schedules, zero closure,
cache churn, lane sharing, generation wrap if selected, strategy thresholds,
profile-hash collisions with full equality, path/budget policies, CUDA memcheck,
and racecheck.

## 16. Performance evidence

Extend the executable Plan-06 benchmark harness rather than recreating it.
Compare the Plan-06 reference kernels, final graph-parallel
resident/partitioned CUDA, and resident/partitioned CPU. Rerun the Plan-06
topology-larger-than-device suite as regression evidence.

Every timed result is checked against untimed CPU first. Report warmup,
distribution, topology/cache state, selected strategies, device memory, and
end-to-end—not kernel-only—time.

## Definition of done

Plan 07 is complete when:

- Plan 06 remains fully passing and owns all partition/cache/lifecycle behavior;
- resident and partitioned CUDA use graph-parallel exact kernels;
- scheduling order cannot change exact responses;
- target, profile, reset, delta, batching, and `Auto` choices are measured and
  recorded;
- CUDA path and finite-budget behavior have explicit proven policies;
- new parallel resources are admitted, cancellable, diagnosable, and
  recoverable through Plan-06 mechanisms;
- unused prototypes/reference kernels are removed unless currently useful;
- agreement, adversarial, sanitizer, benchmark, and workspace gates pass.
