# Public API and safety audit

This is the final pre-release audit for the one current PathHydra Rust API.
It covers the workspace at the Plan 10 closure commit; it is not a promise to
retain unreleased signatures or formats.

## Public boundary

| Crate | Public role | Invalid-input and allocation contract | Executable evidence |
|---|---|---|---|
| `pathhydra-core` | checked stable IDs, exact names, payloads, records, canonical weights | checked constructors reject invalid numeric/name/payload evidence; IDs remain distinct newtypes | core unit tests and `pathhydra-api/tests/dto.rs` |
| `pathhydra-store` | catalog mutation/read, strict inspection, checkpoint, restore, compaction, metrics | typed catalog/operation errors; bounded verification and maintenance; fallible disk operations; no raw public RocksDB handle | store catalog, mutation, scan, and operations suites |
| `pathhydra-routing` | immutable images/bundles, profiles, requests/responses, resident/partitioned CPU routing | checked profile/numeric/bundle decoding; fallible working-set and frontier growth; controlled cancellation | routing unit, controlled, bundle, analytic, and system-conformance suites |
| `pathhydra-cuda` | safe optional CUDA context/images/scheduler and typed diagnostics | checked ABI counts and reservations precede allocation/launch; typed CUDA failures; cancellation/fault paths synchronize before releasing resources | ABI, batching, agreement, recovery, sanitizer, and engine CUDA suites |
| `pathhydra-engine` | sole lifecycle owner for graph, publication, routing, hydration, maintenance, and shutdown | typed configuration/admission/lifecycle/fallback errors; immutable image leases; explicit shutdown reports | engine API, operations, durable publication, CUDA, and system-conformance suites |
| `pathhydra-subgraph` | caller-owned deterministic handle composition | checked endpoint/path invariants; no catalog mutation | subgraph unit/integration suite |
| `pathhydra-api` | finite owned consumer facade and canonical JSON DTO boundary | byte/depth/cardinality/string/payload/diagnostic limits are checked before mutation; no internal paths/handles/reasons escape | DTO, encoding, malformed, lifecycle, and concurrency suites |
| `pathhydra-admin` | finite aggregate inspection/maintenance CLI | strict read-only default; explicit exact target for mutations; no raw record editor | admin parser/privacy/nonmutation integration suite |

The public facade has no generic query language, arbitrary command map, raw
database/image reader, migration/version surface, historical image handle, or
consumer-visible worker/cache/device pointer. Public iterators borrow their
owners; owned DTOs remain valid after engine results are dropped. Constructors
and fallible operations document their return contract, and result/report
types that must be observed are marked `must_use` where applicable.

## Panic and allocation review

Workspace production crates forbid unsafe Rust except the named CUDA boundary.
Searches for `unwrap`, `expect`, direct indexing, and unchecked casts were
reviewed in public-input paths. Remaining production assertions are either
startup/build invariants established by earlier validation or internal
impossibility checks; external malformed records, bundles, API bytes, numeric
evidence, paths, and resource counts return typed errors. Large collections use
checked arithmetic plus admission or fallible reservation before growth.

The benchmark, examples, build script, and tests intentionally use assertions
and `expect` to make evidence failures visible; they are not consumer input
boundaries. The canonical decoder completes syntax, limits, and cross-field
validation before invoking a mutating facade method.

CUDA scheduler construction validates a nonzero lane count and reports OS
worker/lane thread-creation failures as typed `Worker` failures. Scoped lane
joins also translate an unexpected lane panic into a typed caller result while
releasing the active-lane count. Batch and lane-join queues reserve fallibly and
return typed `Allocation` failures before growth. The public boundary does not
use a panic-producing thread-spawn convenience API.

The safe resident and partitioned CUDA route entry points independently
recompute their complete request estimate—including same-image CPU path
evidence—and reject an undersized caller-provided reservation before mapping
state is allocated or a kernel is launched. Engine admission remains the owner
of the corresponding maximum/concurrency reservation.

## Unsafe boundary

All workspace crates inherit `unsafe_code = "forbid"` except
`pathhydra-cuda`, which denies unsafe code crate-wide and permits it only in the
private host launch module. Device code is isolated in the separately compiled
`pathhydra-cuda-kernel` package. No store, file-I/O, routing, engine, API, admin,
or subgraph code uses unsafe Rust.

Every host launch unsafe block has a local `SAFETY` statement naming the ABI,
buffer, lifetime, alignment, and synchronization obligation. Each device
module states the invariant applying to every unsafe block in that module, and
each public unsafe kernel/helper has a `# Safety` contract. The device arrays
are length-checked before access; mutable buffers remain live until explicit
synchronization; status output prevents invalid partial state from becoming a
routing response. Decision 0005 and `docs/cuda-safety.md` remain authoritative
for ownership and ABI details.

The current kernel PTX entry/parameter audit, ABI tests, memcheck, and racecheck
must all pass after any unsafe-boundary edit. The final local commands are:

```powershell
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features cuda -- -D warnings
cargo doc --workspace --no-deps
powershell -NoProfile -File Scripts/sanitize-cuda-tests.ps1
```

No GitHub workflow, network service, paid dependency, or NVIDIA software is
required for the CPU-only build and test path.
