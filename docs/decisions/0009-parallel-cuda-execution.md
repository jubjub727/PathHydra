# Decision 0009: Parallel CUDA execution and request-shape policy

Status: accepted

Resident and partitioned CUDA execute exact relation relaxation with one
logical device thread per compacted edge/source task and enough blocks to cover
each phase's task range. Before a frontier phase, the host expands only the
currently active sources into task ordinals. Before a delta light closure or
heavy pass, it expands only sources in the current bucket or final removed set.
Resident tasks hold a global edge ordinal; partitioned tasks hold a local edge
ordinal and are built only after the required partition is pinned. Unrelated
adjacencies are not launched. Each thread evaluates the complete profile inline
with separate binary64 multiplication and addition and proposes a distance with
an audited unsigned-64 compare-and-swap minimum. The bit ordering is valid
because routing admits only nonnegative finite weights and positive infinity.
Equal values do not modify state. Atomic checked counters and compare-and-swap
retry counts make contention and overflow visible. Examined and attempted
edge counts are validated and accumulated once per thread block rather than
through one contended global atomic per edge; retry totals omit zero-valued
atomic additions. The reported totals remain exact.

The host retains explicit, inspectable phase orchestration. Frontier phases do
not close until all relation chunks, source segments, cache reads, copies,
launches, events, and synchronization complete. Delta light phases repeat until
same-bucket closure, keep a complete removed-source set, and run every heavy
segment for that set before advancing. Bucket indexes are checked `u64` values
and are never clamped. These rules make exact distance bits independent of
block, partition, and lane schedule, including split sources and zero-weight
cycles.

Plan 06 continues to own resident and partitioned topology, immutable bundle
leases, cache/event lifetimes, admission, cancellation, device-loss recovery,
and same-bundle CPU fallback. Search admission reserves the worst host/device
overlap for compact task buffers before any CUDA allocation. Resident mode
uses its complete adjacency count; partitioned mode uses the largest single
partition because it synchronizes and releases task buffers partition by
partition. New scratch-
allocation, task construction/upload, and path-evidence failures enter the
same typed fault and recovery path. Decision 0005's unsafe boundary is
unchanged: private launch code and the separately compiled kernel package are
the only CUDA boundary.

The measured request-shape policies are:

- explicit full state clears, because generation stamps add first-touch and
  wrap coordination and persistent lane-state ownership. The device-inclusive
  primitive medians were explicit/generation 199/135 microseconds resident and
  171/126 partitioned, but their five-sample ranges overlap and the current
  request-owned buffers cannot retain a generation between searches. This is
  not a repeatable complete-request win sufficient to add persistent lane
  state and wrap recovery;
- sorted, deduplicated sparse host target membership, because destination sets
  are small, caller order is restored on the host, and first discovery cannot
  safely terminate these label-correcting phases. Device-inclusive preparation,
  transfer, membership, synchronization, and validation at the configured
  three-target shape measured sparse/dense/generation medians of 65/94/202
  microseconds resident and 61/67/61 partitioned. Dense forms save no relation
  work without a finalization proof;
- inline exact effective weights, because materialization requires a complete
  edge pass, transfer and bounded cache identity based on full profile equality
  before it can help a repeated profile. Device-inclusive candidates across
  127/511/2,256 edges, 2/64 kinds, one/16 uses, and full/128-edge chunks had
  mixed winners; no materialized strategy won repeatably across complete
  shapes, and no production cache identity or eviction mechanism exists;
- bounded independent lanes collected by the existing worker, with graph
  parallelism inside each lane and separate origin, profile, state, counters,
  cancellation, and status. Zero/50/5,000-microsecond collection delays and
  1/2/4/8 lanes were measured. Zero delay with one lane had the best median
  (789 microseconds); wider resident batches and every resident/partitioned mix
  reduced throughput. The post-task-compaction five-repeat rerun again selected
  one lane; zero delay remains the default because delay cannot improve packing
  at width one. Higher bounded widths remain explicit experiments;
- frontier as the normal CUDA algorithm; delta remains an explicit supported
  choice. Delta values 0.01/0.1/1/10 had workload-specific winners, while
  frontier won every broad/dense resident/partitioned median. Unused automatic
  delta-candidate configuration was removed. Automatic dispatch remains CPU
  because end-to-end measurements do not establish a conservative repeatable
  CUDA crossover;
- exact CUDA distances followed, for path requests, by the CPU routing oracle
  over the same acquired resident image or partitioned bundle. Every returned
  state and distance bit is verified before CPU predecessor evidence is
  accepted. Admission reserves the CPU evidence state in addition to CUDA
  state;
- CPU execution for finite `ExaminedEdges` budgets. A parallel relaxation-attempt
  counter is not the CPU oracle's deterministic examined-edge stopping point,
  so CUDA does not reinterpret it.

Worker shutdown is explicit and idempotent. It rejects queued work, lets the
currently owned batch reach its normal terminal path, joins the worker thread,
and reports queued, active, rejected, and joined counts. `Drop` invokes the same
join path as a non-reporting fallback and never detaches a CUDA worker.

The public diagnostics name the parallel, reset, target, profile, and path-
evidence modes, effective batch width, non-overlapping task-compaction and
relation-relaxation durations, `compacted_task_count`, atomic retry count, and
admitted bytes. Task uploads contribute to host-to-device bytes, and control
downloads contribute to device-to-host bytes. They contain no node names, destinations,
complete profiles, paths, or hydrated payloads. There is no API performance
promise; explicit CUDA policy requests acceleration, not a speedup guarantee.
