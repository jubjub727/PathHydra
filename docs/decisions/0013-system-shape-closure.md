# Decision 0013: System-shape implementation selections

Status: accepted

This record closes the implementation alternatives that remained in the
original system-shape document after Decisions 0001--0012. It does not add a
second format or backend. The selected policies are the one current pre-release
implementation; discarded alternatives have no compatibility status.

## Ownership, identity, and host residency

- The workspace is split by current responsibility into `pathhydra-core`,
  `pathhydra-store`, `pathhydra-routing`, `pathhydra-subgraph`,
  `pathhydra-cuda`, `pathhydra-engine`, `pathhydra-api`, the finite
  `pathhydra-admin` operator tool, and `pathhydra-bench` evidence harness.
- Exact-name acceleration uses standard-library `HashMap<Box<str>, stable ID>`
  indexes rebuilt and fully validated from RocksDB at catalog open. Independent
  `RwLock`s serve exact node and relation-kind reads; confirmed mutation is
  serialized by the catalog write mutex and updates durable records and the
  corresponding map coherently. Complete string equality, never the hash,
  decides identity.
- CPU topology selection uses the explicit `max_active_image_bytes` logical
  topology threshold. At or below it, the complete immutable image is
  resident; above it, identities and source directory stay resident while a
  byte/entry/read/staging-bounded source-segment cache serves exact partitioned
  routing. There is no implicit RAM heuristic.
- Stable standalone `EdgeId` identifies every edge. Parallel, identical-looking,
  and self edges remain independent records and path evidence.

## Storage, publication, and hydration

- Decision 0010 selects the default metadata family plus eight named column
  families, fixed big-endian durable keys, exact length-prefixed values, one
  outgoing and one incoming `(NodeId, EdgeId)` index entry per edge, four
  background jobs, and fixed prefix extraction on the adjacency families.
- Decision 0008 selects immutable, independently checksummed source segments
  for the routing bundle, including bounded source splitting for high degree.
- Decision 0010 retains complete deterministic bundle rebuild and one-operation
  immutable publication. No incremental overlay, revision counter, pinned
  image, or hidden delta representation exists.
- Decision 0011 selects current-state hydration. Routing evidence retains the
  acquired image's point-in-time handles, while hydration resolves current
  confirmed records and reports missing evidence explicitly.

## Numeric and CUDA execution policy

- Decisions 0001 and 0002 select canonical nonnegative finite binary32 base
  weights and multipliers, separate binary64 multiplication and addition,
  checked finite sums, positive infinity only as internal unreachable state,
  explicit disabled relations, enabled zero, complete profiles, and stable
  predecessor ties.
- Decisions 0004--0006 select NVIDIA `sm_86` PTX authored in Rust, dynamically
  loaded through cudarc's CUDA Driver API. There is no cross-vendor abstraction
  because there is one production accelerator backend. CPU remains complete
  without NVIDIA software.
- Decision 0009 selects graph-parallel active-source task compaction, explicit
  full state reset, sorted/deduplicated sparse host target membership, inline
  exact profile multiplication, one lane with zero collection delay by
  default, and frontier for automatic/ordinary explicit CUDA execution. Delta
  stepping remains an explicit exact algorithm whose positive delta is supplied
  by the caller; no automatic delta heuristic remains.
- CUDA returns exact distances. Unlimited-budget path requests then run the
  cancellation-aware CPU oracle over the same immutable resident image or
  bundle lease and require state/distance-bit agreement before returning edge
  evidence. Finite examined-edge budgets use CPU or receive a typed
  `RequireCuda` refusal.
- `Auto` selects CPU because current end-to-end evidence establishes no
  conservative CUDA crossover. `PreferCuda` and `RequireCuda` remain explicit
  caller policies, not speed promises.

## Out-of-core transport and external libraries

Conventional bounded worker file reads, checked partition decoding, pageable
host staging, and explicit device copies are selected. The measured 12-GiB
bundle diagnostic on the RTX 3080 spent 18.13 of 50.67 seconds in Frontier
partition scheduling/I/O and 31.05 seconds in relaxation; Delta spent 36.09 of
80.11 seconds in scheduling/I/O and 41.10 seconds in relaxation. File transport
was material but not the repeatable dominant stage, so the DirectStorage
evidence gate did not trigger. DirectStorage and cuGraph are not production or
correctness dependencies. That diagnostic's CUDA topology itself was only 7.2
GiB, so it is not the separate Plan-10 topology-larger-than-device proof; the
strengthened scale gate sizes and reports `topology.bin` independently.

## Consumer and result representation

- Decision 0011 selects one synchronous in-process Rust `PathHydra` facade with
  owned DTOs and process-local request handles/cancellation. There is no hosted
  service, streaming partial response, remote-access policy, or transport
  dependency.
- Decision 0012 selects bounded canonical UTF-8 JSON for requests, responses,
  health, errors, and handle/hydrated subgraphs. Postcard remains development
  evidence and cannot decode several current internally tagged DTOs.
- The in-memory subgraph is a `BTreeSet<NodeId>` plus
  `BTreeMap<EdgeId, (NodeId, NodeId)>`. This gives deterministic ordered
  enumeration, idempotent identity insertion, atomic conflict checks, and
  incident-edge removal without normalizing graph meaning.

These choices are reflected in the current reference documents and traced to
code/tests in `docs/system-conformance.md`.
