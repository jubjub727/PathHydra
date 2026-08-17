# Out-of-core baseline

Measurements below were taken on 2026-08-17 on Windows, an AMD64 16-logical-CPU
host, an SSD-backed NTFS workspace, and an NVIDIA GeForce RTX 3080 (10,240 MiB,
driver 610.88, CUDA Driver API target 13030). The Rust toolchain was 1.95.0; CUDA
kernels used the repository's pinned nightly Rust-to-PTX toolchain. Hardware
disk model queries were access-restricted, so the storage model and bus are not
reported.

The ordinary release matrix uses generated chain, broad-star, dense-region,
zero-weight-closure, disconnected, and mixed-locality fixtures. It times
resident CPU, cold and warm partitioned CPU, a one-entry host-cache thrash case,
resident CUDA frontier/delta, and cold/warm partitioned CUDA frontier/delta.
Every timed response is first compared with an untimed resident CPU oracle;
all matrix rows passed exact state and binary64-distance comparison. These
fixtures establish coverage and counters, not a speedup claim.

After current-bucket delta grouping replaced full partition scans, the cold
disconnected fixture required 2 device-cache misses instead of the earlier 117;
mixed locality required 9 instead of 27. The result remained bit-exact. These
figures demonstrate removal of unrelated partition work, not a general
throughput claim.

## Historical Plan-06 topology proof

The manual scale gate writes its generated bundle beneath the ignored `target`
directory and defaults explicitly to the required 12-GiB target:

```powershell
cargo run -p pathhydra-bench --release --features cuda -- --suite scale target/pathhydra-scale-bundle 12
```

That run generated and reopened a production bundle with:

| Field | Observed value |
| --- | ---: |
| Bundle bytes | 12,885,060,883 |
| Adjacencies | 644,245,096 |
| Partitions | 922 |
| Hard topology bytes per partition | 8 MiB |
| Host topology cache | 64 MiB |
| Device topology cache | 32 MiB |
| CPU partitioned route | 34.892394 s, exact |
| CUDA partitioned frontier route | 571.260466 s, exact |
| Route file bytes | 12,884,935,112 |
| Device transfer bytes | 7,730,952,216 |
| Device-cache misses | 922 |
| Observed process working set during CUDA route | about 154.5 MiB |
| Observed process private bytes during CUDA route | about 393.4 MiB |

The analytic workload has two nodes, one confirmed relation kind, and parallel
unit-weight directed edges. Its exact destination distance is 1.0. The
production reader validated the five files before routing, and both CPU and
CUDA returned that answer without materializing the complete topology in host
or device memory. This run used the explicit host-loading/copying/ready/in-use
cache lifecycle and completion-event-gated eviction. The generated 12 GiB child was deleted after validation; it
is reproducible and is intentionally not committed.

## Superseded Plan-10 bundle regression after task compaction

Plan 10 reran the capacity gate after graph-parallel task compaction. The
two-node workload above had become an unhelpful single-address atomic hotspot,
so the current generator retains one 644,245,096-edge origin expansion but
fans it across 1,048,576 destinations. Every destination still has the exact
analytic distance `1.0`, and every CPU/CUDA route still reads all 922
partitions. The command was:

```powershell
cargo run -p pathhydra-bench --release --features cuda -- --suite scale target/pathhydra-scale-fanout-12gib 12
```

The 12,901,838,083-byte bundle generated in 56.100900 s. Cold/warm CPU routes
were exact in 40.718706/40.576873 s. Partitioned CUDA Frontier was exact in
50.670165 s and Delta 0.1 in 80.109480 s. All pre-cancelled rows returned
`cancelled`; restart validation took 37.915033 s and reported `rebuilt=false`.
Peak process working set was 213,684,224 bytes, observed device use increased
by 1,267,204,096 bytes, host cache/staging high-water was
55,923,904/13,980,976 bytes, and device cache high-water was 16,777,152 bytes.
The run proved bounded partitioned execution over a bundle larger than device
memory, but it did not prove the stricter Plan-10
topology-larger-than-device condition. `topology.bin` was 7,730,974,344 bytes
(about 7.2 GiB); the rest of the 12.9-GB bundle was CPU-only stable-edge
evidence. The final generator therefore sizes CUDA-consumed topology bytes,
uses a fresh directory, and must be rerun with:

```powershell
cargo run -p pathhydra-bench --release --features cuda -- --suite scale target/pathhydra-scale-topology-12gib 12
```

## DirectStorage gate

The current scale report separates phases. Frontier's 50.670165 s comprised
18.129746 s of partition scheduling/I/O, 1.343365 s of task compaction, and
31.047982 s of relation relaxation. Delta's 80.109480 s comprised
36.088368 s, 2.646607 s, and 41.098345 s in those stages. File transport is
material but not repeatably dominant, so no DirectStorage spike or production
dependency was added. Conventional bounded worker reads and explicit CUDA
copies remain the portable correctness baseline. This result is a capacity and
exactness proof only; it is explicitly not a CPU or CUDA speedup claim.
