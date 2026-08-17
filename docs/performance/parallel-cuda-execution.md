# Parallel CUDA execution evidence

Date: 2026-08-17. Hardware: NVIDIA GeForce RTX 3080, compute capability 8.6,
10,240 MiB. Driver: 610.88 (CUDA driver capability 13.3). Host compiler: Rust
1.95.0. Kernel compiler: `nightly-2024-02-17`. PTX target: `sm_86`.

The final kernels use graph-parallel compact edge/source task threads and an
atomic binary64 minimum. The host explicitly constructs tasks only for active
frontier sources, current delta-bucket sources, or the final removed set.
Resident runs use the complete uploaded topology; partitioned runs force the
Plan-06 bounded partition/cache path. Every timed row is accepted only after
exact comparison with its untimed CPU oracle, including destination states and
distance bits.

Commands:

```powershell
cargo run -p pathhydra-bench --release --features cuda -- --suite baseline
cargo run -p pathhydra-bench --release --features cuda -- --suite out-of-core
cargo run -p pathhydra-bench --release --features cuda -- --suite parallel-strategy 5
```

`parallel-strategy` accepts a positive repeat count and emits one CSV row per
sample. It is the reproducible policy-selection gate. Each row names the
executor, algorithm, strategy, sample, requested lanes, observed batch width,
lane index, operation count, route and queue time, correctness, device, and
toolchain. The command above produces exactly five samples per single-route or
isolated-comparator case and five batches for each lane width: 890 CSV data
rows in total. The recorded release run produced 890 rows with zero incorrect
rows.

Earlier six-workload route tables were removed when active-source task
compaction replaced full adjacency scans: their timings no longer described
the production kernels. The post-compaction five-repeat strategy suite below
is the authoritative final end-to-end evidence. It still does not justify a
universal CUDA speedup claim or automatic CUDA dispatch. `Auto` remains CPU,
frontier is the ordinary explicit-CUDA choice, and delta remains available for
explicit selection and workload-specific measurement.

## Reproducible strategy suite

The five-sample release strategy run used broad-star and dense-scc because they
exercise relation width and contention without letting host-controlled chain
phase count dominate the comparison. CPU partition caches and both CUDA images
were warmed before sampling. Values below are minimum/median/maximum
microseconds. Every correctness column in the raw CSV was `true`.

| workload | resident CPU | partitioned CPU | resident frontier | partitioned frontier | resident delta 0.1 | partitioned delta 0.1 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| broad-star | 13 / 13 / 18 | 24 / 24 / 25 | 547 / 705 / 947 | 503 / 513 / 603 | 1,207 / 1,302 / 1,523 | 1,216 / 1,378 / 1,462 |
| dense-scc | 10 / 10 / 13 | 55 / 55 / 56 | 1,682 / 1,693 / 1,788 | 2,328 / 2,451 / 2,976 | 3,197 / 3,681 / 4,578 | 3,420 / 4,186 / 4,633 |

The policy candidates below are actual CUDA launches inside the audited unsafe
boundary. Timings include host preparation, device allocation/transfer, kernel
execution, synchronization, device-to-host validation, and resource release.
Each downloaded state, membership result, or effective-weight bit pattern is
validated. They remain benchmark-only candidates and do not imply an
unimplemented production strategy.

Reset values are minimum/median/maximum microseconds for 512 nodes:

| context | explicit clear | generation stamp | selection |
| --- | ---: | ---: | --- |
| resident | 117 / 199 / 270 | 114 / 135 / 162 | explicit: ranges overlap and request-owned state cannot retain generations |
| partitioned | 118 / 171 / 278 | 111 / 126 / 207 | explicit: generation needs persistent lanes and wrap recovery |

Target values include representation construction, transfer, membership over
all 512 nodes, download, and validation:

| context / targets | sorted sparse | dense bitset | generation dense |
| --- | ---: | ---: | ---: |
| resident / 1 | 65 / 67 / 71 | 61 / 69 / 262 | 60 / 68 / 199 |
| resident / 3 | 61 / 65 / 66 | 82 / 94 / 256 | 72 / 202 / 222 |
| resident / 64 | 63 / 65 / 199 | 60 / 63 / 73 | 59 / 61 / 62 |
| resident / 512 | 64 / 69 / 78 | 67 / 84 / 97 | 108 / 151 / 188 |
| partitioned / 1 | 59 / 61 / 67 | 64 / 133 / 313 | 64 / 194 / 257 |
| partitioned / 3 | 60 / 61 / 63 | 61 / 67 / 80 | 59 / 61 / 63 |
| partitioned / 64 | 58 / 66 / 71 | 60 / 62 / 69 | 84 / 106 / 240 |
| partitioned / 512 | 72 / 75 / 251 | 63 / 72 / 196 | 73 / 83 / 101 |

Sorted sparse is selected for the configured three-target shape and is robust
at one target. Dense candidates occasionally win at larger counts, but no
target-aware finalization proof exists, so their additional production state
would save no relation work.

Profile candidates use exact separate multiplication. Resident uses one full
edge array; partitioned uses 128-edge chunks and includes each chunk transfer.
The compact table reports medians as inline/materialized microseconds; the raw
CSV contains every five-sample distribution.

| shape / context | 2 kinds, use 1 | 64 kinds, use 1 | 2 kinds, use 16 | 64 kinds, use 16 |
| --- | ---: | ---: | ---: | ---: |
| narrow resident | 76 / 195 | 81 / 198 | 116 / 111 | 142 / 108 |
| narrow partitioned | 67 / 76 | 65 / 68 | 189 / 229 | 108 / 109 |
| broad resident | 74 / 80 | 66 / 74 | 397 / 120 | 171 / 134 |
| broad partitioned | 313 / 348 | 450 / 279 | 451 / 451 | 552 / 661 |
| dense resident | 79 / 100 | 89 / 86 | 134 / 427 | 227 / 124 |
| dense partitioned | 1,659 / 1,619 | 1,883 / 1,507 | 2,490 / 2,527 | 2,900 / 2,338 |

Winners change by shape, kind count, reuse, and residency. Materialization
therefore has no conservative total-request rule, and production would still
need a byte-bounded cache keyed by bundle plus complete canonical profile
equality. Inline remains selected and the benchmark-only prototype is retained
solely for this reproducible gate.

The lane test synchronizes submitters at a barrier and records every lane.
Batch wall time is the maximum lane time for each sample:

| topology / delay | 1 lane | 2 lanes | 4 lanes | 8 lanes |
| --- | ---: | ---: | ---: | ---: |
| resident / 0 us | 728 / 880 / 1,233 | 1,546 / 1,803 / 2,547 | 3,451 / 3,701 / 3,918 | 6,778 / 7,025 / 7,269 |
| resident / 50 us | 687 / 902 / 1,333 | 1,345 / 1,754 / 2,100 | 2,883 / 3,052 / 3,427 | 6,834 / 7,320 / 7,569 |
| resident / 5,000 us | 687 / 756 / 952 | 1,371 / 1,440 / 2,324 | 2,774 / 3,470 / 3,639 | 6,951 / 7,378 / 7,525 |
| mixed resident/partitioned / 50 us | 721 / 823 / 881 | 22,372 / 30,591 / 31,683 | 44,771 / 45,986 / 47,394 | 76,672 / 77,109 / 77,220 |

One lane wins across every delay. With zero delay, observed width remains one;
50 and 5,000 microseconds pack resident requests to 2/4/8 as configured but do
not improve throughput. Mixed batches complete without starvation but are
dominated by forced partition-cache churn; their observed widths are 1, 1/2,
and 1/4. Defaults remain one lane and zero collection delay because collection
delay cannot provide packing benefit at width one.

Four explicit delta values were measured. Values are medians in microseconds:

| workload / context | frontier | delta 0.01 | delta 0.1 | delta 1 | delta 10 |
| --- | ---: | ---: | ---: | ---: | ---: |
| broad resident | 705 | 1,337 | 1,302 | 1,286 | 1,382 |
| broad partitioned | 513 | 1,392 | 1,378 | 1,284 | 1,289 |
| dense resident | 1,693 | 3,775 | 3,681 | 3,892 | 3,676 |
| dense partitioned | 2,451 | 3,851 | 4,186 | 4,195 | 3,803 |

Frontier wins every measured context. Delta candidates have no universal
winner, so delta remains an explicitly configured algorithm/value and unused
automatic delta-candidate fields were removed.

The following alternatives were measured or costed as complete-request
strategies, not kernel-only shortcuts:

- explicit clears were retained because the primitive generation result did
  not establish a repeatable complete-request win or provide persistent lane
  state and wrap recovery;
- sorted sparse targets were retained across the configured destination range.
  Dense and generation-stamped membership add transfer/reset cost, while safe
  target-aware early stopping is unavailable at first discovery;
- inline `base * multiplier` was retained. Materialized weights pay a complete
  relation pass and device copy for one use and need a bounded, fully verified
  bundle-plus-profile cache for reuse;
- the existing bounded worker remains available, but the measured default
  maximum is one. Independent lane state and finite collection still preserve
  correctness for explicit wider configurations;
- exact paths use CUDA distance selection followed by the CPU oracle on the
  same image/bundle, with complete state/distance verification. Admission adds
  the CPU oracle's reported working bytes to CUDA search state;
- finite deterministic examined-edge budgets remain CPU-only because schedule-
  independent parity was not proven.

PTX audit confirms the four phase entry points, `atom.global.cas.b64`, separate
`mul.rn.f64` and `add.rn.f64`, and no fused multiply-add. The agreement corpus
covers the six execution modes, 1,025 parallel relations from one source,
forced source splits, normal and reverse partition schedules, sparse huge delta
buckets, zero closure, cache churn, lane isolation, exact path evidence, and
new scratch/compaction/evidence faults. NVIDIA Compute Sanitizer 2026.2.1 ran
the settled Plan 10 15-test agreement binary through `memcheck` and
`racecheck` with the repository safety script. Racecheck uses its documented
blocking-launch mode because WDDM otherwise overflows the tool's launch
tracker during the deliberate concurrent-host-launch test; the ordinary CUDA
suite retains that concurrent scheduling coverage. Memcheck reported 0 errors,
and racecheck reported 0 hazards, 0 errors, and 0 warnings. The matrix includes
the end-to-end unrepresentable-delta refusal case.
