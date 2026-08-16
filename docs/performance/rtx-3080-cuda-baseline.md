# RTX 3080 CUDA baseline

Hardware: NVIDIA GeForce RTX 3080, compute capability 8.6, 10,240 MiB. Driver:
610.88 (CUDA driver capability 13.3). Host compiler: Rust 1.95.0. Kernel
compiler: `nightly-2024-02-17`. PTX target: `sm_86`.

The correctness-first harness emits CSV and validates every timed CUDA result
against CPU before recording it:

```powershell
cargo run -p pathhydra-bench --release --features cuda -- --suite baseline
```

Named workloads are a 128-node long chain, 512-node broad star, 48-node dense
strongly connected graph, and 128-node all-zero closure. Each reports CPU,
frontier, and delta-stepping timing, topology/upload bytes and time, toolchain,
device, driver, and correctness. Module JIT is incurred once before workload
timing; each workload reports its own cold topology upload and warmed route.

The first exact kernels prioritize inspectability and use one device execution
thread per independent search lane. On these local workloads they do not
establish a conservative, repeatable crossover that justifies automatic GPU
selection over the optimized CPU reference. Consequently `Auto` selects CPU;
`PreferCuda` is the explicit accelerator policy. No speedup claim is made.
Raw CSV is intentionally regenerated for the current build rather than treated
as an unreleased compatibility artifact.

Measured warm route time in microseconds (all correctness fields were `true`):

| workload | CPU | CUDA frontier | CUDA delta | cold upload frontier/delta |
| --- | ---: | ---: | ---: | ---: |
| narrow-chain | 1 | 300 | 1,514 | 4,314 / 190 |
| broad-star | 13 | 918 | 1,357 | 289 / 251 |
| dense-scc | 10 | 2,547 | 2,248 | 530 / 379 |
| zero-closure | 1 | 304 | 606 | 206 / 150 |

The first upload includes module/context cold effects; subsequent upload rows
show steady context behavior. The CUDA Driver API version query returned 13030.

NVIDIA Compute Sanitizer 2026.2.1 from the CUDA 13.3.1 toolkit ran the complete
agreement suite. `memcheck` reported 0 errors. `racecheck` reported 0 hazards,
0 errors, and 0 warnings. The run used
`Scripts/sanitize-cuda-tests.ps1` after explicitly enabling NVIDIA's WDDM
debugger interface as documented in `docs/cuda-safety.md`.
