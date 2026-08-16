# Plan 10: System Conformance and Performance Closure

## Outcome

Prove that the implemented PathHydra Rust engine satisfies every in-scope
contract in `PATHHYDRA_SYSTEM_SHAPE.md`, close every deliberately open decision,
replace placeholder performance statements with reproducible evidence, and
leave one current, internally consistent set of code, fixtures, documentation,
and operational procedures.

This plan is not where missing production features from Plans 07-09 are quietly
implemented. Those plans must meet their own definitions of done first. Plan 10
adds system-wide verification, measurement, decision closure, dependency/license
evidence, and documentation corrections that only make sense after all
components exist.

## Entry criteria

Before starting:

- Plans 00-09 are implemented;
- ordinary workspace gates pass;
- real-device CUDA agreement and sanitizer pass for resident and partitioned
  modes;
- backup/restore, shutdown, and consumer encodings have component tests;
- the worktree contains no unexplained generated or benchmark artifacts.

Any failed entry item is returned to its owning plan. Plan 10 may add a missing
cross-component test or metric, but not redesign the component.

## Explicit non-goals

- BAML prompts, models, workflows, factual candidate validation, or final graph
  composition policy;
- a remote service, authentication, tenancy, or cloud deployment;
- approximation, fuzzy names, rule processing, or negative weights;
- a second accelerator without a separate product requirement;
- format/schema versions, compatibility layers, migrations, graph revision
  counters, or pinned historical image APIs before the first release;
- universal latency/throughput guarantees;
- GitHub Actions workflows.

## 1. Build an executable conformance ledger

Create `docs/system-conformance.md` with one row for every normative statement
and open decision in `PATHHYDRA_SYSTEM_SHAPE.md`. Each row has:

- requirement or decision;
- implemented code/API owner;
- decision record where applicable;
- authoritative tests;
- operational/performance evidence where applicable;
- status: implemented, selected policy, or explicitly outside system scope.

No row may be “future,” “TBD,” or implicitly covered. The ledger is reviewed
against the source document section by section. Generate a lightweight local
check that validates referenced paths/test names and fails when stale markers
are reintroduced.

## 2. Close every deliberately open decision

Update the original open-decision list so each entry points to an accepted
decision or measured current implementation. At minimum close:

- workspace/crate boundaries;
- exact-map implementation and lock strategy;
- CPU resident/partition thresholds;
- CUDA vendor/API, algorithms, deltas, batch width, and lane scheduling;
- numeric/edge identity policies;
- column families, key encoding, adjacency representation, and store options;
- complete rebuild versus incremental/overlay publication;
- current-state hydration rather than snapshot retention;
- CUDA path/additional state policy;
- profile inline/materialization policy;
- target membership representation;
- conventional out-of-core transport and DirectStorage gate result;
- selected Rust process/packaging boundary and result shape;
- selected canonical request/response/subgraph encoding;
- ordered in-memory subgraph representation.

Where current behavior wins, record measured reasons and remove discarded spike
code. Do not keep parallel implementations “for later” without an active use.

## 3. Add deterministic generated graph verification

Build an internal deterministic generator without relying on wall-clock seeds.
Generate small directed multigraphs containing:

- isolated nodes;
- parallel edges and self-edges;
- zero and maximum canonical base weights;
- sparse stable IDs after deletion;
- multiple relation kinds with enabled, disabled, zero, and boundary
  multipliers;
- zero-weight strongly connected components;
- equal-cost alternatives;
- unreachable regions;
- high-degree and dense components.

For each graph/profile/origin/destination set:

1. calculate an independent exact Bellman-Ford-style distance oracle using the
   declared numeric operations;
2. run resident CPU;
3. run forced-partition CPU across multiple partition/cache sizes;
4. run every eligible resident/partitioned CUDA algorithm;
5. compare exact distance bits and states;
6. validate every returned path's direction, relation kind, stable edge ID,
   effective weights, continuity, simple predecessor chain, and exact sum;
7. verify destination order/duplicates and completion reasons.

Seeds are printed on failure and can be replayed. Generated tests use bounded
case counts in ordinary runs and a larger explicit local stress command.

## 4. Exhaust numeric and tie boundaries

Create targeted numeric cases around:

- smallest/large canonical binary32 base weights and multipliers;
- enabled zero versus disabled relation;
- separate binary64 multiplication/addition rounding boundaries;
- finite product and path-sum overflow;
- infinity/unreachable representation;
- exact equality and one-ULP distance differences;
- delta bucket-index representability;
- zero cycles and stable predecessor ties.

Audit generated PTX/SASS evidence used by the project to ensure no fused
operation violates Decision 0002. CPU, resident CUDA, and partitioned CUDA must
agree on supported cases. Unsupported numeric shapes fail before partial
results are misclassified.

## 5. Exhaust lifecycle and exact-name behavior

Generate exact names varying case, whitespace, punctuation, embedded null where
the public string type permits it, scripts, combining sequences, and long valid
UTF-8. Prove full equality after hash lookup and across restart, checkpoint,
restore, encoding, and duplicate promotion.

Exercise candidate sequences interleaved with confirmation/deletion/routing and
prove candidates remain absent from confirmed lookup, bundles, all CPU/CUDA
modes, hydration, health/metrics, and encoded confirmed results. External
validation remains a test fixture action, not engine logic.

## 6. Exhaust mutation, publication, and snapshot isolation

Run concurrent schedules combining:

- node/relation/edge candidate insertion;
- duplicate exact-name confirmation;
- edge confirmation/removal;
- high-degree node removal;
- resident/partitioned CPU/CUDA routes;
- hydration and subgraph hydration;
- bundle rebuild/recovery;
- health, checkpoint, and shutdown attempts.

Assert each route uses one complete old or new image. After a confirmed deletion
commits, no newly admitted route can see removed graph material. Old admitted
routes may complete only through their immutable image lease. Hydration retains
documented current-state behavior and identifies missing evidence rather than
substituting records.

## 7. Exhaust resource limits and cancellation

Test zero/maximum/overflow-adjacent configuration and request values for:

- graph/image/bundle counts and bytes;
- destinations, profiles, paths, hydration handles, and subgraphs;
- CPU route slots/bytes;
- host cache entries/bytes/read queue;
- CUDA lanes/search/cache/staging/event/task bytes;
- API encoded bytes and decoder collections;
- retired bundles, checkpoints, and maintenance workers.

For each admitted state, cancel before/while/after work and assert reservations,
IDs, cache pins, I/O waiters, CUDA events, file handles, and worker slots are
released exactly once. A resource limit produces typed refusal/incomplete state
according to its contract, never `Unreachable` or process abort.

## 8. Run the complete corruption and crash campaign

Corrupt or truncate every durable record class, adjacency index, routing pointer,
bundle manifest/file/partition, checkpoint component, and canonical API
encoding. Assert visible typed failure and no normalization or best-effort
guessing.

Rerun the complete Plan-06 subprocess publication/crash matrix with randomized
but replayable termination points, then combine it with Plan-08 checkpoint and
restore failures. Verify:

- committed graph writes survive according to selected durability;
- uncommitted mutations do not appear;
- candidates/confirmed records never cross namespaces;
- startup publishes a fully valid bundle or reports unavailable;
- missing/corrupt bundles rebuild without graph loss;
- deletion cascades remain atomic;
- no cleanup escapes configured roots.

## 9. Verify backup, restore, and rebuild equivalence

For generated and named datasets:

- checkpoint while idle and under allowed concurrent reads;
- restore into a fresh location;
- compare every candidate/confirmed record and exact lookup;
- rebuild a byte-deterministic routing bundle under identical config;
- compare CPU/CUDA responses and hydrated data;
- prove omitted routing bundles regenerate;
- rehearse failed restore without touching source/live destinations.

Document recovery time and space, but do not make restored bundle byte equality
an external compatibility promise.

## 10. Complete the required benchmark harness

Replace the single `--suite baseline` shape with explicit named suites and
machine-readable CSV/JSON reports while retaining human summaries:

- `store-ingest`;
- `store-mutation`;
- `snapshot-build-load`;
- `cpu-routing`;
- `cuda-resident`;
- `cuda-out-of-core`;
- `concurrency`;
- `reconstruction-hydration`;
- `backup-restore`;
- `scale`;
- `all` for a bounded standard local matrix.

Reports include hardware, OS, Rust/kernel toolchains, CUDA driver/device,
storage volume/type when discoverable, configuration, seed/dataset, cold/warm
state, node/relation/edge/partition counts, durable/bundle/resident/search bytes,
and correctness status.

No timing row is emitted before its result agrees with the untimed oracle.

## 11. Measure every performance field from the design

Collect at least:

- vertex and adjacency counts;
- RocksDB and routing-bundle bytes;
- snapshot scan/build/validate/load time and peak memory;
- profile packing/materialization time;
- edges examined and relaxation attempts/updates;
- frontier/bucket/partition/cache statistics;
- time to first completed destination and full completion;
- concurrent routes and aggregate throughput;
- host/device reservation and high-water use;
- path reconstruction and hydration time;
- checkpoint/restore time and bytes;
- near, far, unreachable, narrow, broad, high-degree, dense, and churn cases.

Use distributions with warmup and repeated samples; report median and spread.
Keep raw local reports reproducible but do not check huge generated databases or
bundles into source.

## 12. Regress the real out-of-core scale proof

Rerun the opt-in topology generator and benchmark completed by Plan 06. On the
local RTX 3080, regenerate at least a 12-GiB topology bundle with analytic route
answers. Confirm after Plans 07-09 that:

- complete topology is not resident in host or device memory;
- global metadata/search state stays within declared limits;
- cold and warm partitioned CPU routes are exact;
- partitioned frontier/delta CUDA routes are exact where eligible;
- cache/staging high-water values remain bounded;
- cancellation and device fallback work under scale;
- restart validates/reopens without a RocksDB rebuild when the bundle is valid.

Record disk consumed, generation/build/open duration, peak host/device memory,
cache sizes, partitions read, and total runtime. This is a manual local gate, not
an ordinary test, but it is required completion evidence.

## 13. Finalize the alternative transport decision

Review Plan 06's conventional-I/O traces and DirectStorage evidence-gate result.
Rerun its transport measurements only if Plan 07 materially changed the share
of time in file read, checksum/decode, staging, copy, kernel, or synchronization.
Record the final selected conventional transport policy and the evidence for
not selecting an alternative.

If Plan 06's isolated DirectStorage comparison was triggered, include its
availability, licensing, complexity, integrity, cancellation, and end-to-end
result. Production adoption still requires a separate optional transport plan;
Plan 10 does not add a Windows-only correctness dependency.

Likewise record whether an external GPU library adds useful independent
reference evidence. Do not add cuGraph to the production dependency graph merely
to close a checklist.

## 14. Validate observability against execution

For deterministic runs, reconcile request diagnostics and health counters with
known events:

- queue/execution/reconstruction/hydration durations are monotonic and scoped;
- destination completed/unresolved counts equal response states;
- examined/relaxation/frontier/bucket/partition counters match test hooks;
- reservation/high-water values cover actual owned buffers;
- cache hits/misses/evictions/coalesced loads reconcile;
- image ages/references/build failures and retired bytes reconcile;
- RocksDB write/compaction/cache values distinguish unavailable from zero;
- fallbacks/cancellations/reinitializations increment once.

Audit logs and encoded health for payloads, names, profiles, destinations, paths,
raw file content, and sensitive absolute paths.

## 15. Audit dependencies, licences, and local reproducibility

Create a checked dependency inventory containing package, pinned version,
source, license expression, role, optional/default status, and native/runtime
requirements. Verify:

- Rust dependencies are compatible with the project distribution intent;
- RocksDB is used under its selected compatible license;
- BAML remains optional application-side/open-source context, not a core runtime
  requirement;
- CUDA driver/toolkit requirements and proprietary/free-use boundary are
  documented;
- optional experiments do not become correctness/data-access dependencies;
- no paid service, subscription, hosted database, or network access is required
  to build/test/inspect/backup/recover core PathHydra.

Add local commands/scripts that regenerate the inventory from `Cargo.lock` plus
manually documented native components. Do not add a GitHub Actions workflow.

## 16. Perform public API and panic/safety audit

Audit every public constructor, method, DTO, iterator, error, and documented
example. Ensure malformed external input cannot reach `unwrap`, `expect`, index
panic, unchecked conversion, or unbounded allocation. Internal impossibility
assertions require preceding invariant proof.

Run Clippy with warnings denied, rustdoc, unused feature combinations, CPU-only
builds without NVIDIA software, CUDA builds, and the unsafe boundary audit.
Confirm unsafe Rust remains limited to documented CUDA modules and every block
states current obligations.

## 17. Reconcile all documentation

Update `README.md`, `PATHHYDRA_SYSTEM_SHAPE.md`, decisions, storage/routing/CPU/
CUDA/hydration/subgraph/API/backup/operations references, performance reports,
and examples so they describe one current implementation.

Remove or correct:

- statements that startup always rebuilds from RocksDB;
- statements that CUDA always requires full topology residency after Plan 07;
- pre-release alternatives that were decided and removed;
- claims that encoding, backup, or inspection are unavailable after Plans 08-09;
- unsupported performance claims;
- obsolete development fixtures/layouts.

Do not rewrite history in completed plan documents; they remain implementation
records. Current reference documents and the conformance ledger are normative.

## 18. Final local verification matrix

Document and run:

```powershell
cargo fmt --all --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo doc --workspace --no-deps

cargo check --workspace --all-targets --features cuda
cargo clippy --workspace --all-targets --features cuda -- -D warnings
cargo test --workspace --features cuda
```

Also run:

- generated extended agreement with recorded seeds;
- subprocess crash/recovery campaign;
- checkpoint/restore rehearsal;
- canonical API malformed/round-trip corpus;
- CUDA memcheck and racecheck;
- full named release benchmark suite;
- 12-GiB out-of-core scale suite;
- dependency/license audit;
- conformance-ledger stale-reference check.

Capture date, machine, toolchain, feature flags, and result summary. Failures are
fixed in their owning code/docs; they are not waived by editing the ledger.

## Definition of done

Plan 10—and therefore the original PathHydra system shape—is complete when:

- every in-scope statement has executable/documented evidence in the
  conformance ledger;
- every deliberately open decision is accepted or rejected with evidence;
- generated CPU/resident/partitioned/CUDA results agree exactly;
- lifecycle, name, numeric, mutation, deletion, publication, hydration,
  subgraph, encoding, cancellation, and resource boundaries survive adversarial
  tests;
- crash, corruption, checkpoint, restore, and device-loss campaigns preserve
  authoritative graph state;
- every requested performance/observability field is measured and reconciled;
- the real topology-larger-than-device proof passes;
- dependencies/licenses/local-operation requirements are documented and
  reproducible;
- public APIs are bounded, typed, panic-audited, and accurately documented;
- all current reference documents agree with code;
- ordinary, CUDA, sanitizer, benchmark, recovery, encoding, and conformance
  gates pass without paid services or GitHub Actions.
