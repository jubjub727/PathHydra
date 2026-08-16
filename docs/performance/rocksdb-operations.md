# RocksDB operations workload

The `pathhydra-admin workload` command is the repeatable correctness-first
Plan 08 store workload. It creates a fresh synthetic database and covers:

- sequential candidate insertion and promotion;
- deterministically shuffled node, relation-kind, and edge promotion;
- exact-name hits and misses;
- high-degree outgoing and incoming adjacency reads;
- random edge deletion and cascading high-degree node deletion;
- Plan 06 streaming confirmed scan and bundle build;
- flush followed by restart and complete validation;
- RocksDB checkpoint and validated restore;
- churn, flush, database bytes, and pending-compaction properties;
- confirmed mutation followed by a complete immutable bundle rebuild.

Every reported measurement has `correctness_verified:true`. Latency
distributions contain the individual monotonic nanosecond samples plus median
and p95. The report also contains OS, architecture, scale, final catalog
checksum, database byte counts, and explicit booleans for evidence that the
public API cannot collect.

## Reproduce

Use a fresh ignored destination; never point the workload at a live catalog:

```powershell
cargo run --release --manifest-path crates/pathhydra-admin/Cargo.toml -- `
  workload `
  --root target/pathhydra-plan08-workload `
  --scale 10000 `
  --samples 100 `
  > target/pathhydra-plan08-workload.json
```

Record CPU, memory, storage device/filesystem, operating system, Rust toolchain,
PathHydra commit, RocksDB crate/native version, scale, samples, and whether the
machine was otherwise idle. Compare complete distributions and correctness,
not only a single throughput number.

## Representative local run

The initial local evidence run is intentionally modest so it can be repeated in
the development environment. It is a smoke baseline, not a production capacity
claim.

```text
date: 2026-08-17 (Pacific/Auckland)
command: cargo run --release --manifest-path crates/pathhydra-admin/Cargo.toml -- workload --root target/pathhydra-plan08-admin-evidence-20260817 --scale 64 --samples 10
source: pre-commit Plan 08 worktree based on f46100f7714dd4274e417ddf988c5a479168a3bc
toolchain: rustc/cargo 1.95.0, x86_64-pc-windows-msvc, LLVM 22.1.2
store: rocksdb crate 0.24.0, librocksdb-sys 0.17.3 + RocksDB 10.4.2
machine: AMD64 Family 26 Model 68, 16 logical processors, 33,446,752,256 available memory bytes
OS: Microsoft Windows NT 10.0.26200.0
D: free bytes before run: 352,994,443,264
catalog checksum: b7812adc17928ea8
```

| Workload | Operations | Median | p95 |
| --- | ---: | ---: | ---: |
| Sequential candidate insertion | 64 | 4.9 us | 7.7 us |
| Sequential candidate promotion | 64 | 6.3 us | 10.6 us |
| Random node/relation insertion | 64 | 4.8 us | 5.5 us |
| Random node/relation promotion | 64 | 6.4 us | 8.1 us |
| Random edge insertion | 64 | 4.7 us | 6.3 us |
| Random edge promotion | 64 | 7.3 us | 8.8 us |
| Exact-name hit | 64 | 0.1 us | 0.1 us |
| Exact-name miss | 64 | 0.1 us | 0.2 us |
| High-degree outgoing read (64 edges) | 10 | 27.2 us | 28.0 us |
| High-degree incoming read (64 edges) | 10 | 27.0 us | 27.9 us |
| Random edge deletion | 32 | 6.6 us | 8.3 us |
| Cascading node deletion (128 edges) | 1 | 447.1 us | 447.1 us |
| Streaming bundle scan/build (160 edges) | 1 | 22.3054 ms | 22.3054 ms |
| Restart/full validation (1,128 records) | 1 | 37.4882 ms | 37.4882 ms |
| Checkpoint (118,339 bytes) | 1 | 16.3253 ms | 16.3253 ms |
| Restore/full validation (1,128 records) | 1 | 52.5269 ms | 52.5269 ms |
| Churn plus synchronous flush (64 edges) | 1 | 46.5298 ms | 46.5298 ms |
| Fixed-scope compaction (9 families) | 1 | 19.8031 ms | 19.8031 ms |
| Confirmed mutation before rebuild | 1 | 0.2888 ms | 0.2888 ms |
| Complete rebuild after mutation (160 edges) | 1 | 13.0316 ms | 13.0316 ms |

Database bytes were 118,394 before the churn stage, 346,148 after churn/flush,
and 345,195 after fixed-scope compaction; pending compaction bytes were zero.
Directory size includes current RocksDB operational files, so it is not the
same measure as live SST bytes. Single-sample maintenance results do not define
a latency distribution; production evidence must increase scale and repeat
whole database builds on the target storage system.

## Target-scale store run and option comparison

The required command was rerun at `--scale 10000 --samples 100` on the same
machine. The current-option run produced checksum `e4e106f0c3d1de6d`, used
9,355,041 database bytes before churn, 10,103,113 after churn, and 10,102,275
after fixed-scope compaction. Pending compaction bytes were zero. Selected p95
results were 6.1 us candidate insertion, 7.8 us node promotion, 10.6 us edge
promotion, 0.4 us exact-name hit, 5.52 ms outgoing and 5.55 ms incoming
10,000-degree reads, 54.64 ms cascading deletion of 20,000 adjacencies,
123.88 ms streaming build of 25,000 edges, 203.01 ms restart validation of
175,008 records, 238.95 ms checkpoint, and 267.67 ms restore/full validation.
All correctness flags were true.

An immediately adjacent comparison removed both explicit settings and used
RocksDB's default background-job count with no adjacency prefix extractor. It
was then removed from the codebase, as required for one current configuration.
At the same scale it measured 5.73/5.59 ms outgoing/incoming reads, 135.82 ms
streaming build, 204.92 ms restart validation, 237.43 ms checkpoint, 289.30 ms
restore, and 204.19 ms complete rebuild. The current settings measured
5.52/5.55 ms, 123.88 ms, 203.01 ms, 238.95 ms, 267.67 ms, and 202.28 ms for
those rows. Default settings slightly improved some point writes and the
cascade sample, but lost the read/build/restore improvements that motivated
the bounded current settings; neither alternative materially won every named
workload.

Application write rate is derivable from the emitted complete per-operation
duration arrays (the median point-write rates in this run were approximately
125k--200k operations/s). Directory space amplification from churn was 1.080x
and compaction retained 0.9999x of the post-churn directory bytes. RocksDB
internal physical read/write amplification and ticker cache hit/miss counts are
`Unavailable` in this configuration rather than reported as zero because
statistics are deliberately disabled. Current workload reports also emit
platform process read/write transfer-byte deltas where the operating system
exposes them, the catalog's logical committed-write and confirmed-scan bytes,
and a clearly named process-I/O-to-final-catalog-size ratio. That ratio includes
bundle, checkpoint, restore, and filesystem-cache traffic; it is application
workload amplification evidence, not a claim about RocksDB device write
amplification. Unsupported platform counters remain JSON `null`. The elevated
target-scale rerun measured 57,705,824 process read-transfer bytes, 58,264,463
process write-transfer bytes, 3,440,765 logical committed-write bytes,
3,490,188 confirmed-scan bytes, and an explicitly scoped
process-I/O-to-final-catalog-size ratio of 11.479586. It measured a peak working
set of 72,359,936 bytes using Windows' process peak-working-set counter; the
workload emits `null` on platforms where these counters are unavailable.
Per-family live/total SST, memtable, compaction,
flush, background-error, write-stop, delayed-write, and block-cache
capacity/usage properties remain in the structured metrics snapshot. Exclusive
route blocking is measured by the engine suite below.

## Interpretation and current selection

The retained layout is the default metadata family plus eight current named
families. Adjacency remains one entry per directed edge in each direction with
an eight-byte fixed node prefix. The workload directly exercises the read and
delete amplification this creates. No alternative layout is retained because
there is no repeatable material win across every required workload.

The current options keep four background jobs, adjacency prefix extractors,
WAL-enabled non-sync ordinary writes, and explicit synchronous flush for
checkpoint/shutdown. Block-cache hits/misses are unavailable because global
ticker statistics are not enabled; the report must not call them zero.

The stable store API exposes one bounded `compact_all` operation over the fixed
current family set. `explicit_compaction_available:true` accompanies the
compaction latency and database bytes before churn, after churn, and after
compaction. It does not expose arbitrary RocksDB ranges or family mutation.

## Engine publication blocking

The engine-coordinated suite is repeatable with:

```powershell
cargo run -p pathhydra-bench --release -- --suite operations
```

It ran five samples for node promotion, edge promotion, edge deletion, and a
high-degree cascading node deletion at 256, 1,024, and 4,096 nodes. The cascade
fixture has 12,285 directed edges at the largest size. Every concurrent route
observed the exclusive publication interval and returned a correct current
result. The table records the p95 (maximum of five) in milliseconds from the
2026-08-17 run on the machine identified above.

| Nodes | Mutation | Mutation p95 | Rebuild p95 | Blocked route p95 |
| ---: | --- | ---: | ---: | ---: |
| 256 | node promotion | 17.80 | 16.79 | 16.97 |
| 256 | edge promotion | 22.33 | 21.20 | 21.54 |
| 256 | edge deletion | 20.66 | 19.71 | 19.92 |
| 256 | high-degree node deletion | 23.78 | 20.99 | 22.90 |
| 1,024 | node promotion | 25.13 | 23.87 | 24.09 |
| 1,024 | edge promotion | 26.31 | 25.26 | 25.46 |
| 1,024 | edge deletion | 27.14 | 26.01 | 26.22 |
| 1,024 | high-degree node deletion | 34.35 | 26.35 | 33.41 |
| 4,096 | node promotion | 50.58 | 49.53 | 49.77 |
| 4,096 | edge promotion | 49.35 | 48.31 | 48.46 |
| 4,096 | edge deletion | 49.90 | 48.80 | 48.99 |
| 4,096 | high-degree node deletion | 80.29 | 50.47 | 79.31 |

The named first-release local-interactive bound is graphs up to 4,096 nodes and
12,285 directed edges, with p95 confirmed mutation below 110 ms and p95 route
blocking below 100 ms on this reference machine. These results meet that bound.
Larger or lower-latency deployments must rerun the suite and may require a
separately reviewed overlay design. `route_publication_blocking_measured:true`
and `overlay_implemented:false`: the selected behavior remains one complete
immutable rebuild, with no graph revision API or hidden overlay. Correctness is
checked by comparing the route admitted during publication with a second route
from the stable post-publication image, including exact destination states,
distance bits, path evidence, policies, and completion counters.
