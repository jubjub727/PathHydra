# System closure benchmark evidence

This report records the Plan 10 benchmark harness, its superseded diagnostic
runs, and—after the settled-tree rerun—the final bounded and out-of-core
evidence. Measurements are hardware/dataset evidence, not latency guarantees.
The large topology proof remains opt-in and is not represented by the bounded
`scale` row below.

## Reproduction

The settled-tree bounded run on 2026-08-17 was:

```powershell
cargo run --release -p pathhydra-bench --features cuda -- --suite all --repeats 3 --warmup 1 --format human
```

The same matrix is machine-readable with `--format csv` or `--format json`.
The final JSON reproduction contained 180 rows (135 raw samples and 45
summaries), 58 fields per row, and zero incorrect results. All 135 samples had
a process peak observation, and all 108 routing samples had a full-request
first-destination observation.

The explicit suites are:

```text
store-ingest
store-mutation
snapshot-build-load
cpu-routing
cuda-resident
cuda-out-of-core
concurrency
reconstruction-hydration
backup-restore
scale
all
```

All accept `--repeats`, `--warmup`, and `--format human|csv|json`. The former
`baseline`, `out-of-core`, `parallel-strategy`, `operations`, and positional
`scale [directory] [target-gib]` commands remain available because existing
performance records use them. A bounded machine-readable scale run uses, for
example, `--suite scale --repeats 3 --warmup 1 --format json`; positional scale
without options retains the manual large-bundle command.

Every suite executes an untimed semantic oracle before warmup or measured
work. It buffers timing rows until the oracle passes, checks each measured
result again, and aborts without emitting a report if correctness differs.
Summary rows contain the median of every populated timing, counter, byte, and
throughput field; sample rows retain each observation so the full spread and
other distributions can be recomputed without interpreting a first sample as
the summary.

## Environment

| Field | Recorded value |
|---|---|
| OS / architecture | Windows / x86-64 |
| CPU | AMD64 Family 26 Model 68 Stepping 0, AuthenticAMD |
| GPU | NVIDIA GeForce RTX 3080 |
| CUDA driver API value | 13030 |
| Rust | `rustc 1.95.0 (59807616e 2026-04-14)` |
| CUDA kernel toolchain | `nightly-2024-02-17`, PTX target `sm_86` |
| Storage | Local filesystem; media/type was not discoverable by the harness |

## Recorded distributions

Each row below is the median and full sample spread in nanoseconds from three
measured samples after one warmup. All correctness checks were true.

### Store, snapshots, reconstruction, recovery, and scale

| Suite | Case | Executor / algorithm | Median ns | Spread ns |
|---|---|---|---:|---:|
| store-ingest | chain-128 | RocksDB candidate-confirm | 32,924,900 | 537,600 |
| store-mutation | high-degree-delete-churn | RocksDB atomic-node-cascade | 378,500 | 44,600 |
| snapshot-build-load | snapshot-mixed-512 | scan-build-validate-open | 64,991,100 | 1,206,300 |
| concurrency | mixed-1-routes | CPU parallel requests | 89,200 | 3,400 |
| concurrency | mixed-2-routes | CPU parallel requests | 211,100 | 76,000 |
| concurrency | mixed-4-routes | CPU parallel requests | 396,200 | 15,400 |
| reconstruction-hydration | far-path-63-steps | stable predecessor/current hydration | 244,900 | 2,800 |
| backup-restore | idle-checkpoint-fresh-restore | checkpoint-verify-restore | 107,073,700 | 2,291,500 |
| scale | bounded-scale-2048 | scan-build-validate-open | 146,304,600 | 3,632,200 |

### Resident and partitioned CPU routing

The request corpus contains ordered near, far, midpoint, and origin
destinations. The disconnected case supplies unreachable evidence; the other
named cases cover narrow, broad/high-degree, dense, zero-closure, and churned
mixed-locality shapes.

| Case | Resident median / spread ns | Partitioned median / spread ns |
|---|---:|---:|
| narrow-near-far | 2,000 / 300 | 4,200 / 1,000 |
| broad-high-degree | 18,500 / 1,000 | 28,200 / 2,200 |
| dense | 13,400 / 200 | 55,700 / 1,500 |
| zero-closure | 1,900 / 200 | 4,100 / 400 |
| unreachable | 2,100 / 700 | 6,200 / 1,400 |
| churn-mixed-locality | 7,700 / 1,200 | 16,900 / 1,400 |

### Resident CUDA routing

| Case | Frontier median / spread ns | Delta 0.1 median / spread ns |
|---|---:|---:|
| narrow-near-far | 14,049,400 / 246,300 | 16,801,000 / 596,800 |
| broad-high-degree | 539,500 / 33,700 | 1,307,800 / 70,600 |
| dense | 547,900 / 18,300 | 1,428,800 / 17,900 |
| zero-closure | 14,059,600 / 64,100 | 18,967,900 / 143,700 |
| unreachable | 14,084,700 / 342,400 | 16,669,200 / 332,800 |
| churn-mixed-locality | 3,762,300 / 138,800 | 5,732,100 / 135,200 |

### Partitioned CUDA routing

| Case | Frontier median / spread ns | Delta 0.1 median / spread ns |
|---|---:|---:|
| narrow-near-far | 10,944,200 / 613,100 | 16,869,600 / 257,300 |
| broad-high-degree | 422,500 / 66,400 | 1,323,500 / 86,800 |
| dense | 1,052,600 / 27,400 | 3,123,800 / 52,100 |
| zero-closure | 10,897,200 / 41,900 | 19,131,900 / 45,600 |
| unreachable | 10,876,100 / 81,500 | 16,671,400 / 149,700 |
| churn-mixed-locality | 4,747,100 / 182,900 | 7,972,600 / 229,800 |

These small workloads demonstrate exactness and instrumentation, not GPU
speedup. Kernel launch and synchronization overhead dominate several cases.

## Machine report fields

Raw and summary records identify suite, case, executor, algorithm, the complete
benchmark configuration, sample, warmup/repeat counts, deterministic seed,
cold/warm state, OS, architecture, CPU, storage discovery, Rust/kernel
toolchains, GPU, and driver. Where exposed by the current API, rows also
contain:

- node, relation-kind, edge, and partition counts;
- RocksDB, routing-bundle, resident topology, search reservation, and peak
  partition-buffer bytes;
- snapshot scan, write/build, validation, and reopen time;
- profile packing and full route completion time;
- examined edges, relaxation attempts/updates, frontier high-water, and CUDA
  phase/bucket count;
- host/device cache hits, misses, evictions, file bytes, transfer bytes, and
  reservations;
- concurrent routes and aggregate throughput;
- path reconstruction, hydration, checkpoint, and restore time and bytes.

Unavailable observations are blank in CSV and `null` in JSON rather than
being reported as zero. `first_destination_ns` is the diagnostic timestamp of
the first present destination finalized during the same measured ordered
multi-destination request. Missing destinations are classified before search
and do not manufacture a zero-duration completion. CPU records the first
settled requested node for distance-only work, waits for reconstructed evidence
when a path was requested, and timestamps frontier exhaustion when that first
proves an existing destination unreachable. The current CUDA implementations
finalize requested states only after their synchronized distance pass, and
path-returning CUDA requests include same-image path-evidence verification
before the destination is complete. `reconstruction_ns` is likewise the measured path-evidence
reconstruction interval, not an alias for end-to-end route time.

After each measured distribution, the harness safely samples cumulative peak
process working set. Windows uses `Get-Process -Id <current pid>
PeakWorkingSet64`; Linux reads `VmHWM` from `/proc/self/status`; unsupported or
failed probes alone produce an unavailable value. The earlier targeted JSON run
contained 135 non-null peak samples, no incorrect rows, and a maximum observed
process peak of 132,734,976 bytes. This process-level value is distinct from
routing working-set estimates and component buffer high-water counters.

That run's first resident CUDA frontier summary independently exposed 128 nodes, 127
edges, 2,556 resident bytes, 12,016 reserved search bytes, 127 examined edges,
127 relaxation attempts, 127 updates, and 127 phases. This spot check confirms
that the machine report carries execution diagnostics rather than timing alone.

## Final topology-larger-than-device proof

The settled-tree positional scale command completed successfully on 2026-08-17:

```powershell
$env:CARGO_BUILD_JOBS = "1"
cargo run -p pathhydra-bench --release --features cuda -- --suite scale target/pathhydra-scale-topology-12gib-final 12
```

It emitted ten correct 55-field CSV rows: build/open, cold and warm CPU,
active CPU cancellation, CUDA Frontier and active cancellation, CUDA Delta 0.1
and active cancellation, injected device-loss CPU fallback, and restart-open.

| Field | Observed value |
|---|---:|
| Nodes / directed adjacencies | 1,048,577 / 1,073,741,825 |
| CUDA topology / 12-GiB target | 12,884,957,232 / 12,884,901,888 bytes |
| Total bundle / measured directory | 21,491,878,443 bytes |
| Partitions | 1,537 |
| Generation / initial validation-open | 96.671192 s / 64.509212 s |
| Cold / warm partitioned CPU | 69.425173 s / 69.381145 s, exact |
| Active CPU cancellation | 12.999 ms, cancelled and drained |
| Partitioned CUDA Frontier | 63.774632 s, exact |
| Active Frontier cancellation | 47.560 ms, cancelled and drained |
| Partitioned CUDA Delta 0.1 | 111.066097 s, exact |
| Active Delta cancellation | 37.843 ms, cancelled and drained |
| Injected device-loss CPU fallback | 77.414754 s, exact |
| Restart validation-open | 64.001969 s, `rebuilt=false` |
| Host cache / staging high-water | 55,923,904 / 13,980,976 bytes |
| Device cache high-water | 16,777,152 bytes |
| Frontier / Delta search reservation | 83,886,640 / 92,275,256 bytes |
| Peak process working set | 232,980,480 bytes |
| Reported device capacity / observed use increase | 10,736,893,952 / 67,108,864 bytes |
| Frontier / Delta topology transfer | 12,884,901,900 / 25,761,415,236 bytes |

The CUDA-consumed topology exceeded the reported device capacity by
2,148,063,280 bytes and exceeded the requested 12-GiB threshold by 55,344
bytes. The proof therefore does not rely on CPU-only `evidence.bin` bytes.
Peak process memory was about 1.8% of the topology size. Frontier spent
32.769230 s in partition scheduling/I/O, 3.012119 s in task compaction, and
27.751325 s in relation relaxation. Delta spent 63.413730 s, 4.979332 s, and
42.192297 s in those stages. Conventional transport remained material but was
not the repeatable dominant component, so the DirectStorage comparison trigger
remains false.

Both active CUDA cancellation rows reached the deterministic task-compaction
boundary before signalling cancellation. CPU cancellation reached active
partition work. Every cancellation then waited for queued host I/O to drain
and asserted zero loading/pinned entries, queue depth, staging bytes, and device
slots. The device-loss row invalidated the same scale CUDA context and reran the
complete request exactly through the existing partitioned CPU image. Restart
validated the same bundle and reported `rebuilt=false`.

## Superseded 12-GiB bundle diagnostic run

The bounded `scale` suite deliberately uses a 2,048-node mixed-locality graph.
It does not replace Plan 10 section 12. The positional scale suite was
initially rerun against a 12-GiB generated bundle on the recorded RTX 3080.
That run remains useful diagnostic evidence, but it is not the final Plan-10
scale proof: the five-file bundle exceeded 12 GiB while the CUDA-consumed
`topology.bin` was only 7,730,974,344 bytes and therefore fit in the 10-GiB
device. The remaining 5,153,960,768 bytes were CPU-only stable-edge evidence.
The final proof above sizes `topology.bin` itself above the requested threshold.

The final command uses a fresh target directory so the earlier bundle cannot be
mistaken for the strengthened topology-sized corpus:

```powershell
$env:CARGO_BUILD_JOBS = "1"
cargo run -p pathhydra-bench --release --features cuda -- --suite scale target/pathhydra-scale-topology-12gib-final 12
```

The final positional scale command emitted a dedicated 55-column CSV report
with separate total-bundle and CUDA-topology byte counts, dataset/configuration
and host/toolchain identity, plus a same-graph device-loss/fallback row. It
retains its existing invocation shape:

```powershell
cargo run --release -p pathhydra-bench --features cuda -- --suite scale <directory> <target-gib>
```

It reports generation, initial validation/open, restart validation/open,
per-route and cumulative elapsed time; bundle and measured directory bytes;
host cache current/high-water, staging high-water, hits/misses/evictions,
partitions read and file bytes; device cache current/high-water,
hits/misses/evictions and transfer bytes; route transfer and reserved search
bytes; sampled peak host working set and observed driver-reported device use;
and CUDA initialization, partition scheduling/I/O, task compaction, relation
relaxation, and response-transfer durations.
For every nonzero-GiB proof run the command requires an observable process peak
and fails unless that cumulative peak remains below `topology.bin`. When the
requested target exceeds the detected device capacity, it also fails unless
the measured CUDA topology exceeds that capacity; both values are explicit
columns rather than inferred from total bundle size.
Its proof rows cover cold and warm exact CPU, active partition-I/O CPU
cancellation, exact partitioned CUDA Frontier and Delta 0.1, deterministic
active task-compaction cancellation for both CUDA algorithms, scale-local
device-loss CPU fallback, and restart validation of the same bundle with
`rebuilt=false`. The zero-GiB smoke retains the immediate pre-cancel path
because it has no long-running partition work to hold.

The superseded diagnostic command was:

```powershell
cargo run -p pathhydra-bench --release --features cuda -- --suite scale target/pathhydra-scale-fanout-12gib 12
```

The analytic graph has one origin, 1,048,576 fan-out destinations, one
relation kind, and 644,245,096 unit-weight directed edges distributed across
those destinations. Every present destination is therefore exactly distance
`1.0`. This preserves a complete 12-GiB origin expansion while avoiding the
obsolete two-node generator's single-address atomic hotspot.

| Field | Observed value |
|---|---:|
| Bundle / measured directory bytes | 12,901,838,083 |
| Partitions | 922 |
| Generation / initial validation-open | 56.100900 s / 36.083506 s |
| Cold / warm partitioned CPU | 40.718706 s / 40.576873 s, exact |
| Partitioned CUDA Frontier | 50.670165 s, exact |
| Partitioned CUDA Delta 0.1 | 80.109480 s, exact |
| Restart validation-open | 37.915033 s, `rebuilt=false` |
| Host cache / staging high-water | 55,923,904 / 13,980,976 bytes |
| Device cache high-water | 16,777,152 bytes |
| Frontier / Delta search reservation | 83,886,640 / 92,275,256 bytes |
| Peak process working set | 213,684,224 bytes |
| Observed device-use increase | 1,267,204,096 bytes |
| Frontier / Delta topology transfer | 7,730,941,152 / 15,461,882,304 bytes |

Frontier spent 18.129746 s in partition scheduling/I/O, 1.343365 s in
task compaction, and 31.047982 s in relation relaxation. Delta spent
36.088368 s, 2.646607 s, and 41.098345 s in those stages respectively.
Initialization and response transfer were below 9 ms. Conventional transport
is material but not the repeatable dominant component, so the DirectStorage
comparison gate remains untriggered. The bounded host/device working-state
observations were valid, but total bundle size alone did not prove
topology-larger-than-device behavior.

Pre-cancelled CPU, Frontier, and Delta rows all returned `cancelled` and held
no cache slots. Every correctness field was true. The same valid bundle then
reopened without regeneration.

The earlier small real-device rehearsal was:

```powershell
cargo run --release -p pathhydra-bench --features cuda -- --suite scale target\plan10-scale-smoke-final 0
```

It generated and validated a 571-byte, one-partition analytic bundle, then
reported all nine then-current proof rows correct. Cold/warm CPU were 311/6 microseconds;
CUDA Frontier/Delta were 4,740/1,616 microseconds; all three pre-cancelled
paths completed as `cancelled`; restart validation took 782 microseconds and
did not rebuild. Host cache/staging high-water was 112/56 bytes, device cache
high-water was 24 bytes, and both CUDA algorithms returned exact binary64
distance `1.0`.

The current scale harness additionally injects CUDA context loss and executes
the same large request through the partitioned CPU image; the final proof above
contains that scale-local fallback evidence. The executable engine-level
rehearsal remains an independent ownership/health check:

```powershell
cargo test -p pathhydra-engine --features cuda --test cuda_engine partitioned_context_loss_falls_back_to_cpu_until_explicit_reinitialization -- --exact
```

That test injects partitioned CUDA context loss under `PreferCuda`, verifies
the exact CPU fallback response and degraded health, and proves subsequent
routes remain on CPU until explicit CUDA reinitialization.
