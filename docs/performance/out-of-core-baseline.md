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

## Topology larger than local device residency

The manual `--suite scale` run generated and reopened a production bundle with:

| Field | Observed value |
| --- | ---: |
| Bundle bytes | 12,885,060,883 |
| Adjacencies | 644,245,096 |
| Partitions | 922 |
| Hard topology bytes per partition | 8 MiB |
| Host topology cache | 64 MiB |
| Device topology cache | 32 MiB |
| CPU partitioned route | 40.016492 s, exact |
| CUDA partitioned frontier route | 552.255273 s, exact |
| Route file bytes | 12,884,935,112 |
| Device transfer bytes | 7,730,952,216 |
| Device-cache misses | 922 |
| Observed process working set during CUDA route | about 154.5 MiB |
| Observed process private bytes during CUDA route | about 393.4 MiB |

The analytic workload has two nodes, one confirmed relation kind, and parallel
unit-weight directed edges. Its exact destination distance is 1.0. The
production reader validated the five files before routing, and both CPU and
CUDA returned that answer without materializing the complete topology in host
or device memory. The generated 12 GiB child was deleted after validation; it
is reproducible and is intentionally not committed.

## DirectStorage gate

Conventional checked I/O was not the repeatable dominant stage in the scale
result: the CPU file pass completed in about 40 seconds while partitioned CUDA
processing took about 552 seconds. Device work, synchronization, and eviction
amplification dominate this workload, so no DirectStorage spike or production
dependency was added. Conventional bounded worker reads and explicit CUDA
copies remain the portable correctness baseline. This result is a capacity and
exactness proof only; it is explicitly not a CPU or CUDA speedup claim.
