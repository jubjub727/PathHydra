# Atomic batch-ingestion evidence

Recorded 2026-08-24 on the reference Windows/x86-64 workstation described in
[system closure](system-closure.md): AMD64 Family 26 Model 68 Stepping 0,
local filesystem, `rustc 1.95.0 (59807616e 2026-04-14)`. The source baseline
before this change was `c619824`; these measurements were taken from the
working implementation before its final commit.

Command:

```powershell
$env:CARGO_BUILD_JOBS = "1"
cargo run --release -p pathhydra-bench -- --suite batch-ingestion --repeats 3
```

Every timed sample first checks aligned result cardinality and exact confirmed
node/relation/edge counts. Store metrics must show exactly one
`CandidateInsertion` write and one `ConfirmedPromotion` write for the measured
batch, and records their exact committed entry and byte counts. A failed
correctness check aborts the suite instead of emitting an
accepted timing. Candidate insertion is provisional and performs no routing
publication; the facade/engine publication contract is covered by
`pathhydra-api/tests/batch.rs`.

## Distributions

Times are nanoseconds; each bracket contains all three raw samples in execution
order. Committed bytes are the RocksDB `WriteBatch::size_in_bytes` observation
and were identical across samples.

| Workload | Entries | Insert ns (all) | Confirm ns (all) | Top-K ns (all) | Candidate / confirmed batch entries | Candidate / confirmed bytes |
| --- | ---: | --- | --- | --- | ---: | ---: |
| 10,000 nodes | 10,000 | `[4158100, 3996600, 4287600]` | `[18809400, 18551400, 19322800]` | `[17800, 11400, 9700]` | 10,001 / 30,004 | 458,930 / 727,887 |
| 10,000 relation candidates, 100 exact names | 10,000 | `[3494700, 3420500, 3322800]` | `[9835200, 10608300, 11133100]` | `[58300, 97200, 61300]` | 10,001 / 10,304 | 399,040 / 120,687 |
| 100,000 edges over 1,000 confirmed nodes | 100,000 | `[162654100, 153541400, 165444800]` | `[322531600, 306421000, 319339200]` | `[34500, 29500, 58800]` | 100,004 / 400,007 | 5,200,147 / 11,500,214 |
| Mixed 10,000 nodes, 4 kinds, 100,000 edges | 110,004 | `[38253000, 39516700, 38753800]` | `[225133900, 241089000, 253228300]` | `[29700, 37600, 32700]` | 110,005 / 430,020 | 5,659,086 / 12,228,347 |

The mixed generator includes parallel edges, one self-edge per 1,000 edges,
four shared relation-kind dependencies, and request-local node/relation
references. Median insert/confirm times were 38.754/241.089 ms. The 100,000
confirmed-edge workload medians were 162.654/319.339 ms. No speedup threshold
is treated as an API promise.

Windows peak-process working set is a process-lifetime high-water mark and
cannot reset between rows. It rose from 23,916,544 bytes in the first sample to
91,172,864 bytes at the final mixed sample; unavailable measurements would be
reported as `unavailable`, never zero. The largest measured request remained
well below the selected 1 GiB estimated-batch and 512 MiB decoded-payload
bounds. The 120,000-entry default accommodates the 110,004-entry named mixed
workload with finite headroom.

## Additional executable evidence

- `batch_ingestion::concurrent_same_kind_usage_updates_have_no_lost_counts_or_cleanup`
  runs concurrent provisional increments and confirmed decrements against one
  relation kind, verifies exact transfer counts, and observes one final
  automatic cleanup.
- `batch::duplicate_name_only_batch_consumes_candidates_without_publication`
  proves the no-build path.
- The workspace checkpoint/restore and strict corruption suites recompute the
  candidate dependency graph, both usage domains, and the popularity index.

Deletion/publication timing distributions remain part of the existing
`operations` and system benchmark suites; this batch suite isolates the new
large insertion, confirmation, committed-byte, peak-memory, and top-K phases.
