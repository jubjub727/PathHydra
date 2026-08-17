# Dependencies, licences, and local prerequisites

PathHydra's checked Rust dependency inventory is
[`dependency-inventory.tsv`](dependency-inventory.tsv). It contains every
external package pinned by `Cargo.lock`, including duplicate package versions,
its registry source, declared licence expression, direct/transitive role,
optional/default-feature status, and known native requirement. Regenerate and
verify it without network access:

```powershell
powershell -NoProfile -File Scripts/generate-dependency-inventory.ps1
powershell -NoProfile -File Scripts/generate-dependency-inventory.ps1 -Check
```

The script parses `Cargo.lock` as the version authority, reads direct roles from
the workspace manifests, and reads licence declarations from package manifests
already present in Cargo's local registry cache. It neither resolves new
versions nor modifies `Cargo.lock`. A missing cached licence declaration is
rendered as `UNDECLARED`, making review failure visible rather than guessing.

## Distribution assessment

Every locked package currently declares a permissive option compatible with the
project's no-fee local distribution intent. Expressions containing alternatives
are used under a permissive branch: for example, `r-efi` permits MIT or
Apache-2.0 without selecting LGPL, and `librocksdb-sys` permits MIT,
Apache-2.0, or BSD-3-Clause. The `rocksdb` Rust crate declares Apache-2.0;
PathHydra uses the bundled RocksDB code under its Apache-2.0 option. This is a
technical inventory and selection record, not legal advice; release packaging
must retain the notices required by the selected licences.

Production Rust dependencies are limited to the local graph, store, routing,
engine, subgraph, and API needs. `postcard` is development-only comparison
evidence and is not part of the selected canonical boundary. `cudarc` is an
optional dependency activated only by the `cuda` feature. No crate connects to
a hosted database or paid service.

## Manually reviewed native and application-side components

| Component | Pinned/current requirement | Licence boundary | Role and required status |
| --- | --- | --- | --- |
| Rust host toolchain | Stable 1.95.0 for the recorded local baseline | Rust compiler and standard tooling are dual MIT/Apache-2.0 | Required to build; installed locally. |
| Rust CUDA kernel toolchain | `nightly-2024-02-17` with `rust-src` | Rust dual MIT/Apache-2.0 | Required only to build the `cuda` feature; never downloaded by Cargo. |
| RocksDB native library | Bundled by `librocksdb-sys 0.17.3+10.4.2` | Selected Apache-2.0 option | Required by the durable store; built locally, not a service. |
| C++ compiler | A compiler supported by `cc`/RocksDB (MSVC on the recorded Windows host) | Toolchain vendor terms | Required to compile bundled RocksDB. No PathHydra graph logic is C++. |
| LLVM/libclang | Compatible local LLVM/libclang used by bindgen | Apache-2.0 with LLVM exceptions | Required at RocksDB build time; `LIBCLANG_PATH` or `PATH` selects it. |
| NVIDIA display/compute driver | Driver compatible with embedded `sm_86` PTX and cudarc's dynamic Driver API | NVIDIA proprietary, free-to-use driver terms; no paid edition selected | Optional runtime prerequisite for CUDA. CPU correctness and data access do not require it. |
| NVIDIA Compute Sanitizer | Locally installed toolkit utility | NVIDIA toolkit terms; no paid edition selected | Optional release-validation tool, not a runtime or data-access dependency. |
| BAML | Application-side context; no Cargo dependency | Apache-2.0 | Optional consumer technology. The Rust API remains complete without BAML prompts, models, or hosted services. |
| PowerShell | Local scripts use Windows PowerShell-compatible syntax | MIT for PowerShell itself; Windows PowerShell follows platform terms | Used for reproducible operator and verification commands; not part of the graph runtime. |
| cuGraph | Not in `Cargo.lock` and not linked | Apache-2.0 | Rejected as a production dependency; current custom kernels and CPU oracle provide the required semantics. |
| DirectStorage | Not in `Cargo.lock` and not linked | API/platform terms; samples are MIT | Rejected for the current conventional-I/O path; it is not needed for correctness or recovery. |

## Local-operation boundary

Core build, CPU tests, benchmarks, database inspection, checkpoint, restore,
and recovery use only local files and processes. CUDA adds the optional local
kernel toolchain, driver, and validation utilities described above. No licence
payment, subscription, authentication credential, network access, hosted
database, or cloud service is required for correctness or access to durable
graph data. Optional application integrations cannot bypass the typed Rust API
or become recovery authorities.
